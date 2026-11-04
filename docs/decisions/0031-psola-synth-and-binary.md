# 0031 Built-in TD-PSOLA synth, one `cantor` crate

Date: 2026-11-04

## Context
v2 (0029) needs a concatenative UTAU renderer and one binary. v1's synth
was built around a neural-era backend trait (WORLD features, RMVPE); its
engine/MCP/CLI crates were built for parts, takes and retakes.

## Decision
- `cantor-synth` is new and small: voicebank loading (oto.ini in the
  folder and subfolders, UTF-8 or Shift-JIS; prefix.map; character.txt),
  lyric → alias resolution (CV, VCV with the previous vowel, romaji,
  prefix-map tiers, raw), and TD-PSOLA from `.frq` or YIN pitch marks.
  Grains are not resampled, so formants stay put; `gen` and the `g` flag
  scale the grain read rate. Placement follows UTAU: preutterance before
  the onset, overlap crossfade, velocity scales the consonant, both shrink
  when they'd take more than half the previous note. Unsupported curves
  (`bre`, `bri`, `tone`) and flags produce diagnostics.
- Each phrase is rendered independently on scoped threads and cached by
  a blake3 hash of its inputs (sample, timing, 5 ms pitch/dyn/formant
  grids); output is identical across thread counts.
- `crates/cantor` is a library (engine, http, mcp) plus the CLI binary,
  replacing cantor-server, cantor-mcp and cantor-cli. Uploads are raw
  bytes (`?name=&kind=`), so axum needs no multipart support.
- New direct dependency: `tokio-stream` (already in the tree via rmcp)
  for the SSE stream of `/api/events`.

## Consequences
Breathiness, brightness and tone are not rendered yet; CVVC VC links and
ARPAsing are not phonemized (warnings say so). The old crates remain in
git history (82b5133).
