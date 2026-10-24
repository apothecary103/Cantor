# Architecture

## Crates
cantor-core       document model, Ops, undo tree, schema registry, tempo math
cantor-phon       phonemizer host (WASM), dictionaries, G2P, X-SAMPA mapping
cantor-plugin-sdk guest-side SDK and WIT bindings for plugin authors
                  (stable Rust only)
cantor-render     phrase scheduler, render graph, content-addressed cache
cantor-backends   diffsinger (variance + acoustic), vocoders, world, utau
cantor-analysis   F0 (RMVPE), loudness, timing, artifact detectors
cantor-audio      cpal output, realtime mixer, transport, metering
cantor-io         .cantor, USTX, UST, MusicXML, MIDI, DS, WAV
cantor-server     JSON-RPC 2.0 (WebSocket + stdio)
cantor-mcp        MCP server on top of cantor-server
cantor-py         Python bindings
cantor-bridge     CLAP/VST3/AU bridge plugin
app/              GPUI desktop application
xtask/            build, CI, schema generation, Miri/TSan runs, packaging

## Threads
- UI thread (GPUI): renders views; sends Ops; never blocks on inference.
- Engine thread: owns the Document; applies Ops; publishes immutable
  snapshots (Arc<DocumentSnapshot>) to everyone else.
- Render workers: phrase rendering, prioritized by distance from the
  playhead; inference sessions cached per model and warmed on singer load.
- Analysis workers: run after each phrase render.
- Audio thread (cpal callback): mixes pre-rendered phrase buffers; realtime-safe.

## Data flow
Op → engine validates → new revision + snapshot → render scheduler marks
dirty phrases by cache key → workers render → audio buffers swapped into the
mixer → analysis results published → UI and API clients notified.

## Optional external backends
An adapter speaks OpenUtau's proposed svs.io protocol (msgpack; the backend
describes its own op graph), so third-party engines can plug in without
recompiling Cantor.
