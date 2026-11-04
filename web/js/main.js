// Entry point: wires panels, menus, toolbar, shortcuts, status bar and live sync.
import * as api from './api.js';
import { S, on, emit, adoptState, refresh, loadSchema, notify, call, fmtBarBeat, noteName, externalEdit, loadHistory, sourceName } from './state.js';
import { initRoll, fit, zoomX, invalidate, setPlayhead, editLyric } from './pianoroll.js';
import { initLanes } from './lanes.js';
import { initProps, runTune } from './props.js';
import { initTracks } from './tracks.js';
import { initTransport, togglePlay, stop, toggleLoop, renderNow, fmtReport, setAB, toggleAB } from './transport.js';
import { initHistory, saveCheckpoint } from './history.js';
import { initReport } from './report.js';
import { runAgentDemo } from './mock.js';
import { openConnect } from './connect.js';
import * as edit from './edit.js';
import { h, icon, menuBar, modal, promptText, selectBox, tabs, toggleBtn } from './ui.js';

const $ = (id) => document.getElementById(id);
const MAC = /Mac|iPhone|iPad/.test(navigator.platform);
const MOD = MAC ? '⌘' : 'Ctrl+';
let side;   // right panel tabs: properties, history, report

// ---- theme -------------------------------------------------------------------------------------
function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem('cantor.theme', theme); } catch { /* storage blocked */ }
  emit('theme');
}
const isLight = () => document.documentElement.dataset.theme === 'light';
try { const t = localStorage.getItem('cantor.theme'); if (t) document.documentElement.dataset.theme = t; } catch { /* storage blocked */ }

// ---- tools & toolbar -----------------------------------------------------------------------------
const TOOLS = [
  ['select', 'Select', 'select', '1'], ['draw', 'Draw notes', 'draw', '2'],
  ['knife', 'Knife (split)', 'knife', '3'], ['pen', 'Pitch pen (pitd; alt-drag erases)', 'pen', '4'],
];
const SNAPS = [[1920, '1/1'], [960, '1/2'], [480, '1/4'], [240, '1/8'], [120, '1/16'], [60, '1/32'], [30, '1/64'],
  [320, '1/4T'], [160, '1/8T'], [80, '1/16T'], [40, '1/32T'], [0, 'Off']];

function toggleShow(k) {
  S.show[k] = !S.show[k];
  emit('show');
  invalidate();
}

function setTool(t) {
  S.tool = t;
  emit('tool');
}

function initToolbar(root) {
  const btns = TOOLS.map(([id, label, ic, key]) =>
    h('button', { class: 'icon-btn tool', title: `${label} (${key})`, onclick: () => setTool(id) }, icon(ic)));
  on('tool', () => btns.forEach((b, i) => b.classList.toggle('on', TOOLS[i][0] === S.tool)));
  const snap = selectBox(SNAPS, S.snap, (v) => { S.snap = Number(v); invalidate(); }, 'snap');
  const view = [['pitch', 'Pitch', 'Final pitch curve'], ['ref', 'Ref F0', 'Reference vocal F0'],
    ['diff', 'Ref diff', 'Shade where the pitch line is more than 50¢ from the reference F0'], ['phonemes', 'Phonemes', 'Preutterance markers']]
    .map(([k, label, title]) => {
      const b = toggleBtn(label, title, () => toggleShow(k));
      b.set(S.show[k]);
      if (k === 'ref') b.classList.add('cool');
      on('show', () => b.set(S.show[k]));
      return b;
    });
  root.append(h('div', { class: 'tgroup' }, ...btns), h('span', { class: 'sep' }),
    h('label', { class: 'tgroup' }, h('span', { class: 'tlabel' }, 'Snap'), snap), h('span', { class: 'sep' }),
    h('div', { class: 'tgroup' }, ...view), h('span', { class: 'spacer' }),
    h('span', { class: 'cursor-info num', id: 'cursor-info' }));
  emit('tool');
}

