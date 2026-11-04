// In-browser fake of the Cantor HTTP API (?mock, or when no server answers).
// Holds a demo project, applies the ops the UI sends, and fakes pitch, F0 and audio.

const PPQ = 480;
export const events = new EventTarget();

const fail = (code, message) => {
  throw Object.assign(new Error(message), { code, diagnostics: [{ level: 'error', code, message }] });
};
const clone = (x) => structuredClone(x);
const isObj = (x) => x && typeof x === 'object' && !Array.isArray(x);

/** JSON-merge-patch style deep merge: null clears, objects merge, the rest replace. */
function merge(target, patch) {
  for (const [k, v] of Object.entries(patch)) {
    if (isObj(v) && isObj(target[k])) merge(target[k], v);
    else target[k] = isObj(v) ? merge(DEFAULTS[k] ? clone(DEFAULTS[k]) : {}, v) : v;
  }
  return target;
}

// ---- demo content -------------------------------------------------------------
const KANA = 'あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわをん' +
  'がぎぐげござじずぜだぢづでどばびぶべぼぱぴぷぺぽ';
const BANKS = [
  { id: 'kasane', name: 'Kasane (mock CV)', author: 'cantor-fixtures', kind: 'cv', prefix_map: false, hue: 28,
    aliases: new Set([...KANA.replace('ぞ', ''), 'br']) },
  { id: 'tenor', name: 'Tenor (mock VCV)', author: 'cantor-fixtures', kind: 'vcv', prefix_map: true, hue: 200,
    aliases: new Set([...KANA, 'br']) },
];

const DEFAULTS = {
  vibrato: { length: 65, period_ms: 180, depth_cents: 20, fade_in: 30, fade_out: 20, phase: 0, shift: 0, drift: 0.2 },
  scoop: { cents: -60, length_ms: 60 },
  fall: { cents: -120, length_ms: 100 },
};

let nextId = 1;
function makeNote(n) {
  return merge({
    id: nextId++, start: 0, length: PPQ, key: 60, lyric: 'あ', alias: null, vel: 100, vol: 100, mod: 0, flags: '',
    timing: { preutter_ms: null, overlap_ms: null, offset_ms: 0 },
    pitch: {
      portamento: { length_ms: 80, start_ms: -40, overshoot_cents: 0, preparation_cents: 0, shape: 's' },
      scoop: null, fall: null, vibrato: null, points: [],
    },
  }, n);
}
function makeTrack(t) {
  return merge({
    id: nextId++, name: 'Track', singer: '', phonemizer: 'auto', flags: '',
    mix: { gain_db: 0, pan: 0, mute: false, solo: false }, effects: [], curves: {}, notes: [],
  }, t);
}

// Sakura Sakura (traditional): [lyric, key, beats]; null lyric = rest.
const SONG = [
  [null, 0, 4],
  ['さ', 69, 1], ['く', 69, 1], ['ら', 71, 2], ['さ', 69, 1], ['く', 69, 1], ['ら', 71, 1.5], [null, 0, 0.5],
  ['や', 69, 1], ['よ', 71, 1], ['い', 72, 1], ['の', 71, 1], ['そ', 69, 1], ['ら', 71, 0.5], ['+', 69, 0.5], ['わ', 65, 1.5], [null, 0, 0.5],
  ['み', 64, 1], ['わ', 60, 1], ['た', 64, 1], ['す', 65, 1], ['か', 64, 1], ['ぎ', 64, 0.5], ['+', 60, 0.5], ['り', 59, 1.5], [null, 0, 0.5],
  ['か', 69, 1], ['す', 71, 1], ['み', 72, 1], ['か', 71, 1], ['く', 69, 1], ['も', 71, 0.5], ['+', 69, 0.5], ['か', 65, 1.5], [null, 0, 0.5],
  ['に', 64, 1], ['お', 60, 1], ['い', 64, 1], ['ぞ', 65, 1], ['い', 64, 1], ['ず', 64, 0.5], ['+', 60, 0.5], ['る', 59, 1.5], [null, 0, 0.5],
  ['い', 69, 1], ['ざ', 69, 1], ['や', 71, 1.5], [null, 0, 0.5], ['い', 69, 1], ['ざ', 69, 1], ['や', 71, 1.5], [null, 0, 0.5],
  ['み', 64, 1], ['に', 65, 1], ['ゆ', 71, 0.5], ['+', 69, 0.5], ['か', 65, 1], ['ん', 64, 3.5], [null, 0, 4.5],
];

