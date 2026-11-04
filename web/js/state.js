// Shared editor state, event bus, time math, and the op pipeline.
import * as api from './api.js';

export const PPQ = 480;

export const S = {
  project: null, revision: 0, path: null, canUndo: false, canRedo: false, dirty: false,
  params: {}, voicebanks: [],
  trackId: null, sel: new Set(),
  tool: 'select', snap: PPQ / 4,               // snap in ticks, 0 = off
  view: { x: 0, y: 0, ppt: 0.18, keyH: 14 },   // x: tick at left edge, y: px scrolled, ppt: px per tick
  show: { pitch: true, ref: true, phonemes: true, follow: true, diff: false, report: true },
  pitch: null, phonemes: new Map(), f0: null,  // derived data from the server
  playhead: 0, loop: { on: false, a: 0, b: PPQ * 16 },
  playing: false, ab: 'a', report: null, reportRev: null,
  history: { undo: [], redo: [], checkpoints: [] },
  flash: null,                                 // { ids: Set, t0 }: notes the last outside edit changed
  conn: 'connecting', msg: null, progress: null,
};

// ---- event bus -------------------------------------------------------------
const subs = new Map();
export function on(events, fn) {
  for (const e of events.split(' ')) {
    if (!subs.has(e)) subs.set(e, []);
    subs.get(e).push(fn);
  }
}
export const emit = (event, arg) => subs.get(event)?.forEach((fn) => fn(arg));

// ---- params (ranges/defaults; refined from /api/schema) --------------------
export const CURVE_PARAMS = ['pitd', 'dyn', 'gen', 'bre', 'bri', 'tone'];
const BUILTIN_PARAMS = {
  pitd: { unit: 'cents', min: -1200, max: 1200, default: 0, description: 'Pitch deviation added to the note pitch' },
  dyn: { unit: 'dB', min: -24, max: 12, default: 0, description: 'Loudness' },
  gen: { unit: '%', min: -100, max: 100, default: 0, description: 'Gender/formant shift (+ = deeper)' },
  bre: { unit: '%', min: 0, max: 100, default: 0, description: 'Breathiness' },
  bri: { unit: '%', min: -100, max: 100, default: 0, description: 'Brightness/tension' },
  tone: { unit: 'st', min: -12, max: 12, default: 0, description: 'Pitch-independent timbre shift' },
  vel: { unit: '%', min: 0, max: 200, default: 100, description: 'Consonant velocity' },
  vol: { unit: '%', min: 0, max: 200, default: 100, description: 'Note intensity' },
};
S.params = structuredClone(BUILTIN_PARAMS);

/** Accepts `params` as an array of {id,...} or a map, with min/max or range:[lo,hi]. */
export function loadSchema(schema) {
  const list = schema?.params;
  const entries = Array.isArray(list) ? list.map((p) => [p.id, p]) : Object.entries(list || {});
  for (const [id, p] of entries) {
    if (!id || typeof p !== 'object') continue;
    const [min, max] = p.range || [p.min, p.max];
    S.params[id] = { ...S.params[id], ...p, min: min ?? S.params[id]?.min ?? 0, max: max ?? S.params[id]?.max ?? 100,
      default: p.default ?? S.params[id]?.default ?? 0 };
  }
}

// ---- time -------------------------------------------------------------------
let tempo = [{ tick: 0, ms: 0, bpm: 120 }];
let meter = [{ bar: 0, tick: 0, num: 4, den: 4, len: PPQ * 4 }];

/** Index of the last element with key(el) <= v in an ascending array. */
function lastLE(arr, v, key) {
  let lo = 0, hi = arr.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (key(arr[mid]) <= v) lo = mid; else hi = mid - 1;
  }
  return arr[lo];
}

function rebuildTime(p) {
  const tp = (p.tempo?.length ? [...p.tempo] : [{ tick: 0, bpm: 120 }]).sort((a, b) => a.tick - b.tick);
  tempo = [];
  tp.forEach((t, i) => {
    const prev = tempo[i - 1];
    const ms = prev ? prev.ms + ((t.tick - prev.tick) * 60000) / (prev.bpm * PPQ) : 0;
    tempo.push({ tick: t.tick, ms, bpm: t.bpm });
  });
  const ts = (p.time_sig?.length ? [...p.time_sig] : [{ bar: 0, num: 4, den: 4 }]).sort((a, b) => a.bar - b.bar);
  meter = [];
  ts.forEach((t, i) => {
    const prev = meter[i - 1];
    const tick = prev ? prev.tick + (t.bar - prev.bar) * prev.len : 0;
    meter.push({ bar: t.bar, tick, num: t.num, den: t.den, len: ((PPQ * 4) / t.den) * t.num });
  });
}