// ---- status bar ------------------------------------------------------------------------------------
function initStatus(root) {
  const conn = h('span', { class: 'conn' }), rev = h('span', { class: 'num' }), file = h('span', { class: 'file' });
  const msg = h('span', { class: 'msg' }), sel = h('span', { class: 'num' }), report = h('span', { class: 'report num', title: 'Last render report (click for details)', onclick: () => side.show('report') });
  root.append(conn, rev, file, msg, h('span', { class: 'spacer' }), sel, report);
  const labels = { live: 'Live', mock: 'Mock server', reconnecting: 'Reconnecting…', connecting: 'Connecting…' };
  let timer = 0;
  const update = () => {
    conn.className = `conn ${S.conn}`;
    conn.textContent = labels[S.conn] || S.conn;
    rev.textContent = `rev ${S.revision}${S.dirty ? ' · modified' : ''}`;
    file.textContent = S.path || 'unsaved';
    file.title = S.path || '';
    sel.textContent = S.sel.size ? `${S.sel.size} selected` : '';
    const m = S.progress ? { text: S.progress, kind: 'info' } : S.msg;
    msg.textContent = m?.text || '';
    msg.className = `msg ${m?.kind || ''}`;
    clearTimeout(timer);
    if (S.msg && !S.progress) timer = setTimeout(() => { S.msg = null; update(); }, S.msg.kind === 'error' || S.msg.kind === 'agent' ? 8000 : 4000);
  };
  on('project select status conn', update);
  on('report', () => { report.textContent = fmtReport(S.report); report.classList.add('fresh'); setTimeout(() => report.classList.remove('fresh'), 1500); });
  on('cursor', ({ tick, key }) => {
    $('cursor-info').textContent = `${fmtBarBeat(tick)}${key >= 0 ? `  ${noteName(key)}` : ''}`;
  });
  update();
}

// ---- file operations ---------------------------------------------------------------------------------
const fileInput = h('input', { type: 'file', accept: '.mid,.midi,.ust,.ustx,.musicxml,.xml,.cantor,.txt,.wav', hidden: true });
document.body.append(fileInput);
const KIND = { mid: 'midi', midi: 'midi', ust: 'ust', ustx: 'ustx', musicxml: 'music_xml', xml: 'music_xml', mxl: 'music_xml', cantor: 'cantor', txt: 'lyrics' };
fileInput.addEventListener('change', async () => {
  const f = fileInput.files[0];
  fileInput.value = '';
  if (!f) return;
  const kind = KIND[f.name.split('.').pop().toLowerCase()] || 'audio';
  await call(`api/import?name=${encodeURIComponent(f.name)}&kind=${kind}`, f, `Imported ${f.name}`);
  fit();
});

async function openPath() {
  const path = await promptText('Open project', 'Path on the server', S.path || '');
  if (path) { await call('api/project/open', { path }, `Opened ${path}`); fit(); }
}
async function save(as = false) {
  let path = S.path;
  if (as || !path) path = await promptText('Save project', 'Path on the server', S.path || 'untitled.cantor');
  if (path) call('api/project/save', { path }, `Saved ${path}`);
}
async function importPath() {
  const path = await promptText('Import', 'Path on the server (MIDI, UST, USTX, MusicXML, lyrics, audio)', '');
  if (path) { await call('api/import', { path }, `Imported ${path}`); fit(); }
}
function exportAs(format) {
  const base = (S.project?.meta?.title || 'cantor').replace(/[^\w.-]+/g, '_');
  api.download(`api/export?format=${format}`, `${base}.${format}`);
}

