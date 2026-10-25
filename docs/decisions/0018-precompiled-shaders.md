# 0018 Precompiled Metal shaders (vendored gpui-kit)

- **Problem:** gpui-kit 0.8.0 forces GPUI's `runtime_shaders` on its
  platform dependencies, so `--no-default-features` could not reach the
  precompiled-shader path the spec asks for (0017). gpui-base enables it
  only for its own dev-dependencies, which do not reach us.
- **Decision:** vendor gpui-kit 0.8.0 under `vendor/gpui-kit` and patch it
  in with `[patch.crates-io]`. The only source change is in its manifest:
  `runtime_shaders` moves into a default `runtime-shaders` feature
  (forwarding to `gpui_platform` and, when enabled, `gpui-fast-platform`).
  Test targets and dev-dependencies are dropped; `src/` is unchanged.
  The workspace takes gpui-kit with `default-features = false` plus
  `component` and `assets`; the app's default `runtime-shaders` feature
  forwards to `gpui-kit/runtime-shaders`. The app still depends on gpui-kit
  only, and gpui-kit still pins the matching GPUI.
- **Check:** `cargo xtask shader-check` (macOS) finds the system Xcode with
  `xcode-select -p` (DEVELOPER_DIR cleared, since the Nix shell points it at
  its SDK), puts an `xcrun` shim first on PATH that runs `/usr/bin/xcrun`
  against that Xcode, runs `cargo check -p app --no-default-features`, and
  fails unless GPUI's build script produced `shaders.metallib`. Without the
  Metal toolchain it skips with a message. Only `xcrun` is redirected; the
  compiler and linker remain the Nix toolchain.
- **Release builds** outside Nix need nothing special: `cargo build
  --release -p app --no-default-features` with Xcode installed. Verified on
  the maintainer's Mac (from the Nix shell with the `xcrun` shim): the
  release build produced `shaders.metallib` and `cantor-app --bench 600`
  ran at 120.0 fps median presented, 1.43 ms median frame work.
- **Exit:** drop the vendored copy once upstream gpui-kit exposes the
  toggle (not yet requested upstream). Bumping gpui-kit means re-vendoring
  and reapplying the manifest change.
