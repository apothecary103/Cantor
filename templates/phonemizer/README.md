# Cantor phonemizer template

A complete phonemizer plugin: Japanese lyrics in kana or romaji become the
romaji-style phonemes DiffSinger voicebanks use (`k a`, `sh i`, `N`, `cl`),
each with its X-SAMPA spelling.

A phonemizer is a WebAssembly component implementing the `phonemizer` world
in `cantor-plugin-sdk/wit/phonemizer.wit`. Cantor runs it in a sandbox
with no filesystem, network, environment or wall clock, and a fuel budget
per call. It must be pure: the same notes always give the same phonemes,
because rendering is deterministic.

## Build

Requires stable Rust 1.85+ with the `wasm32-wasip2` target (the Cantor Nix
shells include it).

    cargo build --release --target wasm32-wasip2

Build from outside the Cantor checkout (or after copying this directory
out): the checkout's `.cargo/config.toml` selects nightly-only settings.

## Install

Copy `target/wasm32-wasip2/release/cantor_phonemizer_ja_kana.wasm` into a
directory on Cantor's phonemizer path (`CANTOR_PHONEMIZER_PATH`, separated
like `PATH`). Cantor uses a plugin for every note whose language the plugin
lists in `info().languages`, instead of the voicebank's dictionary.

## Adapt

- `info()` names the plugin, its version (part of render cache keys: bump
  it whenever output changes) and its languages.
- `inventory(lang)` lists every symbol the plugin emits, whether it is a
  vowel, and its X-SAMPA spelling, so users can type phonemes universally.
- `phonemize(notes)` gets a whole phrase, so word context is available.
  Return one entry per note; an empty phoneme list lets Cantor handle
  continuations (`+`), rests and breaths.

Unit tests run natively: `cargo test`.
