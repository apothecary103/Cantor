//! Phrases: runs of notes on a track without a rest of [`MIN_REST_SECS`]
//! or more. A phrase is identified by its first note's id; seeds and takes
//! are stored under that id.

use crate::ids::{NoteId, PhraseId, Tick, TrackId};
use crate::model::{Project, Track};

/// Rests at least this long split phrases.
pub const MIN_REST_SECS: f64 = 0.2;

/// A phrase's extent and notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseSpan {
    /// The phrase (its first note's id).
    pub id: PhraseId,
    /// The track.
    pub track: TrackId,
    /// Notes in time order.
    pub notes: Vec<NoteId>,
    /// Absolute start tick of the first note.
    pub start: Tick,
    /// Absolute end tick of the last-ending note.
    pub end: Tick,
}

/// Splits a track into phrases.
pub fn phrase_spans(project: &Project, track: &Track) -> Vec<PhraseSpan> {
    let tempo = project.tempo();
    let mut notes: Vec<(Tick, Tick, NoteId)> = track
        .parts
        .iter()
        .flat_map(|p| {
            p.notes
                .iter()
                .map(move |n| (p.start + n.start, p.start + n.start + n.length, n.id))
        })
        .collect();
    notes.sort();
    let mut out: Vec<PhraseSpan> = Vec::new();
    for (start, end, id) in notes {
        match out.last_mut() {
            Some(cur) if tempo.tick_to_sec(start) - tempo.tick_to_sec(cur.end) < MIN_REST_SECS => {
                cur.notes.push(id);
                cur.end = cur.end.max(end);
            }
            _ => out.push(PhraseSpan {
                id: PhraseId(id.0),
                track: track.id,
                notes: vec![id],
                start,
                end,
            }),
        }
    }
    out
}

/// Mixes a 64-bit value (SplitMix64 finalizer); used to derive seeds.
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
