# 0021 X-SAMPA phoneme input

- Phonemizers declare a one-to-one map between their native symbols and
  X-SAMPA per language (`cantor_phon::xsampa::XSampaMap`; text format
  `native xsampa` per line). Built-in maps cover the symbol sets
  DiffSinger voicebanks use: Mandarin Opencpop-style initials/finals
  (finals with medials are glide+vowel strings such as `jAN`; apical
  vowels `z=`/`z\`=`), Japanese romaji, English ARPAbet. A voicebank can
  ship `xsampa-<lang>.txt`; a WASM phonemizer declares spellings in its
  inventory and wins for its languages.
- A phoneme override with `alphabet: "x_sampa"` is translated per symbol;
  specials the voicebank knows (`SP`, `AP`) pass through in any alphabet.
  A symbol outside the map is a `phon.xsampa_unknown` warning and the
  note sings its lyric instead. Native and X-SAMPA overrides that name the
  same phonemes render bit-identical audio (tested with Qixuan).
- `get_singer_schema` returns the maps (`xsampa`: language → native →
  X-SAMPA) so agents can type phonemes for any language the singer has.
