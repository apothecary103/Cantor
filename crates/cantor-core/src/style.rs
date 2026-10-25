//! Text style prompts mapped onto layered parameters (M7).
//!
//! A prompt such as "breathier and more intimate, slightly less vibrato"
//! becomes a [`StylePlan`]: signed adjustments to curve parameters (added
//! to the user layer over a range, with short ramps at the edges) and to
//! note expression (vibrato depth, portamento length). The mapping is a
//! small deterministic lexicon, so the same prompt always gives the same
//! edits and an agent can read exactly what a word did. Words the lexicon
//! does not know are returned, not ignored.

use crate::diag::{Diagnostic, EntityRef};
use crate::ids::{Tick, TrackId};
use crate::model::{Interp, Portamento, Project, TickRange, Vibrato};
use crate::op::{NoteExprPatch, Op};
use crate::params::{ParamId, known};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one adjustment changes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum StyleTarget {
    /// A curve parameter; the amount is in the parameter's unit and is
    /// added to the user layer.
    Param(String),
    /// Vibrato depth in cents (added; vibrato is created when missing).
    VibratoDepth,
    /// Portamento length in milliseconds (added to the automatic length).
    PortamentoMs,
}

/// One adjustment and the words that asked for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StyleAdjustment {
    /// What changes.
    pub target: StyleTarget,
    /// By how much (signed, in the target's unit).
    pub amount: f64,
    /// The prompt words that produced it.
    pub because: Vec<String>,
}

/// A prompt's interpretation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StylePlan {
    /// Adjustments, one per target, amounts summed over the prompt.
    pub adjustments: Vec<StyleAdjustment>,
    /// Words that carried no meaning the lexicon knows.
    pub unrecognized: Vec<String>,
}

/// Word stems and the effects they have.
type Entry = (&'static [&'static str], &'static [(Effect, f64)]);

/// (word stems, effects). Amounts are for the plain word; modifiers scale
/// them, negations flip them.
const LEXICON: &[Entry] = &[
    (
        &["breath", "airy", "air"],
        &[(Effect::P(known::BREATHINESS), 6.0)],
    ),
    (
        &["whisper"],
        &[
            (Effect::P(known::BREATHINESS), 12.0),
            (Effect::P(known::VOICING), -6.0),
        ],
    ),
    (
        &["intimate", "soft", "gentle", "tender", "delicate", "hush"],
        &[
            (Effect::P(known::ENERGY), -4.0),
            (Effect::P(known::TENSION), -1.5),
            (Effect::P(known::BREATHINESS), 3.0),
        ],
    ),
    (
        &["power", "strong", "belt", "forceful", "intense", "driv"],
        &[
            (Effect::P(known::ENERGY), 4.0),
            (Effect::P(known::TENSION), 2.0),
            (Effect::P(known::BREATHINESS), -3.0),
        ],
    ),
    (&["loud"], &[(Effect::P(known::ENERGY), 6.0)]),
    (&["quiet"], &[(Effect::P(known::ENERGY), -6.0)]),
    (
        &["tense", "strain", "press", "edg"],
        &[(Effect::P(known::TENSION), 2.5)],
    ),
    (
        &["relax", "loose", "easy", "calm"],
        &[(Effect::P(known::TENSION), -2.0)],
    ),
    (
        &["bright", "young", "light", "child"],
        &[(Effect::P(known::GENDER), 1.5)],
    ),
    (
        &["dark", "warm", "deep", "mature", "rich"],
        &[(Effect::P(known::GENDER), -1.5)],
    ),
    (&["vibrato", "wobbl", "shimmer"], &[(Effect::Vibrato, 25.0)]),
    (&["straight", "steady", "flat"], &[(Effect::Vibrato, -25.0)]),
    (
        &["smooth", "legato", "glid", "slur", "flow"],
        &[(Effect::Portamento, 60.0)],
    ),
    (
        &["crisp", "staccato", "punch", "tight"],
        &[(Effect::Portamento, -40.0)],
    ),
];

