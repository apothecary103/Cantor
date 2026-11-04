//! The rule-based tuner (docs/design.md, "Tuner"; the rules and numbers
//! are in docs/tuning.md). Each pass reads a project and returns ordinary
//! [`Op`]s, wrapped in one labeled batch, so the result is undoable and
//! visible in the UI. Passes never mutate the project.
//!
//! Passes own what they write and overwrite it, so running a pass twice
//! gives the same project as running it once (except `humanize`, which
//! adds to `pitd`). Notes are in scope when their start lies in the
//! request's `range` (`[first, last)` tick); notes outside are untouched.
//!
//! | pass            | writes                                               |
//! |-----------------|------------------------------------------------------|
//! | auto_pitch      | note portamento, scoop, fall, vibrato               |
//! | auto_dynamics   | `dyn` curve over the phrases, note `vel` and `vol`  |
//! | breaths         | adds `br` notes                                     |
//! | section_color   | `bre`, `bri`, `gen`, `dyn` curves over a section    |
//! | match_reference | `pitd` curve over the range                         |
//! | humanize        | adds drift to `pitd`; note `timing.offset_ms`       |

mod dynamics;
mod expression;
mod reference;

pub use dynamics::{AutoDynamicsParams, BreathsParams, Preset, SectionColorParams};
pub use expression::{AutoPitchParams, Style};
pub use reference::{HumanizeParams, MatchReferenceParams};

use crate::apply::apply;
use crate::diag::{Diagnostic, Level, Rejected};
use crate::ids::{Tick, TrackId};
use crate::model::{Note, Project, Track};
use crate::op::Op;
use crate::pitch::GLIDE_GAP_SEC;
use crate::time::TempoMap;
use schemars::{JsonSchema, Schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A tuner pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pass {
    /// Portamento, overshoot, preparation, scoops, falls and vibrato by
    /// interval, tempo, phrase position and style.
    AutoPitch,
    /// Phrase arcs on `dyn`, velocity by note density, accents on strong
    /// beats.
    AutoDynamics,
    /// Inserts `br` notes in rests before phrases.
    Breaths,
    /// Timbre and level (`bre`, `bri`, `gen`, `dyn`) for a section.
    SectionColor,
    /// Writes `pitd` so the pitch follows the reference vocal's F0.
    MatchReference,
    /// Small deterministic pitch drift and timing jitter.
    Humanize,
}

impl Pass {
    /// Every pass.
    pub const ALL: [Pass; 6] = [
        Pass::AutoPitch,
        Pass::AutoDynamics,
        Pass::Breaths,
        Pass::SectionColor,
        Pass::MatchReference,
        Pass::Humanize,
    ];

    /// The wire name, e.g. `"auto_pitch"`.
    pub fn name(self) -> &'static str {
        match self {
            Pass::AutoPitch => "auto_pitch",
            Pass::AutoDynamics => "auto_dynamics",
            Pass::Breaths => "breaths",
            Pass::SectionColor => "section_color",
            Pass::MatchReference => "match_reference",
            Pass::Humanize => "humanize",
        }
    }

    /// JSON Schema of the pass's `params`.
    pub fn params_schema(self) -> Schema {
        use crate::schema::schema_for;
        match self {
            Pass::AutoPitch => schema_for::<AutoPitchParams>(),
            Pass::AutoDynamics => schema_for::<AutoDynamicsParams>(),
            Pass::Breaths => schema_for::<BreathsParams>(),
            Pass::SectionColor => schema_for::<SectionColorParams>(),
            Pass::MatchReference => schema_for::<MatchReferenceParams>(),
            Pass::Humanize => schema_for::<HumanizeParams>(),
        }
    }
}

