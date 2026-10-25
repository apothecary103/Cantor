//! Adapter for flow-matching (rectified-flow) acoustic models, the family
//! newer SVS systems use (flow-matching TechSinger, diffusion-transformer
//! DiTSinger; DiffSinger's own rectified-flow acoustic models).
//!
//! The model is exported as its velocity network only: an ONNX graph
//! `v = f(x, t, cond)` over mel frames. Cantor runs the ODE itself, from
//! `t0` to 1 along `x_t = (1 - t)·x0 + t·x1` (x0 noise, x1 data):
//!
//! ```text
//! x(t0) = depth·noise + (1 - depth)·prior,  t0 = 1 - depth
//! x(t + Δt) = x(t) + Δt·v(x, t, prior)          (Euler)
//! ```
//!
//! with Heun's method available for the final pass. `prior` is the mel of
//! the wrapped backend's acoustic model, so `depth < 1` is shallow flow
//! (refining a coarse prediction, as DiffSinger's shallow diffusion does)
//! and `depth = 1` is generation from noise conditioned on the prior.
//! Noise comes from Cantor's seeded RNG and the step count is the render
//! quality, so renders stay deterministic and the preview/final knob works
//! the same as for every other backend.
//!
//! [`FlowBackend`] wraps any [`Backend`]: predictions, vocoder and timbre
//! warping come from it; only the acoustic stage is replaced.

use crate::onnx::{Model, tensor_f32};
use cantor_render::backend::{AcousticInput, Backend, BackendError, Grid, PhraseInput, Prediction};
use cantor_render::rng::Rng;
use std::path::Path;
use std::sync::Arc;

/// ODE solver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Solver {
    /// One velocity evaluation per step.
    Euler,
    /// Two evaluations per step, second-order accurate.
    Heun,
}

/// A velocity field: `v(x, t)`.
pub type Velocity<'a> = dyn FnMut(&[f32], f32) -> Result<Vec<f32>, BackendError> + 'a;

/// Integrates `dx/dt = v(x, t)` from `t0` to 1 in `steps` equal steps.
pub fn integrate(
    v: &mut Velocity<'_>,
    mut x: Vec<f32>,
    t0: f32,
    steps: u32,
    solver: Solver,
) -> Result<Vec<f32>, BackendError> {
    let steps = steps.max(1);
    let dt = (1.0 - t0) / steps as f32;
    for k in 0..steps {
        let t = t0 + k as f32 * dt;
        let v1 = v(&x, t)?;
        check(&v1, x.len())?;
        match solver {
            Solver::Euler => {
                for (xi, vi) in x.iter_mut().zip(&v1) {
                    *xi += dt * vi;
                }
            }
            Solver::Heun => {
                let pred: Vec<f32> = x.iter().zip(&v1).map(|(xi, vi)| xi + dt * vi).collect();
                let v2 = v(&pred, t + dt)?;
                check(&v2, x.len())?;
                for ((xi, a), b) in x.iter_mut().zip(&v1).zip(&v2) {
                    *xi += dt * 0.5 * (a + b);
                }
            }
        }
    }
    Ok(x)
}

fn check(v: &[f32], n: usize) -> Result<(), BackendError> {
    if v.len() != n {
        return Err(BackendError(format!(
            "velocity has {} values for {n} state values",
            v.len()
        )));
    }
    if v.iter().any(|x| !x.is_finite()) {
        return Err(BackendError("velocity is not finite".into()));
    }
    Ok(())
}

/// `flow.yaml`: how to run a velocity model.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowConfig {
    /// The velocity ONNX model, relative to the config file.
    pub velocity: String,
    /// How much of the path is integrated: 1 = from pure noise.
    #[serde(default = "one")]
    pub depth: f32,
    /// Solver for final-quality renders (previews always use Euler).
    #[serde(default = "heun")]
    pub solver: Solver,
    /// Input and output names of the velocity model.
    #[serde(default)]
    pub names: FlowNames,
    /// Model version, part of cache keys.
    #[serde(default)]
    pub version: String,
}

/// Tensor names of a velocity model.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FlowNames {
    /// State `[1, frames, mel_bins]`.
    pub x: String,
    /// Time `[1]`.
    pub t: String,
    /// Conditioning `[1, frames, mel_bins]` (the prior mel).
    pub cond: String,
    /// Velocity, the shape of `x`.
    pub v: String,
}

impl Default for FlowNames {
    fn default() -> Self {
        Self {
            x: "x".into(),
            t: "t".into(),
            cond: "cond".into(),
            v: "v".into(),
        }
    }
}

fn one() -> f32 {
    1.0
}

fn heun() -> Solver {
    Solver::Heun
}

/// Render steps at or below this count use Euler (previews).
const PREVIEW_STEPS: u32 = 10;

/// A backend whose acoustic stage is a flow-matching model.
pub struct FlowBackend {
    inner: Arc<dyn Backend>,
    model: Model,
    config: FlowConfig,
    id: String,
    version: String,
}

