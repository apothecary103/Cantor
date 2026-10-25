# 0017 Desktop app (M4)

- **Toolkit:** gpui-kit 0.8.0 only (it pins gpui-pre 0.3.9). GPUI builds on
  the pinned nightly with the next-generation trait solver; no
  `-Znext-solver=coherence` fallback was needed.
- **Runtime shaders — spec mismatch.** CLAUDE.md asks for an app feature
  `runtime-shaders` forwarding to gpui-kit's runtime-shader feature, with
  release builds using `--no-default-features` for precompiled shaders.
  gpui-kit 0.8.0 has no such feature: it enables `gpui_platform`'s
  `runtime_shaders` unconditionally, and Cargo features cannot be turned
  off downstream. The app keeps a `runtime-shaders` default feature as a
  marker and `cargo xtask shader-check` runs `cargo check -p app
  --no-default-features` on macOS when Xcode's Metal compiler is present
  (skipping with a message otherwise), but that build still uses runtime
  shaders until gpui-kit exposes a toggle. Upstream ask: make
  `runtime_shaders` a gpui-kit feature.
- **Architecture:** `cantor_app::state::Editor` holds the view model
  (viewport, selection, cursor, Script Log, macro recording) and applies
  every edit as an Op through `cantor_server::Engine` — the same API the
  CLI, Python and MCP use. Renders run on GPUI's background executor.
  Drawing is quads, paths and shaped text in canvases (no custom shaders);
  shaped text is cached across frames.
- **Action registry** (`app/src/actions.rs`): every palette entry and key
  binding. Edit actions declare their Op tag, a builder producing the
  concrete Op from the editor context, and a Python method; a test builds
  each Op against a fixture, applies it, checks the tag against the Op
  JSON Schema and the Python method against the package stubs. History
  actions (undo/redo/checkpoint) are not Ops (they move through the undo
  tree) and carry Python equivalents only; view actions carry neither.
  The Script Log shows each applied Op as the Python line that reproduces
  it (`python_line`), and the macro recorder saves those lines as a
  runnable script.
- **Keys:** handled in one `on_key_down` that maps keystrokes to registry
  ids, so bindings live in the registry (rebinding is a data change).
- **Benchmark:** `cargo xtask bench` runs the release app with a
  2,000-note, two-track demo with curves and scripted scroll/zoom for 600
  frames in a real window. It reports the median frame interval (presented
  fps) and CPU frame work (render, layout and paint of every element) and
  fails below 120 fps median presented. A first version timed only the
  piano-roll paint, which overstated headroom; the frame work is now
  measured to the last painted element. On the maintainer's M-series Mac
  (120 Hz XDR): 120.2 fps median presented, 0.43 ms CPU per frame; the
  whole-run average is lower (65 fps) because of startup and occasional
  long frames. The Linux reference machine still needs a run.
- **Screenshots:** `app/tests/screens.rs` renders the editor (dark and
  light) headlessly with GPUI's real Metal renderer and compares against
  `docs/screens/*.png` (mean absolute difference < 3); `UPDATE_SCREENS=1`
  rewrites them. Linux runs skip (references use the macOS text system).
- **Not in M4:** audio playback (cantor-audio, the realtime mixer, is not
  part of any milestone's done-when yet), the expression pad, the Python
  console, voice-to-MIDI and vocal-mode selectors (no backend modes yet).
