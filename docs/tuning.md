# Tuning reference

How Cantor turns notes into a convincing vocal, and the craft rules the
tuner passes (`cantor-core::tune`) implement. Agents: read this before
tuning; the numbers are starting points, not laws. Units are Cantor's
(docs/design.md): ms, cents, %, dB, PPQ 480.

## 1. Synthesis semantics (what the engine does)

### oto.ini
`file.wav=alias,offset,consonant,cutoff,preutterance,overlap`, all ms.
- offset: left blank from file start.
- consonant: fixed region after offset; played at its original rate.
  After it, up to cutoff, is the vowel, which is stretched to fit.
- cutoff: positive = ms trimmed from the file end; negative = usable
  length measured from offset (end = offset + |cutoff|).
- preutterance: point after offset that lands on the note-on. The sample
  starts `preutter` ms before the note.
- overlap: point after offset where the crossfade with the previous note
  ends (normally < preutter; negative leaves a gap).

### Fitting a note (OpenUtau `ValidateOverlap` semantics)
1. Velocity stretch `s = 2^(1 − vel/100)` scales preutter, overlap and
   the consonant (vel 0 → ×2, 100 → ×1, 200 → ×0.5).
2. If the note touches the previous one: when `preutter − overlap >
   prev/2`, scale preutter down to fit half the previous note; cap
   preutter at the previous note's length; on a gap shorter than
   preutter, preutter = gap. Overlap shrinks by the same ratio.
3. `timing.preutter_ms` / `timing.overlap_ms` override the result;
   `timing.offset_ms` shifts the whole phoneme.
4. The previous note's tail yields `max(preutter, preutter − overlap)` and
   crossfades over `max(overlap, 0)`.

### Aliases and phonemizers
- `jp_cv`: lyric → `ka`/`か`; the consonant sits before the note-on.
- `jp_vcv`: alias `<prev vowel> <cv>` (`a か`); phrase start `- か`;
  phrase end may add `a R` / `a -` release.
- `jp_cvvc`: CV notes plus VC transitions (`a k`) placed in the tail of
  the previous note, length from the next CV's preutter.
- `en_arpasing`: ARPAbet diphones `- dh`, `dh ah`, `ah n`, `n -`.
- `raw`: lyric is the alias.
- `auto`: picked from the voicebank's aliases.
- Multi-pitch banks: `prefix.map` picks a suffix (e.g. `↑`, `_C4`) by key.
- Lyrics: `+` extends the previous syllable; `-` or `R` is a rest;
  `br` (or `息`) is a breath; `alias` forces an exact oto entry.

### Pitch
`final(t) = key×100 + portamento(+overshoot/preparation) + scoop + fall +
vibrato + note points + pitd(t)`, on a 5 ms grid.

## 2. Craft rules (implemented by the tuner)

### Timing and phrasing
- Vowel onset on the beat; preutterance pulls the consonant early.
  Never move notes earlier to "fix" consonants.
- `vel`: fast passages (> ~8 sixteenths/s) 120–160; normal 100;
  slow/tender lines 70–90.
- Before a breath, end the phrase's last note 120–300 ms early (or 1/16–1/8)
  and put a `br` note in the gap: 150–400 ms, `vol` 40–80. Skip breaths
  for gaps under ~150 ms.
- Phrase ends get a release (`a R`) ~60–150 ms or a fall/decay.
- Split long vowels with `+` to change pitch, timbre or dynamics mid-note.

### Pitch transitions (`pitch.portamento`)
| interval      | length_ms | start_ms     |
|---------------|-----------|--------------|
| 0–2 st        | 40–70     | −(length/2)  |
| 3–5 st        | 70–120    | −(length/2)  |
| ≥ 7 st        | 100–180   | −(length/2)  |
At fast tempos keep it ≤ 25% of the shorter note. Shape `s`.
- Overshoot: rising leaps ≥ 3 st, 20–50 cents, settles over 80–150 ms.
  Falling leaps: 10–30 cents undershoot.
- Preparation: 10–30 cents opposite dip 50–100 ms before a leap.
- Scoop (`pitch.scoop`): phrase starts and emphasized notes, −50 to −200
  cents (−300 bluesy) over 60–150 ms.
- Fall (`pitch.fall`): phrase-final notes, −100 to −500 cents over the
  last 100–250 ms with a dyn fade; soft endings ≈ −50.
- Kobushi/turn (pop/enka): +50–100 cents and back within 100–150 ms via
  `pitch.points`.
