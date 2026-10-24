# 0006 MIDI import mapping

Standard MIDI Files are parsed with `midly` and mapped to a `Project` in
`crates/cantor-io/src/import/midi.rs`. Every choice below that loses or
alters data emits a `midi.*` diagnostic; nothing is dropped silently.

## Supported input
- SMF format 0 and 1 with metrical (PPQ) timing. Every tick is rescaled to
  PPQ 1920 with round-half-up; inexact conversions are counted in one
  `midi.ticks_rounded` warning.
- Format 2 (independent sequences) and SMPTE timecode timing return
  `ImportError::Unsupported`: neither has a single musical timeline to map
  onto ticks. Malformed bytes return `ImportError::Parse`.

## Maps
- Tempo, time-signature and key-signature events from all tracks are merged,
  ordered by (tick, track, event). At equal positions the later event wins
  (`midi.tempo_conflict`, `midi.time_sig_conflict`, `midi.key_conflict` when
  they differ). Missing tempo at tick 0 → 120 BPM (`midi.tempo_default`);
  missing time signature at bar 0 → 4/4 (`midi.time_sig_default`). Tempos
  outside 1–1000 BPM are clamped (`midi.tempo_clamped`).
- A time signature off a bar line (measured with the signatures before it)
  moves to the next bar (`midi.time_sig_moved`); invalid ones are dropped.
- Key signature: sharps × 7 semitones mod 12 is the major tonic; minor keys
  use the relative minor (+9). Out-of-range counts are dropped.

## Tracks and notes
- Each SMF track with at least one imported note becomes a track named by
  its first Track Name event (else "Track N", 1-based). In format 1, the
  first track's name becomes the project title when that track has no notes
  (the conventional conductor track).
- One part per track, from the start of the first note's bar to the end of
  the last note's bar, so the part snaps to whole bars.
- NoteOn/NoteOff pair per (channel, key) first-in first-out; NoteOn with
  velocity 0 is a NoteOff. Unterminated notes end at the track's last event
  (`midi.note_unterminated`). Zero-length notes are dropped
  (`midi.note_zero_length`): they have no singable duration. A note that
  rounds to 0 ticks after rescaling keeps 1 tick.
- Tracks are monophonic. Of notes that start together the highest pitch is
  kept (the melody line in most voicings; `midi.note_chord_dropped`); a note
  that overlaps the next one is shortened to end where it starts
  (`midi.note_overlap_trimmed`). Trimming keeps every onset, which matters
  more for singing than the release.
- Channel 10 (drums) notes are skipped (`midi.drum_notes_skipped`).

## Lyrics
- Lyric events (FF 05) attach to the note starting at the same (rescaled)
  tick in the same track. If a track has no Lyric events, its Text events
  (FF 01) are used instead (`midi.text_as_lyrics`), since older karaoke
  files store lyrics there. Text is trimmed; blank lyrics are skipped.
- Several lyrics on one note are concatenated (`midi.lyric_merged`); lyrics
  without a note at their tick are dropped (`midi.lyric_unmatched`).
- Notes without a lyric sing "la" (`midi.lyric_default`). Invalid UTF-8 is
  decoded lossily (`midi.invalid_utf8`). Karaoke markers such as `/` and `\`
  are kept verbatim.

## Ignored
Velocity, pitch bend, control changes, program changes, aftertouch, sysex
and other meta events (markers, copyright, instrument names, …) are not
mapped yet. Each kind gets one `midi.ignored.<kind>` diagnostic with its
count. Pitch bend and CC could later seed pitch-deviation and dynamics
curves; that needs a policy for MIDI's per-channel bend range first.
