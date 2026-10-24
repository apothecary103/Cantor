# Cantor

Cantor is an open-source singing voice synthesis workstation written in Rust:
a headless engine with four equal clients — a GPUI desktop app, a Python
package, a CLI, and an MCP server for AI agents. Targets: macOS 14+ and
Linux (Wayland/X11, PipeWire). Windows is out of scope for v1 but nothing
may preclude it.

## Where things are explained
Read the spec file that matches the work; you don't need all of them.
- docs/spec/00-vision.md       why the product exists, the principles
- docs/spec/10-stack.md        locked dependencies, toolchain, dev environment
- docs/spec/20-architecture.md crates, threads, data flow
- docs/spec/30-data-model.md   the document model as Rust types
- docs/spec/40-algorithms.md   synthesis pipeline and formulas
- docs/spec/50-ux.md           manual editing UX and visual design
- docs/spec/60-api.md          Python, CLI, JSON-RPC and MCP surfaces
- docs/spec/70-milestones.md   milestones and their "done when" criteria
- docs/spec/80-research.md     background on models, vocoders, competitors
PROGRESS.md is the live checklist for the current milestone. Keep it current.
Non-obvious decisions go in docs/decisions/, one short file each.

## Commands
The dev environment is a Nix flake. Enter it with `direnv allow` (once) or
`nix develop`. All commands below run inside it.
- `cargo xtask ci`     fmt check, clippy -D warnings, all tests (the gate)
- `cargo xtask miri`   Miri on crates with unsafe code
- `cargo xtask tsan`   ThreadSanitizer run for the audio and engine threads
- `cargo xtask schema` regenerate JSON Schemas from Rust types
- `cargo xtask app`    run the desktop app
- `cargo xtask bench`  render + UI benchmarks
- `nix flake check`    validate the flake

## Gotchas
- Every edit to a project is an `Op` applied through `cantor-core`.
  The GUI, Python, CLI and MCP never mutate the document directly.
  A capability only the GUI has is a bug.
- The GUI depends on `gpui-kit` only. Never add gpui, gpui-pre or gpui-ce
  directly; gpui-kit pins the matching GPUI.
- Upstream GPUI has no custom shaders. Draw with quads and paths; render
  waveforms and spectrograms to textures off the UI thread.
- macOS dev builds use GPUI's runtime shaders, because Xcode's Metal compiler
  isn't available in the Nix shell. The app crate has a default
  `runtime-shaders` feature forwarding to gpui-kit's runtime-shader feature
  (look up its exact name in the pinned gpui-kit). Release builds for macOS
  run outside Nix with full Xcode, using `--no-default-features` so shaders
  are precompiled.
- The audio callback never allocates, locks, logs or blocks. Talk to it
  through `rtrb` ring buffers and atomically swapped `Arc` buffers.
- ort is pinned to an exact release candidate and uses the `load-dynamic`
  feature with ORT_DYLIB_PATH from the flake, not ort's binary download.
  If nixpkgs' onnxruntime is older than the pinned ort requires, or lacks
  the CoreML execution provider on macOS, fetch the official ONNX Runtime
  release in flake.nix instead and record the decision.
- Tools come from flake.nix. Don't install tools with brew, rustup or pip;
  add them to the flake instead.
- The toolchain is a pinned nightly. fenix reads it from rust-toolchain.toml,
  so change the date only there, and then update the `sha256` in flake.nix
  (set it to `lib.fakeSha256`, run `nix develop`, paste the reported hash).
  Don't change the date as a side effect of other work.
- Use only the unstable features on the allow-list in docs/spec/10-stack.md.
  `cantor-plugin-sdk` must compile on stable Rust; no unstable features there.
- The realtime mixer and anything with `unsafe` must pass `cargo xtask miri`
  where Miri supports it, and `cargo xtask tsan`.
- Rendering is deterministic: same document + same seeds = identical
  audio hash. Diffusion noise uses the per-phrase seed stored in the project.
- JSON Schemas in `schema/` are generated from Rust types with `schemars`.
  Don't hand-edit them; run `cargo xtask schema`.
- Unsupported parameters return a diagnostic. Never drop an edit silently.
- Model weights are never committed. Tests download fixtures by checksum.