impl std::fmt::Debug for FlowBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowBackend")
            .field("id", &self.id)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl FlowBackend {
    /// Wraps `inner` with the velocity model described by `flow.yaml`.
    pub fn load(inner: Arc<dyn Backend>, config_path: &Path) -> Result<Self, BackendError> {
        let text = std::fs::read_to_string(config_path)
            .map_err(|e| BackendError(format!("{}: {e}", config_path.display())))?;
        let config: FlowConfig = serde_saphyr::from_str(&text)
            .map_err(|e| BackendError(format!("{}: {e}", config_path.display())))?;
        if !(0.0..=1.0).contains(&config.depth) {
            return Err(BackendError(format!(
                "{}: depth must be within 0..=1",
                config_path.display()
            )));
        }
        let dir = config_path.parent().unwrap_or(Path::new("."));
        let model = Model::load(&dir.join(&config.velocity))?;
        for name in [&config.names.x, &config.names.t, &config.names.cond] {
            if !model.has_input(name) {
                return Err(BackendError(format!(
                    "{}: the velocity model has no input `{name}` (it has {:?})",
                    config.velocity,
                    model.input_names()
                )));
            }
        }
        Ok(Self {
            id: format!("flow/{}", inner.id()),
            version: format!(
                "{}+flow:{}:{}:{:?}",
                inner.version(),
                config.version,
                config.depth,
                config.solver
            ),
            inner,
            model,
            config,
        })
    }
}

impl Backend for FlowBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn grid(&self) -> Grid {
        self.inner.grid()
    }

    fn params(&self) -> &[&'static str] {
        self.inner.params()
    }

    fn param_default(&self, param: &str) -> f32 {
        self.inner.param_default(param)
    }

    fn param_range(&self, param: &str) -> (f32, f32) {
        self.inner.param_range(param)
    }

    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError> {
        self.inner.predict(input)
    }

    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError> {
        let prior = self.inner.acoustic(input)?;
        let bins = self.grid().mel_bins;
        if bins == 0 || prior.len() % bins != 0 {
            return Err(BackendError(format!(
                "{}: prior of {} values is not whole frames of {bins} bins",
                self.id,
                prior.len()
            )));
        }
        let depth = self.config.depth;
        if depth == 0.0 {
            return Ok(prior);
        }
        let frames = prior.len() / bins;
        let x = initial_state(&prior, depth, input.seed);
        let names = &self.config.names;
        let shape = [1, frames, bins];
        let mut velocity = |x: &[f32], t: f32| -> Result<Vec<f32>, BackendError> {
            let inputs = vec![
                (leak(&names.x), tensor_f32(&shape, x.to_vec())?),
                (leak(&names.t), tensor_f32(&[1], vec![t])?),
                (leak(&names.cond), tensor_f32(&shape, prior.clone())?),
            ];
            let mut out = self.model.run(inputs, 0, 0, &[names.v.as_str()])?;
            Ok(out.remove(0))
        };
        let solver = if input.steps <= PREVIEW_STEPS {
            Solver::Euler
        } else {
            self.config.solver
        };
        integrate(&mut velocity, x, 1.0 - depth, input.steps, solver)
    }

    fn vocode(&self, mel: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError> {
        self.inner.vocode(mel, f0_hz)
    }

    fn warp_timbre(&self, mel: &[f32], factor: f32) -> Option<Vec<f32>> {
        self.inner.warp_timbre(mel, factor)
    }
}

/// The ODE's starting point: `depth·noise + (1 - depth)·prior`, with noise
/// from Cantor's RNG seeded by the phrase's acoustic seed.
pub fn initial_state(prior: &[f32], depth: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed ^ 0xF10F_10F1_0F10_F10F);
    rng.normals_f32(prior.len())
        .iter()
        .zip(prior)
        .map(|(n, p)| depth * n + (1.0 - depth) * p)
        .collect()
}

/// `Model::run` names inputs with `&'static str`; tensor names come from
/// the config, which lives as long as the backend. Interned once per name.
fn leak(name: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = NAMES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .expect("names lock");
    if let Some(n) = set.get(name) {
        return n;
    }
    let n: &'static str = Box::leak(name.to_owned().into_boxed_str());
    set.insert(n);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_follows_straight_paths_exactly() {
        // Rectified flow toward c: v = (c - x) / (1 - t) has straight paths,
        // so any number of Euler steps lands exactly on c.
        let c = [0.25f32, -1.5, 3.0];
        let mut v = |x: &[f32], t: f32| -> Result<Vec<f32>, BackendError> {
            Ok(x.iter()
                .zip(c)
                .map(|(xi, ci)| (ci - xi) / (1.0 - t))
                .collect())
        };
        for steps in [1, 3, 10] {
            let out = integrate(&mut v, vec![1.0, 1.0, 1.0], 0.0, steps, Solver::Euler).unwrap();
            for (o, ci) in out.iter().zip(c) {
                assert!((o - ci).abs() < 1e-5, "{steps} steps: {out:?}");
            }
        }
    }

    #[test]
    fn heun_is_second_order() {
        // dx/dt = -x from x(0) = 1: x(1) = e^-1.
        let mut v = |x: &[f32], _t: f32| -> Result<Vec<f32>, BackendError> {
            Ok(x.iter().map(|xi| -xi).collect())
        };
        let exact = (-1.0f32).exp();
        let mut err = |solver, steps| {
            (integrate(&mut v, vec![1.0], 0.0, steps, solver).unwrap()[0] - exact).abs()
        };
        assert!(err(Solver::Heun, 8) < err(Solver::Euler, 8) / 10.0);
        // Halving the step quarters Heun's error.
        let ratio = err(Solver::Heun, 8) / err(Solver::Heun, 16);
        assert!((3.0..5.0).contains(&ratio), "ratio {ratio}");
    }

    #[test]
    fn bad_velocities_are_errors() {
        let mut short = |_: &[f32], _: f32| Ok(vec![0.0]);
        assert!(integrate(&mut short, vec![0.0, 0.0], 0.0, 2, Solver::Euler).is_err());
        let mut nan = |x: &[f32], _: f32| Ok(vec![f32::NAN; x.len()]);
        assert!(integrate(&mut nan, vec![0.0], 0.0, 2, Solver::Euler).is_err());
    }
}
