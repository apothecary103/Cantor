//! `auto_dynamics`, `breaths` and `section_color` (docs/tuning.md §2
//! "Dynamics", "Timing and phrasing" and "Timbre by section").

use super::{Cx, interp, round};
use crate::diag::{Diagnostic, Rejected};
use crate::ids::{NoteId, PPQ, Tick};
use crate::model::{Interp, Note};
use crate::op::{NotePatch, Op};
use crate::params::{ParamId, known};
use crate::phrase::phrases;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parameters of `auto_dynamics`. It owns the `dyn` curve over the phrases
/// in range and the `vel`/`vol` of their sung notes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AutoDynamicsParams {
    /// Scales every deviation from neutral, 0..=2: the `dyn` arc
    /// (−1.5 dB at the phrase start, +3 dB at its peak note, −4 dB at its
    /// end), velocity by note density (≥ 8 notes/s → up to 150, ≤ 2
    /// notes/s → 85) and downbeat accents (`vol` 112 on bar starts, 106 on
    /// half bars, 94 off the beat). Default 1.
    pub amount: f64,
}

impl Default for AutoDynamicsParams {
    fn default() -> Self {
        Self { amount: 1.0 }
    }
}

/// Velocity by onset rate in notes per second.
const VEL_BY_RATE: &[(f64, f64)] = &[
    (0.0, 80.0),
    (2.0, 85.0),
    (3.0, 100.0),
    (6.0, 100.0),
    (8.0, 125.0),
    (12.0, 150.0),
];

pub(super) fn auto_dynamics(cx: &mut Cx<'_>, p: &AutoDynamicsParams) -> Result<(), Rejected> {
    let a = p.amount.clamp(0.0, 2.0);
    let notes = cx.notes();
    let mut pts: Vec<(Tick, f64)> = Vec::new();
    let (mut lo, mut hi) = (Tick::MAX, Tick::MIN);
    let mut by_patch: BTreeMap<(u64, u64), Vec<NoteId>> = BTreeMap::new();
    let mut n_phrases = 0;
    for ph in phrases(cx.track) {
        let sung: Vec<usize> = ph
            .notes
            .clone()
            .filter(|&i| !notes[i].is_rest_or_breath())
            .collect();
        let (Some(&f), Some(&l)) = (sung.first(), sung.last()) else {
            continue;
        };
        if !cx.in_range(f) {
            continue;
        }
        n_phrases += 1;
        lo = lo.min(notes[f].start);
        hi = hi.max(notes[l].end() - 1);
        // The peak: the highest note, the longest among equals.
        let peak = *sung
            .iter()
            .max_by_key(|&&i| (notes[i].key, notes[i].length, std::cmp::Reverse(i)))
            .expect("non-empty");
        let mut arc = vec![(notes[f].start, if peak == f { a } else { -1.5 * a })];
        if peak != f && peak != l {
            arc.push((notes[peak].start + notes[peak].length / 2, 3.0 * a));
        } else if peak == l && l != f {
            arc.push((notes[l].start, 2.5 * a));
        }
        arc.push((notes[l].end() - 1, -4.0 * a));
        for q in arc {
            if pts.last().is_none_or(|last| q.0 > last.0) {
                pts.push((q.0, round(q.1.clamp(-24.0, 12.0), 100.0)));
            }
        }
        for (k, &i) in sung.iter().enumerate() {
            let n = &notes[i];
            if n.is_extension() {
                continue;
            }
            let iois: Vec<f64> = [
                k.checked_sub(1).map(|j| (sung[j], i)),
                sung.get(k + 1).map(|&j| (i, j)),
            ]
            .into_iter()
            .flatten()
            .map(|(x, y)| cx.secs[y].0 - cx.secs[x].0)
            .filter(|d| *d > 0.0)
            .collect();
            let ioi = if iois.is_empty() {
                cx.dur_ms(i) / 1000.0
            } else {
                iois.iter().sum::<f64>() / iois.len() as f64
            };
            let vel = 100.0 + (interp(VEL_BY_RATE, 1.0 / ioi.max(1e-3)) - 100.0) * a;
            let vol = 100.0 + (accent(cx, n) - 100.0) * a;
            let (vel, vol) = (
                round(vel.clamp(0.0, 200.0), 1.0),
                round(vol.clamp(0.0, 200.0), 1.0),
            );
            if (n.vel, n.vol) != (vel, vol) {
                by_patch
                    .entry((vel.to_bits(), vol.to_bits()))
                    .or_default()
                    .push(n.id);
            }
        }
    }
    if !pts.is_empty() {
        cx.push(Op::SetCurve {
            track: cx.track.id,
            param: ParamId::new(known::DYN),
            range: (lo, hi),
            points: pts,
            interp: Some(Interp::Smooth),
        })?;
    }
    let mut changed = 0;
    for ((vel, vol), ids) in by_patch {
        changed += ids.len();
        cx.push(Op::UpdateNotes {
            track: cx.track.id,
            ids,
            patch: NotePatch {
                vel: Some(f64::from_bits(vel)),
                vol: Some(f64::from_bits(vol)),
                ..NotePatch::default()
            },
        })?;
    }
    cx.summary = format!("{n_phrases} phrase arcs on dyn, vel/vol set on {changed} notes");
    Ok(())
}