function demoProject() {
  let t = 0;
  const notes = [];
  for (const [lyric, key, beats] of SONG) {
    const length = beats * PPQ;
    if (lyric) {
      const n = makeNote({ start: t, length, key, lyric });
      if (beats >= 1.5) n.pitch.vibrato = { ...DEFAULTS.vibrato };
      notes.push(n);
    } else if (notes.length && beats <= 1) {
      notes.push(makeNote({ start: t, length, key: notes.at(-1).key, lyric: 'br' }));   // a breath starts the next phrase
    }
    t += length;
  }
  const bar = PPQ * 4;
  const dyn = [[bar, -6], [bar * 3, 0], [bar * 5, -2], [bar * 9, 2], [bar * 13, 0], [bar * 15, -8]];
  return {
    format: 3,
    meta: { title: 'Sakura Sakura (demo)', artist: 'Traditional', comment: 'Mock project' },
    tempo: [{ tick: 0, bpm: 84 }],
    time_sig: [{ bar: 0, num: 4, den: 4 }],
    key: { tonic: 9, scale: 'minor' },
    sections: [
      { name: 'Verse', start: bar, end: bar * 9 },
      { name: 'Bridge', start: bar * 9, end: bar * 13 },
      { name: 'Outro', start: bar * 13, end: bar * 16 },
    ],
    reference: { path: 'reference/sakura_vocal.wav', offset_ms: 0, gain_db: -3, muted: false },
    tracks: [makeTrack({
      name: 'Vocal', singer: 'kasane', phonemizer: 'jp_cv', flags: 'g-2',
      curves: { dyn: { points: dyn, interp: 'linear' } }, notes,
    })],
  };
}

// ---- engine state ----------------------------------------------------------------
let project = demoProject();
let revision = 1, dirty = false, path = null;
const undo = [], redo = [];          // { project, label, source }: the state before (undo) or after (redo) a step
const checkpoints = new Map();       // name -> project snapshot
const renders = new Map();
let refF0 = null, refAudio = null;

// ---- time ------------------------------------------------------------------------
function msOf(p, t) {
  const tp = [...p.tempo].sort((a, b) => a.tick - b.tick);
  let ms = 0;
  for (let i = 0; i < tp.length; i++) {
    const end = i + 1 < tp.length ? Math.min(t, tp[i + 1].tick) : t;
    if (end <= tp[i].tick) break;
    ms += ((end - tp[i].tick) * 60000) / (tp[i].bpm * PPQ);
  }
  return ms;
}
const tickOf = (p, ms) => (ms * p.tempo[0].bpm * PPQ) / 60000; // demo projects keep one tempo for synthesis

function curveAt(curve, t, dflt = 0) {
  const pts = curve?.points;
  if (!pts?.length) return dflt;
  if (t <= pts[0][0]) return pts[0][1];
  for (let i = 1; i < pts.length; i++) {
    if (t <= pts[i][0]) {
      const [t0, v0] = pts[i - 1], [t1, v1] = pts[i];
      return curve.interp === 'step' ? v0 : v0 + ((v1 - v0) * (t - t0)) / (t1 - t0 || 1);
    }
  }
  return pts[pts.length - 1][1];
}

// ---- fake pitch generation ------------------------------------------------------------
const ease = { s: (u) => (1 - Math.cos(Math.PI * u)) / 2, linear: (u) => u, in: (u) => u * u, out: (u) => 1 - (1 - u) * (1 - u) };

function renderPitch(p, tr, withPitd = true) {
  const ns = tr.notes, step = 5;
  if (!ns.length) return { start_ms: 0, step_ms: step, cents: [] };
  const s = ns.map((n) => msOf(p, n.start)), e = ns.map((n) => msOf(p, n.start + n.length));
  const N = Math.ceil((e[e.length - 1] + 50) / step);
  const c = new Array(N).fill(null);
  let j = 0;
  for (let i = 0; i < N; i++) {
    const t = i * step;
    while (j < ns.length - 1 && s[j + 1] <= t) j++;
    const voiced = t >= s[0] - 40 && (t < e[j] || (j + 1 < ns.length && s[j + 1] - e[j] < 150));
    if (voiced && !REST.has(ns[j].lyric)) c[i] = ns[j].key * 100;
  }
  const span = (a, b, fn) => {
    for (let i = Math.max(0, Math.ceil(a / step)); i < Math.min(N, b / step); i++) {
      if (c[i] !== null) c[i] = fn(c[i], (i * step - a) / (b - a || 1), i * step);
    }
  };
  ns.forEach((n, k) => {
    const P = n.pitch, cur = n.key * 100;
    const prev = ns[k - 1];
    if (prev && s[k] - e[k - 1] < 150 && P.portamento.length_ms > 0) {
      const po = P.portamento, from = prev.key * 100, d = cur - from, sg = Math.sign(d);
      const a = s[k] + po.start_ms, f = ease[po.shape] || ease.s;
      span(a, a + po.length_ms, (_, u) => from + d * f(u)
        + sg * po.overshoot_cents * Math.sin(Math.PI * u) * u - sg * po.preparation_cents * Math.sin(Math.PI * u) * (1 - u));
    }
    if (P.scoop) span(s[k], s[k] + P.scoop.length_ms, (v, u) => v + P.scoop.cents * (1 - ease.s(u)));
    if (P.fall) span(e[k] - P.fall.length_ms, e[k], (v, u) => v + P.fall.cents * ease.in(u));
    const V = P.vibrato;
    if (V && V.depth_cents > 0) {
      const v0 = e[k] - ((e[k] - s[k]) * V.length) / 100, dur = e[k] - v0;
      span(v0, e[k], (v, u, t) => {
        const x = t - v0;
        const fade = Math.min(1, x / (dur * V.fade_in / 100 || 1e-9), (dur - x) / (dur * V.fade_out / 100 || 1e-9));
        return v + V.depth_cents * fade * (Math.sin((2 * Math.PI * x) / V.period_ms + V.phase * 2 * Math.PI) + V.shift / 100);
      });
    }
    if (P.points?.length) {
      span(s[k], e[k], (v, u, t) => v + curveAt({ points: P.points.map((q) => [q.ms, q.cents]) }, t - s[k]));
    }
  });
  if (withPitd && tr.curves.pitd) {
    for (let i = 0; i < N; i++) if (c[i] !== null) c[i] += curveAt(tr.curves.pitd, tickOf(p, i * step));
  }
  return { start_ms: 0, step_ms: step, cents: c.map((v) => (v === null ? null : Math.round(v * 10) / 10)) };
}

