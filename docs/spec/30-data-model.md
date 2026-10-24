# Data model (reference types; adapt names freely, keep the semantics)

```rust
pub const PPQ: u32 = 1920; // exact triplets and quintuplets

pub struct Project {
    pub meta: Meta,
    pub tempo_map: Vec<TempoPoint>,      // (tick, bpm)
    pub time_sig_map: Vec<TimeSigPoint>,
    pub key_map: Vec<KeyPoint>,          // drives scale highlighting/snap
    pub tracks: Vec<Track>,
    pub seeds: BTreeMap<PhraseId, u64>,
    pub takes: BTreeMap<PhraseId, Vec<Take>>, // retakes are kept, not discarded
}

pub struct Track {
    pub id: TrackId,
    pub singer: SingerRef,
    pub backend: BackendRef,
    pub mix: Mix,                        // gain, pan, mute, solo
    pub effects: Vec<EffectSlot>,        // EQ, compressor, de-esser, reverb
    pub doubling: Doubling,              // layers 1..=16, spacing, detune, jitter
    pub voice_mix: Vec<(SingerRef, f32)>,// timbre blending, if supported
    pub parts: Vec<Part>,
}

pub struct Note {
    pub id: NoteId,
    pub start: Tick,
    pub length: Tick,
    pub pitch_cents: f64,                // MIDI*100; microtonal by default
    pub lyric: String,
    pub lang: LangTag,
    pub phonemes: Option<Vec<PhonemeOverride>>, // X-SAMPA or native symbols
    pub expr: NoteExpr,                  // vibrato, portamento, attack, release
    pub modes: VocalModes,               // pitch / timbre / pronunciation
}

/// Every curve parameter is layered. Users and agents edit `user_delta`.
pub struct LayeredCurve {
    pub param: ParamId,
    pub model_prediction: Curve,         // produced by the backend, retakeable
    pub user_delta: Curve,
}

pub struct Curve {
    pub points: Vec<(Tick, f64)>,
    pub interp: Interp,                  // Step | Linear | MonotoneHermite (default)
}

/// The only way to change a project.
pub enum Op {
    AddNote { part: PartId, note: Note },
    MoveNotes { ids: Vec<NoteId>, dt: i64, dcents: f64, carry_curves: bool },
    SetLyric { id: NoteId, lyric: String },
    SetCurve { track: TrackId, param: ParamId, range: TickRange, points: Vec<(Tick, f64)>, interp: Interp },
    SetNoteExpr { id: NoteId, expr: NoteExprPatch },
    Retake { track: TrackId, range: TickRange, dims: Vec<RetakeDim>, n_variants: u8, seed: Option<u64> },
    SelectTake { phrase: PhraseId, take: TakeId },
    // ... every GUI capability has an Op
}

pub struct ApplyResult {
    pub revision: Revision,
    pub diff: DocDiff,
    pub diagnostics: Vec<Diagnostic>,    // never silently drop an edit
}
```

Rules that follow from the types:
- `final = base(notes) + model_prediction + user_delta` for every parameter.
- Moving or stretching notes carries their curves by default (`carry_curves`).
- Project files are versioned JSON (optionally zstd), with generated JSON
  Schema and automatic forward migrations.
