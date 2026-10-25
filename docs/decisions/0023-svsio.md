# 0023 svs.io adapter

OpenUtau's svs.io proposal (wiki, 2022, revised 2025) is a sketch: no
reference implementation, no error format, inconsistent examples. Cantor
pins this interpretation:

- **Transport:** ZeroMQ REQ/REP over TCP, one msgpack map per message
  (the proposal's preferred transport). Cantor speaks ZMTP 3.0 with NULL
  security itself (`cantor_backends::zmtp`, ~200 lines) instead of linking
  libzmq; tests run against pyzmq, so it interoperates with libzmq peers.
  `ops` must answer within 1 s (the proposal's liveness rule); other
  calls may take 120 s. A lost connection is retried once.
- **Messages:** `{"op": "ops"}` → `{name, version, ops: [{op, inputs,
  outputs}]}` with struct names as keys and descriptors as values
  (`required`, `time_unit`, `frame_duration`, `width`, `sample_rate`, …).
  Calls are `{"op": name, <struct>: value, …}`; replies map output struct
  names to values. A reply with an `error` string is a failure (Cantor's
  addition; the proposal defines no errors).
- **Structs:** as proposed — `note_sequence` (`lyric` "" = rest,
  `duration`, MIDI `key`), `phoneme_sequence` (`note_index`, `phoneme`,
  `duration`, `key`), `f0` in cents, `curve:<name>`, `world_sp` /
  `world_ap` (one array per frame, `width` bins), `audio_samples`
  (`sample_rate`, `sample_format` int8/int16/int32/float, `samples` flat
  or one array per channel; the proposal's example is malformed).
  `frame_duration` is accepted as a number or a string; `time_unit` s, ms
  (default) or us. Cantor sends ms. Curves are offered under Cantor's
  parameter ids (`curve:breathiness`), in Cantor's units.
- **Graph:** Cantor supplies `note_sequence`, `f0` and `curve:*`; it picks
  an op producing `phoneme_sequence` (optional), one producing `f0`
  (optional pitch model) and a synthesizer: `world_sp` + `world_ap`
  preferred (Cantor then runs WORLD synthesis locally, so pitch edits are
  vocoder-fast-path and doubling can warp formants), else
  `audio_samples` (each vocode is a remote call; `prefer_audio` forces
  this). `world_mgc`/`world_bap` are not decoded.
- **Singers:** a singer folder with `svsio.yaml` (`endpoint`,
  `prefer_audio`). Cantor connects to a running backend; launching it is
  the user's job. The proposal has no singer selection.