function phonemes(p, tr) {
  const bank = BANKS.find((b) => b.id === tr.singer);
  return tr.notes.filter((n) => !['+', '-'].includes(n.lyric)).map((n) => {
    const alias = n.alias ?? n.lyric;
    const vowel = 'あいうえおん'.includes(alias);
    const preutter = n.timing.preutter_ms ?? (vowel ? 15 : 55);
    return {
      note: n.id, alias, preutter_ms: preutter, overlap_ms: n.timing.overlap_ms ?? (vowel ? 5 : 25),
      start_ms: msOf(p, n.start) - preutter + n.timing.offset_ms, missing: !bank || !bank.aliases.has(alias),
    };
  });
}

function referenceF0() {
  if (refF0) return refF0;
  const p = demoProject(), pc = renderPitch(p, p.tracks[0]).cents;   // breaths are unvoiced there
  const out = [];
  for (let t = 0; t < pc.length * 5; t += 10) {
    const v = pc[Math.max(0, Math.round((t - 35) / 5))];
    out.push(v === null || v === undefined ? null
      : Math.round((v + 18 * Math.sin(t / 900) + 6 * Math.sin(t / 97) + 4 * Math.sin(t / 31)) * 10) / 10);
  }
  return (refF0 = { step_ms: 10, cents: out });
}

// ---- fake audio -------------------------------------------------------------------
const SR = 22050;

function synthTrack(p, tr, out, gain, f0, stepMs, voiceAmp) {
  const harm = [1, 0.55, 0.32, 0.18, 0.1];
  let phase = 0;
  for (let i = 0; i < out.length; i++) {
    const ms = (i / SR) * 1000;
    const c = f0[Math.floor(ms / stepMs)];
    const a = voiceAmp(ms);
    if (c === null || c === undefined || a <= 0) { phase = 0; continue; }
    phase += (2 * Math.PI * 440 * 2 ** ((c - 6900) / 1200)) / SR;
    let v = 0;
    for (let h = 0; h < harm.length; h++) v += harm[h] * Math.sin(phase * (h + 1));
    out[i] += v * a * gain;
  }
}

function noteEnvelope(p, tr) {
  const spans = tr.notes.filter((n) => !['-', 'br'].includes(n.lyric)).map((n) => ({
    a: msOf(p, n.start), b: msOf(p, n.start + n.length), vol: n.vol / 100,
  }));
  return (ms) => {
    const n = spans.find((x) => ms >= x.a - 5 && ms < x.b + 40);
    if (!n) return 0;
    const att = Math.min(1, (ms - n.a + 5) / 25), rel = Math.min(1, (n.b + 40 - ms) / 60);
    return 0.22 * n.vol * Math.max(0, Math.min(att, rel)) * 10 ** (curveAt(tr.curves.dyn, tickOf(p, ms)) / 20);
  };
}

function wav(samples) {
  const buf = new ArrayBuffer(44 + samples.length * 2), d = new DataView(buf);
  const str = (o, s) => [...s].forEach((ch, i) => d.setUint8(o + i, ch.charCodeAt(0)));
  str(0, 'RIFF'); d.setUint32(4, 36 + samples.length * 2, true); str(8, 'WAVEfmt ');
  d.setUint32(16, 16, true); d.setUint16(20, 1, true); d.setUint16(22, 1, true);
  d.setUint32(24, SR, true); d.setUint32(28, SR * 2, true); d.setUint16(32, 2, true); d.setUint16(34, 16, true);
  str(36, 'data'); d.setUint32(40, samples.length * 2, true);
  samples.forEach((v, i) => d.setInt16(44 + i * 2, Math.max(-1, Math.min(1, v)) * 32767, true));
  return buf;
}

