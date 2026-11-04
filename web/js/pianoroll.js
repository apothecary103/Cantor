// Canvas piano roll: grid, notes, pitch curves, ruler, keys, and the editing tools.
import {
  S, PPQ, on, emit, track, notes, select, sendOps, sendOpsNew, snapTick, barOf, barTick, tickToMs, msToTick,
  inScale, isBlack, noteName, curveAt, pitchAt,
} from './state.js';
import { h } from './ui.js';
import { grade, phraseScore } from './report.js';

const FLASH_MS = 2500;   // how long notes an agent just changed stay outlined
const DIFF_CENTS = 50;   // pitch vs reference F0 difference worth shading

export const KEYS_W = 52;
const RULER_H = 36, BAR_ROW = 19;
const MIN_PPT = 0.004, MAX_PPT = 2, MIN_KEYH = 6, MAX_KEYH = 40;

let wrap, base, over, bctx, octx, W = 0, H = 0, dpr = 1;
let dirty = true, overDirty = true, fitPending = false;
let drag = null, pending = null, hover = null, spaceDown = false;
const C = {};

// ---- coordinates -----------------------------------------------------------------------
export const tx = (t) => KEYS_W + (t - S.view.x) * S.view.ppt;
export const xt = (x) => (x - KEYS_W) / S.view.ppt + S.view.x;
const ky = (k) => RULER_H + (127 - k) * S.view.keyH - S.view.y;
const yk = (y) => 127 - Math.floor((y - RULER_H + S.view.y) / S.view.keyH);
const cy = (c) => RULER_H + (127.5 - c / 100) * S.view.keyH - S.view.y;
const yc = (y) => (127.5 - (y - RULER_H + S.view.y) / S.view.keyH) * 100;
const mx = (ms) => tx(msToTick(ms));
const crisp = (v) => Math.round(v) + 0.5;

export function invalidate() { dirty = true; overDirty = true; }
const invalidateOver = () => { overDirty = true; };

// ---- init ---------------------------------------------------------------------------------
export function initRoll(el) {
  wrap = el;
  base = wrap.querySelector('canvas');
  over = h('canvas', { class: 'overlay' });
  wrap.append(over, lyricInput);
  bctx = base.getContext('2d');
  octx = over.getContext('2d');
  new ResizeObserver(resize).observe(wrap);
  readColors();
  base.addEventListener('pointerdown', down);
  base.addEventListener('pointermove', move);
  base.addEventListener('pointerup', up);
  base.addEventListener('pointercancel', up);
  base.addEventListener('pointerleave', () => { hover = null; invalidateOver(); });
  base.addEventListener('dblclick', dblclick);
  base.addEventListener('contextmenu', (e) => e.preventDefault());
  base.addEventListener('wheel', wheel, { passive: false });
  addEventListener('keydown', (e) => { if (e.code === 'Space' && e.target === document.body) spaceDown = true; });
  addEventListener('keyup', (e) => { if (e.code === 'Space') spaceDown = false; });
  on('project track derived select', invalidate);
  on('view', invalidate);
  on('playhead', () => { follow(); invalidateOver(); });
  on('theme', () => { readColors(); invalidate(); });
  on('loop report flash', invalidate);
  on('tool', () => { cursor(null); invalidateOver(); });
  requestAnimationFrame(frame);
}

function readColors() {
  const cs = getComputedStyle(document.documentElement);
  for (const v of ['roll-bg', 'row-off', 'row-line', 'row-c', 'grid-bar', 'grid-beat', 'grid-sub', 'note', 'note-edge',
    'note-text', 'note-dim', 'note-sel', 'note-sel-text', 'accent', 'accent-soft', 'pitch', 'ref', 'danger', 'ruler-bg',
    'text', 'text-dim', 'text-faint', 'key-white', 'key-black', 'key-line', 'section', 'other-note', 'phon', 'line']) {
    C[v] = cs.getPropertyValue('--' + v).trim();
  }
}

function resize() {
  dpr = devicePixelRatio || 1;
  W = wrap.clientWidth;
  H = wrap.clientHeight;
  for (const c of [base, over]) {
    c.width = Math.round(W * dpr);
    c.height = Math.round(H * dpr);
    c.style.width = W + 'px';
    c.style.height = H + 'px';
  }
  clampView();
  invalidate();
  if (fitPending) fit();
}

function frame() {
  if (S.flash) {
    if (performance.now() - S.flash.t0 > FLASH_MS) S.flash = null;
    dirty = true;
  }
  if (dirty) { dirty = false; drawBase(); }
  if (overDirty) { overDirty = false; drawOver(); }
  requestAnimationFrame(frame);
}

