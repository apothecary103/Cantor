# Programmatic surfaces

All surfaces call the same operation API in cantor-core.

## Python (primary scripting language)
```python
import cantor as ct

p = ct.open("song.cantor")
lead = p.track("lead")
with p.transaction("soften chorus ends"):
    for n in lead.notes(section="chorus"):
        if n.is_phrase_end:
            n.curve("breathiness").ramp(to=0.4, over="last 30%")
            n.vibrato(depth_cents=50, rate_hz=5.5, start=0.5)

report = p.render(quality="preview")
for m in report.notes:
    if abs(m.pitch_error_p95) > 25:
        print(m.id, m.lyric, f"{m.pitch_error_p95:+.0f} cents")
```
Ship type stubs so editors autocomplete the API.

## CLI
`cantor render song.cantor -o out.wav --quality final`
`cantor ops apply song.cantor edits.json [--dry-run]`
`cantor analyze song.cantor --json`
`cantor import score.musicxml -o song.cantor`

## JSON-RPC 2.0
WebSocket and stdio. Method names mirror the MCP tools below. Optimistic
concurrency: every mutating call carries `base_revision`; a stale revision
returns a conflict with a diff.

## MCP (2026-07-28 stateless; rmcp 3.x)
There are no sessions, so every call carries `project_id`, and mutating
calls also carry `base_revision`. Implement `server/discover`.

Tools (put usage guidance in each tool's description; keep it short):
- describe_project(project_id) → compact text: tracks, sections, phrases,
  notes with lyrics, phonemes and durations. Token-efficient, not raw arrays.
- describe_range(project_id, track, t0, t1, detail) → a slice; curves
  summarized as key points plus statistics.
- list_singers / get_singer_schema → languages, parameters, ranges, modes,
  license terms.
- apply_ops(project_id, base_revision, ops, dry_run) → atomic; returns new
  revision, diff, diagnostics.
- retake(project_id, base_revision, track, range, dims, n_variants, seed?)
- select_take(project_id, base_revision, phrase_id, take_id)
- render(project_id, range?, quality) → audio URI + analysis report.
- compare(render_a, render_b) → metric deltas.
- checkpoint / restore / list_history
- import_file / export(format)

Annotate tools as read-only or destructive accurately.
Resources: the project JSON, generated schemas, singer docs, analysis reports.
Prompts: "tune this phrase to ≤15 cents p95 pitch error", "make the chorus
breathier and more intimate", "add a two-part harmony a third above".
