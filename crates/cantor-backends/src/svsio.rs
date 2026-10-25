//! The svs.io adapter (docs/spec/20-architecture.md, "Optional external
//! backends"): drives a third-party singing-synthesis process that speaks
//! OpenUtau's proposed svs.io protocol, so engines plug in without
//! recompiling Cantor.
//!
//! svs.io is a sketch rather than a wire contract; Cantor's interpretation
//! is pinned in docs/decisions/0023-svsio.md. In short: ZeroMQ REQ/REP over
//! TCP, one msgpack map per message; `{"op": "ops"}` describes the backend
//! (name, version, ops with `inputs`/`outputs` keyed by struct name); any
//! other request is `{"op": name, <struct>: value, …}` and the reply maps
//! output struct names to values; a reply with an `error` string is a
//! failure. Struct shapes follow the proposal (`note_sequence`,
//! `phoneme_sequence`, `f0` in cents, `curve:<name>`, `world_sp`,
//! `world_ap`, `audio_samples`), with `frame_duration` accepted as a
//! number or a string.
//!
//! The adapter finds a path through the backend's op graph: optionally a
//! phonemizer (→ `phoneme_sequence`) and a pitch model (→ `f0`), then a
//! synthesizer producing WORLD features (`world_sp` + `world_ap`, preferred:
//! pitch edits then re-synthesize locally on the vocoder fast path) or
//! `audio_samples` (every vocode is a remote call).

use crate::zmtp::ReqSocket;
use cantor_core::params;
use cantor_render::backend::{
    AcousticInput, Backend, BackendError, Grid, Phone, PhraseInput, Prediction,
};
use rmpv::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// Samples per frame on Cantor's side.
pub const HOP: usize = 256;
/// The proposal's liveness rule: `ops` must answer within a second.
pub const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(1);
/// Longest wait for a synthesis call.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Separates the parts of an encoded phone symbol.
const SEP: char = '\u{1f}';

/// How to reach a backend, from a singer folder's `svsio.yaml`.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SvsIoConfig {
    /// `tcp://host:port`.
    pub endpoint: String,
    /// Use `audio_samples` even when the backend offers WORLD features.
    #[serde(default)]
    pub prefer_audio: bool,
    /// Languages the backend sings (informational; svs.io has no way to
    /// ask).
    #[serde(default)]
    pub languages: Vec<String>,
}

/// One op as the backend describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct OpInfo {
    /// Op name.
    pub name: String,
    /// Input struct names with their descriptors.
    pub inputs: BTreeMap<String, Value>,
    /// Output struct names with their descriptors.
    pub outputs: BTreeMap<String, Value>,
}

impl OpInfo {
    fn required(&self) -> impl Iterator<Item = &str> {
        self.inputs
            .iter()
            .filter(|(_, d)| field(d, "required").and_then(Value::as_bool) == Some(true))
            .map(|(k, _)| k.as_str())
    }

    fn frame_ms(&self, input: &str) -> f64 {
        self.inputs.get(input).map_or(5.0, frame_duration)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Synth {
    World { op: String, fft_size: usize },
    Audio { op: String },
}

/// A connected svs.io backend.
pub struct SvsIo {
    id: String,
    version: String,
    endpoint: String,
    socket: Mutex<Option<ReqSocket>>,
    ops: Vec<OpInfo>,
    phonemize: Option<String>,
    pitch: Option<String>,
    synth: Synth,
    grid: Grid,
    params: Vec<&'static str>,
}

impl std::fmt::Debug for SvsIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvsIo")
            .field("id", &self.id)
            .field("endpoint", &self.endpoint)
            .field("synth", &self.synth)
            .finish_non_exhaustive()
    }
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_map()?
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_u64().map(|i| i as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// `frame_duration` in ms; the proposal writes it as a number or a string.
fn frame_duration(desc: &Value) -> f64 {
    field(desc, "frame_duration").and_then(num).unwrap_or(5.0) * time_scale(desc) * 1000.0
}

/// Seconds per `time_unit` (`s`, `ms` (default) or `us`).
fn time_scale(desc: &Value) -> f64 {
    match field(desc, "time_unit").and_then(Value::as_str) {
        Some("s") => 1.0,
        Some("us") => 1e-6,
        _ => 1e-3,
    }
}

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(k, v)| (Value::from(k), v))
            .collect(),
    )
}

