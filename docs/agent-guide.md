# Making a UTAU cover with Cantor (guide for AI agents)

You can't hear the result, so Cantor tells you what it rendered in
numbers. Work in a loop: edit → render → read the report → fix. Every
edit you make shows up live in the user's browser if they have the
editor open, and every edit is undoable.

Read docs/tuning.md for the craft rules and their numbers. This file is
the workflow.

## Connect
- MCP over stdio: `cantor mcp` (Claude Code: `claude mcp add cantor --
  cantor mcp`).
- MCP over HTTP while the editor runs: `cantor serve`, then
  `http://127.0.0.1:7878/mcp`. Prefer this when a person is watching.
- CLI one-shot: `cantor cover --midi song.mid --lyrics lyrics.txt
  --singer <id> [--audio vocal.wav] [--style pop] -o out/` runs the steps
  below with defaults. It's a good first draft, not a finished cover.

## Inputs you may get
| You have | Do |
|---|---|
| MIDI with the vocal melody | `import` it; pick the vocal track if asked |
| MIDI + isolated vocal audio | import MIDI, then `import` the audio as the reference |
| Only isolated vocal audio | `import` as reference, then `transcribe` to get notes |
| A full mix only | ask for a vocal stem or MIDI; F0 from a mix is unreliable |
| Lyrics text | `set_lyrics` (Japanese kana/romaji split per mora; English per word or hyphenated syllable) |
| UST/USTX | `import` directly; tuning in them is kept |

## Workflow
1. **Set up.** `new_project` or `import`. `list_voicebanks`, pick one,
   `voicebank_info` to learn its kind (CV/VCV/CVVC) and language. Set the
   track's `singer` and `phonemizer` (`update_track`). Check tempo, time
   signature and key against the original (`project_info`).
2. **Notes.** `get_notes` for a compact table. Fix obvious MIDI problems:
   overlapping or doubled notes, grace notes the singer doesn't need,
   notes far outside the voicebank's comfortable range (transpose the
   whole track in octaves rather than leaving it in chipmunk range).
3. **Lyrics.** One syllable per note; `+` for melismas (a held syllable
   across several notes); `br` for breaths. Render once and read
   `missing_aliases` — fix every one (respell the lyric, or set the note's
   `alias` to an alias that exists; `voicebank_info` lists them).
4. **Draft tuning.** `checkpoint "draft"`, then run tuner passes on the
   track: `auto_pitch {style}` (pop / ballad / rock / natural), then
   `auto_dynamics`, then `breaths`. Passes are idempotent; re-run with
   other params freely.
5. **Follow the reference (if you have one).** `analyze_reference`, then
   `tune match_reference {strength: 0.6–1.0}`. Render and compare
   `ref_pitch_error.p95`; below ~40 cents is close, below ~25 is very close.
   If one phrase is far off, inspect it (`get_notes` for that range): it's
   usually a wrong note, a transcription octave error, or timing.
6. **Color by section.** Set `sections` (verse, chorus, bridge…) and run
   `section_color` with presets: breathier verse, brighter and louder
   chorus. Add effects to the track per docs/tuning.md §Mix.
7. **Detail pass.** Read the per-phrase report. Typical fixes:
   - Muddy or late consonants → raise `vel` (120–160) on fast passages.
   - Robotic long notes → check vibrato exists, onset delayed, depth varied.
   - Abrupt phrase ends → `fall` or a release; check `dyn` tail.
   - Pitch spikes at joins → lengthen portamento or adjust `preutter_ms`.
   - Clipping or LUFS far from about −16 → `dyn`/track gain.
8. **Harmonies/doubles** if the original has them: duplicate the track
   (`add_track` with the notes), transpose by a third/sixth for harmony,
   −4 to −8 dB, half the vibrato depth, pan apart.
9. **Deliver.** `save_project`, `export` WAV (and UST/USTX if the user
   wants to continue in OpenUtau). Summarize what you did and the final
   numbers, and say what you could not fix.

## Rules of thumb
- Prefer semantic parameters (portamento, vibrato, scoop, fall, vel) over
  drawing `pitd`; the user can read and tweak them. Use `pitd` for
  reference matching and small corrections.
- Make one kind of change at a time and render between, so you can tell
  what helped. Use `checkpoint`/`restore` before risky passes.
- Batch related ops (`apply_ops` with a `batch`) so the user can undo them
  as one step. Labels show in the person's History tab next to your
  name; write what changed and why ("Raise vel on fast chorus notes").
- Never claim it sounds good. Report the numbers.
