// Transport bar and playback: renders via the API and plays the WAV with Web Audio, A/B with the reference.
import * as api from './api.js';
import { S, on, emit, notify, sendOps, tickToMs, msToTick, fmtTime, fmtBarBeat, pcName, SCALES } from './state.js';
import { setPlayhead } from './pianoroll.js';
import { h, icon, dragValue, selectBox, fixed } from './ui.js';

let actx = null, cache = { rev: -1, buf: null }, ref = { path: null, buf: null };
let play = null;   // { srcs, gains, t0, pos, loop, from }
let playBtn, timeEl, barEl, bpm, num, den, tonic, scale, abBtns;

export function initTransport(root) {
  playBtn = h('button', { class: 'icon-btn', title: 'Play / pause (Space)', onclick: togglePlay }, icon('play'));
  const loopBtn = h('button', { class: 'icon-btn', title: 'Loop (L)', onclick: toggleLoop }, icon('loop'));
  timeEl = h('span', { class: 'timecode num', title: 'Time' });
  barEl = h('span', { class: 'barbeat num', title: 'Bar.beat' });
  bpm = dragValue({ min: 20, max: 300, step: 0.1, digits: 1, title: 'Tempo (BPM) at the first beat',
    onCommit: (v) => sendOps([{ op: 'set_tempo', tick: 0, bpm: v }], 'Set tempo') });
  num = dragValue({ min: 1, max: 32, step: 0.1, title: 'Beats per bar',
    onCommit: (v) => setMeter(Math.round(v), Number(den.value)) });
  den = selectBox([2, 4, 8, 16].map((d) => [d, d]), 4, (v) => setMeter(meter().num, Number(v)), 'den');
  tonic = selectBox([['', 'No key'], ...Array.from({ length: 12 }, (_, i) => [i, pcName(i)])], '', setKey, 'tonic');
  scale = selectBox(Object.keys(SCALES).map((s) => [s, s.replace('_', ' ')]), 'major', setKey, 'scale');
  abBtns = ['a', 'b', 'ab'].map((m) => h('button', {
    class: 'seg', title: { a: 'Hear the synthesized vocal', b: 'Hear the reference audio', ab: 'Hear both' }[m],
    onclick: () => setAB(m),
  }, { a: 'Synth', b: 'Ref', ab: 'Both' }[m]));
  root.append(
    h('div', { class: 'tgroup' },
      h('button', { class: 'icon-btn', title: 'Return to start (Home)', onclick: () => { stop(); setPlayhead(S.loop.on ? S.loop.a : 0); } }, icon('ret')),
      playBtn,
      h('button', { class: 'icon-btn', title: 'Stop (returns to where playback started)', onclick: stop }, icon('stop')),
      loopBtn),
    h('div', { class: 'tgroup readouts' }, timeEl, barEl),
    h('div', { class: 'tgroup' }, h('span', { class: 'tlabel' }, 'BPM'), bpm),
    h('div', { class: 'tgroup' }, num, h('span', { class: 'slash' }, '/'), den),
    h('div', { class: 'tgroup' }, tonic, scale),
    h('div', { class: 'tgroup segs', title: 'A/B against the reference' }, ...abBtns));
  on('project', syncProject);
  on('playhead', syncTime);
  on('seek', () => { if (play) restart(); });
  on('loop', () => { loopBtn.classList.toggle('on', S.loop.on); if (play) restart(); });
  syncAB();
}

const meter = () => S.project?.time_sig?.[0] || { num: 4, den: 4 };
const setMeter = (n, d) => sendOps([{ op: 'set_time_sig', bar: 0, num: n, den: d }], 'Set time signature');
function setKey() {
  const t = tonic.value;
  sendOps([{ op: 'set_key', key: t === '' ? null : { tonic: Number(t), scale: scale.value } }], 'Set key');
}

function syncProject() {
  const p = S.project;
  bpm.set(p.tempo?.[0]?.bpm ?? 120);
  num.set(meter().num);
  den.value = meter().den;
  tonic.value = p.key ? p.key.tonic : '';
  scale.value = p.key?.scale || 'major';
  scale.disabled = !p.key;
  abBtns.forEach((b) => { b.disabled = !p.reference && b !== abBtns[0]; });
  if (!p.reference && S.ab !== 'a') { S.ab = 'a'; syncAB(); }
  applyAB();
  syncTime();
}

function syncTime() {
  timeEl.textContent = fmtTime(tickToMs(S.playhead));
  barEl.textContent = fmtBarBeat(S.playhead);
}
const AB_NAMES = { a: 'synth', b: 'reference', ab: 'synth + reference' };
/** Chooses what playback sounds: 'a' synth, 'b' reference, 'ab' both. Works while playing. */
export function setAB(m) {
  if (m !== 'a' && !S.project?.reference) { notify('No reference audio in this project', 'warn'); return; }
  S.ab = m;
  applyAB();
  syncAB();
  notify(`Hearing ${AB_NAMES[m]}`);
}
/** A key: flip synth ⇄ reference; shift+A: both. */
export const toggleAB = (both = false) => setAB(both ? (S.ab === 'ab' ? 'a' : 'ab') : S.ab === 'a' ? 'b' : 'a');

function syncAB() { abBtns.forEach((b, i) => b.classList.toggle('on', ['a', 'b', 'ab'][i] === S.ab)); }

