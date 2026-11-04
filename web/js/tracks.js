// Track list: singer, phonemizer, mix; plus the reference-audio row.
import { S, on, sendOps, setTrack } from './state.js';
import { imageUrl } from './api.js';
import { h, icon, dragValue, textField, selectBox, toggleBtn, fixed } from './ui.js';

const PHONEMIZERS = [['auto', 'Auto'], ['jp_cv', 'JP CV'], ['jp_vcv', 'JP VCV'], ['jp_cvvc', 'JP CVVC'],
  ['en_arpasing', 'EN Arpasing'], ['raw', 'Raw aliases']];

let list, stale = false;

export function initTracks(root) {
  list = h('div', { class: 'scroll track-list' });
  root.append(
    h('div', { class: 'panel-head' }, h('span', {}, 'Tracks'),
      h('button', { class: 'icon-btn', title: 'Add track', onclick: addTrack }, icon('plus'))),
    list);
  on('project voicebanks', render);
  on('track', () => list.querySelectorAll('.track[data-id]')
    .forEach((r) => r.classList.toggle('active', r.dataset.id === String(S.trackId))));
  root.addEventListener('focusout', () => setTimeout(() => { if (stale && !editing()) render(); }));
}

const editing = () => list.contains(document.activeElement) && document.activeElement.matches('input');

const updateTrack = (t, patch, label) => sendOps([{ op: 'update_track', track: t.id, patch }], label);

function addTrack() {
  const n = S.project.tracks.length + 1;
  const singer = S.project.tracks.at(-1)?.singer ?? S.voicebanks[0]?.id ?? '';
  sendOps([{ op: 'add_track', track: { name: `Track ${n}`, singer, phonemizer: 'auto' } }], 'Add track');
}

function render() {
  if (!S.project) return;
  if (editing()) { stale = true; return; }
  stale = false;
  const rows = S.project.tracks.map(trackRow);
  if (S.project.reference) rows.push(referenceRow(S.project.reference));
  list.replaceChildren(...rows);
}

function avatar(singer) {
  const vb = S.voicebanks.find((v) => v.id === singer);
  const fallback = h('div', { class: 'avatar blank' }, (vb?.name || singer || '?').slice(0, 1).toUpperCase());
  if (!singer || vb?.image === false) return fallback;
  const img = h('img', { class: 'avatar', src: imageUrl(singer), alt: '' });
  img.addEventListener('error', () => img.replaceWith(fallback));
  return img;
}

const fmtPan = (p) => (Math.abs(p) < 0.005 ? 'C' : `${p < 0 ? 'L' : 'R'}${Math.round(Math.abs(p) * 100)}`);

function trackRow(t) {
  const name = textField({ onCommit: (v) => v.trim() && updateTrack(t, { name: v.trim() }, 'Rename track') });
  name.set(t.name);
  name.classList.add('tname');
  const mute = toggleBtn('M', 'Mute', () => updateTrack(t, { mix: { mute: !t.mix.mute } }, 'Mute'));
  const solo = toggleBtn('S', 'Solo', () => updateTrack(t, { mix: { solo: !t.mix.solo } }, 'Solo'));
  mute.set(t.mix.mute);
  solo.set(t.mix.solo);
  solo.classList.add('solo');
  const banks = S.voicebanks.map((v) => [v.id, v.name]);
  if (t.singer && !banks.some(([id]) => id === t.singer)) banks.push([t.singer, `${t.singer} (missing)`]);
  const singer = selectBox([['', 'No singer'], ...banks], t.singer, (v) => updateTrack(t, { singer: v }, 'Change singer'), 'grow');
  const phon = selectBox(PHONEMIZERS, t.phonemizer, (v) => updateTrack(t, { phonemizer: v }, 'Change phonemizer'), 'grow');
  const gain = dragValue({ value: t.mix.gain_db, min: -60, max: 12, step: 0.1, digits: 1, unit: ' dB', reset: 0,
    title: 'Gain', onCommit: (v) => updateTrack(t, { mix: { gain_db: v } }, 'Track gain') });
  const pan = dragValue({ class: 'pan', value: t.mix.pan, min: -1, max: 1, step: 0.005, digits: 2, fmt: fmtPan, reset: 0,
    title: 'Pan', onCommit: (v) => updateTrack(t, { mix: { pan: v } }, 'Track pan') });
  const remove = h('button', { class: 'icon-btn ghost', title: 'Remove track',
    onclick: (e) => { e.stopPropagation(); sendOps([{ op: 'remove_track', track: t.id }], 'Remove track'); } }, icon('x'));
  const row = h('div', { class: `track${t.id === S.trackId ? ' active' : ''}`, 'data-id': String(t.id) },
    avatar(t.singer),
    h('div', { class: 'tmain' },
      h('div', { class: 'trow' }, name, mute, solo, remove),
      h('div', { class: 'trow' }, singer),
      h('div', { class: 'trow' }, phon, gain, pan)));
  row.addEventListener('pointerdown', () => setTrack(t.id));
  return row;
}

function referenceRow(ref) {
  const set = (patch, label) => sendOps([{ op: 'set_reference', reference: { ...ref, ...patch } }], label);
  const mute = toggleBtn('M', 'Mute reference', () => set({ muted: !ref.muted }, 'Mute reference'));
  mute.set(ref.muted);
  const file = ref.path.split('/').pop();
  return h('div', { class: 'track reference' },
    h('div', { class: 'avatar ref' }, h('span', {}, '~')),
    h('div', { class: 'tmain' },
      h('div', { class: 'trow' }, h('span', { class: 'tname static' }, 'Reference'), mute),
      h('div', { class: 'trow' }, h('span', { class: 'file', title: ref.path }, file)),
      h('div', { class: 'trow' },
        dragValue({ value: ref.gain_db, min: -60, max: 12, step: 0.1, digits: 1, unit: ' dB', reset: 0, title: 'Reference gain',
          onCommit: (v) => set({ gain_db: v }, 'Reference gain') }),
        dragValue({ value: ref.offset_ms, min: -60000, max: 60000, step: 1, fmt: (v) => `${v >= 0 ? '+' : ''}${fixed(v, 0)} ms`,
          reset: 0, title: 'Offset: audio time of tick 0', onCommit: (v) => set({ offset_ms: v }, 'Reference offset') }))));
}