function percentile(xs, q) {
  if (!xs.length) return 0;
  const s = [...xs].sort((a, b) => a - b);
  return Math.round(s[Math.min(s.length - 1, Math.floor(q * s.length))] * 10) / 10;
}

const REST = new Set(['-', 'r', 'R', 'rest', 'br', 'breath', '息', '']);

/** Phrases as in cantor-core::phrase: a gap of a beat or a rest/breath note starts one. */
function phrasesOf(tr) {
  const out = [];
  let cur = null, end = -Infinity;
  for (const n of tr.notes) {
    if (!cur || n.start - end >= PPQ || REST.has(n.lyric)) out.push((cur = []));
    cur.push(n);
    end = n.start + n.length;
  }
  return out;
}

/** { p50, p95, bias, frames } of signed errors, or null when empty. */
function stats(signed) {
  if (!signed.length) return null;
  const abs = signed.map(Math.abs);
  return { p50: percentile(abs, 0.5), p95: percentile(abs, 0.95), bias: percentile(signed, 0.5), frames: signed.length };
}

function render(from = {}) {
  const t0 = performance.now();
  const p = project;
  const end = Math.max(1000, ...p.tracks.flatMap((t) => t.notes.map((n) => msOf(p, n.start + n.length)))) + 500;
  const out = new Float32Array(Math.ceil((end / 1000) * SR));
  const solo = p.tracks.some((t) => t.mix.solo);
  const errScore = [], errRef = [], onsets = [], missing = [], phrases = [], artifacts = [];
  const f0 = referenceF0();
  const sec = (t) => Math.round(msOf(p, t)) / 1000;
  for (const tr of p.tracks) {
    const pc = renderPitch(p, tr).cents;
    const missingIds = new Set(phonemes(p, tr).filter((ph) => ph.missing).map((ph) => ph.note));
    if (tr.mix.mute || (solo && !tr.mix.solo) || !tr.notes.length) continue;
    synthTrack(p, tr, out, 10 ** (tr.mix.gain_db / 20), pc, 5, noteEnvelope(p, tr));
    phrasesOf(tr).forEach((ns, index) => {
      const ps = [], pr = [], po = [];
      let worst = null, sung = 0, silent = 0;
      for (const n of ns) {
        if (REST.has(n.lyric)) continue;
        if (missingIds.has(n.id)) {
          missing.push({ note: n.id, lyric: n.lyric });
          artifacts.push({ kind: 'unexpected_silence', track: tr.id, at_sec: sec(n.start), duration_sec: sec(n.start + n.length) - sec(n.start),
            magnitude: sec(n.start + n.length) - sec(n.start), detail: `No alias for "${n.lyric}": the note is silent` });
          silent++;
        }
        sung++;
        const ns_ = [], nr = [];
        const a = Math.ceil(msOf(p, n.start) / 5), b = Math.floor(msOf(p, n.start + n.length) / 5);
        for (let i = a; i < b; i += 4) {
          if (pc[i] === null) continue;
          ns_.push(pc[i] - n.key * 100);
          const r = f0.cents[Math.round((i * 5) / 10)];
          if (p.reference && r !== null && r !== undefined) nr.push(pc[i] - r);
        }
        if (n.lyric !== '+') po.push(n.timing.offset_ms + 4);
        ps.push(...ns_);
        pr.push(...nr);
        const s = stats(ns_);
        if (s && (!worst || s.p95 > worst.p95)) {
          worst = { note: n.id, lyric: n.lyric, start_sec: sec(n.start), p95: s.p95, bias: s.bias, ref_p95: stats(nr)?.p95 ?? null };
        }
      }
      errScore.push(...ps);
      errRef.push(...pr);
      onsets.push(...po);
      phrases.push({
        track: tr.id, index, start_sec: sec(ns[0].start), end_sec: sec(ns.at(-1).start + ns.at(-1).length),
        pitch_p50: stats(ps)?.p50 ?? null, pitch_p95: stats(ps)?.p95 ?? null, ref_p95: stats(pr)?.p95 ?? null,
        onset_p95_ms: stats(po)?.p95 ?? null, voiced_ratio: sung ? Math.round((1 - silent / sung) * 100) / 100 : 1, worst_note: worst,
      });
    });
  }
  let sq = 0, peak = 0, clipped = 0;
  for (let i = 0; i < out.length; i++) {
    const v = out[i];
    sq += v * v;
    peak = Math.max(peak, Math.abs(v));
    if (Math.abs(v) >= 0.999) {
      if (!clipped) artifacts.push({ kind: 'clipping', track: null, at_sec: i / SR, duration_sec: 0, magnitude: 1, detail: 'The mix clips' });
      clipped++;
    }
  }
  const id = `r${revision}-${renders.size + 1}`;
  renders.set(id, wav(out));
  const report = {
    pitch_error: stats(errScore),
    ref_pitch_error: p.reference ? stats(errRef) : null,
    onset_error_ms: stats(onsets),
    lufs: Math.round((-0.691 + 10 * Math.log10(sq / out.length + 1e-12)) * 10) / 10,
    true_peak_db: Math.round(20 * Math.log10(peak + 1e-12) * 10) / 10,
    clipped, missing_aliases: missing, phrases, artifacts,
    render_ms: Math.round(performance.now() - t0),
  };
  emitEvent({ type: 'render', revision, report, source: from.source ?? 'http', client: from.client ?? null });
  return { render_id: id, revision, report };
}