// ---- view -----------------------------------------------------------------------------------
function clampView() {
  const v = S.view;
  v.ppt = Math.min(MAX_PPT, Math.max(MIN_PPT, v.ppt));
  v.keyH = Math.min(MAX_KEYH, Math.max(MIN_KEYH, v.keyH));
  v.x = Math.max(-PPQ, v.x);
  v.y = Math.max(0, Math.min(128 * v.keyH - (H - RULER_H), v.y));
}
function viewChanged() { clampView(); emit('view'); }

export function scrollBy(dt, dy = 0) {
  S.view.x += dt;
  S.view.y += dy;
  viewChanged();
}

export function zoomX(f, ax = KEYS_W + (W - KEYS_W) / 2) {
  const t = xt(ax);
  S.view.ppt *= f;
  clampView();
  S.view.x = t - (ax - KEYS_W) / S.view.ppt;
  viewChanged();
}
function zoomY(f, ay) {
  const c = yc(ay);
  S.view.keyH *= f;
  clampView();
  S.view.y = RULER_H + (127.5 - c / 100) * S.view.keyH - ay;
  viewChanged();
}

/** Fits the active track's notes (or the selection) into view. */
export function fit() {
  const ns = notes().filter((n) => !S.sel.size || S.sel.has(n.id));
  fitPending = !W || H < RULER_H + 40;
  if (!ns.length || fitPending) return;
  const t0 = Math.min(...ns.map((n) => n.start)), t1 = Math.max(...ns.map((n) => n.start + n.length));
  const k0 = Math.min(...ns.map((n) => n.key)) - 4, k1 = Math.max(...ns.map((n) => n.key)) + 4;
  S.view.ppt = (W - KEYS_W - 40) / Math.max(PPQ, t1 - t0);
  S.view.keyH = Math.min(18, Math.max(MIN_KEYH, (H - RULER_H) / (k1 - k0 + 1)));
  clampView();
  S.view.x = t0 - 20 / S.view.ppt;
  const mid = (k0 + k1 + 1) / 2;
  S.view.y = (127.5 - mid) * S.view.keyH - (H - RULER_H) / 2;
  viewChanged();
}

function follow() {
  if (!S.playing || !S.show.follow || drag) return;
  const x = tx(S.playhead);
  if (x > W - 40 || x < KEYS_W) { S.view.x = S.playhead - 40 / S.view.ppt; viewChanged(); }
}

export function scrollToNote(n) {
  const x0 = tx(n.start), x1 = tx(n.start + n.length), y = ky(n.key);
  if (x0 < KEYS_W || x1 > W) S.view.x = n.start - (W - KEYS_W) / 3 / S.view.ppt;
  if (y < RULER_H || y + S.view.keyH > H) S.view.y += y - (RULER_H + H) / 2;
  viewChanged();
}

// ---- geometry with drag preview ------------------------------------------------------------
function geom(n) {
  const d = drag?.moved ? drag : pending;
  let { start, length, key } = n;
  if (d && d.ids?.includes(n.id)) {
    if (d.type === 'move') { start += d.dt; key += d.dk; }
    if (d.type === 'resize') {
      if (d.edge === 'start') { start -= d.d; length += d.d; } else length += d.d;
    }
  }
  return { start, length, key };
}

function hitNote(x, y) {
  const t = xt(x), k = yk(y);
  for (const n of notes()) {
    if (n.key !== k || t < n.start || t >= n.start + n.length) continue;
    const x0 = tx(n.start), x1 = tx(n.start + n.length), edge = x1 - x0 > 14 ? 5 : 0;
    return { note: n, edge: x - x0 < edge ? 'start' : x1 - x < edge ? 'end' : null };
  }
  return null;
}

