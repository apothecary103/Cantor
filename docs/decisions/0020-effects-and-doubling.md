# 0020 Effects chain and voice doubling

- **Effect registry** (`cantor_core::effects`): every built-in effect and
  parameter with unit, range and default, generated to
  `schema/effects.json` and served as MCP resource `cantor://schema/effects`.
  A slot stores only the parameters it changes. Unknown parameters and
  out-of-range values make the op fail with `track.invalid` (validated in
  `Project::validate`, so they can never reach a render).
- **DSP** (`cantor_render::fx`), f64 state, deterministic: RBJ biquad EQ
  (high-pass, low shelf, two peaks, high shelf; 0 dB bands are skipped so
  a neutral EQ is bit-exact); stereo-linked feed-forward compressor with a
  soft knee, smoothing gain reduction in dB; split-band de-esser on a
  4th-order Linkwitz–Riley crossover (the bands stay in phase, so only the
  sibilance band's level changes; 4:1 above threshold, capped at `range`);
  Freeverb-tuned Schroeder–Moorer reverb (delays scaled to the sample
  rate, constant-power dry/wet, pre-delay). Effects run in slot order, so
  the spec's EQ → compressor → de-esser → reverb is the default order, not
  a constraint. Disabled slots and `mix = 0` are exact bypasses. The
  track is padded by the reverb tail (RT60 of the longest comb, capped at
  10 s).
- **Doubling** (`cantor_render::double`): layers 2…16 re-vocode the lead's
  cached mel, so doubling costs one vocoder pass per layer and no acoustic
  model run. Each layer gets, seeded by (doubling seed, phrase, layer): a
  constant detune within ±`detune_cents` plus Gaussian drift (σ =
  detune/3, control points every 250 ms); a timing offset within
  ±`jitter_ms`; a formant shift within ±4 %·`timbre` (resampling the mel
  along its bin axis — `Backend::warp_timbre`, which backends without a
  log-mel override and report). Layers alternate left/right, widening to
  ±`spacing`; together they carry half the lead's power. `Doubling` gained
  `timbre` (default 0.5). Doubles are cached under keys derived from the
  lead's audio key.
- **Signal flow per track:** lead + doubles (stereo) → effects → track
  gain and pan. A track without doubling or effects renders bit-identical
  to before. Pitch analysis uses the lead layer before effects; loudness
  and artifacts use the final mix.
