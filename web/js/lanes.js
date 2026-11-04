// Expression lane under the piano roll: track curves (set_curve) and per-note vel/vol (update_notes).
import { S, on, track, notes, sendOps, CURVE_PARAMS, curveAt, barOf, barTick, PPQ } from './state.js';
import { KEYS_W, tx, xt, zoomX, scrollBy } from './pianoroll.js';
import { h, fixed } from './ui.js';

const TABS = [...CURVE_PARAMS, 'vel', 'vol'];
const PAD = 8;
let param = 'dyn', cv, ctx, W = 0, H = 0, dpr = 1, dirty = true, drag = null, hoverX = null;
let readout;
const C = {};

export function initLanes(tabsEl, wrapEl, splitter) {
  cv = wrapEl.querySelector('canvas');
  ctx = cv.getContext('2d');
  readout = h('span', { class: 'lane-readout num' });
  const buttons = TABS.map((p) => h('button', { class: 'tab', title: S.params[p]?.description, onclick: () => setParam(p) }, p));
  tabsEl.append(...buttons, h('span', { class: 'spacer' }), readout,
    h('span', { class: 'hint' }, 'drag to draw · alt-drag to reset'));
  const sync = () => buttons.forEach((b, i) => b.classList.toggle('on', TABS[i] === param));
  setParam.sync = sync;
  sync();
  new ResizeObserver(() => {
    dpr = devicePixelRatio || 1;
    W = wrapEl.clientWidth;
    H = wrapEl.clientHeight;
    cv.width = Math.round(W * dpr);
    cv.height = Math.round(H * dpr);
    cv.style.width = W + 'px';
    cv.style.height = H + 'px';
    dirty = true;
  }).observe(wrapEl);
  readColors();
  on('project track derived select view playhead loop', () => { dirty = true; });
  on('theme', () => { readColors(); dirty = true; });
  cv.addEventListener('pointerdown', down);
  cv.addEventListener('pointermove', move);
  cv.addEventListener('pointerup', up);
  cv.addEventListener('pointercancel', up);
  cv.addEventListener('pointerleave', () => { hoverX = null; dirty = true; readout.textContent = ''; });
  cv.addEventListener('contextmenu', (e) => e.preventDefault());
  cv.addEventListener('wheel', (e) => {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) return zoomX(Math.exp(-e.deltaY * 0.004), Math.max(KEYS_W, e.offsetX));
    scrollBy((e.deltaX || e.deltaY) / S.view.ppt);
  }, { passive: false });
  initSplitter(splitter, wrapEl);
  requestAnimationFrame(function loop() {
    if (dirty) { dirty = false; draw(); }
    requestAnimationFrame(loop);
  });
}

function setParam(p) {
  param = p;
  setParam.sync?.();
  dirty = true;
}

function initSplitter(el, wrapEl) {
  try { const v = localStorage.getItem('cantor.laneH'); if (v) wrapEl.style.height = v; } catch { /* storage blocked */ }
  el.addEventListener('pointerdown', (e) => {
    el.setPointerCapture(e.pointerId);
    const y0 = e.clientY, h0 = wrapEl.clientHeight;
    const mv = (ev) => { wrapEl.style.height = `${Math.max(48, Math.min(420, h0 - (ev.clientY - y0)))}px`; };
    const done = () => {
      el.removeEventListener('pointermove', mv);
      el.removeEventListener('pointerup', done);
      try { localStorage.setItem('cantor.laneH', wrapEl.style.height); } catch { /* storage blocked */ }
    };
    el.addEventListener('pointermove', mv);
    el.addEventListener('pointerup', done);
  });
}

function readColors() {
  const cs = getComputedStyle(document.documentElement);
  for (const v of ['roll-bg', 'ruler-bg', 'grid-bar', 'grid-beat', 'line', 'text', 'text-dim', 'text-faint', 'accent',
    'accent-soft', 'curve', 'curve-fill', 'note', 'note-edge', 'danger']) C[v] = cs.getPropertyValue('--' + v).trim();
}

const range = () => S.params[param];
const isNote = () => param === 'vel' || param === 'vol';
const vy = (v) => { const r = range(); return PAD + (1 - (v - r.min) / (r.max - r.min)) * (H - 2 * PAD); };
const yv = (y) => { const r = range(); return Math.max(r.min, Math.min(r.max, r.min + (1 - (y - PAD) / (H - 2 * PAD)) * (r.max - r.min))); };
const digitsFor = () => (range().max - range().min > 50 ? 0 : 1);
const fmtV = (v) => `${fixed(v, digitsFor())} ${range().unit || ''}`.trim();

