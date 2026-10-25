# 0025 DAW bridge (M6)

- **Shape:** Cantor renders; the plugin plays. The engine publishes a
  project's mixdown into a shared-memory channel; `cantor-bridge-clap`
  (an instrument with one stereo output, no GUI) maps the channel and,
  while the host plays, outputs the audio at the host's song position,
  resampled to the host rate (linear). Project time 0 is host time 0. The
  plugin writes the host transport (playing, position in seconds and
  beats, tempo, sample rate, a heartbeat) back into the channel;
  `bridge_status` reports it and flags nothing by itself — clients compare
  `host_tempo` with the project tempo.
- **Channel** (`cantor-bridge`): a file per channel in
  `$CANTOR_BRIDGE_DIR`, else `~/Library/Caches/Cantor/bridge` (macOS),
  `$XDG_RUNTIME_DIR/cantor-bridge`, or `$TMPDIR/cantor-bridge-$USER`. A
  4 KiB header and two slots of 20 min of 48 kHz stereo each; the file is
  sparse, so only published audio uses space. The engine writes the
  inactive slot under a per-slot sequence number (odd while writing) and
  then flips the active index; the plugin copies from the active slot and
  re-checks the sequence, outputting silence for a block that raced a
  publish. Host state uses the same seqlock pattern. All shared words are
  `AtomicU64` (two f32 per frame), so neither side has data races; the
  protocol runs on plain memory in tests and under Miri and TSan.
- **Realtime:** the plugin maps the file when instantiated (main thread)
  and allocates its scratch buffer in `activate`; `process` only touches
  atomics and copies. No locks, allocation, logging or syscalls.
- **Formats:** CLAP via clack 0.2.0. VST3 and AUv2 via clap-wrapper
  v0.17.0 (`cargo xtask bridge --wrap`; it downloads the VST3 and
  AudioUnit SDKs, so it is not part of `xtask ci`), with Cantor's own AU
  codes (`aumu`/`Brdg`/`Cntr`) and the `.clap` embedded in each macOS
  bundle. On Linux the VST3 wrapper loads `CantorBridge.clap` from the
  CLAP search path.
- **Validation:** `cargo xtask bridge --validate` runs clap-validator 0.4.1
  (built from its tag in the flake; not in nixpkgs) — 18 tests pass, 26
  skip (no parameters, state, notes or GUI), 0 fail. An in-process host
  test (clack-host) checks the output against the published render at a
  song position and sample-rate conversion, silence when stopped, and the
  reported transport. macOS CI also builds the wrappers and runs `auval`.
- **Clients:** `bridge_publish` / `bridge_status` over JSON-RPC and MCP,
  `Project.publish_to_bridge()` in Python, "Send to DAW (bridge)"
  (Cmd/Ctrl+Shift+B) in the app.
- **Not done:** ARA2 (deferred by the spec); following host tempo changes
  automatically (the status reports the host tempo; changing the project
  tempo stays an explicit op); streaming while rendering (a publish is a
  whole mixdown).
