# Locked stack (verified October 2026)

Pin exact versions in Cargo.lock. Upgrading a pinned dependency is its own
change with its own passing CI run.

| Area | Choice | Reason |
|---|---|---|
| GUI | gpui-kit 0.8.x (re-exports GPUI, gpui-base, gpui-component) | Zed-grade GPU rendering; one dependency pins a matching GPUI; 60+ components with a11y |
| Audio I/O | cpal 0.18.x; feature `pipewire` on Linux, CoreAudio on macOS | native PipeWire; timestamps reflect hardware output time |
| Realtime plumbing | rtrb + a small custom mixer | Firewheel and similar engines are game-oriented; our needs are narrow |
| Inference | ort = "=2.0.0-rc.15" (ONNX Runtime 1.31), feature `load-dynamic` | runs existing DiffSinger ONNX voicebanks; library supplied by Nix |
| Execution providers | macOS: CoreML → CPU. Linux: CUDA/TensorRT if present → CPU | user-selectable, auto-detected |
| Plugins & scripts | wasmtime 46+, WASI 0.3 components | sandboxed, native async, any language |
| Python | pyo3 0.29 + maturin; ship abi3 and abi3t wheels | covers GIL and free-threaded Python |
| MCP | rmcp 3.x (MCP 2026-07-28, stateless) | official SDK; fallback rust-mcp-sdk 2.x if blocked |
| JSON-RPC | tokio + axum (WebSocket) and stdio | for non-MCP clients and the GUI's remote mode |
| DAW bridge | clack + clap-wrapper (CLAP → VST3/AU) | thin plugin, no GUI inside; ARA2 deferred |
| DSP | rubato, realfft, ebur128, symphonia, hound | resample, FFT, BS.1770 loudness, decode, WAV |
| Formats | quick-xml (MusicXML), midly (MIDI), serde_json + zstd | |
| Core | serde, schemars, blake3, thiserror, tracing, clap (CLI) | |
| Tests | proptest, insta, criterion | |

Packaging: macOS signed + notarized .app in .dmg; Linux Flatpak (primary)
and AppImage. CI matrix: macos-latest + ubuntu-latest.

Known risks and what we do about them:
- gpui-kit builds on gpui-pre, a single-maintainer snapshot of Zed's GPUI.
  Mitigation: the GUI only talks to cantor-core's public API, so a toolkit
  swap touches app/ only.
- ort 2.0 is still a release candidate. Mitigation: an exact pin and an
  inference smoke test in CI.
- The next-generation trait solver (default on nightly) may break a
  dependency, most likely GPUI. Mitigation: fall back with
  `-Znext-solver=coherence` in .cargo/config.toml and record why.

## Toolchain: pinned nightly

We build with a dated nightly (see rust-toolchain.toml) to get the
next-generation trait solver, TAIT/RTN, portable SIMD, f16, the parallel
front end, Cranelift debug builds, sanitizers, and Miri.

Allowed unstable features. Each crate opts in explicitly with
`#![feature(...)]`; anything not on this list needs a decision record first.
| Feature | Where | Why |
|---|---|---|
| portable_simd | cantor-audio, cantor-analysis, cantor-render | SIMD mixing, resampling, analysis |
| f16 | cantor-backends | half-precision tensors at the ONNX boundary |
| type_alias_impl_trait | cantor-render, cantor-server | name async return types in the scheduler and server |
| return_type_notation | cantor-render, cantor-server | Send bounds on async trait methods |
| try_blocks | any engine crate | local error handling without helper fns |

Forbidden: features the compiler marks incomplete
(`#![allow(incomplete_features)]` is never allowed).

Stable-only crates: `cantor-plugin-sdk` and the phonemizer template must
build on the latest stable Rust; CI checks them with a stable toolchain.

Release builds use LLVM only. Cranelift is a dev-build speedup.

Toolchain bumps: one PR per bump, titled "toolchain: nightly-YYYY-MM-DD".
Change the date in rust-toolchain.toml, update the fenix `sha256` in
flake.nix, and pass the full CI matrix plus the Miri and ThreadSanitizer
jobs. If a bump breaks, stay on the old date and record the blocker in
docs/decisions/.

## Development environment: Nix

- A Nix flake (flake-parts + fenix) provides the whole toolchain on macOS
  (nix-darwin) and Linux. fenix's `fromToolchainFile` reads
  rust-toolchain.toml, so the pinned nightly lives in one place; its
  `sha256` in flake.nix must be updated whenever the date changes.
  direnv + nix-direnv enters the shell automatically.
- macOS: runtime GPUI shaders in dev; precompiled shaders in release builds,
  which run outside Nix with full Xcode.
- Linux: the flake supplies Vulkan, Wayland, X11, xkbcommon, fontconfig,
  ALSA and PipeWire, and exports LD_LIBRARY_PATH so GPUI and cpal find them
  at runtime.
- ONNX Runtime comes from nixpkgs and is loaded dynamically by ort via
  ORT_DYLIB_PATH.
- CI runs the same flake: `nix develop --command cargo xtask ci`.
- Distribution packaging (dmg, Flatpak, AppImage) stays as specified; a Nix
  package for the app (`nix build`) is a nice-to-have after M4, not a
  requirement.
