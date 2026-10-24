//! Splitting tracks into phrases, the unit of rendering, caching and seeding.

use crate::key::KeyBuilder;
use cantor_core::{
    LangTag, NoteExpr, NoteId, PhonemeOverride, PhraseId, Project, TempoMap, Tick, Track, TrackId,
};

/// Rests at least this long split phrases.
pub const MIN_REST_SECS: f64 = 0.2;

/// A note placed in absolute time.
#[derive(Clone, Debug, PartialEq)]
pub struct PhraseNote {
    /// The note.
    pub id: NoteId,
    /// Absolute start tick.
    pub start: Tick,
    /// Absolute end tick.
    pub end: Tick,
    /// Start in seconds.
    pub start_sec: f64,
    /// End in seconds.
    pub end_sec: f64,
    /// Pitch in cents (MIDI × 100).
    pub pitch_cents: f64,
    /// Lyric.
    pub lyric: String,
    /// Language override.
    pub lang: Option<LangTag>,
    /// Phoneme overrides.
    pub phonemes: Option<Vec<PhonemeOverride>>,
    /// Expression.
    pub expr: NoteExpr,
}

/// A run of notes without a rest of [`MIN_REST_SECS`] or more.
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    /// Identity: the id of the phrase's first note. Seeds and takes are
    /// stored under it.
    pub id: PhraseId,
    /// The track.
    pub track: TrackId,
    /// Notes in time order.
    pub notes: Vec<PhraseNote>,
    /// Diffusion seed: the project's stored seed for this phrase, or one
    /// derived from the track and phrase ids.
    pub seed: u64,
}

impl Phrase {
    /// Start of the first note in seconds.
    pub fn start_sec(&self) -> f64 {
        self.notes[0].start_sec
    }

    /// End of the last note in seconds.
    pub fn end_sec(&self) -> f64 {
        self.notes
            .iter()
            .map(|n| n.end_sec)
            .fold(f64::MIN, f64::max)
    }
}

/// Splits one track into phrases.
pub fn split(project: &Project, track: &Track, tempo: &TempoMap) -> Vec<Phrase> {
    let mut notes: Vec<PhraseNote> = track
        .parts
        .iter()
        .flat_map(|part| {
            part.notes.iter().map(move |n| {
                let (start, end) = (part.start + n.start, part.start + n.start + n.length);
                PhraseNote {
                    id: n.id,
                    start,
                    end,
                    start_sec: tempo.tick_to_sec(start),
                    end_sec: tempo.tick_to_sec(end),
                    pitch_cents: n.pitch_cents,
                    lyric: n.lyric.clone(),
                    lang: n.lang.clone(),
                    phonemes: n.phonemes.clone(),
                    expr: n.expr.clone(),
                }
            })
        })
        .collect();
    notes.sort_by_key(|n| (n.start, n.id));

    let mut phrases: Vec<Vec<PhraseNote>> = Vec::new();
    let mut last_end = f64::MIN;
    for n in notes {
        match phrases.last_mut() {
            Some(cur) if n.start_sec - last_end < MIN_REST_SECS => {
                last_end = last_end.max(n.end_sec);
                cur.push(n);
            }
            _ => {
                last_end = n.end_sec;
                phrases.push(vec![n]);
            }
        }
    }
    phrases
        .into_iter()
        .map(|notes| {
            let id = PhraseId(notes[0].id.0);
            let seed = project
                .seeds
                .get(&id)
                .copied()
                .unwrap_or_else(|| default_seed(track.id, id));
            Phrase {
                id,
                track: track.id,
                notes,
                seed,
            }
        })
        .collect()
}

/// The seed used when the project stores none for a phrase.
pub fn default_seed(track: TrackId, phrase: PhraseId) -> u64 {
    let k = KeyBuilder::new("default-seed")
        .u64("track", track.0)
        .u64("phrase", phrase.0)
        .finish();
    u64::from_le_bytes(k.0[..8].try_into().expect("8 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_core::{Note, Part, PartId};

    pub(crate) fn note(id: u64, start: Tick, length: Tick, pitch: f64, lyric: &str) -> Note {
        Note {
            id: NoteId(id),
            start,
            length,
            pitch_cents: pitch,
            lyric: lyric.into(),
            lang: None,
            phonemes: None,
            expr: Default::default(),
            modes: Default::default(),
        }
    }

    #[test]
    fn splits_at_long_rests() {
        let mut p = Project::new(); // 120 BPM: 1920 ticks = 0.5 s
        let track = Track {
            id: TrackId(1),
            name: "t".into(),
            singer: Default::default(),
            backend: Default::default(),
            mix: Default::default(),
            effects: vec![],
            doubling: Default::default(),
            voice_mix: vec![],
            curves: Default::default(),
            parts: vec![Part {
                id: PartId(2),
                name: "p".into(),
                start: 0,
                length: 40_000,
                notes: vec![
                    note(10, 0, 1920, 6000.0, "a"),
                    // 0.1 s rest: same phrase
                    note(11, 1920 + 384, 1920, 6200.0, "b"),
                    // 0.25 s rest: new phrase
                    note(12, 2 * 1920 + 384 + 960, 1920, 6400.0, "c"),
                ],
            }],
        };
        p.seeds.insert(PhraseId(12), 99);
        let phrases = split(&p, &track, &p.tempo());
        assert_eq!(phrases.len(), 2);
        assert_eq!(phrases[0].notes.len(), 2);
        assert_eq!(phrases[1].id, PhraseId(12));
        assert_eq!(phrases[1].seed, 99);
        assert_eq!(phrases[0].seed, default_seed(TrackId(1), PhraseId(10)));
    }
}
