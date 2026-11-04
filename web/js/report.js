// Report tab: the last render's numbers, phrases worst-first, missing aliases and artifacts.
// Clicking a row selects and frames what it points at, so a person can go straight to what needs a look.
import { S, on, emit, notes, select, msToTick, fmtTime, sourceName } from './state.js';
import { fit, setPlayhead } from './pianoroll.js';
import { renderNow } from './transport.js';
import { h, fixed } from './ui.js';

// Bands from docs/agent-guide.md ("below ~40 cents is close, below ~25 is very close"). Without a
// reference the score error also counts deliberate vibrato and portamento, so its bands are wider.
const BANDS = { ref: [25, 40], score: [40, 80], onset: [30, 60] };
export function grade(v, band) {
  if (v == null || !Number.isFinite(v)) return '';
  const [ok, warn] = BANDS[band];
  return v <= ok ? 'ok' : v <= warn ? 'warn' : 'bad';
}
/** The number a phrase is judged by: error vs the reference when there is one, else vs the score. */
export const phraseScore = (ph) => (ph.ref_p95 != null ? { v: ph.ref_p95, band: 'ref' } : { v: ph.pitch_p95, band: 'score' });

let body, sortBy = 'error';

export function initReport(el, setLabel) {
  body = h('div', { class: 'scroll report-body' });
  el.append(body);
  const update = () => {
    render();
    const bad = (S.report?.missing_aliases?.length || 0) + phrasesOfTrack().filter((p) => grade(phraseScore(p).v, phraseScore(p).band) === 'bad').length;
    setLabel(bad ? `Report · ${bad}` : 'Report');
  };
  on('report track', update);
  on('project', () => body.querySelector('.stale')?.classList.toggle('hidden', !isStale()));
  update();
}

const isStale = () => S.report && S.reportRev != null && S.reportRev !== S.revision;
const phrasesOfTrack = () => (S.report?.phrases || []).filter((p) => p.track === S.trackId);

/** Selects the notes of the current track that lie in [a, b) seconds and frames them. */
function focusSeconds(a, b, play = true) {
  const t0 = msToTick(a * 1000), t1 = msToTick(b * 1000);
  const ids = notes().filter((n) => n.start < t1 && n.start + n.length > t0).map((n) => n.id);
  if (ids.length) { select(ids); fit(); }
  if (play) setPlayhead(Math.max(0, t0));
  S.loop.a = t0;
  S.loop.b = t1;
  emit('loop');
}

function focusNote(id) {
  if (!notes().some((n) => n.id === id)) return;
  select([id]);
  fit();
}

function metric(label, value, cls = '', title) {
  return h('div', { class: 'metric', title }, h('span', { class: 'mlabel' }, label), h('span', { class: `mval num ${cls}` }, value));
}

