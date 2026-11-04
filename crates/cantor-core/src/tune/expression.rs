//! `auto_pitch`: note pitch expression by interval, tempo and phrase
//! position (docs/tuning.md §2 "Pitch transitions" and "Vibrato").

use super::{Cx, hash_unit, interp, round};
use crate::diag::Rejected;
use crate::model::{Bend, Note, Portamento, Shape, Vibrato};
use crate::op::{NotePatch, Op, PitchPatch, PortamentoPatch, VibratoPatch};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A singing style for `auto_pitch`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// Moderate glides, gentle scoops and falls, medium vibrato.
    #[default]
    Natural,
    /// Quicker glides, more overshoot, clear scoops and falls.
    Pop,
    /// Slow glides, soft falls, slower and deeper vibrato.
    Ballad,
    /// Short glides, big overshoot, deep scoops and falls, short vibrato.
    Rock,
    /// Hard steps, no bends, no vibrato.
    Robotic,
}

/// Parameters of `auto_pitch`. It owns (overwrites) each sung note's
/// portamento, scoop, fall and vibrato; bend points are left alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AutoPitchParams {
    /// Singing style. Default `natural`.
    pub style: Style,
    /// Add vibrato to long notes (≥ 350–450 ms by style) and remove it
    /// from short ones. `false` leaves vibratos as they are (except
    /// `robotic`, which always removes them). Default true.
    pub vibrato: bool,
}

impl Default for AutoPitchParams {
    fn default() -> Self {
        Self {
            style: Style::Natural,
            vibrato: true,
        }
    }
}

struct Profile {
    /// Portamento length factor; 0 = hard steps.
    glide: f64,
    overshoot: f64,
    prep: f64,
    /// (cents, ms) on phrase starts.
    scoop: Option<(f64, f64)>,
    /// (cents, ms) on phrase ends.
    fall: Option<(f64, f64)>,
    vib: Option<VibProfile>,
}

struct VibProfile {
    depth: f64,
    period: f64,
    max_len: f64,
    fade_in: f64,
    min_ms: f64,
    drift: f64,
}

fn profile(style: Style) -> Profile {
    let vib = |depth, period, max_len, fade_in, min_ms, drift| {
        Some(VibProfile {
            depth,
            period,
            max_len,
            fade_in,
            min_ms,
            drift,
        })
    };
    match style {
        Style::Natural => Profile {
            glide: 1.0,
            overshoot: 1.0,
            prep: 1.0,
            scoop: Some((-80.0, 90.0)),
            fall: Some((-80.0, 150.0)),
            vib: vib(22.0, 180.0, 70.0, 35.0, 400.0, 0.3),
        },
        Style::Pop => Profile {
            glide: 0.85,
            overshoot: 1.15,
            prep: 1.0,
            scoop: Some((-120.0, 80.0)),
            fall: Some((-180.0, 150.0)),
            vib: vib(25.0, 170.0, 65.0, 30.0, 400.0, 0.25),
        },
        Style::Ballad => Profile {
            glide: 1.35,
            overshoot: 0.8,
            prep: 1.2,
            scoop: Some((-100.0, 120.0)),
            fall: Some((-60.0, 200.0)),
            vib: vib(32.0, 195.0, 75.0, 40.0, 350.0, 0.4),
        },
        Style::Rock => Profile {
            glide: 0.7,
            overshoot: 1.3,
            prep: 0.8,
            scoop: Some((-200.0, 100.0)),
            fall: Some((-300.0, 180.0)),
            vib: vib(20.0, 160.0, 45.0, 25.0, 450.0, 0.2),
        },
        Style::Robotic => Profile {
            glide: 0.0,
            overshoot: 0.0,
            prep: 0.0,
            scoop: None,
            fall: None,
            vib: None,
        },
    }
}

/// Glide length in ms by interval in semitones (tuning.md table).
const GLIDE_MS: &[(f64, f64)] = &[
    (0.0, 40.0),
    (2.0, 70.0),
    (3.0, 80.0),
    (5.0, 120.0),
    (7.0, 130.0),
    (12.0, 180.0),
];

