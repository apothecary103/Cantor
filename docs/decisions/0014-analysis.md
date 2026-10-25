# 0014 Analysis: RMVPE front end, score-relative pitch error, detectors

- **F0 extractor:** RMVPE, RVC's ONNX export (MIT licence, HF
  lj1995/VoiceConversionWebUI, pinned commit). The yxlllc export embeds the
  STFT but ships no licence file, so we take the MIT model and implement the
  front end in Rust to match RVC's `infer/rmvpe.py`: 16 kHz (windowed-sinc
  resampler), 1024-point Hann STFT hop 160, 128 HTK mel bands 30–8000 Hz
  with Slaney normalization, natural log clamped at 1e-5, frames padded to a
  multiple of 32; decoding = salience-weighted cents over ±4 bins around
  the peak, unvoiced below 0.03.
- **Pitch error is relative to the score** (notes + portamento + vibrato,
  the same base pitch the renderer computes), not to the user's pitch-curve
  edits. "Tune to ≤ 15 cents" then has a meaning, and agents fix errors by
  editing the pitch curve. Only voiced frames in a note's steady part (60 ms
  from each edge) are scored; |error| > 600 cents counts as an octave error.
  Each note report carries a signed error contour (every 20 ms, seconds and
  ticks) so agents know where to correct.
- **Timing:** vowel onset after silence = first voiced frame reaching half
  the note's median energy; legato into a different pitch = midpoint pitch
  crossing; legato at the same pitch = rise after the deepest energy dip;
  `+` continuations have no onset.
- **Loudness:** `ebur128` (integrated, short-term per second, LRA, true and
  sample peak).
- **Artifacts:** clipping (≥ 0.999 FS), clicks at phrase joins (second-
  difference spike ≥ 6× the surrounding 99th percentile above −60 dBFS —
  a first version flagged inaudible level changes in breath noise), crackle
  (isolated frames with > 25% energy above 8 kHz), unexpected silence
  (≥ 100 ms below −60 dBFS inside sung notes), formant jumps (log-band
  envelope distance > 3 in 10 ms within steady parts).
- F0 tracks are cached by (extractor, sample rate, track audio hash).