- Humanize: slow drift ±5–15 cents, fine jitter ±3–8 cents (~10 Hz
  low-passed). Sustains sit 0–10 cents off-center, not dead on.

### Vibrato (`pitch.vibrato`)
- Only on notes ≥ ~350–400 ms (about a beat at moderate tempo).
- Onset 150–350 ms after the vowel: `length` 50–75%, `fade_in` 20–40%,
  `fade_out` 0–20%.
- Rate 5–6.5 Hz (`period_ms` 150–200); 6–7 Hz for tense/excited moments.
- Depth ±15–35 cents in verses, ±30–60 on climaxes.
- Vary consecutive vibratos: depth ±20%, rate ±0.3 Hz (`drift`, `phase`).
- Defaults if unsure: length 75, period 175, depth 25, in 10, out 10.

### Dynamics
- Phrase arc on `dyn`: +2–4 dB toward the stressed peak, −3–6 dB at the end.
- Stressed syllables/plosives: `vol` 110–130. Function words: `vol` 80–90.

### Timbre by section
| section   | bre    | bri      | gen    | dyn     |
|-----------|--------|----------|--------|---------|
| verse     | 10–30  | −10–0    | 0–+5   | 0       |
| pre-chorus| 5–15   | 0–+10    | 0      | +1      |
| chorus    | 0–10   | +10–+30  | 0      | +2–+3   |
| bridge/soft| 20–40 | −20–−10  | 0–+5   | −2      |
High notes: `gen` −5 to −15 to avoid chipmunk formants. Low notes: `gen`
+5 to +15.

### Harmonies and doubles
- Harmony: third/sixth above or below, −4 to −8 dB, vibrato depth ×0.5,
  `gen` ±5–15, tight timing.
- Double: copy the lead, shift 10–30 ms, detune ±5–10 cents (`pitd`),
  different vibrato phase, pan ±40–80%.

### Mix (track `effects`)
- EQ: high-pass 80–120 Hz; −2 to −4 dB at 200–400 Hz; +1–3 dB at 3–5 kHz;
  +1–3 dB shelf at 10–12 kHz.
- De-esser 5–9 kHz. Compressor 3–4:1, 3–6 dB GR, attack 10–30 ms,
  release 50–150 ms.
- Reverb plate/hall 1.2–2.2 s, pre-delay 20–60 ms, 15–25% wet. Delay
  1/8 or dotted 1/4, 10–15%.

## 3. Working from a reference vocal
- Needs an isolated (a cappella) vocal; full mixes give poor F0.
- F0: YIN, 44.1 kHz, frame 2048, hop ~5–10 ms, 65–1100 Hz; median-filter
  5–9 frames; fix octave jumps.
- Transcription: subtract global tuning offset; new note on voicing after
  ≥ 40 ms gap, on a pitch move > 50–80 cents held ≥ 50–80 ms, or on an
  energy onset; drop notes < 60–100 ms (they're ornaments); note pitch =
  median of the middle 50%; don't split on 4–8 Hz vibrato; quantize to
  1/16 or 1/32 and keep the residue as timing.
- Expression transfer (`match_reference`): `pitd = refF0 − rendered
  base pitch`, interpolated over unvoiced gaps, low-passed at 15–25 Hz,
  clipped to ±300 cents; align on vowel onsets (don't shift by
  preutter). Loudness envelope (50 ms RMS) can map to `dyn`.

## 4. A good cover, step by step
1. Import the MIDI (or transcribe the reference). Check tempo and key.
2. Set lyrics: one syllable per note, `+` for melismas. Render once and
   fix every missing alias (report lists them).
3. `tune auto_pitch` with a style, then `auto_dynamics`, then `breaths`.
4. If there's a reference vocal: `match_reference` at strength 0.6–1.0.
5. `section_color` per section; set effects per §Mix.
6. Render; read the report (pitch p95 vs reference, LUFS, clipping); fix
   outliers by hand (scoops, vibrato, `vel` on muddy consonants).
7. Add harmonies/doubles if the original has them. Export WAV.

Sources: OpenUtau source (stakira/OpenUtau: USTx.cs, UNote.cs,
UPhoneme.cs, ResamplerItem.cs); Moresampler and tn_fnds docs
(saoirse042.github.io/ResamplerDocs); Saitou et al. 2009 F0 model;
pYIN (Mauch & Dixon 2014); Tony (Mauch et al. 2015). Section 2's
numbers are practitioner conventions.
