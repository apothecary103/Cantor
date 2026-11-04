//! Phrases: runs of notes sung in one breath. A gap of at least one beat
//! or a rest/breath note starts a new phrase. The synthesizer renders and
//! caches per phrase; the tuner shapes dynamics and pitch per phrase.

use crate::ids::{NoteId, PPQ, Tick};
use crate::model::{Note, Track};
use std::ops::Range;

/// A gap between notes of at least this many ticks (one beat) splits
/// phrases.
pub const MIN_GAP: Tick = PPQ as Tick;

/// A phrase of a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phrase {
    /// Indices into `track.notes`.
    pub notes: Range<usize>,
    /// Start tick of the first note.
    pub start: Tick,
    /// End tick of the latest-ending note.
    pub end: Tick,
}

impl Phrase {
    /// The phrase's notes.
    pub fn notes<'a>(&self, track: &'a Track) -> &'a [Note] {
        &track.notes[self.notes.clone()]
    }

    /// The first note's id, a stable name for the phrase.
    pub fn first_id(&self, track: &Track) -> NoteId {
        track.notes[self.notes.start].id
    }
}

/// Splits a track's notes into phrases, in time order. A rest or breath
/// note ("-", "r", "br", ...) starts a new phrase (a breath belongs to the
/// phrase it inhales for).
pub fn phrases(track: &Track) -> Vec<Phrase> {
    let mut out: Vec<Phrase> = Vec::new();
    for (i, n) in track.notes.iter().enumerate() {
        match out.last_mut() {
            Some(cur) if n.start - cur.end < MIN_GAP && !n.is_rest_or_breath() => {
                cur.notes.end = i + 1;
                cur.end = cur.end.max(n.end());
            }
            _ => out.push(Phrase {
                notes: i..i + 1,
                start: n.start,
                end: n.end(),
            }),
        }
    }
    out
}

/// The phrase containing a note, if the note is on the track.
pub fn phrase_of(track: &Track, id: NoteId) -> Option<Phrase> {
    let i = track.note_index(id)?;
    phrases(track).into_iter().find(|p| p.notes.contains(&i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(notes: &[(Tick, Tick, &str)]) -> Track {
        Track {
            notes: notes
                .iter()
                .enumerate()
                .map(|(i, &(s, l, lyric))| Note {
                    id: NoteId(i as u64 + 1),
                    ..Note::new(s, l, 60, lyric)
                })
                .collect(),
            ..Track::default()
        }
    }

    #[test]
    fn gaps_and_breaths_split() {
        let q = MIN_GAP;
        let t = track(&[
            (0, q, "a"),
            (q, q, "i"),
            (2 * q + q / 2, q, "u"), // half-beat gap: same phrase
            (5 * q, q, "e"),         // 1.5-beat gap: new phrase
            (6 * q, q / 2, "br"),    // breath: new phrase
            (6 * q + q / 2, q, "o"),
        ]);
        let ps = phrases(&t);
        let ranges: Vec<_> = ps.iter().map(|p| p.notes.clone()).collect();
        assert_eq!(ranges, vec![0..3, 3..4, 4..6]);
        assert_eq!((ps[0].start, ps[0].end), (0, 3 * q + q / 2));
        assert_eq!(ps[2].first_id(&t), NoteId(5));
        assert_eq!(phrase_of(&t, NoteId(6)).unwrap().notes, 4..6);
        assert!(phrases(&Track::default()).is_empty());
    }
}