function render() {
  const r = S.report;
  if (!r) {
    body.replaceChildren(h('div', { class: 'psec' },
      h('p', { class: 'empty' }, 'No render yet. Render to measure pitch error, timing, loudness and missing aliases.'),
      h('button', { class: 'btn small', onclick: renderNow }, 'Render now')));
    return;
  }
  const c = (v, d = 0, u = '¢') => (v == null || !Number.isFinite(v) ? '—' : `${fixed(v, d)}${u}`);
  const head = h('div', { class: 'psec' },
    h('div', { class: 'psec-title' }, h('span', {}, 'Last render'),
      h('span', { class: 'scope' }, S.reportRev != null ? `rev ${S.reportRev}${r.source ? ` · ${sourceName(r.source).toLowerCase()}` : ''}` : '')),
    h('div', { class: 'stale hidden' }, 'Edited since this render. ', h('button', { class: 'linkish', onclick: renderNow }, 'Render again')),
    h('div', { class: 'metrics' },
      metric('vs ref p95', c(r.ref_pitch_error?.p95), grade(r.ref_pitch_error?.p95, 'ref'), 'Pitch error against the reference vocal, 95th percentile'),
      metric('vs score p95', c(r.pitch_error?.p95), grade(r.pitch_error?.p95, 'score'), 'Pitch error against the notes, 95th percentile'),
      metric('onset p95', c(r.onset_error_ms?.p95, 0, ' ms'), grade(r.onset_error_ms?.p95, 'onset'), 'Vowel onset vs note start'),
      metric('bias', c(r.ref_pitch_error?.bias ?? r.pitch_error?.bias), '', 'Median signed error: + is sharp'),
      metric('loudness', r.lufs == null ? '—' : `${fixed(r.lufs, 1)} LUFS`, r.lufs != null && Math.abs(r.lufs + 16) > 4 ? 'warn' : ''),
      metric('peak', r.true_peak_db == null ? '—' : `${fixed(r.true_peak_db, 1)} dB`, r.clipped ? 'bad' : r.true_peak_db > -1 ? 'warn' : '',
        r.clipped ? `${r.clipped} clipped samples` : 'True peak')));
  head.querySelector('.stale').classList.toggle('hidden', !isStale());

  const sections = [head];
  const miss = r.missing_aliases || [];
  if (miss.length) {
    sections.push(h('div', { class: 'psec' },
      h('div', { class: 'psec-title' }, h('span', {}, `Missing aliases · ${miss.length}`)),
      h('div', { class: 'rlist' }, miss.map((m) => h('button', { class: 'rrow', onclick: () => focusNote(m.note), title: 'Select the note' },
        h('span', { class: 'dot bad' }), h('span', { class: 'rmain' }, `“${m.lyric}” has no alias`), h('span', { class: 'rnum num' }, `#${m.note}`))))));
  }

  const phrases = phrasesOfTrack();
  if (phrases.length) {
    const sorted = [...phrases].sort(sortBy === 'error'
      ? (a, b) => (phraseScore(b).v ?? -1) - (phraseScore(a).v ?? -1) : (a, b) => a.start_sec - b.start_sec);
    const sortBtn = h('button', { class: 'linkish', onclick: () => { sortBy = sortBy === 'error' ? 'time' : 'error'; render(); } },
      sortBy === 'error' ? 'worst first' : 'in order');
    sections.push(h('div', { class: 'psec' },
      h('div', { class: 'psec-title' }, h('span', {}, `Phrases · ${phrases.length}`), sortBtn),
      h('div', { class: 'rlist' }, sorted.map(phraseRow))));
  }

  const arts = (r.artifacts || []).filter((a) => a.track == null || a.track === S.trackId);
  if (arts.length) {
    sections.push(h('div', { class: 'psec' },
      h('div', { class: 'psec-title' }, h('span', {}, `Artifacts · ${arts.length}`)),
      h('div', { class: 'rlist' }, arts.map((a) => h('button', {
        class: 'rrow', title: a.detail, onclick: () => focusSeconds(a.at_sec - 0.25, a.at_sec + Math.max(a.duration_sec, 0.25) + 0.25),
      }, h('span', { class: 'dot warn' }), h('span', { class: 'rmain' }, a.detail || a.kind.replaceAll('_', ' ')),
      h('span', { class: 'rnum num' }, fmtTime(a.at_sec * 1000)))))));
  }
  body.replaceChildren(...sections);
}

function phraseRow(ph) {
  const { v, band } = phraseScore(ph), g = grade(v, band);
  const lyr = notes().filter((n) => n.start + n.length > msToTick(ph.start_sec * 1000) && n.start < msToTick(ph.end_sec * 1000))
    .map((n) => n.lyric).filter((l) => l !== '+' && l !== 'br' && l !== '-').join('');
  const w = ph.worst_note;
  const detail = [
    `${fmtTime(ph.start_sec * 1000)}–${fmtTime(ph.end_sec * 1000)}`,
    ph.ref_p95 != null ? `vs ref p95 ${fixed(ph.ref_p95, 0)}¢` : null,
    ph.pitch_p95 != null ? `vs score p95 ${fixed(ph.pitch_p95, 0)}¢` : null,
    ph.onset_p95_ms != null ? `onset p95 ${fixed(ph.onset_p95_ms, 0)} ms` : null,
    ph.voiced_ratio < 0.9 ? `${Math.round(ph.voiced_ratio * 100)}% voiced` : null,
    w ? `worst: “${w.lyric}” ${fixed(w.p95, 0)}¢ (${w.bias >= 0 ? '+' : ''}${fixed(w.bias, 0)}¢)` : null,
  ].filter(Boolean).join('\n');
  return h('button', { class: 'rrow', title: `${detail}\nClick: select the phrase and set the loop to it (L loops)`, onclick: () => focusSeconds(ph.start_sec, ph.end_sec) },
    h('span', { class: `dot ${g}` }),
    h('span', { class: 'rmain' }, h('span', { class: 'rnum num' }, String(ph.index + 1).padStart(2, ' ')), ' ', lyr || '…'),
    h('span', { class: `rnum num ${ph.voiced_ratio < 0.9 ? 'bad' : ''}` }, v == null ? '—' : `${fixed(v, 0)}¢`));
}