// ---- drawing ----------------------------------------------------------------------------------
function drawBase() {
  const ctx = bctx, { keyH, ppt } = S.view;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = C['roll-bg'];
  ctx.fillRect(0, 0, W, H);
  if (!S.project) return;
  const t0 = xt(KEYS_W), t1 = xt(W), kTop = yk(RULER_H), kBot = Math.max(0, yk(H));

  // rows
  for (let k = kBot; k <= Math.min(127, kTop); k++) {
    const y = ky(k);
    if (!inScale(k)) { ctx.fillStyle = C['row-off']; ctx.fillRect(KEYS_W, y, W, keyH); }
    ctx.fillStyle = k % 12 === 0 ? C['row-c'] : C['row-line'];
    ctx.fillRect(KEYS_W, Math.round(y + keyH) - 1, W, 1);
  }

  // vertical grid
  for (let b = barOf(Math.max(0, t0)).bar; barTick(b) < t1; b++) {
    const bt = barTick(b), m = barOf(bt).m, beat = (PPQ * 4) / m.den;
    const sub = S.snap && S.snap * ppt >= 8 ? S.snap : beat;
    for (let t = bt; t < bt + m.len; t += sub) {
      const isBeat = (t - bt) % beat === 0;
      if (!isBeat && sub * ppt < 8) continue;
      if (isBeat && t !== bt && beat * ppt < 6) continue;
      ctx.fillStyle = t === bt ? C['grid-bar'] : isBeat ? C['grid-beat'] : C['grid-sub'];
      ctx.fillRect(Math.round(tx(t)), RULER_H, 1, H);
    }
  }
  for (const s of S.project.sections || []) {
    ctx.fillStyle = C.section;
    ctx.fillRect(Math.round(tx(s.start)), RULER_H, 1, H);
  }
  if (S.loop.on) {
    ctx.fillStyle = C['accent-soft'];
    ctx.fillRect(tx(S.loop.a), RULER_H, (S.loop.b - S.loop.a) * ppt, H);
  }

  // other tracks
  ctx.strokeStyle = C['other-note'];
  for (const tr of S.project.tracks) {
    if (tr.id === S.trackId) continue;
    for (const n of tr.notes) {
      if (n.start + n.length < t0 || n.start > t1) continue;
      ctx.strokeRect(crisp(tx(n.start)), crisp(ky(n.key)), Math.round(n.length * ppt) - 1, keyH - 1);
    }
  }

  drawNotes(ctx, t0, t1);
  if (S.show.diff && S.pitch && S.f0 && S.project.reference) drawDiff(ctx, t0, t1);
  if (S.show.ref && S.f0 && S.project.reference) {
    drawCurve(ctx, S.f0.cents, S.f0.step_ms, -(S.project.reference.offset_ms || 0), C.ref, 1.25, t0, t1);
  }
  if (S.show.pitch && S.pitch) {
    ctx.globalAlpha = drag?.moved || pending ? 0.35 : 1;
    drawCurve(ctx, S.pitch.cents, S.pitch.step_ms, S.pitch.start_ms, C.pitch, 1.25, t0, t1);
    ctx.globalAlpha = 1;
  }
  drawLabels(ctx, t0, t1);
  drawRuler(ctx, t0, t1);
  drawKeys(ctx, kBot, kTop);
  ctx.fillStyle = C['ruler-bg'];
  ctx.fillRect(0, 0, KEYS_W, RULER_H);
  ctx.fillStyle = C.line;
  ctx.fillRect(0, RULER_H - 1, W, 1);
  ctx.fillRect(KEYS_W - 1, 0, 1, H);
}

function visibleNotes(t0, t1) {
  return notes().map((n) => [n, geom(n)]).filter(([, g]) => g.start + g.length >= t0 && g.start <= t1);
}

function noteBox(g) {
  const x = Math.round(tx(g.start));
  return { x, w: Math.max(2, Math.round(tx(g.start + g.length)) - x - 1), y: Math.round(ky(g.key)) + 1, hgt: S.view.keyH - 2 };
}

function drawNotes(ctx, t0, t1) {
  for (const [n, g] of visibleNotes(t0, t1)) {
    const { x, w, y, hgt } = noteBox(g);
    const sel = S.sel.has(n.id), ph = S.phonemes.get(n.id);
    if (S.show.phonemes && ph && Number.isFinite(ph.start_ms)) {
      const px = Math.round(mx(ph.start_ms));
      if (px < x - 1) {
        ctx.fillStyle = C.phon;
        ctx.fillRect(px, y + hgt - 2, x - px, 1);
        ctx.fillRect(px, y + hgt - 6, 1, 5);
      }
    }
    ctx.fillStyle = sel ? C['note-sel'] : C.note;
    ctx.globalAlpha = (n.lyric === 'br' || n.lyric === '-') && !sel ? 0.5 : 1;
    ctx.fillRect(x, y, w, hgt);
    ctx.globalAlpha = 1;
    ctx.strokeStyle = sel ? C['note-sel'] : C['note-edge'];
    ctx.strokeRect(x + 0.5, y + 0.5, w - 1, hgt - 1);
    if (ph?.missing) { ctx.fillStyle = C.danger; ctx.fillRect(x + 1, y + hgt - 3, w - 2, 2); }
    if (S.flash?.ids.has(n.id)) {
      ctx.globalAlpha = Math.max(0, 1 - (performance.now() - S.flash.t0) / FLASH_MS);
      ctx.strokeStyle = C.accent;
      ctx.lineWidth = 2;
      ctx.strokeRect(x - 1, y - 1, w + 2, hgt + 2);
      ctx.lineWidth = 1;
      ctx.globalAlpha = 1;
    }
  }
}

