# 0007: MusicXML import

The importer reads uncompressed `score-partwise` MusicXML 3.x and 4.0
(`crates/cantor-io/src/import/musicxml.rs`). Everything it leaves out is
reported as a `musicxml.*` diagnostic.

## Input
- `score-timewise`, compressed `.mxl` (bytes start with `PK`) and UTF-16
  files are `Unsupported`. Malformed XML is a `Parse` error, never a panic.
- The DOCTYPE is skipped and never fetched. Unknown entities stay as literal
  text.
- The file is read into a small element tree and then walked. Positions are
  kept relative to each measure, so a malformed part cannot push the others
  out of line.

## Mapping
- **Meta:** `work-title` (or `movement-title`) becomes the title.
  Composers and lyricists become the artist. The movement title (when a work
  title exists), other creators and rights go into the comment.
- **Tracks:** each `<part>` becomes one track named after its `<part-name>`,
  with one Part from tick 0 to the end of the score. Every track then lines
  up with the global timeline.
- **Time:** `divisions` can change anywhere. Positions convert with
  `rescale`, and inexact conversions produce one `musicxml.rounding`
  warning. `<backup>` and `<forward>` move the cursor.
- **Bars:** a measure lasts as long as the longest part reaches in it.
  Time signatures come from the first part that has any. A measure whose
  length does not match its signature, such as a pickup, gets a signature
  that fits it exactly (a one-quarter pickup in 4/4 becomes a bar of 1/4).
  Cantor bar *n* is then MusicXML measure *n*, and notes stay at tick 0.
- **Pitch:** `(octave + 1) × 12 + step + alter` semitones × 100. A
  fractional alter is microtonal. `<transpose>` is applied (chromatic plus
  octave-change), so notes and keys come out at concert pitch.
- **Monophony:** a part imports the voice of its first pitched note. Notes
  in other voices are dropped (`musicxml.voice`). In a chord the first
  note is kept and the rest are dropped (`musicxml.chord`). If the kept note
  has no lyric, it takes one from a dropped chord note. Grace notes, cue
  notes and unpitched notes are skipped with diagnostics.
- **Ties:** a stop that continues an open tie of the same pitch, starting
  exactly where that note ends, extends it. Other ties are ignored
  (`musicxml.tie_unmatched`).
- **Lyrics:** verse `1` is imported, or else the first verse seen. A lyric
  with no `number` counts by its position on the note. Other verses are
  reported (`musicxml.lyric_verse`). Hyphenation uses a trailing `-` on
  `begin` and `middle` syllables (`hal-`, `le-`, `lu`) and never a leading
  one. Elided syllables are joined with a space.
- **Melisma rule:** a note without a lyric (or with only an `<extend>`)
  sings `+` when it starts exactly where the previous imported note ends.
  That covers `<extend>`, slurs and plain following notes. Otherwise, or
  when the part has no lyrics at all, it sings `la` and is counted in
  `musicxml.lyric_default`.
- **Tempo:** merged from all parts. At one tick, `<sound tempo>` beats
  `<metronome>`, and earlier parts beat later ones (`musicxml.tempo_conflict`).
  A metronome mark converts to quarter BPM (dotted units count). A
  `<direction>` offset applies only with `sound="yes"`. With no tempo at
  tick 0, 120 BPM is inserted (`musicxml.tempo_default`).
- **Keys:** come from the first part that has any. Fifths and mode give the
  tonic. The modes map to Cantor scales, and mode `none` maps to `chromatic`.
  Non-traditional keys are reported (`musicxml.key_unsupported`).

## Not imported (one diagnostic per kind)
These are dynamics, hairpins, text directions, articulations, ornaments,
technical marks, fermatas, arpeggios, glissandos, chord symbols, figured
bass, metric modulations and octave doubling. Repeats, endings and jumps are
also not imported, and they get a warning because the music is not unfolded.
Visual-only markup (clefs, stems, beams, layout) is skipped silently.
