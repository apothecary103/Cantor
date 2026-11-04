//! UTAU synthesis (docs/design.md): voicebanks, lyric → alias resolution,
//! a TD-PSOLA resampler, concatenation, effects and mixing.

pub mod fx;
pub mod psola;
pub mod render;
pub mod voicebank;

pub use psola::SAMPLE_RATE;
pub use render::{Placement, RenderRequest, Rendered, Synth, phonemes, wav_bytes};
pub use voicebank::{BankKind, OtoEntry, Voicebank, VoicebankInfo, scan};
