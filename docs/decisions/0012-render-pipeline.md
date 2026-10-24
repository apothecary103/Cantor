# 0012 Render pipeline: purity, fast path, phrases

- **Rendering is a pure function of the document.** The acoustic model
  receives a *reference* f0 = base pitch + model-predicted pitch, without
  the user's `pitch` curve. User pitch edits only change the vocoder's f0
  (PC-NSF-HiFiGAN shifts pitch while keeping formants). So the mel cache key
  covers everything the mel depends on, a pitch-only edit reuses the cached
  mel (the vocoder fast path, spec §6), and a cold render gives exactly the
  same audio as an incremental one. The alternative — feeding final f0 to
  the acoustic model and reusing stale mels — would make output depend on
  edit history.
- **Cache keys** (BLAKE3, tagged fixed-width fields, floats by bits):
  prediction = backend id/version, RNG version, window, steps, seed and
  every note field; mel = prediction key + steps + final variance curves;
  audio = mel key + final f0. Entries live in the render cache directory
  (`--cache`, `$CANTOR_CACHE`, or the platform cache dir).
- **Phrases** split at rests ≥ 200 ms; a phrase's id is its first note's
  id, under which seeds and takes are stored. Missing seeds derive from
  (track id, phrase id). Windows add 0.5 s before (leading consonants) and
  0.3 s after; neighbors are joined with 10 ms equal-power crossfades at
  the midpoint of the rest.
- **Edit layers are local.** `user_delta` curves contribute only between
  their first and last points (`Curve::value_in_span`); outside, the delta
  is zero, so an edit never changes distant phrases.
- **Not yet rendered** (warnings, not silence): effects chain and doubling
  (M5), voice mixing, portamento overshoot/preparation (smart pitch
  handles, M4). One singer per render in the CLI until M3.
- **Spec note:** 40-algorithms.md §6 should state that the mel uses the
  natural log (`mel_base: e`).