fn floats(v: &[f64]) -> Value {
    Value::Array(v.iter().map(|&x| Value::F64(x)).collect())
}

/// Resamples a per-frame sequence from one frame period to another
/// (linear interpolation, clamped at the ends).
fn resample_frames(src: &[f64], src_secs: f64, dst_secs: f64, dst_len: usize) -> Vec<f64> {
    if src.is_empty() {
        return vec![0.0; dst_len];
    }
    (0..dst_len)
        .map(|i| {
            let x = (i as f64 * dst_secs / src_secs).clamp(0.0, (src.len() - 1) as f64);
            let j = (x.floor() as usize).min(src.len().saturating_sub(2));
            let u = x - j as f64;
            let k = (j + 1).min(src.len() - 1);
            src[j] * (1.0 - u) + src[k] * u
        })
        .collect()
}

impl SvsIo {
    /// Connects, describes the backend and plans the op graph.
    pub fn connect(id: &str, config: &SvsIoConfig) -> Result<Self, BackendError> {
        let err = |m: String| BackendError(format!("svs.io {}: {m}", config.endpoint));
        let mut socket = ReqSocket::connect(&config.endpoint, DESCRIBE_TIMEOUT).map_err(err)?;
        let reply = call(&mut socket, &map(vec![("op", Value::from("ops"))])).map_err(err)?;
        let name = field(&reply, "name")
            .and_then(str_of)
            .unwrap_or_else(|| "svs.io".into());
        let version = field(&reply, "version")
            .and_then(str_of)
            .unwrap_or_default();
        let ops: Vec<OpInfo> = field(&reply, "ops")
            .and_then(Value::as_array)
            .ok_or_else(|| err("`ops` reply has no `ops` list".into()))?
            .iter()
            .filter_map(|op| {
                let entries = |key| -> BTreeMap<String, Value> {
                    field(op, key)
                        .and_then(Value::as_map)
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.clone())))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                Some(OpInfo {
                    name: field(op, "op").and_then(str_of)?,
                    inputs: entries("inputs"),
                    outputs: entries("outputs"),
                })
            })
            .collect();
        socket.set_timeout(CALL_TIMEOUT).map_err(err)?;

        // What Cantor can always supply.
        let mut avail: Vec<String> = vec!["note_sequence".into(), "f0".into()];
        avail.extend(params::registry().iter().map(|d| format!("curve:{}", d.id)));
        let satisfiable =
            |op: &OpInfo, avail: &[String]| op.required().all(|r| avail.iter().any(|a| a == r));
        let producer = |out: &str, avail: &[String], exclude: &str| {
            ops.iter()
                .find(|op| {
                    op.outputs.contains_key(out)
                        && !op.inputs.contains_key(exclude)
                        && satisfiable(op, avail)
                })
                .map(|op| op.name.clone())
        };
        let phonemize = producer("phoneme_sequence", &avail, "phoneme_sequence");
        if phonemize.is_some() {
            avail.push("phoneme_sequence".into());
        }
        let pitch = producer("f0", &avail, "f0");
        let world = ops.iter().find(|op| {
            op.outputs.contains_key("world_sp")
                && op.outputs.contains_key("world_ap")
                && satisfiable(op, &avail)
        });
        let audio = ops
            .iter()
            .find(|op| op.outputs.contains_key("audio_samples") && satisfiable(op, &avail));
        let synth_of_world = |op: &OpInfo| {
            let width = op
                .outputs
                .get("world_sp")
                .and_then(|d| field(d, "width"))
                .and_then(num)
                .unwrap_or(1025.0) as usize;
            Synth::World {
                op: op.name.clone(),
                fft_size: (width.max(2) - 1) * 2,
            }
        };
        let synth = match (world, audio, config.prefer_audio) {
            (_, Some(a), true) | (None, Some(a), false) => Synth::Audio { op: a.name.clone() },
            (Some(w), _, _) => synth_of_world(w),
            (None, None, _) => {
                return Err(err(
                    "no op path from note_sequence to world_sp + world_ap or audio_samples".into(),
                ));
            }
        };
        let synth_op = match &synth {
            Synth::World { op, .. } | Synth::Audio { op } => ops
                .iter()
                .find(|o| &o.name == op)
                .expect("planned op exists"),
        };
        let sample_rate = synth_op
            .outputs
            .get("audio_samples")
            .and_then(|d| field(d, "sample_rate"))
            .and_then(num)
            .unwrap_or(44_100.0) as u32;
        let mut params: Vec<&'static str> = vec![params::known::PITCH];
        params.extend(
            params::registry()
                .iter()
                .map(|d| d.id)
                .filter(|id| *id != params::known::PITCH)
                .filter(|id| synth_op.inputs.contains_key(&format!("curve:{id}"))),
        );
        let fft_size = match &synth {
            Synth::World { fft_size, .. } => *fft_size,
            Synth::Audio { .. } => 0,
        };
        Ok(Self {
            id: format!("svsio/{id}"),
            version: format!("{name}@{version}"),
            endpoint: config.endpoint.clone(),
            socket: Mutex::new(Some(socket)),
            phonemize,
            pitch,
            grid: Grid {
                sample_rate,
                hop: HOP,
                mel_bins: if fft_size > 0 { fft_size + 2 } else { 1 },
            },
            synth,
            ops,
            params,
        })
    }

    /// Ops the backend describes.
    pub fn ops(&self) -> &[OpInfo] {
        &self.ops
    }

    /// Whether synthesis goes through WORLD features (true) or audio.
    pub fn uses_world_features(&self) -> bool {
        matches!(self.synth, Synth::World { .. })
    }

    fn op(&self, name: &str) -> &OpInfo {
        self.ops
            .iter()
            .find(|o| o.name == name)
            .expect("planned op exists")
    }

    /// Sends a request, reconnecting once if the connection was lost.
    fn call(&self, req: &Value) -> Result<Value, BackendError> {
        let err = |m: String| BackendError(format!("svs.io {}: {m}", self.endpoint));
        let mut guard = self.socket.lock().expect("svs.io socket lock");
        for attempt in 0..2 {
            if guard.is_none() {
                let mut s = ReqSocket::connect(&self.endpoint, DESCRIBE_TIMEOUT).map_err(err)?;
                s.set_timeout(CALL_TIMEOUT).map_err(err)?;
                *guard = Some(s);
            }
            match call(guard.as_mut().expect("connected"), req) {
                Ok(v) => return Ok(v),
                Err(e) if attempt == 0 && !e.starts_with("backend error") => *guard = None,
                Err(e) => return Err(err(e)),
            }
        }
        unreachable!("the second attempt returns")
    }

    fn frame_secs(&self) -> f64 {
        self.grid.frame_secs()
    }

    /// The phrase as a `note_sequence` (rests fill the gaps and the
    /// padding), with each sung note's index in it.
    fn note_sequence(&self, input: &PhraseInput<'_>) -> Value {
        let end = input.t0 + input.frames as f64 * self.frame_secs();
        let mut notes = Vec::new();
        let mut t = input.t0;
        let rest = |dur: f64| {
            map(vec![
                ("lyric", Value::from("")),
                ("duration", Value::F64(dur * 1000.0)),
                ("key", Value::from(60)),
            ])
        };
        for n in &input.phrase.notes {
            if n.start_sec > t + 1e-6 {
                notes.push(rest(n.start_sec - t));
            }
            let lyric = match cantor_phon::parse_lyric(&n.lyric) {
                cantor_phon::Lyric::Rest => String::new(),
                _ => n.lyric.trim().to_owned(),
            };
            notes.push(map(vec![
                ("lyric", Value::from(lyric)),
                (
                    "duration",
                    Value::F64((n.end_sec - n.start_sec.max(t)) * 1000.0),
                ),
                ("key", Value::from((n.pitch_cents / 100.0).round() as i64)),
            ]));
            t = n.end_sec;
        }
        if end > t + 1e-6 {
            notes.push(rest(end - t));
        }
        map(vec![
            ("time_unit", Value::from("ms")),
            ("notes", Value::Array(notes)),
        ])
    }

    /// A per-frame Cantor sequence as an svs.io frame struct for `input`
    /// of `op`, under `key` (`f0` or `curve`).
    fn frames_value(&self, op: &OpInfo, input: &str, key: &str, values: &[f64]) -> Value {
        let ms = op.frame_ms(input);
        let len = ((values.len() as f64 * self.frame_secs() * 1000.0) / ms).ceil() as usize;
        let resampled = resample_frames(values, self.frame_secs(), ms / 1000.0, len);
        map(vec![
            ("time_unit", Value::from("ms")),
            ("frame_duration", Value::F64(ms)),
            (key, floats(&resampled)),
        ])
    }

    /// A backend frame struct (`f0`, `curve:*`) on Cantor's frame grid.
    fn frames_from(&self, v: &Value, key: &str, frames: usize) -> Option<Vec<f64>> {
        let src: Vec<f64> = field(v, key)?.as_array()?.iter().filter_map(num).collect();
        Some(resample_frames(
            &src,
            frame_duration(v) / 1000.0,
            self.frame_secs(),
            frames,
        ))
    }

    /// The request for the synthesis op, without `f0` when `f0` is `None`.
    fn synth_request(
        &self,
        op: &OpInfo,
        phones: &[Phone],
        variance: &BTreeMap<String, Vec<f32>>,
        f0_hz: Option<&[f32]>,
    ) -> Value {
        let mut entries = vec![("op", Value::from(op.name.clone()))];
        let seq = phoneme_sequence(phones, self.frame_secs());
        if op.inputs.contains_key("phoneme_sequence") {
            entries.push(("phoneme_sequence", seq));
        }
        let mut owned: Vec<(String, Value)> = Vec::new();
        for (param, v) in variance {
            let key = format!("curve:{param}");
            if op.inputs.contains_key(&key) {
                let vals: Vec<f64> = v.iter().map(|&x| f64::from(x)).collect();
                owned.push((key.clone(), self.frames_value(op, &key, "curve", &vals)));
            }
        }
        if let Some(f0) = f0_hz {
            let cents: Vec<f64> = f0
                .iter()
                .map(|&hz| 6900.0 + 1200.0 * (f64::from(hz).max(1.0) / 440.0).log2())
                .collect();
            owned.push(("f0".into(), self.frames_value(op, "f0", "f0", &cents)));
        }
        let mut m: Vec<(Value, Value)> = entries
            .into_iter()
            .map(|(k, v)| (Value::from(k), v))
            .collect();
        m.extend(owned.into_iter().map(|(k, v)| (Value::from(k), v)));
        Value::Map(m)
    }
}