/// Intensity words and their multipliers.
const INTENSITY: &[(&str, f64)] = &[
    ("slightly", 0.5),
    ("bit", 0.5),
    ("little", 0.5),
    ("somewhat", 0.5),
    ("touch", 0.5),
    ("very", 1.5),
    ("much", 1.5),
    ("really", 1.5),
    ("lot", 1.5),
    ("extremely", 2.0),
    ("super", 2.0),
];

/// Words that flip the next effect.
const NEGATION: &[&str] = &["less", "not", "no", "without", "fewer"];

/// Words with no effect of their own.
const FILLER: &[&str] = &[
    "and", "but", "more", "make", "it", "the", "a", "an", "sound", "sounding", "with", "voice",
    "vocal", "vocals", "singing", "sing", "please", "this", "that", "part", "phrase", "chorus",
    "verse", "bridge", "some", "of", "in", "on", "to", "be", "feel", "tone", "style", "way",
    "while", "keep", "keeping", "add", "use", "less",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Effect {
    P(&'static str),
    Vibrato,
    Portamento,
}

impl Effect {
    fn target(self) -> StyleTarget {
        match self {
            Effect::P(p) => StyleTarget::Param(p.to_owned()),
            Effect::Vibrato => StyleTarget::VibratoDepth,
            Effect::Portamento => StyleTarget::PortamentoMs,
        }
    }
}

fn lookup(word: &str) -> Option<&'static [(Effect, f64)]> {
    LEXICON
        .iter()
        .find(|(stems, _)| stems.iter().any(|s| word.starts_with(s)))
        .map(|(_, e)| *e)
}

/// Interprets a prompt.
pub fn interpret(prompt: &str) -> StylePlan {
    let mut sums: BTreeMap<StyleTarget, (f64, Vec<String>)> = BTreeMap::new();
    let mut unrecognized = Vec::new();
    for clause in prompt
        .to_lowercase()
        .split([',', ';', '.', '!', '\n'])
        .flat_map(|c| c.split(" but "))
    {
        let mut scale = 1.0;
        let mut negate = false;
        for raw in clause.split(|c: char| !c.is_alphanumeric() && c != '-') {
            let word = raw.trim_matches('-');
            if word.is_empty() {
                continue;
            }
            if let Some(&(_, m)) = INTENSITY.iter().find(|(w, _)| *w == word) {
                scale *= m;
                continue;
            }
            if NEGATION.contains(&word) {
                negate = true;
                continue;
            }
            match lookup(word) {
                Some(effects) => {
                    let sign = if negate { -1.0 } else { 1.0 };
                    for &(e, amount) in effects {
                        let entry = sums.entry(e.target()).or_default();
                        entry.0 += sign * scale * amount;
                        entry.1.push(word.to_owned());
                    }
                    scale = 1.0;
                    negate = false;
                }
                None if FILLER.contains(&word) => {}
                None => unrecognized.push(word.to_owned()),
            }
        }
    }
    StylePlan {
        adjustments: sums
            .into_iter()
            .filter(|(_, (a, _))| *a != 0.0)
            .map(|(target, (amount, mut because))| {
                because.dedup();
                StyleAdjustment {
                    target,
                    amount,
                    because,
                }
            })
            .collect(),
        unrecognized,
    }
}

/// Ramp length at each edge of a curve adjustment.
const RAMP: Tick = 240;

/// The ops that apply a plan to the notes and curves of `track` within
/// `range`, as one batch. Curve offsets are added to the existing user
/// layer (clamped to the parameter's limits) with ramps at the edges;
/// unknown parameters, an empty range or no notes become diagnostics.
pub fn ops(
    p: &Project,
    track: TrackId,
    range: TickRange,
    prompt: &str,
    plan: &StylePlan,
) -> (Option<Op>, Vec<Diagnostic>) {
    let mut diags: Vec<Diagnostic> = plan
        .unrecognized
        .iter()
        .map(|w| {
            Diagnostic::warning(
                "style.unrecognized_word",
                format!("`{w}` is not in the style lexicon and was ignored"),
                Some(EntityRef::Track { id: track }),
            )
        })
        .collect();
    let Some(t) = p.track(track) else {
        diags.push(Diagnostic::error(
            "style.no_track",
            format!("{track} does not exist"),
            None,
        ));
        return (None, diags);
    };
    if range.end <= range.start {
        diags.push(Diagnostic::error(
            "style.empty_range",
            "the range is empty",
            Some(EntityRef::Track { id: track }),
        ));
        return (None, diags);
    }
    let notes: Vec<_> = t
        .parts
        .iter()
        .flat_map(|part| part.notes.iter().map(move |n| (part.start + n.start, n)))
        .filter(|(s, n)| *s < range.end && s + n.length > range.start)
        .collect();
    let mut out = Vec::new();
    for adj in &plan.adjustments {
        match &adj.target {
            StyleTarget::Param(id) => {
                let param = ParamId::new(id.clone());
                let Some(desc) = param.descriptor() else {
                    continue;
                };
                let existing = t.curves.get(&param);
                let user = existing.map(|c| &c.user_delta);
                let interp = user
                    .filter(|c| !c.points.is_empty())
                    .map_or(Interp::Linear, |c| c.interp);
                let base = |tick: Tick| user.and_then(|c| c.value_in_span(tick)).unwrap_or(0.0);
                let ramp = RAMP.min((range.end - range.start) / 4).max(1);
                let mut ticks: Vec<Tick> = vec![
                    range.start,
                    range.start + ramp,
                    range.end - 1 - ramp,
                    range.end - 1,
                ];
                if let Some(c) = user {
                    ticks.extend(c.points.iter().map(|p| p.0).filter(|&x| range.contains(x)));
                }
                ticks.sort_unstable();
                ticks.dedup();
                let limit = desc.max_delta();
                let points = ticks
                    .into_iter()
                    .map(|x| {
                        let w = if x < range.start + ramp {
                            (x - range.start) as f64 / ramp as f64
                        } else if x > range.end - 1 - ramp {
                            (range.end - 1 - x) as f64 / ramp as f64
                        } else {
                            1.0
                        };
                        (x, (base(x) + w * adj.amount).clamp(-limit, limit))
                    })
                    .collect();
                out.push(Op::SetCurve {
                    track,
                    param,
                    range,
                    points,
                    interp,
                });
            }
            StyleTarget::VibratoDepth => {
                for (_, n) in &notes {
                    let current = n.expr.vibrato.clone();
                    let depth = current.as_ref().map_or(0.0, |v| v.depth_cents) + adj.amount;
                    let vibrato = (depth > 0.5).then(|| Vibrato {
                        depth_cents: depth.min(200.0),
                        ..current.unwrap_or(Vibrato {
                            depth_cents: 0.0,
                            rate_hz: 5.5,
                            start: 0.35,
                            fade_in: 0.3,
                            fade_out: 0.1,
                            phase: 0.0,
                            drift: 0.5,
                        })
                    });
                    out.push(Op::SetNoteExpr {
                        id: n.id,
                        expr: NoteExprPatch {
                            vibrato: Some(vibrato),
                            ..NoteExprPatch::default()
                        },
                    });
                }
            }
            StyleTarget::PortamentoMs => {
                for (_, n) in &notes {
                    let pm = &n.expr.portamento;
                    let len = (pm.length_ms.unwrap_or(80.0) + adj.amount).clamp(10.0, 500.0);
                    out.push(Op::SetNoteExpr {
                        id: n.id,
                        expr: NoteExprPatch {
                            portamento: Some(Portamento {
                                length_ms: Some(len),
                                ..pm.clone()
                            }),
                            ..NoteExprPatch::default()
                        },
                    });
                }
            }
        }
    }
    let note_targets = plan
        .adjustments
        .iter()
        .any(|a| !matches!(a.target, StyleTarget::Param(_)));
    if note_targets && notes.is_empty() {
        diags.push(Diagnostic::warning(
            "style.no_notes",
            "no notes in the range; vibrato and portamento changes have nothing to act on",
            Some(EntityRef::Track { id: track }),
        ));
    }
    if out.is_empty() {
        diags.push(Diagnostic::warning(
            "style.no_effect",
            "the prompt changes nothing",
            Some(EntityRef::Track { id: track }),
        ));
        return (None, diags);
    }
    let op = Op::Batch {
        label: format!("Style: {prompt}"),
        ops: out,
    };
    (Some(op), diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amount(plan: &StylePlan, target: StyleTarget) -> f64 {
        plan.adjustments
            .iter()
            .find(|a| a.target == target)
            .map_or(0.0, |a| a.amount)
    }

    fn p(id: &str) -> StyleTarget {
        StyleTarget::Param(id.into())
    }

    #[test]
    fn words_modifiers_and_negation() {
        let plan = interpret("breathier and more intimate, slightly less vibrato");
        assert_eq!(amount(&plan, p("breathiness")), 6.0 + 3.0);
        assert_eq!(amount(&plan, p("energy")), -4.0);
        assert_eq!(amount(&plan, StyleTarget::VibratoDepth), -12.5);
        assert!(plan.unrecognized.is_empty(), "{:?}", plan.unrecognized);

        let plan = interpret("very powerful but not tense");
        assert_eq!(amount(&plan, p("energy")), 6.0);
        assert_eq!(amount(&plan, p("tension")), 3.0 - 2.5);

        let plan = interpret("darker, smoother, sparkly");
        assert_eq!(amount(&plan, p("gender")), -1.5);
        assert_eq!(amount(&plan, StyleTarget::PortamentoMs), 60.0);
        assert_eq!(plan.unrecognized, ["sparkly"]);
    }

    #[test]
    fn same_prompt_same_plan() {
        let a = interpret("Warm, gentle, a little breathy!");
        assert_eq!(a, interpret("Warm, gentle, a little breathy!"));
    }

    #[test]
    fn ops_apply_atomically_and_undo_exactly() {
        use crate::document::Document;
        use crate::ids::{NoteId, PartId};
        use crate::model::*;
        let mut doc = Document::new(Project::new());
        let notes = (0..2)
            .map(|i| Note {
                id: NoteId(10 + i),
                start: i as i64 * 1920,
                length: 1920,
                pitch_cents: 6000.0,
                lyric: "la".into(),
                lang: None,
                phonemes: None,
                expr: NoteExpr::default(),
                modes: VocalModes::default(),
            })
            .collect();
        let r = doc.apply(Op::AddTrack {
            index: None,
            track: Track {
                id: TrackId(1),
                name: "t".into(),
                singer: SingerRef::default(),
                backend: BackendRef::default(),
                mix: Mix::default(),
                effects: vec![],
                doubling: Doubling::default(),
                voice_mix: vec![],
                curves: Default::default(),
                parts: vec![Part {
                    id: PartId(2),
                    name: "p".into(),
                    start: 0,
                    length: 3840,
                    notes,
                }],
            },
        });
        assert!(r.applied);
        let before = doc.project().clone();
        let range = TickRange {
            start: 0,
            end: 3840,
        };
        let prompt = "breathier, smoother, more vibrato";
        let plan = interpret(prompt);
        let (op, diags) = ops(doc.project(), TrackId(1), range, prompt, &plan);
        assert!(diags.is_empty(), "{diags:?}");
        let r = doc.apply(op.unwrap());
        assert!(r.applied, "{:?}", r.diagnostics);
        let t = doc.project().track(TrackId(1)).unwrap();
        let b = &t.curves[&ParamId::new("breathiness")].user_delta;
        assert_eq!(b.value_at(1920), Some(6.0), "full amount mid-range");
        assert_eq!(b.value_at(0), Some(0.0), "ramped at the edge");
        for n in &t.parts[0].notes {
            assert_eq!(n.expr.vibrato.as_ref().unwrap().depth_cents, 25.0);
            assert_eq!(n.expr.portamento.length_ms, Some(140.0));
        }
        doc.undo();
        assert_eq!(doc.project(), &before);
    }
}