// ---- menus --------------------------------------------------------------------------------------------
const hasSel = () => S.sel.size > 0;
function initMenus() {
  menuBar($('menubar'), [
    { label: 'File', items: [
      { label: 'New', run: async () => { await call('api/project/new', {}, 'New project'); fit(); } },
      { label: 'Open…', key: `${MOD}O`, run: openPath },
      { label: 'Save', key: `${MOD}S`, run: () => save() },
      { label: 'Save as…', key: `${MOD}⇧S`, run: () => save(true) }, '-',
      { label: 'Import file…', key: `${MOD}I`, run: () => fileInput.click() },
      { label: 'Import from path…', run: importPath }, '-',
      { label: 'Export WAV', run: () => exportAs('wav') },
      { label: 'Export UST', run: () => exportAs('ust') },
      { label: 'Export USTX', run: () => exportAs('ustx') },
      { label: 'Export .cantor', run: () => exportAs('cantor') },
    ] },
    { label: 'Edit', items: [
      { label: 'Undo', key: `${MOD}Z`, run: edit.undo, enabled: () => S.canUndo },
      { label: 'Redo', key: `${MOD}⇧Z`, run: edit.redo, enabled: () => S.canRedo }, '-',
      { label: 'Cut', key: `${MOD}X`, run: edit.cut, enabled: hasSel },
      { label: 'Copy', key: `${MOD}C`, run: edit.copy, enabled: hasSel },
      { label: 'Paste', key: `${MOD}V`, run: () => edit.paste() },
      { label: 'Delete', key: 'Del', run: edit.remove, enabled: hasSel }, '-',
      { label: 'Select all', key: `${MOD}A`, run: edit.selectAll },
      { label: 'Deselect', key: 'Esc', run: edit.deselect, enabled: hasSel }, '-',
      { label: 'Edit lyric', key: 'Enter', run: () => editLyric([...S.sel][0]), enabled: hasSel },
      { label: 'Merge notes', key: `${MOD}J`, run: edit.merge, enabled: () => S.sel.size > 1 }, '-',
      { label: 'Save checkpoint…', run: saveCheckpoint },
      { label: 'History', run: () => side.show('history') },
    ] },
    { label: 'View', items: [
      { label: 'Zoom in', key: '=', run: () => zoomX(1.4) },
      { label: 'Zoom out', key: '-', run: () => zoomX(1 / 1.4) },
      { label: 'Fit', key: 'F', run: fit }, '-',
      { label: 'Pitch curve', checked: () => S.show.pitch, run: () => toggleShow('pitch') },
      { label: 'Reference F0', checked: () => S.show.ref, run: () => toggleShow('ref') },
      { label: 'Difference to reference', key: 'D', checked: () => S.show.diff, run: () => toggleShow('diff') },
      { label: 'Phoneme timing', checked: () => S.show.phonemes, run: () => toggleShow('phonemes') },
      { label: 'Report markers', checked: () => S.show.report, run: () => toggleShow('report') },
      { label: 'Follow playhead', checked: () => S.show.follow, run: () => { S.show.follow = !S.show.follow; } }, '-',
      { label: 'Light theme', checked: isLight, run: () => setTheme(isLight() ? 'dark' : 'light') },
    ] },
    { label: 'Tune', items: [
      { label: 'Auto pitch', run: () => runTune('auto_pitch') },
      { label: 'Auto dynamics', run: () => runTune('auto_dynamics') },
      { label: 'Insert breaths', run: () => runTune('breaths') },
      { label: 'Match reference', run: () => runTune('match_reference'), enabled: () => !!S.project?.reference },
      { label: 'Humanize', run: () => runTune('humanize') },
    ] },
    { label: 'Render', items: [
      { label: 'Play / pause', key: 'Space', run: togglePlay },
      { label: 'Stop', run: stop },
      { label: 'Loop', key: 'L', checked: () => S.loop.on, run: toggleLoop }, '-',
      { label: 'Hear synth', checked: () => S.ab === 'a', run: () => setAB('a') },
      { label: 'Hear reference', key: 'A', checked: () => S.ab === 'b', run: () => setAB('b'), enabled: () => !!S.project?.reference },
      { label: 'Hear both', key: '⇧A', checked: () => S.ab === 'ab', run: () => setAB('ab'), enabled: () => !!S.project?.reference }, '-',
      { label: 'Render now', key: `${MOD}R`, run: renderNow },
      { label: 'Report', run: () => side.show('report') },
      { label: 'Export WAV', run: () => exportAs('wav') },
    ] },
    { label: 'Help', items: [
      { label: 'Keyboard shortcuts', key: '?', run: shortcuts },
      { label: 'Connect an agent…', run: openConnect },
      { label: 'Simulate an agent session (mock)', run: () => runAgentDemo(), enabled: api.isMock }, '-',
      { label: 'About Cantor', run: about },
    ] },
  ]);
}

