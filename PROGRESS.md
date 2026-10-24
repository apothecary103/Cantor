# Progress: M1 Core

Spec: docs/spec/70-milestones.md. Gate: `nix develop --command cargo xtask ci`
on macOS and Linux in CI.

## Parts
- [x] Workspace, pinned nightly (rust-toolchain.toml), fenix hash filled in
- [x] cantor-core: document model, canonical form, validation
- [x] cantor-core: Op system with exact inverses, atomic batches, diagnostics
- [x] cantor-core: undo tree with branches, redo, checkout, Arc snapshots
- [x] cantor-core: tempo and meter math
- [x] cantor-core: parameter registry and schema registry
- [x] cantor-io: .cantor save/load (JSON, zstd) with forward migrations
- [x] cantor-io: USTX importer
- [x] cantor-io: MIDI importer
- [x] cantor-io: MusicXML importer
- [x] cantor-plugin-sdk stub (stable Rust)
- [x] xtask: ci, schema, codegen-check, miri, tsan
- [x] CI workflow: macOS + Linux, miri, stable SDK, tsan
- [x] Decision records 0001–0008

## Done when
- [x] tick → sec → tick property test across random tempo maps
      (`cantor-core` `time::tests::tick_sec_tick_round_trips`)
- [x] apply-then-undo of random Op sequences restores byte-identical project
      (`crates/cantor-core/tests/ops_undo.rs`)
- [x] Importers load fixtures; insta snapshots committed
      (`crates/cantor-io/tests/snapshots/`)
- [x] `cargo xtask schema` regenerates schema/ with no diff in CI
- [ ] `nix flake check` and `nix develop --command cargo xtask ci` pass on
      macOS and Linux in CI; no `lib.fakeSha256` in flake.nix
      (local macOS: passing; CI: pending first run)
- [ ] CI runs `cargo xtask miri` (cantor-core), stable check of
      cantor-plugin-sdk, and a ThreadSanitizer stub (local: all pass; CI: pending)
- [x] Debug builds use Cranelift, release builds use LLVM, asserted by
      `cargo xtask codegen-check` (part of `xtask ci`)

## Open items for later milestones
- M2: nixpkgs ships onnxruntime 1.27.1; ort 2.0.0-rc.15 needs 1.31. Per
  CLAUDE.md, fetch the official ONNX Runtime release in flake.nix then.
- M2/M3: `Retake` op (needs phrase detection and a backend).
- TSan crate list holds only cantor-core until cantor-audio exists.
- Importer diagnostics use "1 notes"-style plurals.
- USTX: curves other than `pitd` and phoneme overrides are reported, not
  converted (units unconfirmed); see 0005.
