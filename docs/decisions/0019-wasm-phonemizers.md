# 0019 WASM phonemizer plugins

- **Contract:** the `phonemizer` world in
  `crates/cantor-plugin-sdk/wit/phonemizer.wit` (`cantor:plugin@0.1.0`):
  `info()`, `inventory(lang)` (symbols, vowel/consonant, X-SAMPA) and
  `phonemize(notes)` over a whole phrase, so plugins see word context.
  An empty phoneme list hands a note back to the host (continuations,
  rests, breaths, unknown words). Plugin API version 1; the host refuses
  other versions.
- **WASI p2, not 0.3.** WASI 0.3 is ratified, but Wasmtime 50 marks its p3
  support experimental and stable Rust only gains `wasm32-wasip3` in 1.100.
  Plugins are `wasm32-wasip2` components built on stable; the host links
  WASI p2 (`wasmtime-wasi` with only the `p2` feature). Moving to 0.3 is a
  host change plus a rebuild of plugins, once both sides are stable.
- **Sandbox and determinism:** no preopens, environment, arguments or
  sockets; stdout/stderr discarded; frozen wall and monotonic clocks;
  seeded randomness; 512 MiB memory cap; 5·10⁹ fuel per call. A trap
  (out of fuel, out of memory, guest panic) poisons a component instance,
  so the host re-instantiates it and reports the error. Plugins must be
  pure; the sandbox removes every ambient input that could make them not.
- **Selection:** `CANTOR_PHONEMIZER_PATH` (engine `phonemizer_paths`) is
  scanned once, `*.wasm` sorted by path; the first plugin listing a note's
  language phonemizes it instead of the voicebank dictionary. Plugin
  output the voicebank lacks, plugin errors and plugin warnings become
  diagnostics, and the dictionary handles the note. X-SAMPA overrides use
  the plugin's declared spellings for its languages.
- **Cache keys:** plugin fingerprints (name, version, BLAKE3 of the bytes)
  are appended to the DiffSinger backend version, so renders made with a
  different phonemizer are never reused.
- **SDK and template:** `cantor-plugin-sdk` (stable, wit-bindgen 0.63)
  re-exports the generated types, `Phonemizer` (the generated `Guest`
  trait), `export_phonemizer!` and the host's lyric conventions.
  `templates/phonemizer` is a complete Japanese kana/romaji phonemizer,
  standalone (own `[workspace]` and lockfile) so it can be copied into a
  repository of its own. CI builds it on stable as a component; host
  tests build it and a hostile probe plugin with the pinned nightly.
- **Not done:** neural G2P fallback (40-algorithms §1) needs a small ONNX
  seq2seq model with a usable licence; plugins can embed their own.