/// `vol` by metric position: bar start 112, half bar 106, beat 100,
/// off-beat 94.
fn accent(cx: &Cx<'_>, n: &Note) -> f64 {
    let sig = cx.project.time_sig_map();
    let (bar, off) = sig.tick_to_bar(n.start);
    let bar_len = sig.bar_to_tick(bar + 1) - sig.bar_to_tick(bar);
    let beat = Tick::from(PPQ);
    if off == 0 {
        112.0
    } else if bar_len % 2 == 0 && off == bar_len / 2 {
        106.0
    } else if off % beat == 0 {
        100.0
    } else {
        94.0
    }
}

/// Parameters of `breaths`. Adds a `br` note in the rest before each
/// phrase, ending at the phrase's first note, when the rest is long
/// enough. Never moves or shortens notes; phrases already preceded by a
/// rest or breath note are skipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct BreathsParams {
    /// Smallest rest, in ms, that gets a breath. Default 300.
    pub min_gap_ms: f64,
    /// Breath `vol` in %, 0..=200. Default 60.
    pub vol: f64,
    /// Longest breath in ms. Default 350.
    pub max_ms: f64,
    /// Shortest breath in ms; a breath would take 60% of the rest and is
    /// skipped when that is shorter. Default 150.
    pub min_ms: f64,
}

impl Default for BreathsParams {
    fn default() -> Self {
        Self {
            min_gap_ms: 300.0,
            vol: 60.0,
            max_ms: 350.0,
            min_ms: 150.0,
        }
    }
}

pub(super) fn breaths(cx: &mut Cx<'_>, p: &BreathsParams) -> Result<(), Rejected> {
    let notes = cx.notes();
    let mut add = Vec::new();
    for i in cx.sung_in_range() {
        let n = &notes[i];
        let (gap_tick, gap_sec) = match i.checked_sub(1).map(|j| &notes[j]) {
            Some(prev) if prev.is_rest_or_breath() => continue,
            Some(prev) => (prev.end(), cx.secs[i - 1].1),
            None => (0, 0.0),
        };
        let gap_ms = (cx.secs[i].0 - gap_sec) * 1000.0;
        if gap_ms < p.min_gap_ms {
            continue;
        }
        let len_ms = (0.6 * gap_ms).min(p.max_ms);
        if len_ms < p.min_ms {
            continue;
        }
        let start = cx
            .tempo
            .sec_to_tick(cx.secs[i].0 - len_ms / 1000.0)
            .max(gap_tick);
        if start >= n.start {
            continue;
        }
        add.push(Note {
            vol: p.vol.clamp(0.0, 200.0),
            ..Note::new(start, n.start - start, n.key, "br")
        });
    }
    cx.summary = format!("{} breaths added", add.len());
    if !add.is_empty() {
        cx.push(Op::AddNotes {
            track: cx.track.id,
            notes: add,
        })?;
    }
    Ok(())
}

/// A section's character, with docs/tuning.md's timbre table values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// bre 20, bri −5, gen 0, dyn 0.
    Verse,
    /// bre 10, bri 5, gen 0, dyn 1.
    PreChorus,
    /// bre 5, bri 20, gen 0, dyn 2.5.
    Chorus,
    /// bre 30, bri −15, gen 3, dyn −2.
    Bridge,
    /// Same as bridge: bre 30, bri −15, gen 3, dyn −2.
    Soft,
}

impl Preset {
    /// (bre, bri, gen, dyn).
    fn values(self) -> [f64; 4] {
        match self {
            Preset::Verse => [20.0, -5.0, 0.0, 0.0],
            Preset::PreChorus => [10.0, 5.0, 0.0, 1.0],
            Preset::Chorus => [5.0, 20.0, 0.0, 2.5],
            Preset::Bridge | Preset::Soft => [30.0, -15.0, 3.0, -2.0],
        }
    }