function referenceAudio() {
  if (refAudio) return refAudio;
  const f0 = referenceF0(), out = new Float32Array(Math.ceil(((f0.cents.length * 10) / 1000) * SR));
  const p = demoProject();
  const env = noteEnvelope(p, p.tracks[0]);
  synthTrack(p, p.tracks[0], out, 0.8, f0.cents, 10, (ms) => env(ms - 35));
  return (refAudio = wav(out));
}

// ---- ops ------------------------------------------------------------------------------
function applyOp(p, op, diags) {
  const tr = () => p.tracks.find((t) => t.id === op.track) || fail('unknown_track', `No track ${op.track}`);
  const pick = (t, ids) => ids.map((id) => t.notes.find((n) => n.id === id) || fail('unknown_note', `No note ${id}`));
  const upsert = (list, key, item) => {
    const i = list.findIndex((x) => x[key] === item[key]);
    if (i >= 0) list[i] = item; else list.push(item);
    list.sort((a, b) => a[key] - b[key]);
  };
  switch (op.op) {
    case 'batch': op.ops.forEach((o) => applyOp(p, o, diags)); break;
    case 'set_meta': Object.assign(p.meta, op.meta); break;
    case 'set_tempo': upsert(p.tempo, 'tick', { tick: op.tick, bpm: op.bpm }); break;
    case 'remove_tempo': p.tempo = p.tempo.filter((t) => t.tick !== op.tick || t.tick === 0); break;
    case 'set_time_sig': upsert(p.time_sig, 'bar', { bar: op.bar, num: op.num, den: op.den }); break;
    case 'remove_time_sig': p.time_sig = p.time_sig.filter((t) => t.bar !== op.bar || t.bar === 0); break;
    case 'set_key': p.key = op.key; break;
    case 'set_sections': p.sections = op.sections; break;
    case 'set_reference': p.reference = op.reference; break;
    case 'add_track': p.tracks.push(makeTrack({ ...op.track, id: undefined })); p.tracks.at(-1).id = nextId++; break;
    case 'remove_track': p.tracks = p.tracks.filter((t) => t.id !== op.track); break;
    case 'update_track': merge(tr(), op.patch); break;
    case 'add_notes': tr().notes.push(...op.notes.map((n) => { const m = makeNote(n); m.id = nextId++; return m; })); break;
    case 'remove_notes': { const t = tr(), ids = new Set(op.ids); t.notes = t.notes.filter((n) => !ids.has(n.id)); break; }
    case 'update_notes': pick(tr(), op.ids).forEach((n) => merge(n, op.patch)); break;
    case 'move_notes':
      for (const n of pick(tr(), op.ids)) {
        n.start += op.dticks; n.key += op.dkeys;
        if (n.start < 0 || n.key < 0 || n.key > 127) fail('out_of_range', 'Notes would leave the editable range');
      }
      break;
    case 'resize_notes':
      for (const n of pick(tr(), op.ids)) {
        if (op.from_start) n.start -= op.dlength;
        n.length += op.dlength;
        if (n.length < 10 || n.start < 0) fail('out_of_range', 'Note length must stay positive');
      }
      break;
    case 'split_note': {
      const t = tr(), [n] = pick(t, [op.id]);
      if (op.at <= n.start || op.at >= n.start + n.length) fail('out_of_range', 'Split point is outside the note');
      const m = makeNote({ ...clone(n), start: op.at, length: n.start + n.length - op.at, lyric: '+' });
      m.id = nextId++;
      n.length = op.at - n.start;
      t.notes.push(m);
      break;
    }
    case 'merge_notes': {
      const t = tr(), ns = pick(t, op.ids).sort((a, b) => a.start - b.start);
      ns[0].length = ns.at(-1).start + ns.at(-1).length - ns[0].start;
      const drop = new Set(ns.slice(1).map((n) => n.id));
      t.notes = t.notes.filter((n) => !drop.has(n.id));
      break;
    }
    case 'set_lyrics': {
      const lyrics = Array.isArray(op.lyrics) ? op.lyrics : op.lyrics.trim().split(/\s+/);
      const ns = pick(tr(), op.ids);
      if (lyrics.length !== ns.length) {
        diags.push({ level: 'warning', code: 'lyric_count', message: `${lyrics.length} lyrics for ${ns.length} notes` });
      }
      ns.forEach((n, i) => { if (i < lyrics.length) n.lyric = lyrics[i]; });
      break;
    }
    case 'set_curve': case 'clear_curve': {
      const t = tr(), [a, b] = op.range;
      const cur = t.curves[op.param] || { points: [], interp: 'linear' };
      cur.points = cur.points.filter(([x]) => x < a || x > b).concat(op.points || []).sort((x, y) => x[0] - y[0]);
      if (cur.points.length) t.curves[op.param] = cur; else delete t.curves[op.param];
      break;
    }
    case 'set_effects': tr().effects = op.effects; break;
    default: fail('unknown_op', `Unknown op "${op.op}"`);
  }
}

