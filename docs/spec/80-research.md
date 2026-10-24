# Background (context, not requirements)

- OpenUtau is the main open-source reference. It is active in 2026, with
  MusicXML import, microtonal tuning, group-aware DiffSinger duration models,
  and a universal pc-nsf-hifigan vocoder fallback. Its wiki proposes svs.io,
  a self-describing msgpack backend API.
- DiffSinger (OpenVPI fork) is the standard open model: a variance model
  (duration, pitch, energy, breathiness...) feeding an acoustic model (mel),
  with UniPC/Rectified Flow acceleration and RMVPE pitch extraction.
- Commercial bar for manual editing: Synthesizer V Studio 2 (separate vocal
  modes for pitch/timbre/pronunciation, multi-dimensional AI retakes, phoneme
  timing panel, smart pitch controls, voice doubling up to 16 layers,
  built-in effects chain, MusicXML import, unified X-SAMPA in 2.3 beta,
  parameters that move with notes); ACE Studio 2 (VoiceMix timbre blending,
  DAW bridge plugin); VOCALOID6 (VOCALO CHANGER audio-to-voice).
- Research frontier: score-native systems (VocalRender), zero-shot models
  (SoulX-Singer, TCSinger 2 with singing-method control), diffusion
  transformers (DiTSinger), flow matching (TechSinger).