    /// Guesses the preset from a section name ("Chorus 2", "pre-chorus").
    fn from_name(name: &str) -> Option<Self> {
        let n = name.to_lowercase();
        Some(if n.contains("pre") && n.contains("chorus") {
            Preset::PreChorus
        } else if n.contains("chorus") || n.contains("hook") {
            Preset::Chorus
        } else if n.contains("verse") {
            Preset::Verse
        } else if n.contains("bridge") {
            Preset::Bridge
        } else if ["intro", "outro", "soft"].iter().any(|k| n.contains(k)) {
            Preset::Soft
        } else {
            return None;
        })
    }
}

/// Parameters of `section_color`: constant `bre`/`bri`/`gen`/`dyn` over a
/// section, with the values just outside it kept as they were. Values
/// given explicitly override the preset; with neither, the preset is
/// guessed from the section name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SectionColorParams {
    /// Section name; matches every section whose name equals it or starts
    /// with it, ignoring case ("verse" matches "Verse 1" and "Verse 2").
    /// Omitted = the request's range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Starting values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<Preset>,
    /// Breathiness, 0..=100 %.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bre: Option<f64>,
    /// Brightness, -100..=100 %.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bri: Option<f64>,
    /// Gender, -100..=100 %.
    #[serde(rename = "gen", skip_serializing_if = "Option::is_none")]
    pub gen_: Option<f64>,
    /// Dynamics, -24..=12 dB. Note that this replaces `auto_dynamics`'s
    /// arcs in the section.
    #[serde(rename = "dyn", skip_serializing_if = "Option::is_none")]
    pub dyn_: Option<f64>,
}

pub(super) fn section_color(cx: &mut Cx<'_>, p: &SectionColorParams) -> Result<(), Rejected> {
    let spans: Vec<(String, Tick, Tick)> = match &p.section {
        Some(name) => {
            let want = name.to_lowercase();
            let found: Vec<_> = cx
                .project
                .sections
                .iter()
                .filter(|s| s.name.to_lowercase().starts_with(&want))
                .map(|s| (s.name.clone(), s.start.max(cx.lo), s.end.min(cx.hi)))
                .filter(|s| s.1 < s.2)
                .collect();
            if found.is_empty() {
                let known: Vec<_> = cx
                    .project
                    .sections
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect();
                return Err(Diagnostic::error(
                    "tune.section",
                    format!(
                        "no section `{name}` in range (sections: {})",
                        known.join(", ")
                    ),
                )
                .into());
            }
            found
        }
        None if cx.hi < Tick::MAX => vec![("range".into(), cx.lo, cx.hi)],
        None => {
            return Err(Diagnostic::error(
                "tune.params",
                "section_color needs `section` or a request `range`",
            )
            .into());
        }
    };
    let mut colored = Vec::new();
    for (name, start, end) in spans {
        let preset = p.preset.or_else(|| Preset::from_name(&name));
        let base = preset.map(Preset::values);
        let given = [p.bre, p.bri, p.gen_, p.dyn_];
        if base.is_none() && given.iter().all(Option::is_none) {
            return Err(Diagnostic::error(
                "tune.params",
                format!("section_color: give bre/bri/gen/dyn or a preset for `{name}`"),
            )
            .into());
        }
        let ids = [known::BRE, known::BRI, known::GEN, known::DYN];
        for (k, id) in ids.into_iter().enumerate() {
            let Some(v) = given[k].or(base.map(|b| b[k])) else {
                continue;
            };
            let param = ParamId::new(id);
            let d = param.descriptor().expect("known curve");
            let v = v.clamp(d.min, d.max);
            let old = |t: Tick| {
                cx.current()
                    .curves
                    .get(id)
                    .and_then(|c| c.value_at(t))
                    .unwrap_or(d.default)
            };
            let (before, after) = (old(start - 1), old(end));
            let last = end - 1;
            let mut points = vec![(start, v)];
            if last > start {
                points.push((last, v));
            }
            // Anchors just outside the span keep the old values there, so
            // the new value does not hold beyond the section.
            let mut ops = Vec::new();
            if start > 0 {
                ops.push(curve_op(cx, &param, start - 1, vec![(start - 1, before)]));
            }
            ops.push(curve_op(cx, &param, start, points));
            ops.push(curve_op(cx, &param, end, vec![(end, after)]));
            for op in ops {
                cx.push(op)?;
            }
        }
        colored.push(format!(
            "{name} ({})",
            preset.map_or("custom".into(), |p| format!("{p:?}").to_lowercase())
        ));
    }
    cx.summary = format!("colored {}", colored.join(", "));
    Ok(())
}

fn curve_op(cx: &Cx<'_>, param: &ParamId, at: Tick, points: Vec<(Tick, f64)>) -> Op {
    let hi = points.last().map_or(at, |p| p.0);
    Op::SetCurve {
        track: cx.track.id,
        param: param.clone(),
        range: (at, hi),
        points,
        interp: None,
    }
}
