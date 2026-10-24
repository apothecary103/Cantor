# Vision

Cantor lets a musician, a scripter and an AI agent produce a release-quality
sung vocal from notes and lyrics, using the same engine and the same
operations. It should feel as fast and precise as the best commercial
editors (Synthesizer V Studio 2, ACE Studio 2), while being open, fully
scriptable, and drivable end to end by an AI.

## Who it serves
1. Musicians editing by hand: dense, fast, keyboard-friendly editing.
2. Scripters: every action is a documented operation they can automate.
3. AI agents: an agent can compose, tune, render and evaluate a vocal
   without a human opening the GUI. Agents can't hear, so the engine
   reports what it rendered as numbers.

## Principles, and why they exist
- Headless core. The engine is a library and a server; every interface is
  a client. This keeps the three audiences equal and lets us replace the GUI
  toolkit without touching the engine.
- Operations, not mutations. Edits are typed, serializable `Op`s. Undo,
  history branches, macros, replay, diffing and AI dry-runs all come from this.
- GUI actions teach scripting. Each GUI action shows its equivalent Op and
  Python line in a Script Log panel, so manual users discover automation
  naturally and can save any workflow as a macro.
- Determinism. Identical inputs and seeds produce identical audio; without
  this, neither humans nor agents can tell whether a change helped.
- Closed loop. Renders return measurements (pitch error, timing, loudness,
  artifacts), not just audio.
- Introspection. Singers, parameters, phonemizers and Ops all publish
  machine-readable schemas with units, ranges, defaults and support flags.
- Honest failure. If something can't be honored (unsupported parameter,
  phoneme collision), say so in a diagnostic.

## Ethics and licensing
Singer manifests declare license terms; surface them via the API and at
export. Voice cloning or conversion requires a consent attestation in the
manifest; refuse to load a cloned voice without it. Respect upstream licenses
and keep THIRD_PARTY_NOTICES current.