pub(super) fn auto_pitch(cx: &mut Cx<'_>, p: &AutoPitchParams) -> Result<(), Rejected> {
    let prof = profile(p.style);
    let notes = cx.notes();
    let edges = cx.phrase_edges();
    let (mut changed, mut vibs, mut scoops, mut falls) = (0, 0, 0, 0);
    let mut vib_count = 0usize;
    let mut ops = Vec::new();
    for i in 0..notes.len() {
        let n = &notes[i];
        if n.is_rest_or_breath() {
            continue;
        }
        let dur = cx.dur_ms(i);
        // Count every vibrato candidate on the track so alternation does
        // not depend on the range.
        let vib_idx = prof.vib.as_ref().filter(|v| dur >= v.min_ms).map(|_| {
            vib_count += 1;
            vib_count
        });
        if !cx.in_range(i) {
            continue;
        }
        let (start, end) = edges[i];
        let portamento = portamento(cx, i, &prof);
        let scoop = prof
            .scoop
            .filter(|_| start && !n.is_extension() && dur >= 150.0)
            .map(|(cents, ms)| Bend {
                cents,
                length_ms: round(ms.min(0.4 * dur), 1.0),
            });
        let fall = prof
            .fall
            .filter(|_| end && dur >= 250.0)
            .map(|(cents, ms)| Bend {
                cents,
                length_ms: round(ms.min(0.4 * dur), 1.0),
            });
        let vibrato = match (&prof.vib, vib_idx) {
            _ if !p.vibrato && p.style != Style::Robotic => None,
            (Some(v), Some(k)) => Some(Some(vibrato(n, dur, v, k, end, fall))),
            _ => Some(None),
        };
        let patch = NotePatch {
            pitch: Some(PitchPatch {
                portamento: Some(PortamentoPatch {
                    length_ms: Some(portamento.length_ms),
                    start_ms: Some(portamento.start_ms),
                    overshoot_cents: Some(portamento.overshoot_cents),
                    preparation_cents: Some(portamento.preparation_cents),
                    shape: Some(portamento.shape),
                }),
                scoop: Some(scoop),
                fall: Some(fall),
                vibrato: vibrato.map(|v| v.map(|v| VibratoPatch::from_vibrato(&v))),
                points: None,
            }),
            ..NotePatch::default()
        };
        let mut new = n.clone();
        patch.apply_to(&mut new);
        vibs += usize::from(new.pitch.vibrato.is_some());
        scoops += usize::from(scoop.is_some());
        falls += usize::from(fall.is_some());
        if new != *n {
            changed += 1;
            ops.push(Op::UpdateNotes {
                track: cx.track.id,
                ids: vec![n.id],
                patch,
            });
        }
    }
    for op in ops {
        cx.push(op)?;
    }
    cx.summary = format!(
        "{:?} style, {changed} notes changed; {vibs} vibratos, {scoops} scoops, {falls} falls",
        p.style
    )
    .to_lowercase();
    Ok(())
}

fn portamento(cx: &Cx<'_>, i: usize, prof: &Profile) -> Portamento {
    if prof.glide == 0.0 {
        return Portamento {
            length_ms: 0.0,
            start_ms: 0.0,
            overshoot_cents: 0.0,
            preparation_cents: 0.0,
            shape: Shape::S,
        };
    }
    if !cx.connected(i) {
        return Portamento::default();
    }
    let notes = cx.notes();
    let iv = i32::from(notes[i].key) - i32::from(notes[i - 1].key);
    let a = f64::from(iv.unsigned_abs());
    let bpm = cx.tempo.bpm_at(notes[i].start);
    let tempo_factor = (120.0 / bpm).powf(0.3).clamp(0.85, 1.2);
    let cap = 0.25 * cx.dur_ms(i).min(cx.dur_ms(i - 1));
    let len = (interp(GLIDE_MS, a) * prof.glide * tempo_factor)
        .min(cap)
        .max(10.0);
    let (overshoot, prep) = if iv >= 3 {
        ((20.0 + 6.0 * (a - 3.0)).min(50.0) * prof.overshoot, 0.0)
    } else if iv <= -3 {
        let c = (10.0 + 4.0 * (a - 3.0)).min(30.0);
        (0.6 * c * prof.overshoot, c * prof.prep)
    } else {
        (0.0, 0.0)
    };
    let len = round(len, 1.0);
    Portamento {
        length_ms: len,
        start_ms: -round(len / 2.0, 1.0),
        overshoot_cents: round(overshoot, 1.0).min(200.0),
        preparation_cents: round(prep, 1.0).min(200.0),
        shape: Shape::S,
    }
}

/// Vibrato for a long note: onset 180–350 ms after the vowel, depth and
/// rate alternating between consecutive vibratos and varied by note id.
fn vibrato(
    n: &Note,
    dur: f64,
    v: &VibProfile,
    k: usize,
    phrase_end: bool,
    fall: Option<Bend>,
) -> Vibrato {
    let onset = (0.3 * dur).clamp(180.0, 350.0);
    let length = (100.0 * (1.0 - onset / dur)).clamp(40.0, v.max_len);
    let alt = if k % 2 == 0 { 1.0 } else { -1.0 };
    let id = n.id.0;
    let long = (0.85 + 0.15 * dur / 1000.0).clamp(0.85, 1.25);
    let climax = if phrase_end { 1.15 } else { 1.0 };
    let depth = v.depth * (1.0 + 0.12 * alt + 0.06 * hash_unit(id, 1)) * long * climax;
    let rate = 1000.0 / v.period - 0.25 * alt + 0.1 * hash_unit(id, 2);
    let vlen = dur * length / 100.0;
    let fade_out = match fall {
        Some(f) => (100.0 * f.length_ms / vlen).max(15.0),
        None => 15.0,
    }
    .min(100.0 - v.fade_in);
    Vibrato {
        length: round(length, 10.0),
        period_ms: round(1000.0 / rate, 10.0),
        depth_cents: round(depth, 10.0).min(200.0),
        fade_in: v.fade_in,
        fade_out: round(fade_out, 10.0),
        phase: round((hash_unit(id, 3) + 1.0) / 2.0, 1000.0).min(1.0),
        shift: 0.0,
        drift: v.drift,
    }
}
