//! ONNX Runtime sessions: loading (with deterministic noise), named inputs,
//! and tensor helpers.

use crate::onnx_noise::{NOISE_INPUT_PREFIX, rewrite_random_normal};
use cantor_render::backend::BackendError;
use cantor_render::rng::Rng;
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{DynValue, Tensor};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Intra-op threads per session. Fixed so results do not depend on the
/// machine's core count.
pub const INTRA_THREADS: usize = 4;

/// The ONNX Runtime environment, loaded from ORT_DYLIB_PATH once.
pub fn environment() -> Result<&'static Environment, BackendError> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let path = std::env::var("ORT_DYLIB_PATH").map_err(|_| {
            "ORT_DYLIB_PATH is not set; run inside the Nix dev shell or point it at libonnxruntime"
                .to_owned()
        })?;
        ort::init_from(&path)
            .map_err(|e| format!("loading {path}: {e}"))?
            .with_name("cantor")
            .build()
            .map_err(|e| format!("creating the ONNX Runtime environment: {e}"))
    })
    .as_ref()
    .map_err(|e| BackendError(e.clone()))
}

/// A loaded model.
#[derive(Debug)]
pub struct Model {
    name: String,
    session: Mutex<Session>,
    inputs: Vec<String>,
    noise_inputs: Vec<String>,
}

impl Model {
    /// Loads a model, replacing in-graph noise with inputs.
    pub fn load(path: &Path) -> Result<Self, BackendError> {
        let env = environment()?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let bytes =
            std::fs::read(path).map_err(|e| BackendError(format!("{}: {e}", path.display())))?;
        let (bytes, noise_inputs) = match rewrite_random_normal(&bytes)
            .map_err(|e| BackendError(format!("{name}: {e}")))?
        {
            Some(r) => (r.bytes, r.noise_inputs),
            None => (bytes, Vec::new()),
        };
        let opt = |e: ort::Error<ort::session::builder::SessionBuilder>| {
            BackendError(format!("{name}: session options: {e}"))
        };
        let mut session = Session::builder(env)
            .map_err(|e| BackendError(format!("{name}: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(opt)?
            .with_intra_threads(INTRA_THREADS)
            .map_err(opt)?
            .with_inter_threads(1)
            .map_err(opt)?
            .with_deterministic_compute(true)
            .map_err(opt)?;
        let session = session
            .commit_from_memory(&bytes)
            .map_err(|e| BackendError(format!("{name}: {e}")))?;
        let inputs = session
            .inputs()
            .iter()
            .map(|o| o.name().to_owned())
            .collect();
        Ok(Self {
            name,
            session: Mutex::new(session),
            inputs,
            noise_inputs,
        })
    }

    /// Whether the model declares an input.
    pub fn has_input(&self, name: &str) -> bool {
        self.inputs.iter().any(|i| i == name)
    }

    /// Runs the model. `available` holds every input the caller can supply;
    /// only those the model declares are passed. Noise inputs are filled
    /// from `noise_seed` with `noise_per_frame × frames` samples. Returns
    /// the named outputs as f32 data.
    pub fn run(
        &self,
        mut available: Vec<(&'static str, DynValue)>,
        noise_seed: u64,
        noise_len: usize,
        outputs: &[&str],
    ) -> Result<Vec<Vec<f32>>, BackendError> {
        let mut feed: Vec<(String, DynValue)> = Vec::new();
        for want in &self.inputs {
            if want.starts_with(NOISE_INPUT_PREFIX) {
                continue;
            }
            let v = available
                .iter()
                .position(|(n, _)| n == want)
                .ok_or_else(|| {
                    BackendError(format!("{}: no value for input `{want}`", self.name))
                })?;
            feed.push((want.clone(), available.swap_remove(v).1));
        }
        for (k, n) in self.noise_inputs.iter().enumerate() {
            let mut rng = Rng::new(noise_seed ^ (k as u64).wrapping_mul(0xA24B_AED4_963E_E407));
            let noise = rng.normals_f32(noise_len);
            feed.push((n.clone(), tensor_f32(&[noise_len], noise)?));
        }
        let mut session = self.session.lock().expect("session lock");
        let out = session
            .run(feed)
            .map_err(|e| BackendError(format!("{}: {e}", self.name)))?;
        outputs
            .iter()
            .map(|o| {
                let v = out
                    .get(o)
                    .ok_or_else(|| BackendError(format!("{}: no output `{o}`", self.name)))?;
                let (_, data) = v
                    .try_extract_tensor::<f32>()
                    .map_err(|e| BackendError(format!("{}: output `{o}`: {e}", self.name)))?;
                Ok(data.to_vec())
            })
            .collect()
    }
}

/// An f32 tensor.
pub fn tensor_f32(shape: &[usize], data: Vec<f32>) -> Result<DynValue, BackendError> {
    Tensor::from_array((shape.to_vec(), data))
        .map(|t| t.into_dyn())
        .map_err(|e| BackendError(e.to_string()))
}

/// An i64 tensor.
pub fn tensor_i64(shape: &[usize], data: Vec<i64>) -> Result<DynValue, BackendError> {
    Tensor::from_array((shape.to_vec(), data))
        .map(|t| t.into_dyn())
        .map_err(|e| BackendError(e.to_string()))
}

/// A bool tensor.
pub fn tensor_bool(shape: &[usize], data: Vec<bool>) -> Result<DynValue, BackendError> {
    Tensor::from_array((shape.to_vec(), data))
        .map(|t| t.into_dyn())
        .map_err(|e| BackendError(e.to_string()))
}

/// A rank-0 f32 tensor.
pub fn scalar_f32(x: f32) -> Result<DynValue, BackendError> {
    Tensor::from_array(((), vec![x]))
        .map(|t| t.into_dyn())
        .map_err(|e| BackendError(e.to_string()))
}

/// A rank-0 i64 tensor.
pub fn scalar_i64(x: i64) -> Result<DynValue, BackendError> {
    Tensor::from_array(((), vec![x]))
        .map(|t| t.into_dyn())
        .map_err(|e| BackendError(e.to_string()))
}
