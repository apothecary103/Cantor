# Cantor design (v2)

This file is the contract. Code, docs and the web UI follow it; when they
disagree, fix the code or change this file in the same commit.

## What Cantor is
A UTAU-style singing synthesis workstation. Concatenative synthesis from
UTAU voicebanks (oto.ini) through a built-in pitch-synchronous (TD-PSOLA) resampler. No neural
models anywhere in the product. One binary, `cantor`, serves a web editor,
a JSON API, and an MCP server, and has a CLI.

The main use: an AI agent is handed a song (MIDI and/or audio, plus
lyrics) and produces a finished, well-tuned UTAU cover. Humans open the
same project in the browser to listen and fine-tune. Everything the UI
can do is an `Op` the agent can send too, and vice versa.

## Principles
- One way to edit: `Op`s applied through `cantor-core`. The UI, CLI and
  MCP never mutate the document directly. Each op returns its inverse.
- Semantic parameters over raw curves. Portamento, overshoot, vibrato,
  scoops and falls are named note parameters with units, so an agent can
  reason about them; the per-track `pitd` curve is for what's left.
- Deterministic: same project + same voicebank = identical audio hash.
- Closed loop: renders report numbers (pitch error vs score and vs the
  reference vocal, timing, loudness, clipping), because agents can't hear.
- Honest failure: unknown params, missing aliases, out-of-range values
  produce diagnostics; nothing is dropped silently.
- Small and fast: a cold render of a 3-minute song should take seconds;
  edits re-render only dirty phrases.

## Crates
```
crates/cantor-core      model, Op, apply/undo, tempo math, param registry,
                        tuning passes (rule-based "tuner"), JSON schemas
crates/cantor-synth     voicebanks (oto.ini, prefix.map, character.txt),
                        phonemizers, PSOLA resampler, concatenation,
                        pitch-curve generation, effects, mixing, phrase cache
crates/cantor-analysis  DSP only: YIN/pYIN F0, onsets, note transcription,
                        loudness (BS.1770), pitch/timing error reports
crates/cantor-io        import MIDI, UST, USTX, MusicXML, lyrics text;
                        export UST/USTX, WAV
crates/cantor           the binary: engine (projects, history, render
                        jobs), HTTP JSON API, MCP (stdio + streamable
                        HTTP), CLI, embedded web UI
crates/cantor-fixtures  generated CC0 test voicebank (no downloads)
web/                    the editor: static HTML/CSS/JS, no build step,
                        embedded into the binary at compile time
xtask/                  ci, schema, serve
```
Removed in v2: GPUI app, DiffSinger/ONNX/RMVPE/flow-matching, svs.io,
WASM phonemizer host and plugin SDK, Python bindings, DAW bridge, audio-
to-voice conversion, retakes/takes, style prompts. See
docs/decisions/0029-v2-utau-web.md.

## Time
`Tick` is i64, PPQ = 480 (same as UST/USTX). Tempo map and time-signature
map as before; seconds ↔ ticks via `TempoMap`.

