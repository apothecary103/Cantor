// Properties panel: edits the selected notes (update_notes with only changed fields) and runs tuner passes.
import { S, on, track, selected, sendOps, call, noteName } from './state.js';
import { h, dragValue, textField, selectBox } from './ui.js';

const get = (o, path) => path.split('.').reduce((a, k) => a?.[k], o);
const patchOf = (path, v) => path.split('.').reduceRight((acc, k) => ({ [k]: acc }), v);

const ms = { unit: ' ms', step: 1 }, pct = { unit: '%', step: 0.5, min: 0, max: 100 }, cents = { unit: '¢', step: 1 };
const SECTIONS = [
  { title: 'Note', fields: [
    { p: 'lyric', label: 'Lyric', text: true },
    { p: 'alias', label: 'Alias', text: true, placeholder: 'auto', nullable: true },
    { p: 'key', label: 'Key', min: 0, max: 127, step: 0.12, fmt: noteName },
    { p: 'vel', label: 'Velocity', ...pct, max: 200, reset: 100 },
    { p: 'vol', label: 'Intensity', ...pct, max: 200, reset: 100 },
    { p: 'mod', label: 'Modulation', ...pct, reset: 0 },
    { p: 'flags', label: 'Flags', text: true, placeholder: 'none' },
  ] },
  { title: 'Timing', fields: [
    { p: 'timing.preutter_ms', label: 'Preutter', ...ms, min: 0, max: 1000, nullText: 'auto', reset: null },
    { p: 'timing.overlap_ms', label: 'Overlap', ...ms, min: -500, max: 500, nullText: 'auto', reset: null },
    { p: 'timing.offset_ms', label: 'Offset', ...ms, min: -500, max: 500, reset: 0 },
  ] },
  { title: 'Portamento', fields: [
    { p: 'pitch.portamento.length_ms', label: 'Length', ...ms, min: 0, max: 1000, reset: 80 },
    { p: 'pitch.portamento.start_ms', label: 'Start', ...ms, min: -1000, max: 1000, reset: -40 },
    { p: 'pitch.portamento.overshoot_cents', label: 'Overshoot', ...cents, min: -400, max: 400, reset: 0 },
    { p: 'pitch.portamento.preparation_cents', label: 'Preparation', ...cents, min: -400, max: 400, reset: 0 },
    { p: 'pitch.portamento.shape', label: 'Shape', options: [['s', 'S-curve'], ['linear', 'Linear'], ['in', 'Ease in'], ['out', 'Ease out']] },
  ] },
  { title: 'Scoop', toggle: 'pitch.scoop', dflt: { cents: -60, length_ms: 60 }, fields: [
    { p: 'pitch.scoop.cents', label: 'Depth', ...cents, min: -1200, max: 1200, reset: -60 },
    { p: 'pitch.scoop.length_ms', label: 'Length', ...ms, min: 0, max: 1000, reset: 60 },
  ] },
  { title: 'Fall', toggle: 'pitch.fall', dflt: { cents: -120, length_ms: 100 }, fields: [
    { p: 'pitch.fall.cents', label: 'Depth', ...cents, min: -1200, max: 1200, reset: -120 },
    { p: 'pitch.fall.length_ms', label: 'Length', ...ms, min: 0, max: 1000, reset: 100 },
  ] },
  { title: 'Vibrato', toggle: 'pitch.vibrato',
    dflt: { length: 65, period_ms: 180, depth_cents: 20, fade_in: 30, fade_out: 20, phase: 0, shift: 0, drift: 0.2 }, fields: [
      { p: 'pitch.vibrato.length', label: 'Length', ...pct, reset: 65 },
      { p: 'pitch.vibrato.period_ms', label: 'Period', ...ms, min: 40, max: 1000, reset: 180 },
      { p: 'pitch.vibrato.depth_cents', label: 'Depth', ...cents, min: 0, max: 400, step: 0.5, reset: 20 },
      { p: 'pitch.vibrato.fade_in', label: 'Fade in', ...pct, reset: 30 },
      { p: 'pitch.vibrato.fade_out', label: 'Fade out', ...pct, reset: 20 },
      { p: 'pitch.vibrato.phase', label: 'Phase', min: 0, max: 1, step: 0.005, digits: 2, reset: 0 },
      { p: 'pitch.vibrato.shift', label: 'Shift', ...pct, min: -100, reset: 0 },
      { p: 'pitch.vibrato.drift', label: 'Drift', min: 0, max: 1, step: 0.005, digits: 2, reset: 0.2 },
    ] },
];