function shortcuts() {
  const rows = [
    ['1 2 3 4', 'Select, draw, knife, pitch pen'], ['Space', 'Play / pause'], ['Home', 'Return to start'],
    ['L', 'Toggle loop (shift-drag the ruler to set it)'], ['A / Shift+A', 'Hear synth ⇄ reference / both'],
    ['D', 'Shade pitch far from the reference'], ['F', 'Fit selection or track'], ['= / -', 'Zoom in / out'],
    [`${MOD}wheel`, 'Zoom time'], ['Alt+wheel', 'Zoom pitch'], ['Shift+wheel', 'Scroll time'], ['Space+drag', 'Pan'],
    ['↑ ↓', 'Transpose a semitone (shift: octave)'], ['← →', 'Move by the snap grid'], [`${MOD}← →`, 'Shorten / lengthen'],
    ['Enter / double-click', 'Edit lyric (Tab: next note)'], ['Del', 'Delete notes'], [`${MOD}Z / ${MOD}⇧Z`, 'Undo / redo'],
    [`${MOD}C X V`, 'Copy, cut, paste at the playhead; plain text pastes as lyrics'], [`${MOD}A`, 'Select all'],
    ['Alt-click a field', 'Reset to default'], ['Shift-drag a field', 'Fine adjustment'],
  ];
  modal('Keyboard shortcuts', h('table', { class: 'keys' }, rows.map(([k, d]) => h('tr', {}, h('td', {}, h('kbd', {}, k)), h('td', {}, d)))));
}

function about() {
  modal('About Cantor', h('div', { class: 'connect' },
    h('p', {}, 'Cantor is a UTAU-style singing synthesis workstation: concatenative synthesis from UTAU voicebanks through a built-in PSOLA resampler. No neural models.'),
    h('p', { class: 'dim' }, api.isMock() ? 'Running against the in-browser mock server.' : `Connected to ${location.host}.`)));
}