## Model (`cantor-core::model`, project format v3, JSON, `.cantor`)
```rust
Project {
  format: u32,                         // 3
  meta: Meta { title, artist, comment },
  tempo: Vec<TempoPoint { tick, bpm }>,           // first at tick 0
  time_sig: Vec<TimeSigPoint { bar, num, den }>,  // first at bar 0
  key: Option<Key { tonic: u8, scale: Scale }>,   // for highlighting/snapping
  sections: Vec<Section { name, start, end }>,
  reference: Option<Reference>,        // the original song's vocal/mix
  tracks: Vec<Track>,
}
Reference {
  path: String,                        // audio file, relative to project
  offset_ms: f64,                      // audio time of tick 0
  gain_db: f64, muted: bool,           // for A/B listening in the UI
}
Track {
  id: TrackId, name: String,
  singer: String,                      // voicebank id ("" = none)
  phonemizer: Phonemizer,              // auto | jp_cv | jp_vcv | jp_cvvc | en_arpasing | raw
  flags: String,                       // default resampler flags, e.g. "g-5B40"
  mix: Mix { gain_db, pan, mute, solo },
  effects: Vec<EffectSlot { kind: Eq|Compressor|DeEsser|Reverb|Delay, enabled, params }>,
  curves: BTreeMap<ParamId, Curve>,    // track-level expression curves
  notes: Vec<Note>,                    // sorted by (start, id); absolute ticks; no overlaps
}
Note {
  id: NoteId, start: Tick, length: Tick,
  key: u8,                             // MIDI note number (60 = C4)
  lyric: String,                       // "あ", "ka", "+" (extend), "-" (rest-breath), "br" (breath)
  alias: Option<String>,               // force an exact oto alias; None = phonemizer
  vel: f64,                            // consonant velocity %, 0..200, default 100
  vol: f64,                            // intensity %, 0..200, default 100
  mod_: f64,                           // modulation: % of the sample's own pitch wobble kept, 0..100, default 0
  flags: String,                       // per-note resampler flags appended to track flags
  timing: Timing { preutter_ms: Option<f64>, overlap_ms: Option<f64>, offset_ms: f64 },
  pitch: NotePitch,
}
NotePitch {
  // Transition from the previous note's pitch into this note.
  portamento: Portamento {
    length_ms: f64,      // default 80; 0 = hard step
    start_ms: f64,       // where it starts relative to note start; default -40
    overshoot_cents: f64,    // past the target then settle; default 0
    preparation_cents: f64,  // dip opposite to the jump before it; default 0
    shape: Shape,        // s (ease in-out, default) | linear | in | out
  },
  scoop: Option<Bend { cents, length_ms }>,  // start below (cents<0) and rise in
  fall:  Option<Bend { cents, length_ms }>,  // drop at note end (cents<0)
  vibrato: Option<Vibrato {
    length: f64,         // % of note from the end that vibrates, 0..100
    period_ms: f64,      // 1000/rate; 150..220 typical
    depth_cents: f64,    // half peak-to-peak
    fade_in: f64,        // % of vibrato length
    fade_out: f64,       // % of vibrato length
    phase: f64,          // 0..1 of a cycle
    shift: f64,          // center offset, % of depth, -100..100
    drift: f64,          // 0..1 slow random rate/depth wander, seeded by note id
  }>,
  points: Vec<PitchPoint { ms: f64, cents: f64 }>,  // optional extra bend, relative to note start
}
Curve { points: Vec<(Tick, f64)>, interp: Linear|Step|Smooth }
```
Track curves (`ParamId`, registry in `cantor-core::params` with unit,
range, default, description):

| id    | meaning                                  | unit | range       | default |
|-------|------------------------------------------|------|-------------|---------|
| pitd  | pitch deviation added to the note pitch  | cents| -1200..1200 | 0       |
| dyn   | loudness                                 | dB   | -24..12     | 0       |
| gen   | gender/formant shift (+ = deeper/male)   | %    | -100..100   | 0       |
| bre   | breathiness (aperiodicity mix)           | %    | 0..100      | 0       |
| bri   | brightness/tension (spectral tilt)       | %    | -100..100   | 0       |
| tone  | pitch-independent timbre shift ("tone")  | semitones | -12..12 | 0     |

Final pitch at time t = note key ×100 + portamento/overshoot/preparation +
scoop + fall + vibrato + note points + `pitd`(t). Computed by
`cantor-core::pitch::render_pitch(&Track, &TempoMap, range) -> PitchCurve` (pure math, shared by synth, tuner and API)
on a 5 ms grid; the UI draws the exact same curve from `GET .../pitch`.

