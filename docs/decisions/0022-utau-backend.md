# 0022 UTAU concatenative backend

- **Pipeline on the backend stages.** `predict` resolves each lyric to an
  `oto.ini` alias (as typed, then lowercased) and places it: the vowel
  onset on the note onset, the consonant (preutterance) borrowing at most
  half of the previous note or the silence before it; compression scales
  preutterance and overlap together. Phones are `alias#c` / `alias#v`
  spans plus `SP`. `acoustic` builds WORLD features (`[sp | ap]` per frame,
  2048-point FFT at 44.1 kHz, 256-sample frames): each recording is
  analyzed once per process (RMVPE F0 → CheapTrick, D4C), the consonant
  maps 1:1 or compressed, the fixed region after the onset maps 1:1, the
  rest of the usable region stretches linearly over the note, and
  neighbours cross-fade over the overlap with gA = cos(πu/2),
  gB = sin(πu/2) applied as gA², gB² to the power spectra (equal power).
  `vocode` is WORLD synthesis with the renderer's f0, so pitch edits take
  the vocoder fast path; `warp_timbre` shifts the envelope's frequency
  axis, so doubling varies real formants.
- **Parameters:** pitch, energy (dB on the envelope), breathiness (dB on
  the aperiodicity) and gender (formant shift in semitones). Others are
  reported as unsupported. Phoneme overrides are reported (UTAU sings
  aliases), as are missing aliases and missing breath samples.
- **WORLD:** `world-rs` 0.1.0 (pure Rust port, BSD-3), pinned exactly; it
  is young and single-maintainer, so its output is covered by our own
  tests (pitch accuracy, determinism). Its synthesis and D4C reseed their
  PRNG per call, so renders are deterministic.
- **Voicebanks:** a folder with `oto.ini` at the top or one level down
  (multi-pitch banks; the first entry for an alias wins, as in UTAU).
  `oto.ini` and `character.txt` are read as UTF-8 or Shift-JIS. UTAU banks
  declare no language; Cantor assumes Japanese unless `character.yaml`
  says English. Prefix maps, per-pitch alias selection, CVVC/VCV aliasing,
  envelopes and flags are not implemented.
- **Test voicebank:** no small UTAU bank has terms that clearly allow
  redistribution and automated tests, so `cantor-fixtures::utau` generates
  one (source-filter voice, 40 CV/V samples, `oto.ini`), released CC0.