function validate(p) {
  for (const t of p.tracks) {
    t.notes.sort((a, b) => a.start - b.start || a.id - b.id);
    for (let i = 1; i < t.notes.length; i++) {
      const a = t.notes[i - 1], b = t.notes[i];
      if (a.start + a.length > b.start) fail('overlap', `Notes "${a.lyric}" and "${b.lyric}" would overlap`);
    }
  }
}

const sentence = (s) => s.replaceAll('_', ' ').replace(/^./, (c) => c.toUpperCase());
function labelOf(ops) {
  if (ops.length !== 1) return `${ops.length} edits`;
  return ops[0].op === 'batch' ? ops[0].label || 'Edit' : sentence(ops[0].op);
}

/** Applies ops as one undo step. `from` = { source, client } of the caller, echoed in the event. */
function commit(ops, from = {}, label = labelOf(ops)) {
  const next = clone(project), diagnostics = [];
  for (const op of ops) applyOp(next, op, diagnostics);
  validate(next);
  undo.push({ project, label, source: from.source ?? 'http' });
  redo.length = 0;
  project = next;
  bump(label, from);
  return { revision, diagnostics };
}

function bump(label = '', from = {}) {
  revision++;
  dirty = true;
  emitEvent({ type: 'revision', revision, label, source: from.source ?? 'http', client: from.client ?? null });
}
const emitEvent = (detail) => events.dispatchEvent(new CustomEvent('event', { detail }));

function history() {
  const step = ({ label, source }) => ({ label, source });
  return { undo: undo.map(step), redo: redo.map(step).reverse(), checkpoints: [...checkpoints.keys()] };
}

// ---- tuner -------------------------------------------------------------------------
function tune({ pass, track, range, params = {} }, from = {}) {
  const p = project, tr = p.tracks.find((t) => t.id === track) || fail('unknown_track', `No track ${track}`);
  const [a, b] = range || [0, Infinity];
  const ns = tr.notes.filter((n) => n.start >= a && n.start < b && !['+', '-', 'br'].includes(n.lyric));
  const prevOf = (n) => tr.notes[tr.notes.indexOf(n) - 1];
  const ops = [];
  const upd = (n, patch) => ops.push({ op: 'update_notes', track, ids: [n.id], patch });
  const rnd = mulberry(params.seed ?? 7);
  if (pass === 'auto_pitch') {
    const st = { natural: [1, 1], pop: [0.8, 1.1], ballad: [1.4, 1.3], rock: [0.6, 0.7], robotic: [0, 0] }[params.style || 'natural'] || [1, 1];
    for (const n of ns) {
      const pv = prevOf(n), iv = pv ? n.key - pv.key : 0, len = (60 + 12 * Math.abs(iv)) * st[0];
      const durMs = msOf(p, n.start + n.length) - msOf(p, n.start);
      upd(n, { pitch: {
        portamento: { length_ms: Math.round(len), start_ms: Math.round(-len / 2), overshoot_cents: iv >= 3 ? 20 * st[1] : 0,
          preparation_cents: iv <= -3 ? 12 * st[1] : 0 },
        vibrato: durMs >= 400 && st[1] ? { ...DEFAULTS.vibrato, depth_cents: Math.round(18 * st[1]) } : null,
      } });
    }
  } else if (pass === 'auto_dynamics') {
    const pts = ns.flatMap((n) => [[n.start, -3 + 4 * Math.sin((n.start / (PPQ * 16)) * Math.PI)], [n.start + n.length - 1, -2]]);
    if (pts.length) ops.push({ op: 'set_curve', track, param: 'dyn', range: [pts[0][0], pts.at(-1)[0]], points: pts });
  } else if (pass === 'breaths') {
    const add = [];
    tr.notes.forEach((n, i) => {
      const pv = tr.notes[i - 1], gapStart = pv ? pv.start + pv.length : 0, gap = n.start - gapStart;
      if (n.start >= a && n.start < b && msOf(p, n.start) - msOf(p, gapStart) >= 300 && n.lyric !== 'br') {
        const len = Math.min(gap, PPQ / 2);
        add.push({ start: n.start - len, length: len, key: n.key, lyric: 'br' });
      }
    });
    if (add.length) ops.push({ op: 'add_notes', track, notes: add });
  } else if (pass === 'match_reference') {
    const base = renderPitch(p, tr, false).cents, f0 = referenceF0().cents, k = params.strength ?? 0.8;
    const pts = [];
    for (let i = 0; i < base.length; i += 4) {
      const r = f0[Math.round((i * 5) / 10)], t = Math.round(tickOf(p, i * 5));
      if (base[i] !== null && r != null && t >= a && t < b) pts.push([t, Math.round((r - base[i]) * k)]);
    }
    if (pts.length) ops.push({ op: 'set_curve', track, param: 'pitd', range: [pts[0][0], pts.at(-1)[0]], points: pts });
  } else if (pass === 'humanize') {
    const c = params.cents ?? 8, ms = params.ms ?? 10;
    for (const n of ns) {
      upd(n, { timing: { offset_ms: Math.round((rnd() * 2 - 1) * ms) },
        pitch: { points: [{ ms: 0, cents: 0 }, { ms: 120, cents: Math.round((rnd() * 2 - 1) * c) }] } });
    }
  } else fail('unknown_pass', `Unknown tune pass "${pass}"`);
  if (!ops.length) return { revision, ops_applied: 0, diagnostics: [{ level: 'info', code: 'noop', message: `${pass}: nothing to do` }] };
  const r = commit([{ op: 'batch', label: sentence(pass), ops }], from);
  return { ...r, ops_applied: ops.length };
}