## Ops (`cantor-core::op::Op`, `#[serde(tag = "op")]`)
```
batch { label, ops }
set_meta { meta } · set_tempo { tick, bpm } · remove_tempo { tick }
set_time_sig { bar, num, den } · remove_time_sig { bar } · set_key { key }
set_sections { sections } · set_reference { reference }
add_track { track } · remove_track { track } · update_track { track, patch }
add_notes { track, notes } · remove_notes { track, ids }
update_notes { track, ids, patch: NotePatch }   // every Note field optional
move_notes { track, ids, dticks, dkeys }
resize_notes { track, ids, dlength, from_start: bool }
split_note { track, id, at } · merge_notes { track, ids }
set_lyrics { track, ids, lyrics: Vec<String> }  // or a single string split by spaces
set_curve { track, param, range: (Tick,Tick), points }  // replaces points in range
clear_curve { track, param, range }
set_effects { track, effects }
```
`apply(project, op) -> Result<ApplyResult { inverse: Op, diagnostics }, Rejected { diagnostics }>`;
ops validate fully before mutating. History is a linear undo/redo stack
of (op, inverse, label) plus named checkpoints (snapshots).

Op details (as implemented in `cantor-core`):
- Ids: note ids are unique per project. A `Note`/`Track` with `id` 0 or
  omitted in `add_notes`/`add_track` gets the next free id; the inverse
  names the assigned ids, and redo replays the inverse's inverse, so
  ids are stable across undo/redo.
- `add_track` takes an optional `index` (default: last), so undoing
  `remove_track` restores the track order.
- `update_track.patch`: `name, singer, phonemizer, flags, mix: { gain_db,
  pan, mute, solo }`, all optional. Notes, curves and effects have their
  own ops.
- `update_notes.patch` merges nested objects (`{"pitch": {"vibrato":
  {"depth_cents": 30}}}` touches only the depth; a missing vibrato starts
  from the default) and `null` clears optional fields (alias, scoop,
  fall, vibrato, timing.preutter_ms/overlap_ms).
- `move_notes.dkeys` is semitones. `resize_notes` with `from_start`
  moves the start edge (`start -= dlength`, end fixed).
- `split_note`: the left part keeps id, lyric, scoop and portamento; the
  right part gets a new id, lyric `+`, the vibrato, the fall and the
  bend points after `at`. `merge_notes` extends the earliest note to the
  latest end and removes the others.
- `set_lyrics` needs exactly one lyric per id.
- `set_curve` takes an optional `interp`; `range` is inclusive and all
  points must lie in it. `clear_curve.range` is optional (= all). Empty
  curves are dropped.
- An op that makes notes overlap is rejected (`note.overlap`); so are
  out-of-range values. Undo/redo skip the overlap check (they return to
  states that existed). `Project::validate` reports overlaps as errors.
- `Document::restore(checkpoint)` and `Document::replace(project)` (for
  imports) are ordinary undoable batches built by `Op::transition`.

Phrases (`cantor-core::phrase`): a gap ≥ 1 beat (480 ticks) or a
rest/breath note (`-`, `r`, `R`, `rest`, `br`, `breath`, `息`, empty)
starts a new phrase; a breath belongs to the phrase it inhales for. The
synth caches per phrase.

## Tuner (`cantor-core::tune`)
Rule-based passes that emit ordinary ops (so they're undoable and visible
in the UI). Each takes a track, an optional tick range, and parameters.
- `auto_pitch { style: natural|pop|ballad|rock|robotic }` — portamento
  length/start by interval and tempo, overshoot on upward leaps ≥ 3 st,
  preparation on downward leaps, vibrato on notes ≥ ~400 ms with delayed
  onset, falls at phrase ends, scoops on phrase starts.
- `auto_dynamics` — phrase arcs on `dyn`, consonant velocity by tempo,
  softer phrase tails, accent on downbeats.
- `breaths` — insert `br` notes in rests ≥ ~300 ms before phrases.
- `section_color { section, gen, bre, bri }` — e.g. breathy verse, bright chorus.
- `match_reference { strength, smooth_ms }` — writes `pitd` so the rendered
  pitch follows the reference vocal's F0 (expression transfer), with
  octave-error correction and vibrato kept.
- `humanize { cents, ms, seed }` — small deterministic drift.
Rules and numbers are documented in docs/tuning.md.

## Analysis
- `analyze_reference(path) -> F0 track (10 ms hop), voiced flags, onsets`
  with YIN + octave-jump cleanup; cached by file hash.