let root, setTitle, body, tuneBox, trackBox;
const bound = [];      // { f, el } for value refresh
const toggles = [];    // { s, box, fields }
const tuneOpts = { style: 'natural', strength: 0.8, smooth_ms: 40, cents: 8, ms: 10 };

export function initProps(el, title = () => {}) {
  root = el;
  setTitle = title;
  body = h('div', { class: 'props-body' });
  for (const s of SECTIONS) body.append(section(s));
  trackBox = trackSection();
  tuneBox = tuneSection();
  root.append(h('div', { class: 'scroll' }, trackBox, body, tuneBox));
  on('project select', refresh);
  refresh();
}

function send(patch, label) {
  const ids = selected().map((n) => n.id);
  if (ids.length) sendOps([{ op: 'update_notes', track: S.trackId, ids, patch }], label);
}

function control(f) {
  if (f.text) {
    return textField({ placeholder: f.placeholder,
      onCommit: (v) => send(patchOf(f.p, f.nullable && !v.trim() ? null : v.trim()), `Edit ${f.label.toLowerCase()}`) });
  }
  if (f.options) {
    const el = selectBox(f.options, '', (v) => send(patchOf(f.p, v), `Edit ${f.label.toLowerCase()}`));
    el.set = (v, mixed) => { el.value = mixed ? '' : v; };
    return el;
  }
  return dragValue({ ...f, title: f.reset !== undefined ? 'Drag, double-click to type, alt-click to reset' : undefined,
    onCommit: (v) => send(patchOf(f.p, f.p === 'key' ? Math.round(v) : v), `Edit ${f.label.toLowerCase()}`) });
}

function row(label, ctl) {
  return h('div', { class: 'prow' }, h('span', { class: 'plabel' }, label), ctl);
}

function section(s) {
  const fields = h('div', { class: 'psec-body' });
  for (const f of s.fields) {
    const el = control(f);
    bound.push({ f, el, s });
    fields.append(row(f.label, el));
  }
  const title = h('div', { class: 'psec-title' });
  if (s.toggle) {
    const box = h('input', { type: 'checkbox', class: 'check' });
    box.addEventListener('change', () => send(patchOf(s.toggle, box.checked ? { ...s.dflt } : null), `${box.checked ? 'Add' : 'Remove'} ${s.title.toLowerCase()}`));
    toggles.push({ s, box, fields });
    title.append(h('label', {}, box, h('span', {}, s.title)));
  } else title.append(h('span', {}, s.title));
  return h('div', { class: 'psec' }, title, fields);
}

function uniform(ns, path) {
  const vs = ns.map((n) => get(n, path));
  const same = vs.every((v) => JSON.stringify(v) === JSON.stringify(vs[0]));
  return { v: vs[0], mixed: !same };
}

function refresh() {
  const ns = selected(), tr = track();
  body.classList.toggle('hidden', !ns.length);
  trackBox.classList.toggle('hidden', !!ns.length || !tr);
  setTitle(ns.length ? `Note${ns.length > 1 ? 's' : ''} · ${ns.length}` : 'Properties');
  if (!ns.length) { trackBox.update?.(tr); updateTune(); return; }
  for (const t of toggles) {
    const on = ns.map((n) => get(n, t.s.toggle) != null);
    t.box.checked = on.every(Boolean);
    t.box.indeterminate = !t.box.checked && on.some(Boolean);
    t.fields.classList.toggle('off', !t.box.checked);
  }
  for (const { f, el } of bound) {
    const { v, mixed } = uniform(ns, f.p);
    el.set(v, mixed);
  }
  updateTune();
}