function mulberry(a) {
  return () => {
    a |= 0; a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---- export ----------------------------------------------------------------------------
function exportFile(format) {
  const enc = (s) => new TextEncoder().encode(s).buffer;
  if (format === 'wav') return renders.get(render().render_id).slice(0);
  if (format === 'cantor') return enc(JSON.stringify(project, null, 2));
  const tr = project.tracks[0];
  if (format === 'ust') {
    const lines = ['[#VERSION]', 'UST Version1.2', '[#SETTING]', `Tempo=${project.tempo[0].bpm}`];
    let t = 0, i = 0;
    for (const n of tr?.notes || []) {
      if (n.start > t) lines.push(`[#${String(i++).padStart(4, '0')}]`, `Length=${n.start - t}`, 'Lyric=R', 'NoteNum=60');
      lines.push(`[#${String(i++).padStart(4, '0')}]`, `Length=${n.length}`, `Lyric=${n.lyric}`, `NoteNum=${n.key}`,
        `Velocity=${n.vel}`, `Intensity=${n.vol}`, `Modulation=${n.mod}`);
      t = n.start + n.length;
    }
    return enc(lines.concat('[#TRACKEND]').join('\r\n'));
  }
  if (format === 'ustx') return enc(`name: ${project.meta.title}\nustx_version: "0.6"\nbpm: ${project.tempo[0].bpm}\n`);
  fail('bad_format', `Unknown export format ${format}`);
}

// ---- router -------------------------------------------------------------------------------
export function imageUrl(id) {
  const b = BANKS.find((x) => x.id === id);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><rect width="40" height="40" fill="hsl(${b?.hue ?? 0} 30% 32%)"/>` +
    `<text x="20" y="26" font-family="system-ui" font-size="17" fill="#eee" text-anchor="middle">${(b?.name || '?')[0]}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

const state = () => ({ project: clone(project), revision, path, can_undo: undo.length > 0, can_redo: redo.length > 0, dirty });

export async function handle(method, fullPath, body, client = null) {
  await new Promise((r) => setTimeout(r, 15));
  return dispatch(method, fullPath, body, { source: 'http', client });
}

function dispatch(method, fullPath, body, from) {
  const [route, query] = fullPath.split('?');
  const q = new URLSearchParams(query);
  const trackOf = (id) => project.tracks.find((t) => String(t.id) === id) || fail('unknown_track', `No track ${id}`);
  let m;
  if (method === 'GET') {
    if (route === 'api/state') return state();
    if (route === 'api/history') return history();
    if (route === 'api/schema') return { params: {}, ops: {}, effects: {} };
    if (route === 'api/voicebanks') {
      return BANKS.map(({ aliases, hue, ...b }) => ({ ...b, image: true, aliases_count: aliases.size }));
    }
    if ((m = route.match(/^api\/tracks\/([^/]+)\/pitch$/))) return renderPitch(project, trackOf(m[1]));
    if ((m = route.match(/^api\/tracks\/([^/]+)\/phonemes$/))) return phonemes(project, trackOf(m[1]));
    if (route === 'api/reference/f0') return project.reference ? referenceF0() : fail('no_reference', 'No reference');
    if (route === 'api/reference/audio') return referenceAudio().slice(0);
    if ((m = route.match(/^api\/render\/(.+)\.wav$/))) return (renders.get(m[1]) || fail('not_found', 'Unknown render')).slice(0);
    if (route === 'api/export') return exportFile(q.get('format'));
  } else {
    if (route === 'api/ops') {
      if (body.base_revision != null && body.base_revision !== revision) {
        fail('revision_conflict', `The project is at revision ${revision}, not ${body.base_revision}`);
      }
      return commit(body.ops, from);
    }
    if (route === 'api/undo' || route === 'api/redo') {
      const isUndo = route === 'api/undo', [src, dst] = isUndo ? [undo, redo] : [redo, undo];
      if (!src.length) fail('empty', 'Nothing to ' + route.slice(4));
      const steps = Math.max(1, Math.floor(body?.steps ?? 1));
      let label = '';
      for (let i = 0; i < steps && src.length; i++) {
        const s = src.pop();
        dst.push({ project, label: s.label, source: s.source });
        project = s.project;
        label = s.label;
      }
      bump(`${isUndo ? 'Undo' : 'Redo'} ${label}`, from);
      return { revision };
    }
    if (route === 'api/checkpoint') {
      const name = String(body.name || '').trim() || fail('invalid', 'A checkpoint needs a name');
      checkpoints.delete(name);
      checkpoints.set(name, clone(project));
      emitEvent({ type: 'checkpoint', name, source: from.source, client: from.client });
      return history();
    }
    if (route === 'api/restore') {
      const snap = checkpoints.get(body.name) || fail('checkpoint.not_found', `No checkpoint \`${body.name}\``);
      const label = `Restore \`${body.name}\``;
      undo.push({ project, label, source: from.source });
      redo.length = 0;
      project = clone(snap);
      bump(label, from);
      return { revision, diagnostics: [] };
    }
    if (route === 'api/tune') return tune(body, from);
    if (route === 'api/render') return render(from);
    if (route === 'api/project/new') {
      project = { ...demoProject(), meta: { title: 'Untitled', artist: '', comment: '' }, sections: [], reference: null,
        tracks: [makeTrack({ name: 'Vocal', singer: 'kasane', phonemizer: 'jp_cv' })] };
      undo.length = redo.length = 0;
      checkpoints.clear();
      path = null;
      bump();
      dirty = false;
      return state();
    }
    if (route === 'api/project/save') { path = body.path || path || 'untitled.cantor'; dirty = false; bump(); dirty = false; return { path }; }
    if (route === 'api/project/open') fail('mock', 'The mock server cannot open files');
    if (route === 'api/import') fail('mock', 'The mock server cannot import files');
  }
  fail('not_found', `${method} ${route} is not mocked`);
}

// ---- scripted agent (Help ▸ Simulate agent session, mock only) -------------------------------
/** Replays what an MCP agent does in docs/agent-guide.md, as source "mcp", so the activity UI can be tried. */
export async function runAgentDemo(step = 900) {
  const from = { source: 'mcp', client: 'mock-agent' };
  const id = project.tracks[0]?.id;
  if (id == null) return;
  const tr = { id, get notes() { return project.tracks.find((t) => t.id === id)?.notes ?? []; },
    get singer() { return project.tracks.find((t) => t.id === id)?.singer; } };
  const call = (path, body) => dispatch('POST', path, body, from);
  const wait = () => new Promise((r) => setTimeout(r, step));
  const steps = [
    () => call('api/checkpoint', { name: 'draft' }),
    () => call('api/tune', { pass: 'auto_pitch', track: tr.id, params: { style: 'pop' } }),
    () => call('api/tune', { pass: 'auto_dynamics', track: tr.id, params: {} }),
    () => call('api/tune', { pass: 'breaths', track: tr.id, params: {} }),
    () => call('api/render', {}),
    () => {
      const bad = tr.notes.filter((n) => !BANKS.find((b) => b.id === tr.singer)?.aliases.has(n.alias ?? n.lyric) && !['+', '-'].includes(n.lyric));
      if (bad.length) call('api/ops', { ops: [{ op: 'batch', label: 'Use そ for missing ぞ', ops: [{ op: 'update_notes', track: tr.id, ids: bad.map((n) => n.id), patch: { alias: 'そ' } }] }] });
    },
    () => {
      const ph = phrasesOf(tr)[2];
      if (ph) call('api/tune', { pass: 'match_reference', track: tr.id, range: [ph[0].start, ph.at(-1).start + ph.at(-1).length], params: { strength: 0.7, smooth_ms: 40 } });
    },
    () => call('api/render', {}),
  ];
  for (const s of steps) {
    try { s(); } catch (e) { console.warn('mock agent:', e.message); }
    await wait();
  }
}