export function tickToMs(t) {
  const s = lastLE(tempo, t, (x) => x.tick);
  return s.ms + ((t - s.tick) * 60000) / (s.bpm * PPQ);
}
export function msToTick(ms) {
  const s = lastLE(tempo, ms, (x) => x.ms);
  return s.tick + ((ms - s.ms) * s.bpm * PPQ) / 60000;
}
export const bpmAt = (t) => lastLE(tempo, t, (x) => x.tick).bpm;
export const meterAt = (t) => lastLE(meter, t, (x) => x.tick);

/** Musical position of a tick: zero-based bar and beat. */
export function barOf(t) {
  const m = meterAt(t);
  const rel = t - m.tick, bar = m.bar + Math.floor(rel / m.len), within = rel - Math.floor(rel / m.len) * m.len;
  const beatLen = (PPQ * 4) / m.den;
  return { bar, beat: Math.floor(within / beatLen), sub: within % beatLen, beatLen, start: t - within, m };
}
export function barTick(bar) {
  const m = lastLE(meter, bar, (x) => x.bar);
  return m.tick + (bar - m.bar) * m.len;
}
export const fmtBarBeat = (t) => { const b = barOf(Math.max(0, t)); return `${b.bar + 1}.${b.beat + 1}`; };
export function fmtTime(ms) {
  ms = Math.max(0, ms);
  const m = Math.floor(ms / 60000), s = (ms % 60000) / 1000;
  return `${String(m).padStart(2, '0')}:${s.toFixed(2).padStart(5, '0')}`;
}

export function snapTick(t, mode = 'round') {
  if (!S.snap) return Math.round(t);
  const b = barOf(t);
  return b.start + Math[mode]((t - b.start) / S.snap) * S.snap;
}

// ---- pitch names & scales ---------------------------------------------------
const NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
export const noteName = (k) => `${NAMES[((k % 12) + 12) % 12]}${Math.floor(k / 12) - 1}`;
export const pcName = (pc) => NAMES[pc];
export const SCALES = {
  major: [0, 2, 4, 5, 7, 9, 11], minor: [0, 2, 3, 5, 7, 8, 10],
  harmonic_minor: [0, 2, 3, 5, 7, 8, 11], dorian: [0, 2, 3, 5, 7, 9, 10],
  pentatonic: [0, 2, 4, 7, 9], minor_pentatonic: [0, 3, 5, 7, 10],
};
export const isBlack = (k) => [1, 3, 6, 8, 10].includes(((k % 12) + 12) % 12);
export function inScale(k) {
  const key = S.project?.key;
  if (!key) return !isBlack(k);
  return (SCALES[key.scale] || SCALES.major).includes((((k - key.tonic) % 12) + 12) % 12);
}

// ---- selection helpers -------------------------------------------------------
export const track = () => S.project?.tracks.find((t) => t.id === S.trackId) || null;
export const notes = () => track()?.notes || [];
export const selected = () => notes().filter((n) => S.sel.has(n.id));
export function select(ids, add = false) {
  if (!add) S.sel.clear();
  for (const id of ids) S.sel.add(id);
  emit('select');
}
export function setTrack(id) {
  if (id === S.trackId) return;
  S.trackId = id;
  S.sel.clear();
  S.pitch = null;
  S.phonemes = new Map();
  emit('track');
  emit('select');
  loadDerived(false);
}

// ---- status -------------------------------------------------------------------
export function notify(text, kind = 'info') {
  S.msg = { text, kind, at: performance.now() };
  emit('status');
}
function showDiagnostics(diags) {
  if (!diags?.length) return;
  const d = diags[0];
  const more = diags.length > 1 ? ` (+${diags.length - 1} more)` : '';
  notify(`${d.message || d.code || JSON.stringify(d)}${more}`, d.level === 'error' ? 'error' : 'warn');
  console.warn('cantor diagnostics', diags);
}

// ---- server state ----------------------------------------------------------------
let inflight = null, again = false;

/** Re-reads /api/state (coalescing concurrent calls) and the derived curves. */
export function refresh() {
  if (inflight) { again = true; return inflight; }
  inflight = (async () => {
    do { again = false; await load(); } while (again);
  })().finally(() => { inflight = null; });
  return inflight;
}

export function adoptState(st) {
  const prevRef = S.project?.reference?.path;
  const prevRev = S.revision;
  S.project = st.project;
  S.revision = st.revision;
  S.path = st.path;
  S.canUndo = st.can_undo;
  S.canRedo = st.can_redo;
  S.dirty = st.dirty;
  rebuildTime(S.project);
  const tracks = S.project.tracks;
  if (!tracks.some((t) => t.id === S.trackId)) S.trackId = tracks[0]?.id ?? null;
  const live = new Set(notes().map((n) => n.id));
  for (const id of S.sel) if (!live.has(id)) S.sel.delete(id);
  emit('project');
  emit('select');
  if (prevRev !== S.revision) loadHistory();
  const refChanged = prevRef !== S.project.reference?.path || S.f0 === null;
  return prevRev !== S.revision || refChanged ? loadDerived(refChanged) : null;
}