function trackSection() {
  const name = textField({ onCommit: (v) => v.trim() && sendOps([{ op: 'update_track', track: S.trackId, patch: { name: v.trim() } }], 'Rename track') });
  const flags = textField({ placeholder: 'none', onCommit: (v) => sendOps([{ op: 'update_track', track: S.trackId, patch: { flags: v.trim() } }], 'Track flags') });
  const box = h('div', { class: 'psec' }, h('div', { class: 'psec-title' }, h('span', {}, 'Track')),
    h('div', { class: 'psec-body' }, row('Name', name), row('Flags', flags)),
    h('p', { class: 'empty' }, 'Select notes to edit them. Click, drag a box, or press ⌘A.'));
  box.update = (tr) => { if (tr) { name.set(tr.name); flags.set(tr.flags); } };
  return box;
}

// ---- tuner -----------------------------------------------------------------------------------
let scopeEl, refBtn;

export function runTune(pass) {
  const tr = track();
  if (!tr) return;
  const ns = selected();
  const range = ns.length ? [Math.min(...ns.map((n) => n.start)), Math.max(...ns.map((n) => n.start + n.length))] : undefined;
  const o = tuneOpts;
  const params = {
    auto_pitch: { style: o.style }, auto_dynamics: {}, breaths: {},
    match_reference: { strength: o.strength, smooth_ms: o.smooth_ms }, humanize: { cents: o.cents, ms: o.ms, seed: 1 },
  }[pass];
  call('api/tune', { pass, track: tr.id, ...(range ? { range } : {}), params },
    (r) => `${pass.replace('_', ' ')}: ${r.ops_applied ?? 0} op${r.ops_applied === 1 ? '' : 's'} applied`);
}

function tuneSection() {
  const opt = (k, o) => dragValue({ ...o, value: tuneOpts[k], onCommit: (v) => { tuneOpts[k] = v; } });
  const run = (pass, label) => h('button', { class: 'btn small', onclick: () => runTune(pass) }, label);
  scopeEl = h('span', { class: 'scope' });
  refBtn = run('match_reference', 'Match');
  const styles = [['natural', 'Natural'], ['pop', 'Pop'], ['ballad', 'Ballad'], ['rock', 'Rock'], ['robotic', 'Robotic']];
  return h('div', { class: 'psec tune' },
    h('div', { class: 'psec-title' }, h('span', {}, 'Tune'), scopeEl),
    h('div', { class: 'psec-body' },
      h('div', { class: 'prow' }, h('span', { class: 'plabel' }, 'Auto pitch'),
        h('div', { class: 'pair' }, selectBox(styles, tuneOpts.style, (v) => { tuneOpts.style = v; }), run('auto_pitch', 'Run'))),
      h('div', { class: 'prow' }, h('span', { class: 'plabel' }, 'Dynamics'), h('div', { class: 'pair' }, h('span'), run('auto_dynamics', 'Run'))),
      h('div', { class: 'prow' }, h('span', { class: 'plabel' }, 'Breaths'), h('div', { class: 'pair' }, h('span'), run('breaths', 'Insert'))),
      h('div', { class: 'prow' }, h('span', { class: 'plabel' }, 'Reference'),
        h('div', { class: 'pair' }, opt('strength', { min: 0, max: 1, step: 0.005, digits: 2, title: 'Strength' }), refBtn)),
      h('div', { class: 'prow' }, h('span', { class: 'plabel' }, 'Ref. smooth'),
        h('div', { class: 'pair' }, opt('smooth_ms', { min: 0, max: 500, unit: ' ms', title: 'Smoothing' }), h('span'))),
      h('div', { class: 'prow' }, h('span', { class: 'plabel' }, 'Humanize'),
        h('div', { class: 'pair' }, h('div', { class: 'pair tight' }, opt('cents', { min: 0, max: 100, unit: '¢' }), opt('ms', { min: 0, max: 100, unit: ' ms' })),
          run('humanize', 'Run')))));
}

function updateTune() {
  const n = selected().length;
  scopeEl.textContent = n ? `${n} selected` : 'whole track';
  refBtn.disabled = !S.project?.reference;
  refBtn.title = S.project?.reference ? 'Write pitd so the pitch follows the reference vocal' : 'No reference audio in this project';
}

