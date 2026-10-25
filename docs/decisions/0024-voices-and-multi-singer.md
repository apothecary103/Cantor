# 0024 Voices and multi-singer renders

- The engine loads a `Voice` per singer id: DiffSinger (`dsconfig.yaml`),
  UTAU (`oto.ini`, needs RMVPE for analysis of its recordings) or svs.io
  (`svsio.yaml`). The singer registry detects the kind from the folder.
  `Voice` gives the renderer a `Backend` and describe/schema calls their
  phonemes (DiffSinger), aliases (UTAU) and X-SAMPA maps.
- `Renderer` takes a backend per singer (`with_singer_backend`); each
  track renders with its singer's backend and the tracks are mixed. The
  first singer's sample rate is the mix rate; a track at another rate is
  skipped with an error diagnostic (`render.sample_rate_mismatch`) rather
  than resampled — every current backend renders at 44.1 kHz. This
  removes the one-singer-per-call limit of 0015.
- `cantor render` without `--voicebank`/`--vocoder` renders through the
  engine ($CANTOR_SINGERS, any backend, any number of singers);
  `cantor analyze` renders and prints the analysis report (60-api.md lists
  it; it was missing). The explicit `--voicebank` path stays for
  rendering with a DiffSinger folder outside the search path.