async function load() {
  try {
    await adoptState(await api.get('api/state'));
  } catch (e) {
    notify(e.message, 'error');
  }
}

export async function loadDerived(withF0) {
  const t = track(), rev = S.revision, tid = S.trackId;
  const hasRef = !!S.project?.reference;
  const [pitch, ph, f0] = await Promise.all([
    t ? api.get(`api/tracks/${t.id}/pitch`).catch(() => null) : null,
    t ? api.get(`api/tracks/${t.id}/phonemes`).catch(() => []) : [],
    withF0 ? (hasRef ? api.get('api/reference/f0').catch(() => null) : null) : undefined,
  ]);
  if (rev !== S.revision || tid !== S.trackId) return;
  S.pitch = pitch;
  S.phonemes = new Map((ph || []).map((p) => [p.note, p]));
  if (f0 !== undefined) S.f0 = f0;
  emit('derived');
}

/** Undo/redo labels (oldest first / next first) and checkpoint names. */
export async function loadHistory() {
  try {
    const h = await api.get('api/history');
    const norm = (xs) => (xs || []).map((x) => (typeof x === 'string' ? { label: x } : x));
    S.history = { undo: norm(h.undo), redo: norm(h.redo), checkpoints: h.checkpoints || [] };
    emit('history');
  } catch { /* older server: no history endpoint */ }
}

export const sourceName = (source) => ({ mcp: 'Agent', cli: 'CLI' })[source] || 'Another window';

/** An edit someone else made (an agent over MCP, the CLI, another tab): refresh, then flash what changed. */
export async function externalEdit(ev) {
  const tid = S.trackId, before = new Map(notes().map((n) => [n.id, JSON.stringify(n)]));
  const curves = JSON.stringify(track()?.curves ?? {});
  await refresh();
  if (tid !== S.trackId) return;
  const changed = notes().filter((n) => before.get(n.id) !== JSON.stringify(n)).map((n) => n.id);
  S.flash = { ids: new Set(changed), t0: performance.now() };
  emit('flash');
  const what = [changed.length ? `${changed.length} note${changed.length > 1 ? 's' : ''}` : '',
    JSON.stringify(track()?.curves ?? {}) !== curves ? 'curves' : ''].filter(Boolean).join(', ');
  notify(`${sourceName(ev.source)}: ${ev.label || 'edit'}${what ? ` · ${what}` : ''}`, 'agent');
}

/** Sends ops (batched into one undo step when several), then refreshes. */
export async function sendOps(ops, label = 'Edit') {
  if (!ops.length) return null;
  const list = ops.length > 1 ? [{ op: 'batch', label, ops }] : ops;
  let res = null;
  try {
    res = await api.post('api/ops', { ops: list, base_revision: S.revision });
    showDiagnostics(res.diagnostics);
  } catch (e) {
    notify(e.message, 'error');
    showDiagnostics(e.diagnostics);
  }
  await refresh();
  return res;
}

/** Runs an op and returns the ids of notes that appeared in the active track. */
export async function sendOpsNew(ops, label) {
  const before = new Set(notes().map((n) => n.id));
  await sendOps(ops, label);
  return notes().filter((n) => !before.has(n.id)).map((n) => n.id);
}

export async function call(path, body, okMsg) {
  try {
    const r = await api.post(path, body);
    showDiagnostics(r?.diagnostics);
    if (okMsg) notify(typeof okMsg === 'function' ? okMsg(r) : okMsg);
    await refresh();
    return r;
  } catch (e) {
    notify(e.message, 'error');
    showDiagnostics(e.diagnostics);
    return null;
  }
}

// ---- curves ------------------------------------------------------------------------
/** Value of a track curve ({ points: [[tick, v]], interp }) at tick t. */
export function curveAt(curve, t, dflt = 0) {
  const pts = curve?.points;
  if (!pts?.length) return dflt;
  if (t <= pts[0][0]) return pts[0][1];
  const p = lastLE(pts, t, (x) => x[0]), i = pts.indexOf(p);
  const q = pts[i + 1];
  if (!q) return p[1];
  if (curve.interp === 'step') return p[1];
  let u = (t - p[0]) / (q[0] - p[0] || 1);
  if (curve.interp === 'smooth') u = (1 - Math.cos(Math.PI * u)) / 2;
  return p[1] + (q[1] - p[1]) * u;
}

/** Final pitch (cents) at tick t from the server's 5 ms curve, or null. */
export function pitchAt(t) {
  const pc = S.pitch;
  if (!pc?.cents?.length) return null;
  return pc.cents[Math.round((tickToMs(t) - pc.start_ms) / pc.step_ms)] ?? null;
}
