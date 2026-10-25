# 0011 DiffSinger backend contract

Follows OpenUtau's DiffSinger renderer (OpenUtau.Core/DiffSinger) and
OpenVPI's exporters; tensor names are read from each model and only the
inputs a model declares are fed.

- **Voicebank layout:** OpenUtau format (`dsconfig.yaml`, `dsdur/`,
  `dspitch/`, `dsvariance/`, `character.yaml`); vocoder from an unpacked
  `.oudep` (`vocoder.yaml`). Sample rate, hop, mel bins and mel log base
  must match (natural log for PC-NSF-HiFiGAN). Only rectified-flow
  (continuous acceleration) acoustic models are supported; others error.
- **Phonemes:** `dsdict-<lang>.yaml` (else `dsdict.yaml`) from `dsdur/`;
  language from the note, else the singer's default phonemizer. Lyrics:
  `+` merges into the previous syllable; `SP`/`R`/empty is silence; `AP`
  a breath; unknown lyrics are silent with a `phon.unknown_lyric` warning.
  Native phoneme overrides are honored; X-SAMPA arrives in M5.
- **Groups (words for the duration model):** head SP + first leading
  consonants; then each syllable's nucleus, an SP for any rest, and the
  next syllable's leading consonants; finally a tail SP. Each group spans
  onset to onset, so nuclei land on note onsets. `word_dur` and phone
  frames use cumulative rounding so frames sum exactly to the window.
- **Placement** (cantor-phon::place): predicted durations, consonants
  capped at 50% of the group, ≥ 10 ms each, pinned lengths kept; failures
  scale proportionally and report `phon.collision`.
- **Pitch model:** notes = head rest, notes and gap rests, tail rest (rest
  notes take the nearest pitch); `pitch` input is Cantor's base pitch,
  `retake` all true, `expr` 1.
- **Variance model:** conditioned on the predicted pitch; outputs
  `<param>_pred` in dB. Final = prediction + user delta, clamped to
  [−96, 0] dB.
- **Acoustic:** f0 is the *reference* pitch (see 0012); `depth` =
  `max_depth`; gender is passed as a key shift of −gender semitones,
  clamped to the bank's augmentation range; velocity is a ratio.
- **Steps:** preview 10, final 20.
- **Unsupported curves** (e.g. tension on a bank without a tension embed)
  produce `render.param_unsupported` warnings.