/** Shades where the final pitch line is more than DIFF_CENTS away from the reference F0. */
function drawDiff(ctx, t0, t1) {
  const pc = S.pitch, f0 = S.f0, off = S.project.reference.offset_ms || 0;
  const m0 = tickToMs(Math.max(0, t0)), m1 = tickToMs(t1);
  const i0 = Math.max(0, Math.floor((m0 - pc.start_ms) / pc.step_ms));
  const i1 = Math.min(pc.cents.length - 1, Math.ceil((m1 - pc.start_ms) / pc.step_ms));
  ctx.fillStyle = C.danger;
  ctx.globalAlpha = 0.28;
  for (let i = i0; i <= i1; i++) {
    const p = pc.cents[i], ms = pc.start_ms + i * pc.step_ms;
    const r = f0.cents[Math.round((ms + off) / f0.step_ms)];
    if (p == null || r == null || Math.abs(p - r) < DIFF_CENTS) continue;
    const xa = mx(ms), xb = mx(ms + pc.step_ms), ya = cy(p), yb = cy(r);
    ctx.fillRect(xa, Math.min(ya, yb), Math.max(1, xb - xa), Math.abs(yb - ya));
  }
  ctx.globalAlpha = 1;
}

/** Report markers: one bar per phrase under the ruler, colored by its pitch error. */
function drawPhraseMarks(ctx) {
  const r = S.report;
  if (!S.show.report || !r?.phrases) return;
  const stale = S.reportRev != null && S.reportRev !== S.revision;
  const colors = { ok: C['text-faint'], warn: C.accent, bad: C.danger, '': C['text-faint'] };
  ctx.globalAlpha = stale ? 0.4 : 0.9;
  for (const ph of r.phrases) {
    if (ph.track !== S.trackId) continue;
    const x0 = Math.round(mx(ph.start_sec * 1000)), x1 = Math.round(mx(ph.end_sec * 1000));
    if (x1 < KEYS_W || x0 > W) continue;
    const { v, band } = phraseScore(ph);
    ctx.fillStyle = colors[ph.voiced_ratio < 0.9 ? 'bad' : grade(v, band)];
    ctx.fillRect(x0, RULER_H - 4, Math.max(2, x1 - x0 - 1), 3);
  }
  ctx.globalAlpha = 1;
}

/** Lyric + resolved alias, drawn on a chip above the curves so they stay legible. */
function drawLabels(ctx, t0, t1) {
  const { keyH } = S.view;
  if (keyH < 9) return;
  ctx.font = `${keyH < 13 ? 10 : 11}px ${getComputedStyle(document.body).fontFamily}`;
  ctx.textBaseline = 'middle';
  for (const [n, g] of visibleNotes(t0, t1)) {
    const { x, w, y, hgt } = noteBox(g);
    if (w < 12) continue;
    const sel = S.sel.has(n.id), ph = S.phonemes.get(n.id);
    const alias = ph && ph.alias !== n.lyric && keyH >= 13 ? ph.alias : '';
    const lw = ctx.measureText(n.lyric).width, aw = alias ? ctx.measureText(alias).width + 5 : 0;
    ctx.save();
    ctx.beginPath();
    ctx.rect(x + 1, y + 1, w - 2, hgt - 2);
    ctx.clip();
    ctx.fillStyle = sel ? C['note-sel'] : C.note;
    ctx.fillRect(x + 1, y + 2, lw + aw + 6, hgt - 5);
    const ty = y + hgt / 2;
    ctx.fillStyle = ph?.missing && !alias ? C.danger : sel ? C['note-sel-text'] : n.lyric === '+' ? C['note-dim'] : C['note-text'];
    ctx.fillText(n.lyric, x + 4, ty);
    if (alias) {
      ctx.fillStyle = ph.missing ? C.danger : sel ? C['note-sel-text'] : C['note-dim'];
      ctx.fillText(alias, x + 9 + lw, ty);
    }
    ctx.restore();
  }
}

function drawCurve(ctx, cents, stepMs, startMs, color, width, t0, t1) {
  if (!cents?.length) return;
  const m0 = tickToMs(Math.max(0, t0)), m1 = tickToMs(t1);
  const i0 = Math.max(0, Math.floor((m0 - startMs) / stepMs) - 1);
  const i1 = Math.min(cents.length - 1, Math.ceil((m1 - startMs) / stepMs) + 1);
  const pxPerSample = (mx(startMs + stepMs * 100) - mx(startMs)) / 100;
  const skip = Math.max(1, Math.floor(0.75 / pxPerSample));
  ctx.strokeStyle = color;
  ctx.lineWidth = width;
  ctx.lineJoin = 'round';
  ctx.beginPath();
  let pen = false;
  for (let i = i0; i <= i1; i += skip) {
    const c = cents[i];
    if (c == null) { pen = false; continue; }
    const x = mx(startMs + i * stepMs), y = cy(c);
    if (pen) ctx.lineTo(x, y); else ctx.moveTo(x, y);
    pen = true;
  }
  ctx.stroke();
  ctx.lineWidth = 1;
}

