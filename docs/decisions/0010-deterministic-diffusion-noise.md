# 0010 Deterministic diffusion noise: rewrite RandomNormalLike in memory

**Problem.** DiffSinger ONNX exports (acoustic, pitch and variance models)
draw their initial noise inside the graph with one `RandomNormalLike` op and
no `seed` attribute. ONNX Runtime seeds that kernel from the clock when the
session is created and advances it on every run; the C API cannot reseed it.
Two renders of the same phrase therefore differ, which breaks the
determinism principle (same document + seeds = identical audio).

**Decision.** When a model is loaded, `cantor-backends::onnx_noise` edits
the protobuf *in memory*: each top-level `y = RandomNormalLike(x)` becomes
`y = Reshape(Slice(cantor_noise_k, [0], Unsqueeze(Size(x))), Shape(x))`,
and `cantor_noise_k` becomes a graph input filled from Cantor's versioned
RNG (`cantor-render::rng`, xoshiro256** + Box–Muller) seeded with the
phrase seed and a per-model domain. Every other byte, including all weights,
is copied unchanged; nothing is written to disk. A test runs a tiny model
through real ONNX Runtime and checks the output equals the seeded stream.

**Licence note — please confirm.** The test fixture (Qixuan v2.7.0) forbids
"reverse-engineering, converting, modifying, fine-tuning, splitting or
merging the model weight files" and allows inference with newly written
programs. We read the in-memory rewrite as inference, not modification of
the weight files: the files are untouched, the weights are untouched, and
the edit is of the same kind as the graph rewrites ONNX Runtime's own
optimizer performs on load. This is our reading, not the author's; it
should be confirmed with the voicebank author (OpenVPI) before Cantor is
released, and banks whose terms forbid it can opt out later via a manifest
flag (losing determinism, reported as a diagnostic).

**Alternatives rejected.** Setting `depth = 0` skips diffusion (deterministic
but lower quality). Injecting a `seed` attribute fixes the stream per
session, not per phrase, and needs a session per render. Training a small
Apache-2.0 fixture that takes noise as an input is the long-term fix for CI
size and licensing; it is out of scope for M2.

**Scope of the guarantee.** Hashes are identical across renders on the same
platform. Floating-point kernels differ between CPUs, so hashes are not
expected to match across machines. Sessions use 4 intra-op threads and
ONNX Runtime's deterministic-compute option, so results do not depend on
the machine's core count.