/// Encodes a request, sends it and decodes the reply; `error` replies fail.
fn call(socket: &mut ReqSocket, req: &Value) -> Result<Value, String> {
    let mut body = Vec::new();
    rmpv::encode::write_value(&mut body, req).map_err(|e| e.to_string())?;
    let reply = socket.request(&body)?;
    let v = rmpv::decode::read_value(&mut reply.as_slice()).map_err(|e| e.to_string())?;
    if let Some(e) = field(&v, "error") {
        return Err(format!(
            "backend error: {}",
            e.as_str().map_or_else(|| e.to_string(), str::to_owned)
        ));
    }
    Ok(v)
}

/// Phones (encoded `phoneme␟note_index␟key`) back into a
/// `phoneme_sequence` in ms.
fn phoneme_sequence(phones: &[Phone], frame_secs: f64) -> Value {
    let list = phones
        .iter()
        .map(|p| {
            let mut parts = p.symbol.split(SEP);
            let phoneme = parts.next().unwrap_or("");
            let phoneme = if phoneme == "SP" { "" } else { phoneme };
            let note_index = parts
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let key = parts
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(60);
            map(vec![
                ("note_index", Value::from(note_index)),
                ("phoneme", Value::from(phoneme)),
                (
                    "duration",
                    Value::F64(p.frames as f64 * frame_secs * 1000.0),
                ),
                ("key", Value::from(key)),
            ])
        })
        .collect();
    map(vec![
        ("time_unit", Value::from("ms")),
        ("phonemes", Value::Array(list)),
    ])
}

