//! Audio-to-voice conversion (M7): a sung recording becomes a project a
//! Cantor singer can sing — notes from the recording's pitch, its exact
//! contour as the pitch edit layer, its dynamics as the energy layer — so
//! the performance is re-sung in another voice (like VOCALO CHANGER).
//!
//! Converting someone's performance requires their consent: callers pass a
//! [`ConsentAttestation`], it is checked and recorded in the project's
//! provenance, and conversion is refused without it.

use cantor_analysis::F0Track;
use cantor_core::params::known;
use cantor_core::{
    BackendRef, Doubling, Interp, LayeredCurve, Meta, Mix, Note, NoteExpr, NoteId, PPQ, ParamId,
    Part, PartId, Portamento, Project, Provenance, SingerRef, Tick, Track, TrackId, VocalModes,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The performer's consent to converting their recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsentAttestation {
    /// Who sang the recording.
    pub performer: String,
    /// What they agreed to, in their words or the license's.
    pub statement: String,
    /// When (ISO 8601 recommended).
    pub date: String,
    /// The caller confirms the performer gave this consent.
    pub confirmed: bool,
}

impl ConsentAttestation {
    /// Why the attestation is not sufficient, if it is not.
    pub fn problem(&self) -> Option<&'static str> {
        if !self.confirmed {
            Some("the attestation is not confirmed")
        } else if self.performer.trim().is_empty() {
            Some("the attestation names no performer")
        } else if self.statement.trim().len() < 10 {
            Some("the attestation has no consent statement")
        } else if self.date.trim().is_empty() {
            Some("the attestation has no date")
        } else {
            None
        }
    }
}

/// Tempo of converted projects; notes keep the recording's timing exactly.
pub const TEMPO_BPM: f64 = 120.0;
/// Unvoiced gaps at least this long end a note.
const GAP_SECS: f64 = 0.06;
/// A pitch change this large, held this long, starts a new note.
const STEP_CENTS: f64 = 70.0;
const STEP_SECS: f64 = 0.08;
/// Shortest note kept.
const MIN_NOTE_SECS: f64 = 0.08;
/// Spacing of the pitch and energy curve points.
const CURVE_STEP_SECS: f64 = 0.02;

fn secs_to_tick(s: f64) -> Tick {
    (s * TEMPO_BPM / 60.0 * f64::from(PPQ)).round() as Tick
}

