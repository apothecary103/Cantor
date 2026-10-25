# 0026 Text style prompts

- `cantor_core::style` maps a prompt to adjustments with a small,
  deterministic lexicon (word stems → signed amounts on breathiness,
  energy, tension, voicing, gender, vibrato depth, portamento length),
  intensity words ("slightly" ×0.5, "very" ×1.5, "extremely" ×2), and
  negation ("less", "not", "no", "without"). Clauses split on punctuation
  and "but". Amounts sum per target. Unknown words are returned and
  reported (`style.unrecognized_word`), never silently dropped.
- Curve adjustments are added to the existing user layer over the range
  (the layer's interpolation is kept), ramped in and out over a beat/8 at
  each edge, clamped to the parameter's delta limits. Vibrato depth and
  portamento length change per note in the range. Everything is one
  `Batch` op labelled "Style: <prompt>", so it is one undo step and
  undoes byte-exactly.
- Why a lexicon and not a language model: renders must be reproducible
  and agents must be able to see what each word did; the plan is returned
  with every adjustment's source words. A learned text encoder can later
  produce the same `StylePlan` type.
- Surfaces: `apply_style` (JSON-RPC, MCP, with dry-run), `Track.style()`
  (Python), `cantor style` (CLI). The app has no style action yet: the
  Script Log renders batches op by op, and a style line needs its own
  Python rendering first.
