# 0030 Web UI for reviewing an agent's cover

Date: 2026-11-03

## Context
The web editor is the only GUI (0029). Its main job is the second half of
the workflow: an agent made a cover over MCP and a person checks and
polishes it. The editor could already edit everything, but a person could
not see what the agent did, could not use the checkpoints the agent guide
tells agents to make (a capability only MCP had), and had to read the
render report as a one-line summary.

## Decision
- The right panel becomes three tabs: Properties, History, Report. Tabs,
  not new panels, so the layout stays three columns.
- HTTP gains `GET /api/history`, `POST /api/checkpoint`,
  `POST /api/restore` and `steps` on undo/redo, mirroring the MCP tools.
- Events carry `label`, `source` (http|mcp|cli) and the caller's
  `X-Cantor-Client` id. The UI flashes the notes an outside edit changed
  and names it in the status bar; History tags agent steps.
- The Report tab and ruler marks grade phrases with the agent guide's
  numbers (vs reference p95 ≤ 25 / ≤ 40 ¢). Clicking selects, frames and
  loops the phrase.
- Reference listening: `A` / `Shift+A` switch synth, reference or both
  during playback; `D` shades pitch ≥ 50 ¢ from the reference F0. The diff
  compares the planned pitch line (`cantor_core::pitch`), not the
  rendered audio; the report covers the rendered audio.

## Consequences
The engine must store a source per undo step and echo client ids in
events. Not added: an activity feed or chat (History is the record),
side-by-side checkpoint comparison (restore, listen, undo instead).