// ---- drawing ----------------------------------------------------------------------------------
function draw() {
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = C['roll-bg'];
  ctx.fillRect(0, 0, W, H);
  const tr = track();
  if (!tr || !W) return;
  const r = range(), t0 = xt(KEYS_W), t1 = xt(W), font = getComputedStyle(document.body).fontFamily;

  for (let b = barOf(Math.max(0, t0)).bar; barTick(b) < t1; b++) {
    ctx.fillStyle = C['grid-bar'];
    ctx.fillRect(Math.round(tx(barTick(b))), 0, 1, H);
  }
  ctx.fillStyle = C['grid-beat'];
  for (const y of [vy(r.max), vy(r.min)]) ctx.fillRect(KEYS_W, Math.round(y), W, 1);
  ctx.fillStyle = C['text-faint'];
  ctx.fillRect(KEYS_W, Math.round(vy(r.default)), W, 1);

  for (const n of tr.notes) {
    if (n.start + n.length < t0 || n.start > t1) continue;
    ctx.fillStyle = S.sel.has(n.id) ? C.accent : C['note-edge'];
    ctx.globalAlpha = 0.6;
    ctx.fillRect(Math.round(tx(n.start)), H - 3, Math.max(1, Math.round(n.length * S.view.ppt) - 1), 3);
    ctx.globalAlpha = 1;
  }

  if (isNote()) drawNoteBars(tr, t0, t1); else drawCurve(tr, t0, t1);

  const px = Math.round(tx(S.playhead));
  if (px >= KEYS_W) { ctx.fillStyle = C.accent; ctx.fillRect(px, 0, 1, H); }
  if (hoverX !== null && hoverX > KEYS_W && !drag) {
    ctx.fillStyle = C['text-faint'];
    ctx.fillRect(Math.round(hoverX), 0, 1, H);
  }

  // gutter
  ctx.fillStyle = C['ruler-bg'];
  ctx.fillRect(0, 0, KEYS_W, H);
  ctx.fillStyle = C.line;
  ctx.fillRect(KEYS_W - 1, 0, 1, H);
  ctx.font = `10px ${font}`;
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = C['text-dim'];
  ctx.fillText(String(r.max), KEYS_W - 6, Math.max(7, vy(r.max)));
  ctx.fillText(String(r.min), KEYS_W - 6, Math.min(H - 7, vy(r.min)));
  if (r.default !== r.min && r.default !== r.max && H > 60) ctx.fillText(String(r.default), KEYS_W - 6, vy(r.default));
  ctx.textAlign = 'left';
}

function drawCurve(tr, t0, t1) {
  const r = range(), curve = tr.curves?.[param], y0 = vy(r.default);
  const pts = curve?.points || [];
  const path = new Path2D();
  if (pts.length) {
    path.moveTo(KEYS_W, vy(curveAt(curve, t0, r.default)));
    const step = 2 / S.view.ppt;
    for (let t = t0; t <= t1 + step; t += step) path.lineTo(tx(t), vy(curveAt(curve, t, r.default)));
    const fill = new Path2D(path);
    fill.lineTo(W, y0);
    fill.lineTo(KEYS_W, y0);
    ctx.fillStyle = C['curve-fill'];
    ctx.fill(fill);
    ctx.strokeStyle = C.curve;
    ctx.lineWidth = 1.25;
    ctx.stroke(path);
    ctx.lineWidth = 1;
    if (S.view.ppt * PPQ > 40) {
      ctx.fillStyle = C.curve;
      for (const [t, v] of pts) if (t >= t0 && t <= t1 && pts.length < 400) ctx.fillRect(tx(t) - 1.5, vy(v) - 1.5, 3, 3);
    }
  }
  if (drag?.pts?.length > 1) {
    ctx.strokeStyle = drag.erase ? C.danger : C.accent;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    drag.pts.forEach(([t, v], i) => (i ? ctx.lineTo(tx(t), vy(v)) : ctx.moveTo(tx(t), vy(v))));
    ctx.stroke();
    ctx.lineWidth = 1;
  }
}