- `transcribe(path, tempo) -> Vec<Note>` from F0 segmentation (no lyrics).
- Render report per phrase and overall: p50/p95 pitch error vs score
  (cents), vs reference when present, onset error (ms), integrated
  loudness (LUFS), true peak, clipped samples, missing aliases.

## HTTP API (`cantor serve`, default `127.0.0.1:7878`)
JSON in/out. Errors: `{ "error": { "code", "message", "diagnostics" } }`.
```
GET  /                       web UI
GET  /api/state              { project, revision, path, can_undo, can_redo, dirty }
GET  /api/schema             params, ops, effects JSON schemas
GET  /api/voicebanks         [{ id, name, image, author, aliases_count, kind: cv|vcv|cvvc|mixed, prefix_map }]
GET  /api/voicebanks/{id}/image
POST /api/ops                { ops: [Op], base_revision? } → { revision, diagnostics }
POST /api/undo · /api/redo   { steps? } → { revision }      (steps default 1)
GET  /api/history            { undo: [{ label, source }], redo: [{ label, source }], checkpoints: [name] }
                             (undo oldest first, redo next first; source: http|mcp|cli)
POST /api/checkpoint         { name } → same as GET /api/history
POST /api/restore            { name } → { revision, diagnostics }   (one undoable step)
POST /api/tune               { pass, track, range?, params } → { revision, ops_applied, diagnostics }
POST /api/render             { range?, tracks? } → { render_id, report }
GET  /api/render/{id}.wav
GET  /api/tracks/{id}/pitch  → { start_ms, step_ms: 5, cents: [..] }  (final pitch curve)
GET  /api/tracks/{id}/phonemes → [{ note, alias, start_ms, preutter_ms, overlap_ms, missing }]
GET  /api/reference/f0       → { step_ms: 10, cents: [..|null] }
POST /api/project/new · /api/project/open { path } · /api/project/save { path? }
POST /api/import             JSON { path, kind? }, or the file's bytes with ?name=&kind=
                             (kind: midi|ust|ustx|music_xml|cantor|lyrics|audio; default from the extension)
POST /api/transcribe         reference vocal → notes on a new track
GET  /api/notes?track=&from=&to=   compact note table (as MCP get_notes)
GET  /api/voicebanks/{id}    aliases, prefix map, load warnings · POST /api/voicebanks/rescan
GET  /api/export?format=wav|ust|ustx|cantor
GET  /api/events             SSE: { type: "revision"|"render"|"progress"|"checkpoint", ... }
POST /mcp                    MCP streamable HTTP
```
GET  /api/reference/audio    raw bytes of the reference file (A/B playback)
```
The UI subscribes to `/api/events`, so edits an agent makes over MCP show
up live in an open browser.

Events say who caused them, so the UI can tell its own edits from an
agent's. Every HTTP request may carry `X-Cantor-Client: <id>` (the web UI
sends a random id per tab); events echo it:
```
revision   { revision, label, source: http|mcp|cli, client: id|null }
render     { revision, report, source, client }
checkpoint { name, source, client }
progress   { message?, fraction? }
```
The engine records the source of every undo step for `GET /api/history`.

Wire details the web UI relies on:
- Static files are served at the root (`/`, `/style.css`, `/js/...`);
  API under `/api/`.
- `update_notes`/`update_track` patches are partial and nested: a missing
  field is unchanged, `null` clears an optional field (`alias`,
  `pitch.vibrato`, `timing.preutter_ms`, ...). Note field `mod_` is `mod`
  on the wire.
- Curve points are `[tick, value]`; `range` is `[start, end]`. Pan is -1..1.
- `resize_notes { from_start: true, dlength > 0 }` extends to the left.
- Ops that would overlap notes are rejected with a diagnostic, not trimmed.
- Diagnostics: `[{ level: error|warning|info, code, message, entity? }]`.
- Render report: `cantor_analysis::RenderReport` — `{ render_ms, lufs,
  true_peak_db, clipped, pitch_error, ref_pitch_error, onset_error_ms`
  (each `{ p50, p95, bias, frames } | null`)`, missing_aliases: [{ note,
  lyric }], phrases: [{ track, index, start_sec, end_sec, pitch_p50,
  pitch_p95, ref_p95, onset_p95_ms, voiced_ratio, worst_note }],
  artifacts: [{ kind, track, at_sec, duration_sec, magnitude, detail }] }`.
  `POST /api/render` returns `{ render_id, revision, report }`.
- `POST /api/ops` with a stale `base_revision` is rejected
  (`revision_conflict`); the UI refreshes and the user retries.
- Scales: `major|minor|harmonic_minor|dorian|pentatonic|minor_pentatonic`.

## MCP tools (same names as CLI subcommands where they overlap)
`project_info`, `open_project`, `new_project`, `save_project`, `import`
(midi/ust/ustx/musicxml/lyrics/audio), `list_voicebanks`,
`voicebank_info` (aliases, kind, prefix map), `get_notes` (range, compact
table form), `apply_ops`, `undo`, `redo`, `checkpoint`, `restore`, `tune`
(any tuner pass), `set_lyrics`, `analyze_reference`, `transcribe`,
`render` (returns report + wav path), `export`, `schema`. Resource:
`cantor://guide` serves docs/agent-guide.md so agents can read how to make
a cover.

