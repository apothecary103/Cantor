// Editing commands shared by menus and shortcuts: clipboard, delete, nudge, undo/redo.
import { S, notes, selected, select, sendOps, sendOpsNew, call, snapTick, PPQ } from './state.js';
import { fillLyrics } from './pianoroll.js';

const MARK = 'cantor-notes:';
let clip = null;

const tid = () => S.trackId;

export const undo = () => S.canUndo && call('api/undo', {});
export const redo = () => S.canRedo && call('api/redo', {});
export const selectAll = () => select(notes().map((n) => n.id));
export const deselect = () => select([]);

export function remove() {
  const ids = [...S.sel];
  if (ids.length) sendOps([{ op: 'remove_notes', track: tid(), ids }], 'Delete notes');
}

/** Copies the selection; returns clipboard text (also kept internally for menu paste). */
export function copy() {
  const ns = selected();
  if (!ns.length) return null;
  const t0 = ns[0].start;
  clip = ns.map(({ id, ...n }) => ({ ...n, start: n.start - t0 }));
  return MARK + JSON.stringify(clip);
}

export function cut() {
  const text = copy();
  remove();
  return text;
}

/** Pastes notes at the playhead, or fills lyrics of the selection from plain text. */
export async function paste(text) {
  let ns = clip;
  if (text != null) {
    if (text.startsWith(MARK)) {
      try { ns = JSON.parse(text.slice(MARK.length)); } catch { ns = null; }
    } else {
      const words = text.trim().split(/\s+/).filter(Boolean);
      const first = selected()[0];
      if (words.length && first) fillLyrics(words, first.id);
      return;
    }
  }
  if (!ns?.length || !tid()) return;
  const at = snapTick(S.playhead);
  const ids = await sendOpsNew([{ op: 'add_notes', track: tid(), notes: ns.map((n) => ({ ...n, start: n.start + at })) }], 'Paste notes');
  select(ids);
}

export function nudge(dticks, dkeys) {
  const ns = selected();
  if (!ns.length) return;
  dticks = Math.max(dticks, -Math.min(...ns.map((n) => n.start)));
  dkeys = Math.max(-Math.min(...ns.map((n) => n.key)), Math.min(127 - Math.max(...ns.map((n) => n.key)), dkeys));
  if (dticks || dkeys) sendOps([{ op: 'move_notes', track: tid(), ids: ns.map((n) => n.id), dticks, dkeys }], 'Nudge notes');
}

export const gridStep = () => S.snap || PPQ / 4;

export function resize(dlength) {
  const ns = selected();
  if (!ns.length || Math.min(...ns.map((n) => n.length)) + dlength < 10) return;
  sendOps([{ op: 'resize_notes', track: tid(), ids: ns.map((n) => n.id), dlength, from_start: false }], 'Resize notes');
}

export function merge() {
  const ids = selected().map((n) => n.id);
  if (ids.length > 1) sendOps([{ op: 'merge_notes', track: tid(), ids }], 'Merge notes');
}