function drawNoteBars(tr, t0, t1) {
  const r = range();
  for (const n of tr.notes) {
    if (n.start + n.length < t0 || n.start > t1) continue;
    const v = drag?.values?.get(n.id) ?? n[param];
    const x0 = Math.round(tx(n.start)), x1 = Math.round(tx(n.start + n.length)), y = Math.round(vy(v));
    const sel = S.sel.has(n.id) || drag?.values?.has(n.id);
    ctx.fillStyle = sel ? C['accent-soft'] : C['curve-fill'];
    ctx.fillRect(x0, y, Math.max(1, x1 - x0 - 1), Math.round(vy(r.min)) - y);
    ctx.fillStyle = sel ? C.accent : C.curve;
    ctx.fillRect(x0, y, Math.max(1, x1 - x0 - 1), 1);
    ctx.fillRect(x0, y, 2, Math.round(vy(r.min)) - y);
  }
}

// ---- interaction -------------------------------------------------------------------------------
const pt = (e) => { const b = cv.getBoundingClientRect(); return [e.clientX - b.left, e.clientY - b.top]; };

function down(e) {
  const [x, y] = pt(e);
  if (x < KEYS_W || !track()) return;
  cv.setPointerCapture(e.pointerId);
  const erase = e.altKey || e.button === 2;
  drag = isNote() ? { values: new Map(), lastX: x, erase } : { pts: [[xt(x), erase ? range().default : yv(y)]], erase };
  move(e);
}

function move(e) {
  const [x, y] = pt(e);
  hoverX = x;
  if (x > KEYS_W) {
    const tr = track(), t = xt(x);
    const v = isNote() ? notes().find((n) => t >= n.start && t < n.start + n.length)?.[param] : curveAt(tr?.curves?.[param], t, range().default);
    readout.textContent = `${param} ${v == null ? '—' : fmtV(drag && !isNote() ? yv(y) : v)}`;
  }
  dirty = true;
  if (!drag) return;
  if (isNote()) {
    const a = xt(Math.min(drag.lastX, x)), b = xt(Math.max(drag.lastX, x)), v = drag.erase ? range().default : Math.round(yv(y));
    for (const n of notes()) {
      if (S.sel.size && !S.sel.has(n.id)) continue;
      if (n.start <= b && n.start + n.length >= a) drag.values.set(n.id, v);
    }
    drag.lastX = x;
  } else {
    drag.pts.push([xt(x), drag.erase ? range().default : yv(y)]);
  }
}

function up() {
  const d = drag;
  drag = null;
  dirty = true;
  if (!d) return;
  const tid = S.trackId;
  if (isNote()) {
    const byValue = new Map();
    for (const [id, v] of d.values) {
      if (notes().find((n) => n.id === id)?.[param] === v) continue;
      if (!byValue.has(v)) byValue.set(v, []);
      byValue.get(v).push(id);
    }
    const ops = [...byValue].map(([v, ids]) => ({ op: 'update_notes', track: tid, ids, patch: { [param]: v } }));
    sendOps(ops, `Edit ${param}`);
    return;
  }
  const pts = [...d.pts].sort((p, q) => p[0] - q[0]);
  const a = Math.max(0, Math.floor(pts[0][0])), b = Math.ceil(pts.at(-1)[0]);
  if (b - a < 5) return;
  if (d.erase) {
    sendOps([{ op: 'clear_curve', track: tid, param, range: [a, b] }], `Reset ${param}`);
    return;
  }
  const step = Math.max(5, Math.round(3 / S.view.ppt)), out = [], dg = digitsFor() + 1;
  let j = 0;
  for (let t = a; t <= b; t += step) {
    while (j < pts.length - 2 && pts[j + 1][0] < t) j++;
    const [t0, v0] = pts[j], [t1, v1] = pts[Math.min(j + 1, pts.length - 1)];
    out.push([t, Number(fixed(t1 > t0 ? v0 + ((v1 - v0) * (t - t0)) / (t1 - t0) : v0, dg))]);
  }
  const curve = track().curves?.[param], lo = Math.max(0, a - 1), hi = b + 1;
  if (lo < a) out.unshift([lo, curveAt(curve, lo, range().default)]);
  out.push([hi, curveAt(curve, hi, range().default)]);
  sendOps([{ op: 'set_curve', track: tid, param, range: [lo, hi], points: out }], `Draw ${param}`);
}
