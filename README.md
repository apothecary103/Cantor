# Cantor

**An open, fully scriptable singing voice synthesis workstation, built in Rust.**

Cantor turns notes and lyrics into sung vocals. One engine serves three kinds
of users equally:

- **Musicians** editing by hand in a fast, precise desktop editor.
- **Scripters** automating anything through Python or the CLI.
- **AI agents** composing, tuning, rendering and evaluating vocals end to end
  over MCP, with numeric feedback on every render.

> **Status: pre-alpha.** Cantor is being built milestone by milestone (see
> [Roadmap](#roadmap)). Most features below are planned, not yet available.

## Why Cantor

- **Headless engine, equal clients.** The desktop app, Python package, CLI and
  MCP server all drive the same operation API. Nothing is GUI-only.
- **Every edit is an operation.** Undo branches, macros, replay, diffs and AI
  dry-runs come for free. Each GUI action shows its Python equivalent in a
  Script Log, so manual work turns into automation naturally.
- **Deterministic rendering.** The same project and seeds always produce
  identical audio.
- **Closed loop.** Every render reports pitch error, timing, loudness and
  artifacts, so both people and agents can see exactly what came out.
- **Works with existing voices.** Runs DiffSinger ONNX voicebanks and classic
  UTAU (oto.ini) banks.

## Planned features

**Editing:** piano roll with pitch, phoneme and parameter lanes; smart pitch
handles; phoneme timing lane; retakes kept as switchable take lanes; layered
parameters (model prediction + your edits); microtonal pitch; command palette;
branching history with A/B compare; macro recorder.

**Synthesis:** DiffSinger variance and acoustic models, PC-NSF-HiFiGAN vocoder
with a fast path for pitch-only edits, incremental phrase rendering with a
content-addressed cache, voice doubling, built-in effects chain.

**Languages and phonemes:** WASM phonemizer plugins, neural G2P fallback,
universal X-SAMPA input.

**Interop:** import MusicXML, MIDI, USTX, UST and DiffSinger `.ds`; CLAP/VST3/AU
bridge plugin for DAWs; external backends via OpenUtau's proposed svs.io protocol.

## Platforms

| Platform | Status |
|---|---|
| macOS 14+ (Apple Silicon) | primary target |
| macOS (Intel) | best effort |
| Linux (Wayland/X11, PipeWire) | primary target |
| Windows | not in v1 |

## Getting started (development)

The development environment is a Nix flake that provides everything,
including the pinned nightly Rust toolchain (via fenix), ONNX Runtime and the
native libraries.

### Prerequisites

- [Nix](https://nixos.org) with flakes enabled (works with nix-darwin)
- [direnv](https://direnv.net) with [nix-direnv](https://github.com/nix-community/nix-direnv) (recommended)

### Enter the environment

```sh
git clone <repo-url> cantor
cd cantor
direnv allow        # or: nix develop
```

### Common commands

```sh
cargo xtask ci       # format check, clippy, all tests (the CI gate)
cargo xtask app      # run the desktop app
cargo xtask bench    # render and UI benchmarks
cargo xtask schema   # regenerate JSON Schemas from Rust types
cargo xtask miri     # Miri on crates with unsafe code
cargo xtask tsan     # ThreadSanitizer for the audio and engine threads
nix flake check      # validate the flake
```

### Voicebanks and models

Model weights are never stored in this repository. Tests download fixtures
by checksum into `tests/fixtures/downloaded/`. For your own use, add
voicebanks through the app or the CLI and respect each voicebank's license.

## Using Cantor from code (planned API)

```python
import cantor as ct

p = ct.open("song.cantor")
with p.transaction("soften chorus ends"):
    for n in p.track("lead").notes(section="chorus"):
        if n.is_phrase_end:
            n.curve("breathiness").ramp(to=0.4, over="last 30%")

report = p.render(quality="preview")
print(report.summary())
```

```sh
cantor render song.cantor -o out.wav --quality final
cantor analyze song.cantor --json
```

AI agents connect through the MCP server (spec revision 2026-07-28) and get
tools to describe, edit, retake, render and compare projects. See
[docs/spec/60-api.md](docs/spec/60-api.md).

## Project layout

```
crates/       engine crates (core, phon, render, backends, analysis, audio,
              io, server, mcp, py, bridge, plugin-sdk)
app/          GPUI desktop application
xtask/        build, CI, schema generation, packaging
schema/       generated JSON Schemas (do not edit by hand)
docs/spec/    product and technical specification
docs/decisions/  short records of non-obvious decisions
```

## Documentation

| Document | Contents |
|---|---|
| [00-vision](docs/spec/00-vision.md) | goals and principles |
| [10-stack](docs/spec/10-stack.md) | dependencies, toolchain, Nix setup |
| [20-architecture](docs/spec/20-architecture.md) | crates, threads, data flow |
| [30-data-model](docs/spec/30-data-model.md) | project model and operations |
| [40-algorithms](docs/spec/40-algorithms.md) | synthesis pipeline and formulas |
| [50-ux](docs/spec/50-ux.md) | editor UX and visual design |
| [60-api](docs/spec/60-api.md) | Python, CLI, JSON-RPC, MCP |
| [70-milestones](docs/spec/70-milestones.md) | roadmap and acceptance criteria |
| [80-research](docs/spec/80-research.md) | background and prior art |

## Roadmap

- [ ] **M1 Core**: document model, operations, undo tree, importers, Nix + CI
- [ ] **M2 Neural render**: DiffSinger + vocoder, phrase cache, deterministic output
- [ ] **M3 Analysis and APIs**: analysis reports, JSON-RPC, Python, MCP
- [ ] **M4 Desktop app**: piano roll, lanes, takes, Script Log, command palette
- [ ] **M5 Breadth**: phonemizer SDK, X-SAMPA, UTAU backend, effects, doubling
- [ ] **M6 DAW bridge**: CLAP/VST3/AU
- [ ] **M7 Frontier**: newer SVS model adapters, style prompts, voice conversion

## Contributing

- Every change must pass `cargo xtask ci` inside the Nix shell.
- The toolchain is a pinned nightly. Changing it is a dedicated PR; see
  [10-stack](docs/spec/10-stack.md#toolchain-pinned-nightly).
- Only the unstable Rust features on the allow-list may be used, and
  `cantor-plugin-sdk` must build on stable Rust.
- AI coding agents: start with [CLAUDE.md](CLAUDE.md).

## Ethics

Voicebanks declare their license terms, and Cantor shows them in the app,
the API and at export. Voice cloning and conversion require a consent
attestation from the voice owner; Cantor refuses to load a cloned voice
without one. Don't use Cantor to imitate anyone's voice without permission.

## License

TBD. Planned: dual-licensed under MIT or Apache-2.0, at your option.
Third-party components keep their own licenses; see THIRD_PARTY_NOTICES.

## Acknowledgements

Cantor builds on ideas and work from [OpenUtau](https://github.com/openutau/OpenUtau),
[DiffSinger](https://github.com/openvpi/DiffSinger) and the OpenVPI community,
[Zed's GPUI](https://github.com/zed-industries/zed), and
[GPUI Kit](https://github.com/longbridge/gpui-kit).
