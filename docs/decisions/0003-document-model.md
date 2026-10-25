# 0003 Document model and Op details

Choices made while turning docs/spec/30-data-model.md into code:

- **Note positions are relative to their part; curves live on the track in
  absolute ticks.** The spec's `SetCurve` addresses a track, and the spec's
  `Track` had no curve field, so `Track::curves: BTreeMap<ParamId,
  LayeredCurve>` was added. Moving a part or notes with `carry_curves` moves
  the curve points under them; points already at the destination are
  replaced.
- **Canonical form.** Parts and notes are sorted by (start, id), maps by
  position, curve points by tick, and empty curves are removed. Ops keep this
  form, loaders and importers establish it, so equal documents serialize to
  equal bytes.
- **Undo stores values, not negated deltas.** `MoveNotes` with `dcents`
  cannot be undone by applying `-dcents`: `(p + d) - d != p` in floating
  point when |d| > |p|. Every inverse captures the previous values
  (`ReplaceNotes`, `RestoreCurve`, patches holding old fields). The property
  test generates low pitches with large deltas so a negating inverse fails it.
  Cost: inverses of curve edits hold the whole previous curve; acceptable
  now, worth revisiting if curve histories grow large.
- **Ids** are u64 newtypes chosen by the caller (`Project::next_*_id` helps).
  Uniqueness is validated, never assumed.
- **Revisions** increase on every change including undo and redo, so a
  revision identifies a state for optimistic concurrency (M3).
- **Snapshots** are `Arc<Project>`; edits use `Arc::make_mut`, copying only
  while a snapshot is held.
- **Retake** is not in M1: it needs phrase detection and a backend (M2/M3).
  `SelectTake`, `SetSeed` and the take lanes in the model are in place.
- **Time signatures** are indexed by bar, as in OpenUtau; tempo and keys by
  tick.
- Parameter values are deltas in the parameter's unit; a delta is valid when
  |v| ≤ max − min of the registry entry. Unknown parameters are an error
  diagnostic.
