# 0029 v2: UTAU-only engine, web UI, one binary

Date: 2026-11-03

## Context
v1 grew to 15 crates plus a GPUI app: DiffSinger/ONNX, RMVPE, flow
matching, svs.io, a WASM phonemizer host, Python bindings, a DAW bridge,
audio-to-voice conversion, and a web prototype with a built-in "AI
copilot". The main real use is narrower: an agent turns a song (MIDI or
audio + lyrics) into a tuned UTAU cover, and a person polishes it.

## Decision
- Synthesis is concatenative UTAU only (oto.ini + built-in TD-PSOLA
  resampler). No neural models ship in or are loaded by Cantor; F0 for
  voicebank samples and reference audio comes from DSP (YIN) or `.frq`.
- The only GUI is a static web editor embedded in the `cantor` binary.
  No AI chat in the UI; agents connect over MCP from outside.
- One binary (`cantor`) holds the engine, HTTP API, MCP server and CLI.
  Crates: core, synth, analysis, io, cantor, fixtures.
- The model is UTAU-shaped: notes on tracks (no parts), MIDI keys, PPQ
  480, named pitch parameters (portamento, overshoot, preparation, scoop,
  fall, vibrato) plus `pitd`, `dyn`, `gen`, `bre`, `bri`, `tone` curves.
- Tuning knowledge lives in rule-based tuner passes that emit ordinary
  ops, documented in docs/tuning.md.

## Consequences
Removed: app/, vendor/gpui-kit, cantor-py, cantor-bridge(-clap),
cantor-plugin-sdk, cantor-phon's WASM host, DiffSinger/flow/RMVPE/svs.io,
convert, style prompts, takes and seeds. They remain in git history
(tag `v1-final`, 82b5133, with the v1 CI jobs). Decisions 0009–0011, 0016–0019, 0023, 0025–0028 are
superseded. Project files are format v3; v1 project files (envelope versions 1–2) are converted on open by `cantor_io::v1`, with diagnostics for what was dropped; they open without a save path so the original is never overwritten.