export function toggleLoop() {
  if (!S.loop.on && S.sel.size) {
    const ns = S.project.tracks.flatMap((t) => t.notes).filter((n) => S.sel.has(n.id));
    S.loop.a = Math.min(...ns.map((n) => n.start));
    S.loop.b = Math.max(...ns.map((n) => n.start + n.length));
  }
  S.loop.on = !S.loop.on;
  emit('loop');
}

// ---- rendering ----------------------------------------------------------------------------
function ctx() {
  actx ||= new AudioContext();
  return actx;
}

async function ensureRender(force = false) {
  if (!force && cache.rev === S.revision && cache.buf) return cache.buf;
  const rev = S.revision, t0 = performance.now();
  S.progress = 'Rendering…';
  emit('status');
  try {
    const r = await api.post('api/render', {});
    S.report = { ...r.report, wall_ms: Math.round(performance.now() - t0) };
    S.reportRev = r.revision ?? rev;
    emit('report');
    const wav = await api.getBinary(`api/render/${encodeURIComponent(r.render_id)}.wav`);
    cache = { rev, buf: await ctx().decodeAudioData(wav) };
    return cache.buf;
  } catch (e) {
    notify(`Render failed: ${e.message}`, 'error');
    return null;
  } finally {
    S.progress = null;
    emit('status');
  }
}

export const renderNow = () => ensureRender(true);

async function ensureRef() {
  const r = S.project.reference;
  if (!r) return null;
  if (ref.path === r.path) return ref.buf;
  ref = { path: r.path, buf: null };
  try {
    ref.buf = await ctx().decodeAudioData(await api.getBinary('api/reference/audio'));
  } catch (e) {
    notify(`Reference audio unavailable: ${e.message}`, 'warn');
  }
  return ref.buf;
}

// ---- playback ---------------------------------------------------------------------------------
export async function togglePlay() {
  if (play) return pause();
  await ctx().resume();
  const buf = await ensureRender();
  if (!buf) return;
  start(buf, await ensureRef(), S.playhead);
}

function start(buf, refBuf, fromTick) {
  const c = ctx();
  const loop = S.loop.on && S.loop.b > S.loop.a ? [tickToMs(S.loop.a) / 1000, tickToMs(S.loop.b) / 1000] : null;
  let pos = tickToMs(fromTick) / 1000;
  if (loop && (pos < loop[0] || pos >= loop[1])) pos = loop[0];
  const gains = { a: c.createGain(), b: c.createGain() };
  gains.a.connect(c.destination);
  gains.b.connect(c.destination);
  const now = c.currentTime + 0.02, srcs = [];
  const source = (b, gain, offset) => {
    const s = c.createBufferSource();
    s.buffer = b;
    if (loop) { s.loop = true; s.loopStart = Math.max(0, loop[0] + offset); s.loopEnd = loop[1] + offset; }
    s.connect(gain);
    const at = pos + offset;
    if (at >= 0) s.start(now, at); else s.start(now - at, 0);
    srcs.push(s);
    return s;
  };
  const main = source(buf, gains.a, 0);
  if (refBuf) source(refBuf, gains.b, (S.project.reference.offset_ms || 0) / 1000);
  main.onended = () => { if (play?.srcs[0] === main) finish(); };
  play = { srcs, gains, t0: now, pos, loop, from: play?.from ?? fromTick, buf, refBuf };
  applyAB();
  S.playing = true;
  playBtn.replaceChildren(icon('pause'));
  requestAnimationFrame(tick);
}

function tick() {
  if (!play) return;
  let s = play.pos + Math.max(0, actx.currentTime - play.t0);
  const L = play.loop;
  if (L && s >= L[1]) s = L[0] + ((s - L[0]) % (L[1] - L[0]));
  S.playhead = Math.max(0, msToTick(s * 1000));
  emit('playhead');
  requestAnimationFrame(tick);
}

function halt() {
  if (!play) return;
  for (const s of play.srcs) { s.onended = null; try { s.stop(); } catch { /* not started */ } }
  play.gains.a.disconnect();
  play.gains.b.disconnect();
  play = null;
  S.playing = false;
  playBtn.replaceChildren(icon('play'));
}

function restart() {
  const { buf, refBuf, from } = play;
  halt();
  play = { from };
  start(buf, refBuf ?? ref.buf, S.playhead);
}

function pause() { halt(); emit('playhead'); }
function finish() { halt(); emit('playhead'); }

export function stop() {
  const from = play?.from;
  halt();
  setPlayhead(from ?? 0);
}

function applyAB() {
  if (!play?.gains) return;
  const r = S.project.reference;
  play.gains.a.gain.value = S.ab === 'b' ? 0 : 1;
  play.gains.b.gain.value = S.ab === 'a' || !r || r.muted ? 0 : 10 ** ((r.gain_db || 0) / 20);
}

export function fmtReport(r) {
  if (!r) return '';
  const parts = [];
  if (r.pitch_error) parts.push(`p95 ${fixed(r.pitch_error.p95, 0)}¢`);
  if (r.ref_pitch_error) parts.push(`ref p95 ${fixed(r.ref_pitch_error.p95, 0)}¢`);
  if (Number.isFinite(r.lufs)) parts.push(`${fixed(r.lufs, 1)} LUFS`);
  const miss = Array.isArray(r.missing_aliases) ? r.missing_aliases.length : r.missing_aliases;
  if (miss != null) parts.push(`${miss} missing`);
  if (r.clipped) parts.push(`${r.clipped} clipped`);
  const t = r.render_ms ?? r.wall_ms;
  if (t != null) parts.push(`${fixed(t / 1000, 2)} s`);
  return parts.join(' · ');
}