fn hz_to_cents(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

/// A detected note: start and end in seconds and its pitch in cents
/// (rounded to the nearest semitone).
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedNote {
    /// Onset in seconds.
    pub start: f64,
    /// End in seconds.
    pub end: f64,
    /// Pitch in cents, a whole semitone.
    pub cents: f64,
}

/// Segments an F0 track into notes.
pub fn detect_notes(f0: &F0Track) -> Vec<DetectedNote> {
    let n = f0.hz.len();
    // Cents per frame, median-filtered over 5 frames; None when unvoiced.
    let raw: Vec<Option<f64>> = (0..n)
        .map(|i| (f0.voiced[i] && f0.hz[i] > 0.0).then(|| hz_to_cents(f64::from(f0.hz[i]))))
        .collect();
    let cents: Vec<Option<f64>> = (0..n)
        .map(|i| {
            raw[i]?;
            let mut w: Vec<f64> = raw[i.saturating_sub(2)..(i + 3).min(n)]
                .iter()
                .flatten()
                .copied()
                .collect();
            w.sort_by(f64::total_cmp);
            Some(w[w.len() / 2])
        })
        .collect();
    let time = |i: usize| f0.t0 + i as f64 * f0.hop_secs;
    let gap_frames = (GAP_SECS / f0.hop_secs).ceil() as usize;
    let step_frames = (STEP_SECS / f0.hop_secs).ceil() as usize;

    let mut notes = Vec::new();
    let mut i = 0;
    while i < n {
        if cents[i].is_none() {
            i += 1;
            continue;
        }
        // A voiced run, allowing short unvoiced gaps (consonants).
        let start = i;
        let mut end = i;
        let mut j = i;
        while j < n {
            if cents[j].is_some() {
                end = j;
                j += 1;
            } else if (j..(j + gap_frames).min(n)).any(|k| cents[k].is_some()) {
                j += 1;
            } else {
                break;
            }
        }
        // Split the run where the pitch moves and stays moved.
        let mut seg_start = start;
        let mut k = start;
        while k <= end {
            let seg: Vec<f64> = (seg_start..=k).filter_map(|x| cents[x]).collect();
            let level = median(&seg);
            let held = (k..(k + step_frames).min(end + 1))
                .filter_map(|x| cents[x])
                .all(|c| (c - level).abs() > STEP_CENTS);
            if k > seg_start + step_frames && held {
                push_note(&mut notes, &cents, seg_start, k - 1, &time, f0.hop_secs);
                seg_start = k;
            }
            k += 1;
        }
        push_note(&mut notes, &cents, seg_start, end, &time, f0.hop_secs);
        i = j.max(end + 1);
    }
    notes
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[s.len() / 2]
}

fn push_note(
    out: &mut Vec<DetectedNote>,
    cents: &[Option<f64>],
    a: usize,
    b: usize,
    time: &dyn Fn(usize) -> f64,
    hop: f64,
) {
    let (start, end) = (time(a), time(b) + hop);
    if end - start < MIN_NOTE_SECS {
        return;
    }
    let seg: Vec<f64> = (a..=b).filter_map(|x| cents[x]).collect();
    out.push(DetectedNote {
        start,
        end,
        cents: (median(&seg) / 100.0).round() * 100.0,
    });
}

/// Builds the project: one track sung by `singer`, notes from `f0`, the
/// recording's contour as pitch edits and its loudness as energy edits.
/// `lyrics` are assigned to notes in order (`la` when they run out).
#[allow(clippy::too_many_arguments)]
pub fn build_project(
    f0: &F0Track,
    samples: &[f32],
    sample_rate: u32,
    singer: &str,
    backend: &str,
    lyrics: &[String],
    source: &str,
    consent: &ConsentAttestation,
) -> Project {
    let detected = detect_notes(f0);
    let notes: Vec<Note> = detected
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let start = secs_to_tick(d.start);
            Note {
                id: NoteId(1000 + i as u64),
                start,
                length: (secs_to_tick(d.end) - start).max(1),
                pitch_cents: d.cents,
                lyric: lyrics.get(i).cloned().unwrap_or_else(|| "la".into()),
                lang: None,
                phonemes: None,
                // The recording's own glides are in the pitch layer.
                expr: NoteExpr {
                    portamento: Portamento {
                        length_ms: Some(10.0),
                        ..Portamento::default()
                    },
                    ..NoteExpr::default()
                },
                modes: VocalModes::default(),
            }
        })
        .collect();

    // Pitch edits: recorded cents minus the note's pitch, inside notes.
    let mut pitch = Vec::new();
    let mut energy = Vec::new();
    let rms_db = |t: f64| -> Option<f64> {
        let a = (t * f64::from(sample_rate)) as usize;
        let b = ((t + CURVE_STEP_SECS) * f64::from(sample_rate)) as usize;
        let s = samples.get(a..b.min(samples.len()))?;
        if s.is_empty() {
            return None;
        }
        let p = s.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / s.len() as f64;
        Some(10.0 * p.max(1e-12).log10())
    };
    let mut levels = Vec::new();
    for d in &detected {
        let mut t = d.start;
        while t < d.end {
            if let Some(i) = f0.frame_at(t)
                && f0.voiced[i]
            {
                let c = hz_to_cents(f64::from(f0.hz[i]));
                pitch.push((secs_to_tick(t), (c - d.cents).clamp(-1200.0, 1200.0)));
            }
            if let Some(db) = rms_db(t) {
                levels.push(db);
                energy.push((secs_to_tick(t), db));
            }
            t += CURVE_STEP_SECS;
        }
    }
    let reference = median(&levels);
    let energy: Vec<(Tick, f64)> = energy
        .into_iter()
        .map(|(t, db)| (t, (db - reference).clamp(-24.0, 12.0)))
        .collect();
    let dedup = |mut v: Vec<(Tick, f64)>| {
        v.sort_by_key(|p| p.0);
        v.dedup_by_key(|p| p.0);
        v
    };
    let mut curves = BTreeMap::new();
    for (param, points) in [(known::PITCH, pitch), (known::ENERGY, energy)] {
        let points = dedup(points);
        if !points.is_empty() {
            curves.insert(
                ParamId::new(param),
                LayeredCurve {
                    model_prediction: Default::default(),
                    user_delta: cantor_core::Curve {
                        points,
                        interp: Interp::Linear,
                    },
                },
            );
        }
    }

    let end = notes.iter().map(|n| n.start + n.length).max().unwrap_or(1);
    let mut p = Project::new();
    p.tempo_map[0].bpm = TEMPO_BPM;
    p.meta = Meta {
        title: format!("{source} (converted)"),
        artist: consent.performer.clone(),
        comment: String::new(),
        provenance: vec![Provenance {
            method: "audio_to_voice".into(),
            source: source.into(),
            performer: consent.performer.clone(),
            consent: consent.statement.clone(),
            date: consent.date.clone(),
        }],
    };
    p.tracks.push(Track {
        id: TrackId(1),
        name: "Converted".into(),
        singer: SingerRef {
            id: singer.into(),
            version: None,
        },
        backend: BackendRef(backend.into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves,
        parts: vec![Part {
            id: PartId(1),
            name: "Converted".into(),
            start: 0,
            length: end + PPQ as Tick,
            notes,
        }],
    });
    p.canonicalize();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(segments: &[(f64, f64, Option<f64>)]) -> F0Track {
        let hop = 0.01;
        let mut hz = Vec::new();
        let mut voiced = Vec::new();
        for &(start, end, cents) in segments {
            let frames = ((end - start) / hop).round() as usize;
            for _ in 0..frames {
                hz.push(cents.map_or(0.0, |c| {
                    (440.0 * 2f64.powf((c / 100.0 - 69.0) / 12.0)) as f32
                }));
                voiced.push(cents.is_some());
            }
        }
        F0Track {
            hop_secs: hop,
            t0: 0.0,
            hz,
            voiced,
        }
    }

    #[test]
    fn detects_notes_across_gaps_and_steps() {
        // Rest, C4 with a 30 ms consonant gap, legato step to E4, rest, G4.
        let f0 = track(&[
            (0.0, 0.2, None),
            (0.2, 0.5, Some(6005.0)),
            (0.5, 0.53, None),
            (0.53, 0.8, Some(5995.0)),
            (0.8, 1.2, Some(6403.0)),
            (1.2, 1.5, None),
            (1.5, 2.0, Some(6698.0)),
        ]);
        let notes = detect_notes(&f0);
        let cents: Vec<f64> = notes.iter().map(|n| n.cents).collect();
        assert_eq!(cents, [6000.0, 6400.0, 6700.0], "{notes:?}");
        assert!((notes[0].start - 0.2).abs() < 0.02);
        assert!((notes[1].start - 0.8).abs() < 0.05);
        assert!((notes[2].end - 2.0).abs() < 0.02);
    }

    #[test]
    fn attestations_must_be_complete() {
        let ok = ConsentAttestation {
            performer: "A. Singer".into(),
            statement: "I agree to this recording being re-sung by Cantor voices.".into(),
            date: "2026-10-25".into(),
            confirmed: true,
        };
        assert_eq!(ok.problem(), None);
        assert!(
            ConsentAttestation {
                confirmed: false,
                ..ok.clone()
            }
            .problem()
            .is_some()
        );
        assert!(
            ConsentAttestation {
                performer: " ".into(),
                ..ok.clone()
            }
            .problem()
            .is_some()
        );
        assert!(
            ConsentAttestation {
                statement: "ok".into(),
                ..ok
            }
            .problem()
            .is_some()
        );
    }
}
