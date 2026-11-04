# 0028 Flow-matching acoustic models

- Newer SVS acoustic models (rectified flow / flow matching, diffusion
  transformers) are adapted through one contract: an ONNX velocity
  network `v = f(x, t, cond)` over mel frames (`flow.yaml` names the
  tensors). Cantor owns the ODE: start at `t0 = 1 − depth` from
  `depth·noise + (1 − depth)·prior`, integrate to 1 with Euler (previews,
  ≤ 10 steps) or Heun (final), steps = render quality. Noise comes from
  Cantor's seeded RNG, so renders stay deterministic and cacheable; the
  model version, depth and solver are part of the backend version.
- `FlowBackend` wraps any backend and replaces only its acoustic stage;
  the wrapped backend's mel is the conditioning prior (shallow flow when
  depth < 1). The engine wraps a DiffSinger voicebank automatically when
  it ships `flow.yaml`.
- No public singing model ships a velocity-only ONNX export today, so the
  adapter is tested with a hand-built ONNX graph (`v = cond − x`) against
  a reference integration in Rust (exact to 1e-5 for both solvers), plus
  analytic solver tests (straight paths exact under Euler, Heun second
  order). Supporting a specific model means exporting its velocity
  network to this contract and writing its `flow.yaml`.