function drawRuler(ctx, t0, t1) {
  const { ppt } = S.view;
  ctx.fillStyle = C['ruler-bg'];
  ctx.fillRect(KEYS_W, 0, W, RULER_H);
  ctx.fillStyle = C.line;
  ctx.fillRect(KEYS_W, BAR_ROW, W, 1);
  if (S.loop.a < S.loop.b) {
    ctx.fillStyle = S.loop.on ? C.accent : C['text-faint'];
    ctx.globalAlpha = S.loop.on ? 0.35 : 0.25;
    ctx.fillRect(tx(S.loop.a), 0, (S.loop.b - S.loop.a) * ppt, BAR_ROW);
    ctx.globalAlpha = 1;
  }
  ctx.font = `10.5px ${getComputedStyle(document.body).fontFamily}`;
  ctx.textBaseline = 'middle';
  const barPx = barOf(0).m.len * ppt, every = barPx < 28 ? Math.ceil(28 / barPx) : 1;
  for (let b = barOf(Math.max(0, t0)).bar; barTick(b) < t1; b++) {
    const x = Math.round(tx(barTick(b))), m = barOf(barTick(b)).m;
    if (b % every === 0) {
      ctx.fillStyle = C['grid-bar'];
      ctx.fillRect(x, 4, 1, BAR_ROW - 4);
      ctx.fillStyle = C['text-dim'];
      ctx.fillText(String(b + 1), x + 4, BAR_ROW / 2 + 1);
    }
    const beat = (PPQ * 4) / m.den;
    if (beat * ppt >= 10) {
      ctx.fillStyle = C['grid-beat'];
      for (let t = barTick(b) + beat; t < barTick(b) + m.len; t += beat) ctx.fillRect(Math.round(tx(t)), BAR_ROW - 5, 1, 5);
    }
  }
  for (const s of S.project.sections || []) {
    const x0 = Math.round(tx(s.start)), x1 = Math.round(tx(s.end));
    if (x1 < KEYS_W || x0 > W) continue;
    ctx.fillStyle = C.section;
    ctx.fillRect(x0, BAR_ROW + 1, Math.max(1, x1 - x0 - 1), RULER_H - BAR_ROW - 2);
    ctx.fillStyle = C['grid-bar'];
    ctx.fillRect(x0, BAR_ROW + 1, 1, RULER_H - BAR_ROW - 2);
    ctx.save();
    ctx.beginPath();
    ctx.rect(x0, BAR_ROW, x1 - x0 - 4, RULER_H - BAR_ROW);
    ctx.clip();
    ctx.fillStyle = C.text;
    ctx.fillText(s.name, Math.max(x0, KEYS_W) + 5, BAR_ROW + (RULER_H - BAR_ROW) / 2 - 1);
    ctx.restore();
  }
  drawPhraseMarks(ctx);
}

function drawKeys(ctx, kBot, kTop) {
  const { keyH } = S.view;
  ctx.fillStyle = C['key-white'];
  ctx.fillRect(0, RULER_H, KEYS_W, H);
  ctx.font = `10px ${getComputedStyle(document.body).fontFamily}`;
  ctx.textBaseline = 'middle';
  for (let k = kBot; k <= Math.min(127, kTop); k++) {
    const y = Math.round(ky(k));
    if (isBlack(k)) { ctx.fillStyle = C['key-black']; ctx.fillRect(0, y, KEYS_W * 0.62, keyH); }
    if (k % 12 === 0 || k % 12 === 5) { ctx.fillStyle = C['key-line']; ctx.fillRect(0, Math.round(y + keyH) - 1, KEYS_W, 1); }
    if (hover?.key === k) { ctx.fillStyle = C['accent-soft']; ctx.fillRect(0, y, KEYS_W, keyH); }
    if (k % 12 === 0 && keyH >= 8) {
      ctx.fillStyle = C['text-dim'];
      ctx.fillText(noteName(k), KEYS_W - 26, y + keyH / 2 + 0.5);
    }
  }
}

