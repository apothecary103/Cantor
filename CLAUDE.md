# Cantor

Cantor is an open-source UTAU-style singing synthesis workstation in Rust:
one `cantor` binary with a web editor, a JSON API, an MCP server and a CLI.
Synthesis is concatenative from UTAU voicebanks (oto.ini) through a built-in
PSOLA resampler. No neural models, no AI features inside the product.
Targets macOS 14+ and Linux; nothing may preclude Windows.

The main use: an AI agent gets a song (MIDI and/or audio + lyrics) and
produces a finished, well-tuned UTAU cover; a person polishes it in the
browser.

## Read first
- docs/design.md      the contract: crates, model, ops, API, MCP, UI
- docs/tuning.md      UTAU semantics and the tuning craft (with numbers)
- docs/agent-guide.md how an agent makes a cover with Cantor (served to
                      agents as the MCP resource `cantor://guide`)
- PROGRESS.md         live checklist; keep it current
- docs/decisions/     one short file per non-obvious decision

## Layout
```
crates/cantor-core      model, Op/apply/undo, tempo, params, pitch curve, tuner
crates/cantor-synth     voicebanks, phonemizers, resampler, render, cache, fx
crates/cantor-analysis  YIN F0, transcription, loudness, render reports
crates/cantor-io        MIDI/UST/USTX/MusicXML/lyrics import, UST/USTX/WAV export
crates/cantor           the binary: engine, HTTP API, MCP, CLI; embeds web/
crates/cantor-fixtures  generated CC0 test voicebank
web/                    static editor (HTML/CSS/ES modules, no build step)
```

## Commands
Dev environment is a Nix flake: `direnv allow` once, or `nix develop`.
- `cargo xtask ci`     fmt check, clippy -D warnings, all tests (the gate)
- `cargo xtask schema` regenerate schema/ from Rust types
- `cargo xtask serve`  run `cantor serve` (web UI at http://127.0.0.1:7878)
- `cargo run -p cantor -- <subcommand>`  CLI; `cantor mcp` is stdio MCP
- Web UI without a server: `python3 -m http.server -d web` and open
  `/?mock` (in-browser fake API, for UI work)

## Rules
- Every edit is an `Op` applied through `cantor-core`. The web UI, CLI and
  MCP never mutate the project directly. A capability only the UI has is a
  bug; so is one only MCP has.
- The UI and API must agree: the pitch line the UI draws comes from
  `cantor_core::pitch`, the same code the renderer uses.
- Tuner passes (`cantor_core::tune`) emit ordinary ops in one batch, are
  idempotent, and respect their range. Their rules live in docs/tuning.md;
  change both together.
- Deterministic: same project + voicebank = identical audio hash, also
  across thread counts. Randomness is seeded from note ids.
- Honest failure: unknown params, missing aliases, unsupported flags and
  unconvertible import data produce diagnostics. Never drop an edit
  silently.
- No neural models, ONNX, or downloads at runtime. Voicebanks are never
  committed; tests use the generated CC0 bank in cantor-fixtures.
- Keep it small. Prefer deleting code to adding options. No new crate,
  dependency or UI panel without a reason in docs/decisions/.
- web/ has no build step and no third-party JS. Keep the visual language:
  flat, dense, dark-first, one pink accent (see design.md "Web UI").
- JSON Schemas in `schema/` are generated (`cargo xtask schema`); don't
  hand-edit them.
- Tools come from flake.nix; don't install with brew/rustup/pip.
- The toolchain is a pinned nightly in rust-toolchain.toml. Change the date
  only there, then update the fenix `sha256` in flake.nix (set
  `lib.fakeSha256`, run `nix develop`, paste the reported hash). Never
  change it as a side effect of other work.
- `target/` may be a symlink to an external volume; if it's dangling, set
  `CARGO_TARGET_DIR`.