## CLI
```
cantor serve [--port] [--voicebanks DIR]... [project]
cantor mcp                          # stdio MCP
cantor new|import|render|tune|ops|info|voicebanks|export ...
cantor cover --midi song.mid [--audio vocal.wav] --lyrics lyrics.txt \
             --singer ID [--style pop] -o out/   # whole pipeline, one shot
```
Voicebank search path: `--voicebanks`, `CANTOR_VOICEBANKS` (colon
separated), `./voicebanks`, `~/.local/share/cantor/voicebanks`,
`~/Library/Application Support/Cantor/voicebanks`.

## Web UI
Look: Graphite-like and egui-like — flat, dense, dark (#1f1f1f-ish
panels, 1 px separators, 11–12 px UI font, one pink accent), every
control small and immediate, no gradients/shadows/rounded blobs, light
theme available. Layout:
```
┌ menu bar: File Edit View Tune Render Help ···· ▶ ■ ⟲ 00:12.34 120bpm 4/4 · [Connect] ┐
├ tracks (left)  ┬ ruler + sections + phrase error marks ───────┬ Properties│History│Report ┤
│ singer image,  │ piano roll: notes w/ lyric + alias, final     │ (tabs, right panel)        │
│ name, M/S,     │ pitch line, reference F0 (ghost), pitch-vs-   │                            │
│ gain, phonem., │ reference shading, phoneme boundaries, play-  │                            │
│ reference row  │ head; notes an agent just changed flash       │                            │
│                ├───────────────────────────────────────────────┤                            │
│                │ expression lane (tabs: pitd dyn gen bre ...)  │                            │
└ status: connection · revision · file · last message (agent edits named) · report summary ┘
```
Right panel tabs (docs/decisions/0030-agent-review-ui.md):
- Properties edits the selected notes (lyric, alias, vel, vol,
  portamento, vibrato, scoop/fall, flags) with numeric drag fields, and
  runs tuner passes.
- History lists every undo step with its source (agent steps tagged);
  clicking one undoes/redoes to it. Checkpoints can be saved and restored.
- Report shows the last render: totals, missing aliases, phrases worst
  first, artifacts; clicking selects and frames the notes and sets the
  loop. It says when the project changed since that render. Grades:
  vs reference p95 ≤ 25 ¢ good, ≤ 40 ¢ close (agent-guide.md), vs score
  ≤ 40 / ≤ 80 ¢, onsets ≤ 30 / ≤ 60 ms.
Listening: `A` flips synth ⇄ reference while playing, `Shift+A` plays
both, `D` shades where the pitch line is ≥ 50 ¢ from the reference F0.
Tools: select, draw note, pitch pen, curve pen, knife. Keyboard-first
(OpenUtau-compatible shortcuts where sensible). Canvas-drawn piano roll,
60 fps with 3,000 notes. "Connect" opens a small modal with MCP/CLI
connection snippets; there is no chat or AI UI.