function drawOver() {
  const ctx = octx;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, W, H);
  if (!S.project) return;
  ctx.save();
  ctx.beginPath();
  ctx.rect(KEYS_W, 0, W - KEYS_W, H);
  ctx.clip();
  const d = drag;
  if (d?.type === 'band') {
    ctx.fillStyle = C['accent-soft'];
    ctx.strokeStyle = C.accent;
    const x = Math.min(d.x0, d.x1), y = Math.min(d.y0, d.y1);
    ctx.fillRect(x, y, Math.abs(d.x1 - d.x0), Math.abs(d.y1 - d.y0));
    ctx.strokeRect(crisp(x), crisp(y), Math.round(Math.abs(d.x1 - d.x0)), Math.round(Math.abs(d.y1 - d.y0)));
  }
  if (d?.type === 'draw') {
    ctx.fillStyle = C['accent-soft'];
    ctx.strokeStyle = C.accent;
    const x = tx(d.start), w = (d.end - d.start) * S.view.ppt;
    ctx.fillRect(x, ky(d.key) + 1, w, S.view.keyH - 2);
    ctx.strokeRect(crisp(x), crisp(ky(d.key) + 1), Math.round(w) - 1, S.view.keyH - 3);
  }
  if (d?.type === 'pen' && d.pts.length > 1) {
    ctx.strokeStyle = d.erase ? C.danger : C.accent;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    d.pts.forEach(([t, c], i) => (i ? ctx.lineTo(tx(t), cy(c)) : ctx.moveTo(tx(t), cy(c))));
    ctx.stroke();
  }
  if (!d && hover && S.tool === 'knife' && hover.note) {
    const t = snapTick(hover.tick), n = hover.note;
    if (t > n.start && t < n.start + n.length) {
      ctx.fillStyle = C.accent;
      ctx.fillRect(Math.round(tx(t)), ky(n.key) - 3, 1, S.view.keyH + 6);
    }
  }
  if (!d && hover && S.tool === 'draw' && !hover.note && hover.key >= 0) {
    ctx.strokeStyle = C['text-faint'];
    const t = snapTick(hover.tick, 'floor');
    ctx.strokeRect(crisp(tx(t)), crisp(ky(hover.key) + 1), Math.round((S.snap || PPQ / 4) * S.view.ppt) - 1, S.view.keyH - 3);
  }
  ctx.restore();
  const px = Math.round(tx(S.playhead));
  if (px >= KEYS_W && px <= W) {
    ctx.fillStyle = C.accent;
    ctx.fillRect(px, 0, 1, H);
    ctx.beginPath();
    ctx.moveTo(px - 5, 0);
    ctx.lineTo(px + 6, 0);
    ctx.lineTo(px + 0.5, 7);
    ctx.fill();
  }
}

// ---- pointer interaction ---------------------------------------------------------------------
const pos = (e) => { const r = base.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; };

function cursor(hit, y = RULER_H + 1, x = KEYS_W + 1) {
  let c = 'default';
  if (y < RULER_H || x < KEYS_W) c = y < RULER_H && x >= KEYS_W ? 'text' : 'default';
  else if (spaceDown) c = 'grab';
  else if (S.tool === 'pen') c = 'crosshair';
  else if (S.tool === 'knife') c = hit ? 'col-resize' : 'default';
  else if (hit?.edge) c = 'ew-resize';
  else if (hit) c = 'move';
  else if (S.tool === 'draw') c = 'crosshair';
  base.style.cursor = c;
}

function down(e) {
  if (lyricInput.style.display === 'block') lyricInput.blur();
  const { x, y } = pos(e);
  base.setPointerCapture(e.pointerId);
  if (e.button === 1 || (e.button === 0 && spaceDown)) {
    drag = { type: 'pan', x, y, vx: S.view.x, vy: S.view.y };
    return;
  }
  if (y < RULER_H) {
    if (x < KEYS_W) return;
    const t = Math.max(0, snapTick(xt(x)));
    if (y >= BAR_ROW && !e.shiftKey) {
      const s = (S.project.sections || []).find((sec) => t >= sec.start && t < sec.end);
      if (s) { setPlayhead(s.start); return; }
    }
    drag = e.shiftKey || e.altKey ? { type: 'loop', a: t } : { type: 'seek' };
    if (drag.type === 'seek') setPlayhead(t);
    return;
  }
  if (x < KEYS_W || !track()) return;
  const t = xt(x), k = yk(y), hit = hitNote(x, y);
  if (S.tool === 'pen') {
    drag = { type: 'pen', pts: [[t, yc(y)]], erase: e.altKey || e.button === 2 };
    return;
  }
  if (S.tool === 'knife') {
    if (hit) {
      const at = snapTick(t);
      if (at > hit.note.start && at < hit.note.start + hit.note.length) {
        sendOps([{ op: 'split_note', track: S.trackId, id: hit.note.id, at }], 'Split note');
      }
    }
    return;
  }
  if (e.button === 2) {
    if (hit && S.tool === 'draw') sendOps([{ op: 'remove_notes', track: S.trackId, ids: [hit.note.id] }], 'Delete note');
    return;
  }
  if (hit) {
    const was = S.sel.has(hit.note.id);
    if (!was) select([hit.note.id], e.shiftKey);
    drag = {
      type: hit.edge ? 'resize' : 'move', edge: hit.edge, anchor: hit.note, ids: [...S.sel],
      x0: x, y0: y, t0: t, k0: k, dt: 0, dk: 0, d: 0, moved: false, wasSelected: was, shift: e.shiftKey,
    };
  } else if (S.tool === 'draw') {
    const start = Math.max(0, snapTick(t, 'floor'));
    drag = { type: 'draw', start, key: k, end: start + (S.snap || PPQ / 4) };
  } else {
    if (!e.shiftKey) select([]);
    drag = { type: 'band', x0: x, y0: y, x1: x, y1: y, base: new Set(S.sel) };
  }
  invalidateOver();
}

