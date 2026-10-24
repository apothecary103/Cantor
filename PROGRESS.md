# Progress

Spec: docs/spec/70-milestones.md. Gate: `nix develop --command cargo xtask ci`
on macOS and Linux in CI.

## M1 Core — done (CI green at 3b0ecee)
- [x] tick → sec → tick property test across random tempo maps
- [x] apply-then-undo of random Op sequences restores byte-identical project
- [x] Importers load fixtures; insta snapshots committed
- [x] `cargo xtask schema` regenerates schema/ with no diff in CI
- [x] `nix flake check` and `xtask ci` pass on macOS and Linux in CI; fenix
      hash filled in
- [x] CI runs miri (cantor-core), stable check of cantor-plugin-sdk, TSan stub
- [x] Debug builds use Cranelift (workspace crates), release builds LLVM;
      asserted by `cargo xtask codegen-check`

## M2 Neural render — in progress
Parts:
- [x] ONNX Runtime 1.31.0 official release in the flake (0009)
- [x] cantor-render: phrases, base pitch, cache keys, disk cache, mixing,
      render trace, WAV output
- [x] cantor-phon: dsdict dictionaries, lyric parsing, phoneme placement
- [x] cantor-backends: DiffSinger (duration, pitch, variance, acoustic) +
      PC-NSF-HiFiGAN; deterministic noise via in-memory graph rewrite (0010)
- [x] cantor-fixtures: pinned voicebank/vocoder downloads by SHA-256
- [x] cantor-cli: `cantor render`, `cantor import`, `cantor ops apply`

Done when:
- [x] `cantor render` produces a WAV from a public DiffSinger voicebank
      fixture with ONNX Runtime from ORT_DYLIB_PATH
      (`crates/cantor-cli/tests/render_cli.rs`) — local; CI pending
- [x] Rendering the same project twice yields identical audio hashes
      (`crates/cantor-backends/tests/diffsinger.rs`, warm and cold cache)
- [x] Re-rendering an unchanged project hits the cache for 100% of phrases
- [x] A pitch-only edit re-renders through the vocoder fast path
      (render-trace tests with the real voicebank and a synthetic backend)

## Open items
- Confirm with OpenVPI that the in-memory noise rewrite fits the Qixuan
  terms of use (0010).
- The spec's 40-algorithms §6 should say the mel uses the natural log.
- Model fixtures are ~470 MB; CI caches them. A small Apache-2.0 fixture
  model would cut CI time and licence questions.
- Not rendered yet (reported as warnings): effects, doubling, voice mix,
  portamento overshoot/preparation, attack/release.
- Importer diagnostics use "1 notes"-style plurals.
- USTX: curves other than `pitd` and phoneme overrides are reported, not
  converted (0005).
- TSan crate list holds only cantor-core until cantor-audio exists.
