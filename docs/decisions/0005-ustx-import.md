# 0005 — OpenUtau USTX import

Status: accepted. Code: `crates/cantor-io/src/import/ustx.rs`.

## Scope
USTX versions 0.5–0.9 as OpenUtau writes them, parsed with serde-saphyr into
lenient private structs (unknown fields ignored, missing fields defaulted,
`null` lists read as empty, any scalar accepted where text is expected, so an
unquoted lyric `1` stays `"1"`). Versions outside 0.5–0.9 import with an
`ustx.version` info. Older per-note fields (phoneme lists, note
`expressions` maps from before 0.5) are not read.

## Mapping
- Ticks: every value is rescaled from `resolution` (default 480) with
  `rescale`. Absolute positions are rescaled as sums (`part + note`), so
  notes, parts and curves land on one grid; any rounding gives one
  `ustx.resolution_inexact` warning with a count.
- Tempo: `tempos`, else the legacy top-level `bpm`. Time signatures:
  `time_signatures` (`bar_position` is a 0-based bar), else legacy
  `beat_per_bar`/`beat_unit`. Duplicates keep the last in file order. A
  missing tick-0 tempo extends the first tempo; a missing bar-0 signature
  becomes 4/4.
- `key` (0–11) becomes a Major key at tick 0: OpenUtau's key is the major
  "do" of its movable-do display and stores no mode.
- Tracks: `singer` → `SingerRef.id`; `track_name` (or "Track N");
  `volume` dB → `gain_db`; `pan` / 100 → `pan`. Renderer
  (`renderer_settings.renderer`, or legacy `renderer`): `CLASSIC` and
  `WORLDLINE-R` → backend `utau`; `DIFFSINGER` → `diffsinger`; none → `utau`
  (OpenUtau's default). Other renderers (ENUNU, VOGEN, …) keep their
  lowercased name as the backend with a warning, so the user's choice is
  visible rather than silently replaced. Resampler, wavtool and phonemizer
  are reported (info); Cantor derives these from the backend and singer.
- Parts: `duration` → length; missing/0 derives from the notes; parts are
  lengthened to cover their notes.
- Notes: `tone` × 100 → `pitch_cents`. Lyrics starting with `+` (`+`, `+~`,
  `+*`, …) become Cantor's `+`; variants are reported because their extra
  meaning (vowel/consonant handling) is lost.
- Vibrato, only when `length` > 0 and `depth` > 0: `start = 1 − length/100`
  (OpenUtau's vibrato covers the last `length`% of the note), `fade_in =
  in/100`, `fade_out = out/100` (both % of the vibrato in OpenUtau too),
  `phase = shift/100`, `rate_hz = 1000/period`, `depth_cents = depth`.
  OpenUtau `drift` shifts the vibrato's center pitch, unlike Cantor's rate
  jitter, so it is dropped with a warning; so is `vol_link` (tremolo).
- Portamento from note pitch points (x = ms from the note start, y = tenths
  of a semitone from the tone; `snap_first` starts from the previous note):
  `length_ms = last.x − first.x`, `before = −first.x / length_ms`. A single
  interior point past the target (judged against the adjacent previous
  note) becomes `overshoot_cents`. Non-`io` shapes, other interior points,
  windows not spanning the onset, ends off target and fixed (non-snapped)
  starts are approximated with an info. Flat, unsnapped points mean no
  glide (`length_ms = 0`).
- Curves: only `pitd` (pitch deviation, cents, linear) maps, to the `pitch`
  param's `user_delta` in absolute ticks with linear interpolation. Points
  landing on the same tick of a track (overlapping parts, rounding) keep the
  last in file order; values beyond the registry's delta bound are clamped.
  Both are reported. Other OpenUtau curves (`dyn`, `ene`, `brec`, `genc`,
  `tenc`, `voic`, …) use renderer-specific percent scales with no confirmed
  conversion to Cantor's dB/semitone/logit units, so they are reported, not
  guessed.

## Not imported (diagnosed)
Wave (audio) parts; phoneme overrides (they patch the phonemizer's output,
which is unknown at import time, while Cantor's override replaces the whole
sequence); phoneme expressions (`vel`, …: per-phoneme, no Cantor slot); part
comments. Cosmetic and app settings (`track_color`, `output_dir`,
`cache_dir`, expression descriptors, selectors) are ignored without a
diagnostic since they carry no musical data.

## Errors
Malformed YAML, non-UTF-8 input, a non-positive resolution or tick values
beyond ±2^40 are `ImportError::Parse`. YAML with no `ustx_version`, tracks or
voice parts is `ImportError::Unsupported`. Everything else imports with
diagnostics; repeated per-note or per-point problems are aggregated into one
diagnostic per kind with a count, targeting the first occurrence.