function move(e) {
  const { x, y } = pos(e), t = xt(x), k = yk(y);
  if (!drag) {
    const hit = y > RULER_H && x > KEYS_W ? hitNote(x, y) : null;
    const prevKey = hover?.key;
    hover = { tick: t, key: y > RULER_H ? k : -1, note: hit?.note };
    cursor(hit, y, x);
    emit('cursor', { tick: t, key: hover.key });
    if (prevKey !== hover.key) dirty = true;
    invalidateOver();
    return;
  }
  const d = drag;
  if (d.type === 'pan') {
    S.view.x = d.vx - (x - d.x) / S.view.ppt;
    S.view.y = d.vy - (y - d.y);
    viewChanged();
  } else if (d.type === 'seek') {
    setPlayhead(Math.max(0, snapTick(t)));
  } else if (d.type === 'loop') {
    const b = Math.max(0, snapTick(t));
    S.loop = { on: true, a: Math.min(d.a, b), b: Math.max(d.a, b) };
    emit('loop');
    invalidate();
  } else if (d.type === 'move' || d.type === 'resize') {
    if (!d.moved && Math.hypot(x - d.x0, y - d.y0) < 3) return;
    d.moved = true;
    const sel = notes().filter((n) => d.ids.includes(n.id)), a = d.anchor, minLen = Math.min(S.snap || 15, 60);
    if (d.type === 'move') {
      d.dt = Math.max(snapTick(a.start + t - d.t0) - a.start, -Math.min(...sel.map((n) => n.start)));
      d.dk = Math.max(-Math.min(...sel.map((n) => n.key)), Math.min(127 - Math.max(...sel.map((n) => n.key)), k - d.k0));
    } else if (d.edge === 'end') {
      const end = snapTick(a.start + a.length + t - d.t0);
      d.d = Math.max(end - a.start - a.length, minLen - Math.min(...sel.map((n) => n.length)));
    } else {
      const start = snapTick(a.start + t - d.t0);
      d.d = Math.max(Math.min(a.start - start, Math.min(...sel.map((n) => n.start))),
        minLen - Math.min(...sel.map((n) => n.length)));
    }
    dirty = true;
  } else if (d.type === 'draw') {
    d.end = Math.max(d.start + Math.min(S.snap || 15, 60), snapTick(t, 'ceil'));
  } else if (d.type === 'band') {
    d.x1 = x;
    d.y1 = y;
    const ta = xt(Math.min(d.x0, x)), tb = xt(Math.max(d.x0, x)), ka = yk(Math.max(d.y0, y)), kb = yk(Math.min(d.y0, y));
    const hits = notes().filter((n) => n.start < tb && n.start + n.length > ta && n.key >= ka && n.key <= kb).map((n) => n.id);
    S.sel = new Set([...d.base, ...hits]);
    emit('select');
  } else if (d.type === 'pen') {
    d.pts.push([t, yc(y)]);
  }
  invalidateOver();
}

async function up(e) {
  const d = drag;
  drag = null;
  if (base.hasPointerCapture(e.pointerId)) base.releasePointerCapture(e.pointerId);
  if (!d) return;
  invalidate();
  const tid = S.trackId;
  if (d.type === 'move' || d.type === 'resize') {
    if (!d.moved) {
      if (d.shift && d.wasSelected) { S.sel.delete(d.anchor.id); emit('select'); } else if (!d.shift) select([d.anchor.id]);
      return;
    }
    const op = d.type === 'move'
      ? (d.dt || d.dk) && { op: 'move_notes', track: tid, ids: d.ids, dticks: d.dt, dkeys: d.dk }
      : d.d && { op: 'resize_notes', track: tid, ids: d.ids, dlength: d.d, from_start: d.edge === 'start' };
    if (!op) return;
    pending = d;
    await sendOps([op], d.type === 'move' ? 'Move notes' : 'Resize notes');
    pending = null;
    invalidate();
  } else if (d.type === 'draw') {
    const ids = await sendOpsNew([{ op: 'add_notes', track: tid, notes: [{ start: d.start, length: d.end - d.start, key: d.key, lyric: defaultLyric() }] }], 'Add note');
    select(ids);
  } else if (d.type === 'pen') {
    const op = penOp(d);
    if (op) await sendOps([op], d.erase ? 'Clear pitch' : 'Draw pitch');
  }
}

