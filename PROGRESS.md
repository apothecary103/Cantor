# Progress

Current milestone: **v2** — UTAU-only engine, web editor, one binary
(docs/design.md, docs/decisions/0029-v2-utau-web.md). v1 (M1–M7) is in git
history: tag `v1-final` (82b5133). Gate: `nix develop --command cargo xtask ci`.

## v2 parts
- [x] Removed GPUI app, neural backends, Python, DAW bridge, WASM phonemizers
- [x] cantor-core: v3 model, ops with inverses, undo/checkpoints, params
- [x] cantor-core: pitch curve + tuner passes
- [x] cantor-analysis: YIN, transcription, render reports
- [x] cantor-io: MIDI/UST/USTX/MusicXML/lyrics import, UST/USTX export
- [x] cantor-synth: voicebanks, phonemizers, PSOLA resampler, cached render (0031)
- [x] cantor binary: engine, HTTP API + SSE, MCP (stdio + /mcp), CLI incl. `cantor cover`
- [x] web/: editor against the design.md API (tested on the mock)
- [x] web/: agent review — History/Report tabs, checkpoints, agent-edit
      flash, phrase grades, A/B keys, reference diff (0030)
- [ ] web/ served by `cantor serve`: every API call the UI makes verified with curl; not yet clicked through in a browser
- [x] CLAUDE.md, docs/tuning.md, docs/agent-guide.md

## Done when
- [ ] `cantor cover` turns a MIDI + lyrics into a WAV with no missing
      aliases on the fixture voicebank
- [ ] An agent using only MCP tools brings a phrase within 25 cents p95 of
      a reference vocal
- [x] Rendering is deterministic across runs and thread counts; unchanged
      phrases come from cache (cantor-synth/tests/render.rs)
- [ ] Edits made over MCP appear in an open browser without reload
- [ ] UST/USTX export opens in OpenUtau (round-trip tests)

## Open items
- Prebuilt release binaries (users currently build with `cargo install`).
- Not rendered yet: bre/bri/tone curves, resampler flags other than `g`,
  CVVC VC links, ARPAsing (all produce warnings).