// ---- keyboard ----------------------------------------------------------------------------------------------
function initKeys() {
  addEventListener('keydown', (e) => {
    const t = e.target;
    if (t.matches?.('input, select, textarea') || document.querySelector('.modal-back')) return;
    const mod = e.metaKey || e.ctrlKey, k = e.key;
    const step = edit.gridStep();
    const map = {
      ' ': () => togglePlay(), Home: () => { stop(); setPlayhead(S.loop.on ? S.loop.a : 0); },
      Delete: edit.remove, Backspace: edit.remove, Escape: edit.deselect,
      Enter: () => S.sel.size && editLyric([...S.sel][0]),
      ArrowUp: () => edit.nudge(0, e.shiftKey ? 12 : 1), ArrowDown: () => edit.nudge(0, e.shiftKey ? -12 : -1),
      ArrowLeft: () => (mod ? edit.resize(-step) : edit.nudge(-step, 0)),
      ArrowRight: () => (mod ? edit.resize(step) : edit.nudge(step, 0)),
    };
    const modMap = {
      z: () => (e.shiftKey ? edit.redo() : edit.undo()), y: edit.redo, a: edit.selectAll, s: () => save(e.shiftKey),
      o: openPath, i: () => fileInput.click(), r: renderNow, j: edit.merge,
    };
    const plain = {
      1: () => setTool('select'), 2: () => setTool('draw'), 3: () => setTool('knife'), 4: () => setTool('pen'),
      f: fit, l: toggleLoop, a: () => toggleAB(e.shiftKey), d: () => toggleShow('diff'), '=': () => zoomX(1.4), '+': () => zoomX(1.4), '-': () => zoomX(1 / 1.4), '?': shortcuts,
    };
    const fn = mod ? modMap[k.toLowerCase()] ?? (k.startsWith('Arrow') ? map[k] : null) : map[k] ?? plain[k.toLowerCase()];
    if (fn) { e.preventDefault(); fn(); }
  });
  // Buttons never take focus, so Space/Enter keep driving the editor rather than re-clicking them.
  document.addEventListener('mousedown', (e) => { if (e.target.closest?.('button')) e.preventDefault(); });
  const inField = () => document.activeElement?.matches('input, textarea, select');
  document.addEventListener('copy', (e) => {
    if (inField()) return;
    const text = edit.copy();
    if (text) { e.clipboardData.setData('text/plain', text); e.preventDefault(); }
  });
  document.addEventListener('cut', (e) => {
    if (inField()) return;
    const text = edit.cut();
    if (text) { e.clipboardData.setData('text/plain', text); e.preventDefault(); }
  });
  document.addEventListener('paste', (e) => {
    if (inField()) return;
    e.preventDefault();
    edit.paste(e.clipboardData.getData('text/plain'));
  });
}

// ---- boot ------------------------------------------------------------------------------------------------
async function boot() {
  initMenus();
  initTransport($('transport'));
  initToolbar($('toolbar'));
  initRoll($('roll-wrap'));
  initLanes($('lane-tabs'), $('lane-wrap'), $('splitter'));
  initTracks($('tracks'));
  side = tabs($('props'), [['props', 'Properties'], ['history', 'History'], ['report', 'Report']]);
  initProps(side.panes.props, (t) => side.label('props', t));
  initHistory(side.panes.history);
  initReport(side.panes.report, (t) => side.label('report', t));
  initStatus($('status'));
  initKeys();
  $('connect-btn').addEventListener('click', openConnect);
  try {
    const st = await api.connect();
    const [schema, vbs] = await Promise.all([api.get('api/schema').catch(() => null), api.get('api/voicebanks').catch(() => [])]);
    loadSchema(schema);
    S.voicebanks = vbs;
    emit('voicebanks');
    await adoptState(st);
    loadHistory();
    fit();
    if (!vbs.length) notify('No voicebanks found. Put UTAU voicebank folders in ./voicebanks (or start with --voicebanks DIR), then restart Cantor.', 'warn');
  } catch (e) {
    notify(`Cannot load project: ${e.message}`, 'error');
  }
  api.subscribe((ev) => {
    const mine = ev.client === api.CLIENT;
    if (ev.type === 'revision' && ev.revision !== S.revision) {
      // Someone else's edit: an agent over MCP, the CLI, or another browser tab.
      const outside = ev.client ? !mine : ev.source === 'mcp' || ev.source === 'cli';
      if (outside) externalEdit(ev); else refresh();
    } else if (ev.type === 'checkpoint') {
      loadHistory();
      if (!mine) notify(`${sourceName(ev.source)}: checkpoint “${ev.name}”`, 'agent');
    } else if (ev.type === 'progress') { S.progress = ev.message || (ev.fraction != null ? `Rendering ${Math.round(ev.fraction * 100)}%` : null); emit('status'); }
    else if (ev.type === 'render' && ev.report && !mine) {
      S.report = { ...ev.report, source: ev.source };
      S.reportRev = ev.revision ?? null;
      emit('report');
    }
  }, (status) => {
    const was = S.conn;
    S.conn = status;
    emit('conn');
    if (status === 'live' && was === 'reconnecting') refresh();
  });
}

boot();
