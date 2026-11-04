# 0027 Audio-to-voice conversion and consent

- `Engine::convert_audio` turns a sung WAV into a project another singer
  sings: RMVPE F0 → 5-frame median → notes (unvoiced gaps ≥ 60 ms end a
  note; a pitch move > 70 cents held ≥ 80 ms starts one; notes < 80 ms are
  dropped), note pitch = median rounded to the semitone, the recording's
  exact contour minus the note pitch as the pitch edit layer, and RMS
  loudness relative to its median as the energy layer (every 20 ms).
  Tempo is fixed at 120 BPM so timing is exact. Lyrics come in order.
- **Consent gate.** Conversion requires a `ConsentAttestation`
  (performer, statement, date, `confirmed`); incomplete or unconfirmed
  attestations are refused before any audio is read. The attestation is
  stored in the new `Meta.provenance` (method, source, performer,
  consent, date), so it travels with the project and its exports.
- **Cloned voices.** Singer manifests may declare `cloned: true`
  (`character.yaml`); the engine refuses to load such a voice unless the
  manifest also carries a `consent_attestation` (00-vision.md), for every
  operation, not only conversion.
- Surfaces: `convert_audio` (JSON-RPC, MCP), `cantor.convert_audio()`
  (Python, keyword-only consent arguments), `cantor convert` (CLI, with an
  explicit `--confirm`).
- Not done: lyric transcription from the audio (lyrics are supplied);
  converting into a *new* voice (timbre transfer) — the output is sung by
  an existing, consented singer.
