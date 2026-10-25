# Progress

Spec: docs/spec/70-milestones.md. Gate: `nix develop --command cargo xtask ci`
on macOS and Linux in CI.

## M1 Core — done (CI green at 3b0ecee)
- [x] tick → sec → tick property test across random tempo maps
- [x] apply-then-undo of random Op sequences restores byte-identical project
- [x] Importers load fixtures; insta snapshots committed
- [x] `cargo xtask schema` regenerates schema/ with no diff in CI
- [x] `nix flake check` and `xtask ci` pass on macOS and Linux in CI; fenix
      hash filled in
- [x] CI runs miri (cantor-core), stable check of cantor-plugin-sdk, TSan stub
- [x] Debug builds use Cranelift (workspace crates), release builds LLVM;
      asserted by `cargo xtask codegen-check`

## M2 Neural render — done (xtask ci green on macOS + Linux at 775899b)
Parts:
- [x] ONNX Runtime 1.31.0 official release in the flake (0009)
- [x] cantor-render: phrases, base pitch, cache keys, disk cache, mixing,
      render trace, WAV output
- [x] cantor-phon: dsdict dictionaries, lyric parsing, phoneme placement
- [x] cantor-backends: DiffSinger (duration, pitch, variance, acoustic) +
      PC-NSF-HiFiGAN; deterministic noise via in-memory graph rewrite (0010)
- [x] cantor-fixtures: pinned voicebank/vocoder downloads by SHA-256
- [x] cantor-cli: `cantor render`, `cantor import`, `cantor ops apply`

Done when:
- [x] `cantor render` produces a WAV from a public DiffSinger voicebank
      fixture with ONNX Runtime from ORT_DYLIB_PATH
      (`crates/cantor-cli/tests/render_cli.rs`)
- [x] Rendering the same project twice yields identical audio hashes
      (`crates/cantor-backends/tests/diffsinger.rs`, warm and cold cache)
- [x] Re-rendering an unchanged project hits the cache for 100% of phrases
- [x] A pitch-only edit re-renders through the vocoder fast path
      (render-trace tests with the real voicebank and a synthetic backend)

## M3 Analysis, server, Python, MCP — done locally; CI pending
Parts:
- [x] cantor-analysis: pitch (score-relative, error contours), timing,
      BS.1770 loudness and true peak, artifact detectors (0014)
- [x] RMVPE F0 extraction (MIT ONNX + Rust front end) with F0 cache
- [x] Renders return the full analysis report; range rendering
- [x] Sections, Retake/take lanes, phrase spans in cantor-core; format v2
- [x] cantor-server: engine API with base_revision conflicts, checkpoints,
      compare, singer registry; JSON-RPC 2.0 over stdio and WebSocket (0015)
- [x] cantor-mcp: tools, resources, prompts on rmcp 3.7 (2026-07-28),
      stdio + Streamable HTTP
- [x] cantor-py: `cantor` package (abi3), stubs, transactions (0016)

Done when:
- [x] render returns the full analysis report described in 40-algorithms.md
      (`cantor-backends/tests/diffsinger.rs::render_returns_an_analysis_report`)
- [x] The Python package installs from a locally built wheel and the example
      in 60-api.md runs unchanged (`cargo xtask python`)
- [x] An agent-loop integration test using only MCP tools lowers p95 pitch
      error of a fixture phrase below 20 cents
      (`cantor-mcp/tests/agent_loop.rs`: 76.3 → 6.1 cents)
- [x] MCP conformance tests for 2026-07-28 pass (`cargo xtask
      conformance`: 197 checks, 0 failures, fixture server; 0013)

## Open items
- Confirm with OpenVPI that the in-memory noise rewrite fits the Qixuan
  terms of use (0010).
- The spec's 40-algorithms §6 should say the mel uses the natural log.
- Model fixtures are ~830 MB (voicebank, vocoder, RMVPE); CI caches them. A small Apache-2.0 fixture
  model would cut CI time and licence questions.
- Not rendered yet (reported as warnings): effects, doubling, voice mix,
  portamento overshoot/preparation, attack/release.
- Importer diagnostics use "1 notes"-style plurals.
- USTX: curves other than `pitd` and phoneme overrides are reported, not
  converted (0005).
- TSan crate list holds only cantor-core until cantor-audio exists.
- abi3t (free-threaded) Python wheels need Python 3.15 in the flake (0016).
- Multi-singer projects render one singer per call (0015).
- macOS wheels built in the Nix shell link Nix's libiconv; release wheels
  must be built outside Nix like release app builds.
