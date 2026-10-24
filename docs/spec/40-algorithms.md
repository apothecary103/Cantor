# Synthesis pipeline and algorithms

## Time
sec(t) = Σ over tempo segments k up to t of (Δticks_k / PPQ) · 60 / bpm_k.
Property-test that tick → sec → tick round-trips.

## 1. Phonemization
- The note's language tag picks a WASM phonemizer: dictionary first, then
  neural G2P (small ONNX seq2seq) as fallback.
- Universal X-SAMPA input: each phonemizer declares a mapping between X-SAMPA
  and its native symbols, so users can type phonemes for any language.
- Syllable continuation: "+" and leading/trailing "-".
- Placement: the vowel nucleus lands exactly on the note onset. Leading
  consonants sit before it and borrow time from the previous note. If they
  need more than 50% of it, compress proportionally down to per-phoneme
  minimums (never below 10 ms). Any remaining collision becomes a diagnostic.

## 2. Duration
A word-group-aware duration model predicts phoneme durations. Rescale each
word's unpinned phonemes so the word fills its musical time exactly;
user-pinned phonemes keep their length.

## 3. Pitch
- Base pitch, cosine-eased portamento between notes:
  u = clamp((t − t0)/w, 0, 1); p = pa + (pb − pa)(1 − cos πu)/2,
  with the transition window centered about 60% before / 40% after the boundary.
- Procedural vibrato: v(t) = D · env(t) · sin(2π ∫ f(τ)dτ + φ), with linear
  fade-in/out envelope and f = rate + smoothed-noise drift (about ±5%).
- Neural: the variance model predicts a delta over base. Unvoiced phonemes
  get a voicing mask.
- Smart pitch handles edit named parameters (transition length, overshoot,
  preparation dip, vibrato depth/rate), never raw points, so edits stay
  readable and scriptable.

## 4. Variance parameters
Energy, breathiness, voicing, tension, gender (formant), velocity, and
mouth opening where the backend supports it. Each is registered with unit,
range, default and support flag.

## 5. Acoustic model
Shallow diffusion or Rectified Flow; sampling steps are the quality knob
(preview few, final many). Noise is seeded per phrase.

## 6. Vocoder
- Default: PC-NSF-HiFiGAN (44.1 kHz, 128 mel bins, hop 512, window 2048,
  mel range 40–16000 Hz). It supports pitch shifting while preserving
  formants, so pitch-only edits re-vocode the cached mel with new F0 and
  skip the acoustic model (near-instant pitch tuning).
- WORLD (CheapTrick + D4C) for concatenative banks.

## 7. Concatenative UTAU backend
Parse oto.ini (offset, consonant, cutoff, preutterance, overlap). Use RMVPE
for F0, WORLD to resample/stretch, and equal-power crossfades:
gA = cos(πu/2), gB = sin(πu/2).

## 8. Incremental rendering
- Split tracks into phrases at rests ≥ 200 ms.
- Cache key = BLAKE3(backend id, model version, phonemes, frame-quantized
  durations, all curves resampled to the frame grid, seed, steps).
- Re-render only phrases whose key changed. Cache mel + audio on disk.
  Mix with 10 ms crossfades at padded phrase boundaries.

## 9. Retakes
Retake any subset of {pitch, timing, timbre, expression} over any range.
Keep every take as a switchable lane. Agents may generate N takes and pick
by metrics.

## 10. Post-processing
Per-track chain: EQ → compressor → de-esser → reverb (room simulation).
Doubling: up to 16 layers with seeded micro-variations in timing, pitch
and timbre, spread by a stereo spacing control.

## 11. Analysis (returned with every render, shown in the GUI too)
- Pitch: RMVPE F0 vs target; mean/p95/max cents error; octave errors;
  voiced/unvoiced mismatches.
- Timing: detected vowel onsets vs note onsets (ms).
- Loudness: integrated and short-term LUFS (BS.1770), true peak.
- Artifacts: clipping, discontinuities at phrase joins, crackle (HF energy
  spikes), unexpected silence, formant jumps.
- Optional: singing-MOS predictor as a rough naturalness score.