impl fmt::Display for Pass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A tuning request: `{pass, track, range?, params}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TuneRequest {
    /// Which pass.
    pub pass: Pass,
    /// The track to tune.
    pub track: TrackId,
    /// `[first, last)` tick; notes starting inside are tuned. Omitted =
    /// the whole track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(Tick, Tick)>,
    /// The pass's parameters (see [`Pass::params_schema`]); omitted fields
    /// take their defaults.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Data the engine supplies to passes that need it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TuneContext {
    /// The reference vocal's F0, for `match_reference`.
    pub reference_f0: Option<ReferenceF0>,
}

/// An F0 track in project time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReferenceF0 {
    /// Project time of the first frame in seconds (tick 0 = 0 s; the
    /// engine has already applied `reference.offset_ms`).
    pub start_sec: f64,
    /// Frame spacing in seconds.
    pub step_sec: f64,
    /// Pitch in cents (MIDI note × 100) per frame; `None` = unvoiced.
    pub cents: Vec<Option<f32>>,
}

impl ReferenceF0 {
    /// Pitch at `sec`, interpolated between voiced frames; `None` when
    /// unvoiced or outside the track.
    pub fn at(&self, sec: f64) -> Option<f64> {
        let x = (sec - self.start_sec) / self.step_sec;
        if !x.is_finite() || x < -0.5 {
            return None;
        }
        let i = x.floor().max(0.0) as usize;
        let u = (x - i as f64).clamp(0.0, 1.0);
        let get = |k: usize| self.cents.get(k).copied().flatten().map(f64::from);
        match (get(i), get(i + 1)) {
            (Some(a), Some(b)) => Some(a + (b - a) * u),
            (Some(a), None) if u <= 0.5 => Some(a),
            (None, Some(b)) if u >= 0.5 => Some(b),
            _ => None,
        }
    }
}

/// What a pass would do.
#[derive(Clone, Debug, PartialEq)]
pub struct TuneResult {
    /// One labeled `batch`, or nothing when the pass changes nothing.
    pub ops: Vec<Op>,
    /// Notes about the run (skipped notes, missing data, ...).
    pub diagnostics: Vec<Diagnostic>,
    /// One line for humans and agents, e.g. "auto_pitch (pop): 24 notes".
    pub summary: String,
}

/// Runs a pass. Fails when the track is missing, the parameters are
/// invalid or required data (the reference F0) is absent.
pub fn run(
    project: &Project,
    req: &TuneRequest,
    ctx: &TuneContext,
) -> Result<TuneResult, Rejected> {
    let track = project.track(req.track).ok_or_else(|| {
        Rejected::from(Diagnostic::error(
            "track.not_found",
            format!("no {}", req.track),
        ))
    })?;
    let (lo, hi) = req.range.unwrap_or((0, Tick::MAX));
    if lo > hi {
        return Err(Diagnostic::error("tune.range", format!("range {lo}..{hi} is empty")).into());
    }
    let mut cx = Cx::new(project, track, lo, hi, req.pass);
    match req.pass {
        Pass::AutoPitch => expression::auto_pitch(&mut cx, &params(req)?)?,
        Pass::AutoDynamics => dynamics::auto_dynamics(&mut cx, &params(req)?)?,
        Pass::Breaths => dynamics::breaths(&mut cx, &params(req)?)?,
        Pass::SectionColor => dynamics::section_color(&mut cx, &params(req)?)?,
        Pass::MatchReference => reference::match_reference(&mut cx, &params(req)?, ctx)?,
        Pass::Humanize => reference::humanize(&mut cx, &params(req)?)?,
    }
    Ok(cx.finish())
}

fn params<T: DeserializeOwned>(req: &TuneRequest) -> Result<T, Rejected> {
    let v = match &req.params {
        serde_json::Value::Null => serde_json::Value::Object(Default::default()),
        v => v.clone(),
    };
    serde_json::from_value(v).map_err(|e| {
        Diagnostic::error("tune.params", format!("{}: invalid params: {e}", req.pass)).into()
    })
}

/// A pass in progress: the source project, the notes in scope, and a
/// scratch copy the emitted ops are applied to as they are made (so later
/// ops see earlier ones, and a no-op run is detected).
pub(crate) struct Cx<'a> {
    pub(crate) project: &'a Project,
    pub(crate) track: &'a Track,
    pub(crate) tempo: TempoMap,
    pub(crate) lo: Tick,
    pub(crate) hi: Tick,
    /// (start, end) of each note in seconds.
    pub(crate) secs: Vec<(f64, f64)>,
    pass: Pass,
    scratch: Project,
    ops: Vec<Op>,
    diags: Vec<Diagnostic>,
    pub(crate) summary: String,
}

impl<'a> Cx<'a> {
    fn new(project: &'a Project, track: &'a Track, lo: Tick, hi: Tick, pass: Pass) -> Self {
        let tempo = project.tempo_map();
        let secs = track
            .timed_notes(&tempo)
            .map(|t| (t.start_sec, t.end_sec))
            .collect();
        Self {
            project,
            track,
            tempo,
            lo,
            hi,
            secs,
            pass,
            scratch: project.clone(),
            ops: Vec::new(),
            diags: Vec::new(),
            summary: String::new(),
        }
    }

    pub(crate) fn notes(&self) -> &'a [Note] {
        &self.track.notes
    }

    /// Whether note `i` is in scope.
    pub(crate) fn in_range(&self, i: usize) -> bool {
        let s = self.track.notes[i].start;
        s >= self.lo && s < self.hi
    }

    /// Indices of sung (not rest/breath) notes in scope.
    pub(crate) fn sung_in_range(&self) -> Vec<usize> {
        (0..self.track.notes.len())
            .filter(|&i| self.in_range(i) && !self.track.notes[i].is_rest_or_breath())
            .collect()
    }

    pub(crate) fn dur_ms(&self, i: usize) -> f64 {
        (self.secs[i].1 - self.secs[i].0) * 1000.0
    }

    /// Whether note `i` glides from the previous note (both sung, gap
    /// below [`GLIDE_GAP_SEC`]).
    pub(crate) fn connected(&self, i: usize) -> bool {
        let n = &self.track.notes;
        i > 0
            && !n[i].is_rest_or_breath()
            && !n[i - 1].is_rest_or_breath()
            && self.secs[i].0 - self.secs[i - 1].1 < GLIDE_GAP_SEC
    }

    /// For each note: (first sung note of its phrase, last sung note of
    /// its phrase).
    pub(crate) fn phrase_edges(&self) -> Vec<(bool, bool)> {
        let notes = &self.track.notes;
        let mut out = vec![(false, false); notes.len()];
        for p in crate::phrase::phrases(self.track) {
            let sung: Vec<usize> = p.notes.filter(|&i| !notes[i].is_rest_or_breath()).collect();
            if let (Some(&a), Some(&b)) = (sung.first(), sung.last()) {
                out[a].0 = true;
                out[b].1 = true;
            }
        }
        out
    }

    /// The scratch project's copy of the track (with the ops so far).
    pub(crate) fn current(&self) -> &Track {
        self.scratch
            .track(self.track.id)
            .expect("the track exists in the scratch copy")
    }

    /// Emits an op, applying it to the scratch copy.
    pub(crate) fn push(&mut self, op: Op) -> Result<(), Rejected> {
        let r = apply(&mut self.scratch, &op)?;
        self.diags.extend(r.diagnostics);
        self.ops.push(op);
        Ok(())
    }

    pub(crate) fn note(&mut self, d: Diagnostic) {
        self.diags.push(d.on_track(self.track.id));
    }

    fn finish(self) -> TuneResult {
        let name = self.pass.name();
        let mut diagnostics = self.diags;
        let summary = if self.summary.is_empty() {
            name.to_string()
        } else {
            format!("{name}: {}", self.summary)
        };
        if self.ops.is_empty() || self.scratch == *self.project {
            diagnostics.push(Diagnostic::new(
                Level::Info,
                "tune.noop",
                format!("{name}: nothing to change"),
            ));
            return TuneResult {
                ops: Vec::new(),
                diagnostics,
                summary: format!("{name}: nothing to change"),
            };
        }
        TuneResult {
            ops: vec![Op::batch(name.replace('_', " "), self.ops)],
            diagnostics,
            summary,
        }
    }
}

/// Deterministic value in [-1, 1) from a seed and a salt.
pub(crate) fn hash_unit(seed: u64, salt: u64) -> f64 {
    let h = crate::mix64(seed ^ crate::mix64(salt));
    (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// Piecewise-linear lookup in `(x, y)` knots sorted by x, clamped at the
/// ends.
pub(crate) fn interp(knots: &[(f64, f64)], x: f64) -> f64 {
    let i = knots.partition_point(|k| k.0 <= x);
    match (i.checked_sub(1).map(|j| knots[j]), knots.get(i)) {
        (Some(a), Some(b)) => a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0),
        (Some(a), None) => a.1,
        (None, Some(b)) => b.1,
        (None, None) => 0.0,
    }
}

/// Rounds to `1/k`.
pub(crate) fn round(x: f64, k: f64) -> f64 {
    (x * k).round() / k
}

/// Ramer–Douglas–Peucker on curve points by vertical error: keeps the
/// fewest points whose linear interpolation stays within `tol`.
pub(crate) fn simplify(pts: &[(Tick, f64)], tol: f64) -> Vec<(Tick, f64)> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let ((x0, y0), (x1, y1)) = (pts[a], pts[b]);
        let mut worst = (0.0, a);
        for (k, &(x, y)) in pts.iter().enumerate().take(b).skip(a + 1) {
            let line = y0 + (y1 - y0) * (x - x0) as f64 / (x1 - x0) as f64;
            let e = (y - line).abs();
            if e > worst.0 {
                worst = (e, k);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}
