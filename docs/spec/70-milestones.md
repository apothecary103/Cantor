# Milestones

Each milestone is finished when every "done when" item holds and
`nix develop --command cargo xtask ci` passes on macOS and Linux.

## M1 Core
Document model, Op system, undo tree with branches, tempo math, generated
JSON Schema, project save/load with migrations, import of USTX, MIDI and
MusicXML. Also the workspace, xtask, Nix flake and CI.
Done when:
- A property test shows tick → sec → tick round-trips across random tempo maps.
- Applying then undoing any random sequence of Ops (proptest) restores a
  byte-identical project.
- The three importers load fixture files and insta snapshots of the
  resulting documents are committed.
- `cargo xtask schema` regenerates schema/ with no diff in CI.
- `nix flake check` passes, and `nix develop --command cargo xtask ci`
  passes on macOS and Linux in CI, with fenix's toolchain hash filled in
  (no `lib.fakeSha256` left in flake.nix).
- CI runs three extra jobs: `cargo xtask miri` (cantor-core), a stable-Rust
  check of `cantor-plugin-sdk`, and a ThreadSanitizer job stub ready for
  cantor-audio.
- A debug build of the workspace uses Cranelift and a release build uses
  LLVM (asserted by xtask).

## M2 Neural render
DiffSinger variance + acoustic models via ort, PC-NSF-HiFiGAN, phrase cache,
vocoder fast path for pitch-only edits.
Done when:
- `cantor render` produces a WAV from a public DiffSinger voicebank fixture,
  with ONNX Runtime loaded from ORT_DYLIB_PATH.
- Rendering the same project twice yields identical audio hashes.
- Re-rendering an unchanged project hits the cache for 100% of phrases.
- A pitch-only edit re-renders through the vocoder fast path (asserted by a
  render-trace test).

## M3 Analysis, server, Python, MCP
Done when:
- render returns the full analysis report described in 40-algorithms.md.
- The Python package installs from a locally built wheel and the example in
  60-api.md runs unchanged.
- An agent-loop integration test, using only MCP tools, lowers p95 pitch
  error of a fixture phrase below 20 cents.
- MCP conformance tests for the targeted spec revision pass.

## M4 Desktop app
Piano roll, lanes, phoneme timing lane, smart pitch handles, take lanes,
Script Log, command palette, history panel, macro recorder.
Done when:
- The UI benchmark (2,000 notes plus curves, scripted scroll/zoom) holds
  120 fps median on the reference Mac and Linux machines.
- Every GUI action in the action registry has an Op and a Python equivalent
  (a test enumerates the registry).
- The app passes the visual direction in 50-ux.md, checked against
  screenshots committed under docs/screens/.
- On macOS, CI also runs `cargo check -p app --no-default-features`
  (precompiled-shader path), skipped with a clear message when the Metal
  toolchain is absent.

## M5 Breadth
WASM phonemizer SDK (with a template repo), X-SAMPA mode, UTAU concatenative
backend, svs.io adapter, doubling, effects chain.

## M6 DAW bridge
CLAP/VST3/AU bridge that streams audio from the running app over local IPC
and syncs transport and tempo; validated with clap-validator.

## M7 Frontier
Adapters for newer flow-matching / diffusion-transformer SVS models; text
style prompts mapped onto layered parameters; audio-to-voice conversion
gated on consent attestation.
