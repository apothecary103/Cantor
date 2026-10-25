//! Splitting tracks into phrases, the unit of rendering, caching and seeding.

use crate::key::KeyBuilder;
use cantor_core::{
    LangTag, NoteExpr, NoteId, PhonemeOverride, PhraseId, Project, RetakeDim, TempoMap, Tick,
    Track, TrackId, phrase_spans,
};
use std::collections::HashMap;

pub use cantor_core::phrase::MIN_REST_SECS;

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
    /// The phrase's seed: the project's stored seed, or one derived from
    /// the track and phrase ids.
    pub seed: u64,
    /// Noise seeds per model stage, after applying the selected take.
    pub seeds: StageSeeds,
    /// Retake dimensions the selected take asks for that no stage can vary.
    pub unsupported_dims: Vec<RetakeDim>,
}

/// Noise seeds for each stochastic stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageSeeds {
    /// Pitch model (retake dimension: pitch).
    pub pitch: u64,
    /// Variance model (retake dimension: expression).
    pub variance: u64,
    /// Acoustic model (retake dimension: timbre).
    pub acoustic: u64,
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

/// Splits one track into phrases (see [`cantor_core::phrase_spans`]).
pub fn split(project: &Project, track: &Track, tempo: &TempoMap) -> Vec<Phrase> {
    let mut by_id: HashMap<NoteId, PhraseNote> = HashMap::new();
    for part in &track.parts {
        for n in &part.notes {
            let (start, end) = (part.start + n.start, part.start + n.start + n.length);
            by_id.insert(
                n.id,
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
                },
            );
        }
    }
    phrase_spans(project, track)
        .into_iter()
        .map(|span| {
            let id = span.id;
            let seed = project
                .seeds
                .get(&id)
                .copied()
                .unwrap_or_else(|| default_seed(track.id, id));
            let take = project.takes.get(&id).and_then(|lane| {
                lane.selected
                    .and_then(|s| lane.takes.iter().find(|t| t.id == s))
            });
            let pick = |dim: RetakeDim| match take {
                Some(t) if t.dims.contains(&dim) => t.seed,
                _ => seed,
            };
            let unsupported_dims = take
                .map(|t| {
                    t.dims
                        .iter()
                        .copied()
                        .filter(|d| *d == RetakeDim::Timing)
                        .collect()
                })
                .unwrap_or_default();
            Phrase {
                id,
                track: track.id,
                notes: span.notes.iter().map(|n| by_id[n].clone()).collect(),
                seed,
                seeds: StageSeeds {
                    pitch: pick(RetakeDim::Pitch),
                    variance: pick(RetakeDim::Expression),
                    acoustic: pick(RetakeDim::Timbre),
                },
                unsupported_dims,
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
