# 0016 Python package

- `cantor` is a maturin mixed project: native module `cantor._native`
  (pyo3 0.29, `abi3-py312`), a pure-Python `__init__.py`, `__init__.pyi`
  stubs and `py.typed`. The crate also enables pyo3's `abi3t` feature;
  building the free-threaded abi3t wheel needs a Python 3.15 interpreter,
  which the flake does not ship yet (disk budget), so only the abi3 wheel
  is built and tested today.
- Handles (`Project`, `Track`, `Note`) hold ids and read fresh snapshots;
  writes are ops submitted at the current revision, or collected by
  `with p.transaction(label):` and applied as one atomic batch (nothing is
  applied if the block raises). Rendering releases the GIL.
- Curve values are in the parameter's registry unit; `ramp(to, over=
  "last 30%")` writes a linear user-delta segment over that part of the
  note.
- Unmeasured report values are NaN so expressions like
  `abs(m.pitch_error_p95) > 25` never raise (the spec example).
- `cargo xtask python` builds the wheel, installs it into a fresh venv
  without network, prepares the fixture song (adds a "chorus" section) and
  runs the first Python block of docs/spec/60-api.md verbatim, then the API
  tests.