function penOp(d) {
  const pts = [...d.pts].sort((a, b) => a[0] - b[0]);
  const a = Math.max(0, Math.floor(pts[0][0])), b = Math.ceil(pts.at(-1)[0]);
  if (b - a < 5) return null;
  if (d.erase) return { op: 'clear_curve', track: S.trackId, param: 'pitd', range: [a, b] };
  const pitd = track().curves?.pitd, out = [];
  const lim = S.params.pitd;
  let j = 0;
  for (let t = a; t <= b; t += 10) {
    while (j < pts.length - 2 && pts[j + 1][0] < t) j++;
    const [t0, c0] = pts[j], [t1, c1] = pts[Math.min(j + 1, pts.length - 1)];
    const target = t1 > t0 ? c0 + ((c1 - c0) * (t - t0)) / (t1 - t0) : c0;
    const fin = pitchAt(t);
    if (fin == null) continue;
    const v = target - (fin - curveAt(pitd, t));
    out.push([t, Math.round(Math.max(lim.min, Math.min(lim.max, v)))]);
  }
  if (!out.length) return null;
  // Anchor both ends to the existing curve so the stroke doesn't leak past its range.
  const lo = Math.max(0, a - 1), hi = b + 1;
  if (lo < a) out.unshift([lo, curveAt(pitd, lo)]);
  out.push([hi, curveAt(pitd, hi)]);
  return { op: 'set_curve', track: S.trackId, param: 'pitd', range: [lo, hi], points: out };
}

function defaultLyric() {
  const ph = track()?.phonemizer || 'auto';
  return ph.startsWith('en') ? 'la' : ph === 'raw' ? 'a' : 'あ';
}

function wheel(e) {
  e.preventDefault();
  const { x, y } = pos(e);
  if (e.ctrlKey || e.metaKey) return zoomX(Math.exp(-e.deltaY * 0.004), Math.max(KEYS_W, x));
  if (e.altKey) return zoomY(Math.exp(-e.deltaY * 0.003), Math.max(RULER_H, y));
  let dx = e.deltaX, dy = e.deltaY;
  if (e.shiftKey && !dx) { dx = dy; dy = 0; }
  if (y < RULER_H) { dx = dx || dy; dy = 0; }
  scrollBy(dx / S.view.ppt, dy);
}

export function setPlayhead(t) {
  S.playhead = t;
  emit('playhead');
  emit('seek');
}

// ---- lyric editing -----------------------------------------------------------------------------
const lyricInput = h('input', { class: 'lyric-input', spellcheck: 'false' });
let editingId = null;

function dblclick(e) {
  const { x, y } = pos(e);
  if (y < RULER_H || S.tool === 'knife' || S.tool === 'pen') return;
  const hit = hitNote(x, y);
  if (hit) editLyric(hit.note.id);
}

export function editLyric(id) {
  const n = notes().find((m) => m.id === id);
  if (!n) return;
  scrollToNote(n);
  drawBase();
  editingId = id;
  select([id]);
  const x = tx(n.start), w = Math.max(64, (n.length * S.view.ppt));
  Object.assign(lyricInput.style, {
    display: 'block', left: `${x}px`, top: `${ky(n.key) - Math.max(0, 22 - S.view.keyH) / 2}px`,
    width: `${w}px`, height: `${Math.max(22, S.view.keyH)}px`,
  });
  lyricInput.value = n.lyric;
  lyricInput.focus();
  lyricInput.select();
}

function commitLyric() {
  const id = editingId, value = lyricInput.value.trim();
  editingId = null;
  lyricInput.style.display = 'none';
  const ns = notes(), i = ns.findIndex((n) => n.id === id);
  if (i < 0 || !value || value === ns[i].lyric) return;
  const words = value.split(/\s+/);
  if (words.length === 1) {
    sendOps([{ op: 'update_notes', track: S.trackId, ids: [id], patch: { lyric: value } }], 'Edit lyric');
  } else {
    fillLyrics(words, id);
  }
}

/** Fills consecutive notes (skipping "+" extensions) starting at note `fromId`. */
export function fillLyrics(words, fromId) {
  const ns = notes(), i = ns.findIndex((n) => n.id === fromId);
  const ids = ns.slice(i).filter((n) => n.lyric !== '+').slice(0, words.length).map((n) => n.id);
  sendOps([{ op: 'set_lyrics', track: S.trackId, ids, lyrics: words.slice(0, ids.length) }], 'Set lyrics');
}

lyricInput.addEventListener('keydown', (e) => {
  e.stopPropagation();
  if (e.key === 'Enter') { e.preventDefault(); lyricInput.blur(); }
  else if (e.key === 'Escape') { editingId = null; lyricInput.style.display = 'none'; }
  else if (e.key === 'Tab') {
    e.preventDefault();
    const ns = notes(), i = ns.findIndex((n) => n.id === editingId), next = ns[i + (e.shiftKey ? -1 : 1)];
    lyricInput.blur();
    if (next) editLyric(next.id);
  }
});
lyricInput.addEventListener('blur', () => { if (editingId !== null) commitLyric(); });