/// Splits `total` frames by `durations` (seconds), rounding boundaries.
fn frames_by(durations: &[f64], frame_secs: f64, total: i64) -> Vec<i64> {
    let mut out = Vec::with_capacity(durations.len());
    let mut acc = 0.0;
    let mut prev = 0i64;
    for d in durations {
        acc += d;
        let b = ((acc / frame_secs).round() as i64).clamp(prev, total);
        out.push(b - prev);
        prev = b;
    }
    if let Some(last) = out.last_mut() {
        *last += total - prev;
    }
    out
}

impl Backend for SvsIo {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn grid(&self) -> Grid {
        self.grid
    }

    fn params(&self) -> &[&'static str] {
        &self.params
    }

    fn param_default(&self, param: &str) -> f32 {
        params::lookup(param).map_or(0.0, |d| d.default as f32)
    }

    fn param_range(&self, param: &str) -> (f32, f32) {
        params::lookup(param).map_or((0.0, 0.0), |d| (d.min as f32, d.max as f32))
    }

    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError> {
        let fs = self.frame_secs();
        let total = input.frames as i64;
        let notes = self.note_sequence(input);
        let note_keys: Vec<i64> = field(&notes, "notes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|n| field(n, "key").and_then(Value::as_i64).unwrap_or(60))
                    .collect()
            })
            .unwrap_or_default();
        let (phones, seq) = match &self.phonemize {
            Some(op) => {
                let reply = self.call(&map(vec![
                    ("op", Value::from(op.clone())),
                    ("note_sequence", notes.clone()),
                ]))?;
                let seq = field(&reply, "phoneme_sequence")
                    .cloned()
                    .ok_or_else(|| BackendError(format!("`{op}` returned no phoneme_sequence")))?;
                let scale = time_scale(&seq);
                let list = field(&seq, "phonemes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let durs: Vec<f64> = list
                    .iter()
                    .map(|p| field(p, "duration").and_then(num).unwrap_or(0.0) * scale)
                    .collect();
                let frames = frames_by(&durs, fs, total);
                let phones = list
                    .iter()
                    .zip(frames)
                    .map(|(p, f)| {
                        let ph = field(p, "phoneme").and_then(str_of).unwrap_or_default();
                        let idx = field(p, "note_index").and_then(Value::as_i64).unwrap_or(0);
                        let key = field(p, "key")
                            .and_then(Value::as_i64)
                            .or_else(|| note_keys.get(idx as usize).copied())
                            .unwrap_or(60);
                        let ph = if ph.is_empty() { "SP".to_owned() } else { ph };
                        Phone {
                            symbol: format!("{ph}{SEP}{idx}{SEP}{key}"),
                            frames: f,
                        }
                    })
                    .filter(|p| p.frames > 0)
                    .collect::<Vec<_>>();
                (phones, Some(seq))
            }
            None => {
                let list = field(&notes, "notes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let durs: Vec<f64> = list
                    .iter()
                    .map(|n| field(n, "duration").and_then(num).unwrap_or(0.0) / 1000.0)
                    .collect();
                let phones = list
                    .iter()
                    .zip(frames_by(&durs, fs, total))
                    .enumerate()
                    .map(|(i, (n, f))| {
                        let lyric = field(n, "lyric").and_then(str_of).unwrap_or_default();
                        let key = note_keys.get(i).copied().unwrap_or(60);
                        let ph = if lyric.is_empty() {
                            "SP".to_owned()
                        } else {
                            lyric
                        };
                        Phone {
                            symbol: format!("{ph}{SEP}{i}{SEP}{key}"),
                            frames: f,
                        }
                    })
                    .filter(|p| p.frames > 0)
                    .collect();
                (phones, None)
            }
        };
        let pitch = match &self.pitch {
            Some(op_name) => {
                let op = self.op(op_name);
                let mut req = vec![("op", Value::from(op_name.clone()))];
                if op.inputs.contains_key("note_sequence") {
                    req.push(("note_sequence", notes.clone()));
                }
                if let Some(seq) = seq.filter(|_| op.inputs.contains_key("phoneme_sequence")) {
                    req.push(("phoneme_sequence", seq));
                }
                let reply = self.call(&map(req))?;
                let f0 = field(&reply, "f0")
                    .ok_or_else(|| BackendError(format!("`{op_name}` returned no f0")))?;
                let cents = self
                    .frames_from(f0, "f0", input.frames)
                    .ok_or_else(|| BackendError(format!("`{op_name}` returned malformed f0")))?;
                Some(cents.into_iter().map(|c| c as f32).collect())
            }
            None => None,
        };
        Ok(Prediction {
            phones,
            pitch,
            variance: BTreeMap::new(),
            diagnostics: Vec::new(),
        })
    }

    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError> {
        let frames = input.f0_ref_hz.len();
        match &self.synth {
            Synth::World { op, fft_size } => {
                let op = self.op(op);
                let req =
                    self.synth_request(op, input.phones, input.variance, Some(input.f0_ref_hz));
                let reply = self.call(&req)?;
                let bins = fft_size / 2 + 1;
                let rows = |key: &str| -> Result<(Vec<Vec<f64>>, f64), BackendError> {
                    let v = field(&reply, key)
                        .ok_or_else(|| BackendError(format!("`{}` returned no {key}", op.name)))?;
                    let rows: Vec<Vec<f64>> = field(v, key)
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|r| {
                                    Some(r.as_array()?.iter().filter_map(num).collect())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if rows.is_empty() || rows.iter().any(|r: &Vec<f64>| r.len() != bins) {
                        return Err(BackendError(format!(
                            "`{}` returned {key} rows that are not {bins} wide",
                            op.name
                        )));
                    }
                    Ok((rows, frame_duration(v) / 1000.0))
                };
                let (sp, sp_secs) = rows("world_sp")?;
                let (ap, ap_secs) = rows("world_ap")?;
                let mut out = Vec::with_capacity(frames * 2 * bins);
                let pick = |rows: &[Vec<f64>], secs: f64, i: usize| -> usize {
                    ((i as f64 * self.frame_secs() / secs).round() as usize).min(rows.len() - 1)
                };
                for i in 0..frames {
                    out.extend(sp[pick(&sp, sp_secs, i)].iter().map(|&x| x as f32));
                    out.extend(ap[pick(&ap, ap_secs, i)].iter().map(|&x| x as f32));
                }
                Ok(out)
            }
            Synth::Audio { op } => {
                // The request (minus f0) travels as the "features", so the
                // vocoder stage can call the backend with the final f0.
                let req = self.synth_request(self.op(op), input.phones, input.variance, None);
                let mut bytes = Vec::new();
                rmpv::encode::write_value(&mut bytes, &req)
                    .map_err(|e| BackendError(e.to_string()))?;
                let mut out = Vec::with_capacity(bytes.len() + 1);
                out.push(frames as f32);
                out.extend(bytes.iter().map(|&b| f32::from(b)));
                Ok(out)
            }
        }
    }

    fn vocode(&self, features: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError> {
        match &self.synth {
            Synth::World { fft_size, .. } => {
                crate::world::synthesize(features, *fft_size, f0_hz, HOP, self.grid.sample_rate)
            }
            Synth::Audio { op } => {
                let bytes: Vec<u8> = features[1..].iter().map(|&b| b as u8).collect();
                let mut req = rmpv::decode::read_value(&mut bytes.as_slice())
                    .map_err(|e| BackendError(format!("internal: {e}")))?;
                let op = self.op(op);
                let cents: Vec<f64> = f0_hz
                    .iter()
                    .map(|&hz| 6900.0 + 1200.0 * (f64::from(hz).max(1.0) / 440.0).log2())
                    .collect();
                if let Value::Map(m) = &mut req {
                    m.push((Value::from("f0"), self.frames_value(op, "f0", "f0", &cents)));
                }
                let reply = self.call(&req)?;
                let audio = field(&reply, "audio_samples").ok_or_else(|| {
                    BackendError(format!("`{}` returned no audio_samples", op.name))
                })?;
                let samples = decode_audio(audio).map_err(BackendError)?;
                let sr = field(audio, "sample_rate")
                    .and_then(num)
                    .map_or(self.grid.sample_rate, |r| r as u32);
                let mut samples = if sr == self.grid.sample_rate {
                    samples
                } else {
                    world_rs::resample::resample(&samples, sr, self.grid.sample_rate)
                };
                samples.resize(f0_hz.len() * HOP, 0.0);
                Ok(samples)
            }
        }
    }

    fn warp_timbre(&self, features: &[f32], factor: f32) -> Option<Vec<f32>> {
        match &self.synth {
            Synth::World { fft_size, .. } => Some(crate::world::warp_features(
                features,
                *fft_size,
                f64::from(factor),
            )),
            Synth::Audio { .. } => None,
        }
    }
}

/// Mono f32 from an `audio_samples` struct: `samples` as a flat array or
/// one array per channel (first channel used), in `int8`/`int16`/`int32`
/// (full-scale) or `float` format.
fn decode_audio(v: &Value) -> Result<Vec<f32>, String> {
    let scale = match field(v, "sample_format").and_then(Value::as_str) {
        Some("int8") => 128.0,
        Some("int16") => 32768.0,
        Some("int32") => 2_147_483_648.0,
        _ => 1.0,
    };
    let raw = field(v, "samples")
        .or_else(|| field(v, "audio_samples"))
        .and_then(Value::as_array)
        .ok_or("audio_samples has no samples")?;
    let first = match raw.first() {
        Some(Value::Array(ch)) => ch.as_slice(),
        _ => raw.as_slice(),
    };
    Ok(first
        .iter()
        .filter_map(num)
        .map(|x| (x / scale) as f32)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_durations_accept_numbers_and_strings() {
        let a = map(vec![("frame_duration", Value::from("5"))]);
        let b = map(vec![
            ("frame_duration", Value::F64(0.01)),
            ("time_unit", Value::from("s")),
        ]);
        assert_eq!(frame_duration(&a), 5.0);
        assert!((frame_duration(&b) - 10.0).abs() < 1e-12);
    }

    #[test]
    fn frames_split_exactly() {
        let f = frames_by(&[0.1, 0.25, 0.05], 0.01, 40);
        assert_eq!(f.iter().sum::<i64>(), 40);
        assert_eq!(f[..2], [10, 25]);
    }

    #[test]
    fn audio_decodes_int16_and_channels() {
        let v = map(vec![
            ("sample_format", Value::from("int16")),
            (
                "samples",
                Value::Array(vec![Value::Array(vec![
                    Value::from(16384),
                    Value::from(-32768),
                ])]),
            ),
        ]);
        assert_eq!(decode_audio(&v).unwrap(), [0.5, -1.0]);
    }
}
