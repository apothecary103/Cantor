//! Rendering a project: place each note's sample the UTAU way, resample it
//! with TD-PSOLA onto the track's final pitch curve, crossfade, mix.
//!
//! Placement (docs/tuning.md): the sample starts `preutter` before the note
//! so its vowel lands on the beat, and fades in over `overlap` while the
//! previous note fades out. Velocity scales the fixed (consonant) region by
//! 2^((100 − vel)/100). When the preutterance would take more than half of
//! the previous note, preutterance and overlap shrink together.
//!
//! Each phrase is synthesized independently (in parallel) and cached by a
//! hash of everything that determines its audio, so editing one phrase
//! re-renders only that phrase, and output does not depend on the thread
//! count.

use crate::fx::{self, Stereo};
use crate::psola::{self, Analysis, SAMPLE_RATE};
use crate::voicebank::{OtoEntry, Voicebank};
use cantor_analysis::ReferenceF0;
use cantor_analysis::report::{
    MissingAlias, NoteSpan, RenderReport, TargetPitch, VoiceInput, render_report,
};
use cantor_core::params::known;
use cantor_core::pitch::render_pitch;
use cantor_core::{
    Diagnostic, NoteId, Phonemizer, PitchCurve, Project, TempoMap, Track, TrackId, phrases,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const SR: f64 = SAMPLE_RATE as f64;
/// Fade-out after a note that no other note follows, seconds.
const RELEASE: f64 = 0.06;
/// Notes closer than this (s) count as connected for crossfades.
const CONNECT: f64 = 0.03;
/// Grid of the expression curves fed to synthesis (and hashed), seconds.
const GRID: f64 = 0.005;
/// Cached phrases kept.
const CACHE_PHRASES: usize = 4096;

/// A note's sample as placed on the timeline.
#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    /// The note.
    pub note: NoteId,
    /// Resolved alias (the lyric when missing).
    pub alias: String,
    /// Whether no alias was found.
    pub missing: bool,
    /// Note onset (vowel onset), seconds.
    #[serde(skip)]
    pub onset: f64,
    /// When the sample starts, ms (onset − preutterance) — for the UI.
    pub start_ms: f64,
    /// Effective preutterance, ms.
    pub preutter_ms: f64,
    /// Effective overlap, ms.
    pub overlap_ms: f64,
    #[serde(skip)]
    entry: Option<OtoEntry>,
    /// End of the note (including `+` extensions), seconds.
    #[serde(skip)]
    end: f64,
    /// End of this sample's audio (next sample's start + overlap, or a release), seconds.
    #[serde(skip)]
    audio_end: f64,
    /// Fade-out length at `audio_end`, seconds.
    #[serde(skip)]
    fade_out: f64,
    /// Consonant time scale (velocity × compression).
    #[serde(skip)]
    k: f64,
    #[serde(skip)]
    index: usize,
}

/// Places a track's notes. Rests make no sound; `+` extends the previous
/// note; breaths use a `息`/`br`/`breath` alias when the bank has one.
pub fn place(
    track: &Track,
    tempo: &TempoMap,
    bank: Option<&Voicebank>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Placement> {
    let mut out: Vec<Placement> = Vec::new();
    let mut prev_lyric: Option<String> = None;
    if track.phonemizer == Phonemizer::EnArpasing {
        diags.push(Diagnostic::warning("phonemizer.unsupported",
            "The en_arpasing phonemizer is not implemented; lyrics are used as aliases unchanged").on_track(track.id));
    }
    let phonemizer = if track.phonemizer == Phonemizer::EnArpasing {
        Phonemizer::Raw
    } else {
        track.phonemizer
    };
    for (index, n) in track.notes.iter().enumerate() {
        let start = tempo.tick_to_sec(n.start);
        let end = tempo.tick_to_sec(n.end());
        if n.is_extension() {
            if let Some(p) = out.last_mut().filter(|p| (p.end - start).abs() < CONNECT) {
                p.end = end;
            }
            continue;
        }
        let breath = matches!(n.lyric.trim(), "br" | "息" | "breath");
        if n.is_rest_or_breath() && !breath {
            prev_lyric = None;
            continue;
        }
        let entry = match (&n.alias, bank) {
            (_, None) => None,
            (Some(a), Some(b)) => b.entries.get(a.as_str()),
            (None, Some(b)) if breath => ["息", "br", "breath", "breath_in"]
                .iter()
                .find_map(|a| b.entries.get(*a)),
            (None, Some(b)) => b.resolve(&n.lyric, n.key, prev_lyric.as_deref(), phonemizer),
        };
        if breath && entry.is_none() {
            prev_lyric = None;
            continue; // a breath the bank can't sing is a short rest
        }
        prev_lyric = Some(n.lyric.clone());
        let onset = start + n.timing.offset_ms / 1000.0;
        let vel = 2f64.powf((100.0 - n.vel.clamp(0.0, 200.0)) / 100.0);
        let (pre, ovl) = match entry {
            Some(e) => (
                n.timing.preutter_ms.map_or(e.preutter, |v| v / 1000.0),
                n.timing.overlap_ms.map_or(e.overlap, |v| v / 1000.0),
            ),
            None => (0.0, 0.0),
        };
        let (mut pre, mut ovl, mut k) = (pre.max(0.0) * vel, ovl * vel, vel);
        if let Some(p) = out.last().filter(|p| onset - p.end < CONNECT) {
            let room = (onset - p.onset) / 2.0;
            if pre > room && pre > 0.0 {
                let s = room / pre;
                pre *= s;
                ovl *= s;
                k *= s;
            }
        }
        let alias = entry.map_or_else(
            || n.alias.clone().unwrap_or_else(|| n.lyric.clone()),
            |e| e.alias.clone(),
        );
        out.push(Placement {
            note: n.id,
            missing: entry.is_none(),
            alias,
            onset,
            start_ms: (onset - pre) * 1000.0,
            preutter_ms: pre * 1000.0,
            overlap_ms: ovl * 1000.0,
            entry: entry.cloned(),
            end,
            audio_end: end,
            fade_out: RELEASE,
            k,
            index,
        });
    }
    // Each sample sounds until the next one has faded in.
    for i in 0..out.len() {
        let next = out
            .get(i + 1)
            .map(|q| (q.onset - q.preutter_ms / 1000.0, q.overlap_ms / 1000.0));
        let p = &mut out[i];
        match next {
            Some((ns, ov)) if ns - p.end < CONNECT => {
                p.fade_out = ov.max(0.005);
                p.audio_end = (ns + p.fade_out).max(p.onset + 0.01);
            }
            _ => {
                p.fade_out = RELEASE;
                p.audio_end = p.end + RELEASE;
            }
        }
    }
    out
}

/// One phrase's synthesis inputs; its hash is the cache key.
#[derive(Serialize)]
struct PhraseJob {
    bank: String,
    start: f64,
    len: usize,
    notes: Vec<NoteJob>,
}

#[derive(Serialize)]
struct NoteJob {
    wav: PathBuf,
    offset: f64,
    consonant: f64,
    cutoff: f64,
    preutter: f64,
    start: f64,
    audio_end: f64,
    fade_in: f64,
    fade_out: f64,
    k: f64,
    modulation: f64,
    vol: f64,
    /// Target pitch (cents) every GRID s from `start`; NaN = keep own.
    pitch: Vec<f32>,
    /// Loudness (dB) every GRID s.
    dyn_db: Vec<f32>,
    /// Formant factor every GRID s.
    formant: Vec<f32>,
}

/// Renders projects; holds the sample analyses and the phrase cache.
#[derive(Debug, Default)]
pub struct Synth {
    analyses: Mutex<HashMap<PathBuf, Arc<Analysis>>>,
    cache: Mutex<HashMap<blake3::Hash, Arc<Vec<f32>>>>,
}

/// What to render.
#[derive(Clone, Copy, Debug)]
pub struct RenderRequest<'a> {
    /// The project.
    pub project: &'a Project,
    /// Available voicebanks.
    pub banks: &'a [Arc<Voicebank>],
    /// Reference F0 in project time, for the report.
    pub reference: Option<&'a ReferenceF0>,
    /// Only these tracks (default: all, honoring mute/solo).
    pub tracks: Option<&'a [TrackId]>,
}

/// A finished render.
#[derive(Debug)]
pub struct Rendered {
    /// Interleaved stereo at [`SAMPLE_RATE`].
    pub samples: Vec<f32>,
    /// Numbers for agents.
    pub report: RenderReport,
    /// Problems (missing aliases, unsupported flags, ...).
    pub diagnostics: Vec<Diagnostic>,
    /// Phrases synthesized now.
    pub phrases_rendered: usize,
    /// Phrases taken from the cache.
    pub phrases_cached: usize,
}

impl Synth {
    /// A synth with empty caches.
    pub fn new() -> Self {
        Self::default()
    }

    fn analysis(&self, wav: &PathBuf) -> Result<Arc<Analysis>, String> {
        if let Some(a) = self.analyses.lock().expect("analysis lock").get(wav) {
            return Ok(a.clone());
        }
        let a = Arc::new(Analysis::load(wav)?);
        self.analyses
            .lock()
            .expect("analysis lock")
            .insert(wav.clone(), a.clone());
        Ok(a)
    }

    /// Renders the project.
    pub fn render(&self, req: &RenderRequest<'_>) -> Rendered {
        let t0 = std::time::Instant::now();
        let p = req.project;
        let tempo = p.tempo_map();
        let mut diags = Vec::new();
        let any_solo = p.tracks.iter().any(|t| t.mix.solo);
        let audible = |t: &Track| match req.tracks {
            Some(ids) => ids.contains(&t.id),
            None => !t.mix.mute && (!any_solo || t.mix.solo),
        };
        let (mut rendered, mut cached) = (0, 0);
        let mut missing = Vec::new();
        struct Voice {
            track: TrackId,
            mono: Vec<f32>,
            target: TargetPitch,
            notes: Vec<NoteSpan>,
            joins: Vec<f64>,
        }
        let mut voices: Vec<Voice> = Vec::new();
        let mut stereo_tracks: Vec<Stereo> = Vec::new();
        for t in &p.tracks {
            let bank = req.banks.iter().find(|b| b.id == t.singer).map(|b| &**b);
            if bank.is_none() && !t.notes.is_empty() {
                let msg = if t.singer.is_empty() {
                    "The track has no singer".to_owned()
                } else {
                    format!("Voicebank `{}` not found", t.singer)
                };
                diags.push(Diagnostic::warning("track.no_voicebank", msg).on_track(t.id));
            }
            let mut tdiags = Vec::new();
            let places = place(t, &tempo, bank, &mut tdiags);
            diags.extend(tdiags);
            for pl in places.iter().filter(|pl| pl.missing) {
                let lyric = t.notes[pl.index].lyric.clone();
                missing.push(MissingAlias {
                    note: pl.note,
                    lyric: lyric.clone(),
                });
                diags.push(
                    Diagnostic::warning("alias.missing", format!("No alias for `{lyric}`"))
                        .on_track(t.id)
                        .on_note(pl.note),
                );
            }
            unsupported(t, &mut diags);
            if !audible(t) {
                continue;
            }
            let curve = render_pitch(t, &tempo, None);
            // Phrase jobs.
            let phr = phrases(t);
            let mut jobs: Vec<(usize, PhraseJob)> = Vec::new();
            for (pi, ph) in phr.iter().enumerate() {
                let ps: Vec<&Placement> = places
                    .iter()
                    .filter(|pl| ph.notes.contains(&pl.index) && !pl.missing)
                    .collect();
                if ps.is_empty() {
                    continue;
                }
                let start = ps
                    .iter()
                    .map(|pl| pl.onset - pl.preutter_ms / 1000.0)
                    .fold(f64::INFINITY, f64::min)
                    .max(0.0);
                let end = ps.iter().map(|pl| pl.audio_end).fold(0.0, f64::max);
                let notes = ps
                    .iter()
                    .map(|pl| note_job(t, &tempo, &curve, pl))
                    .collect();
                jobs.push((
                    pi,
                    PhraseJob {
                        bank: format!(
                            "{}:{}",
                            bank.map_or("", |b| &b.id),
                            bank.map_or("", |b| &b.version)
                        ),
                        start,
                        len: ((end - start) * SR).ceil() as usize + 1,
                        notes,
                    },
                ));
            }
            let audio = self.run_jobs(&jobs, &mut rendered, &mut cached, &mut diags);
            let total = jobs
                .iter()
                .map(|(_, j)| (j.start * SR) as usize + j.len)
                .max()
                .unwrap_or(0);
            let mut mono = vec![0.0f32; total];
            for ((_, j), a) in jobs.iter().zip(&audio) {
                let at = (j.start * SR) as usize;
                for (i, v) in a.iter().enumerate() {
                    mono[at + i] += v;
                }
            }
            let notes: Vec<NoteSpan> = places
                .iter()
                .filter(|pl| !t.notes[pl.index].is_rest_or_breath())
                .map(|pl| NoteSpan {
                    note: pl.note,
                    lyric: t.notes[pl.index].lyric.clone(),
                    start_sec: pl.onset,
                    end_sec: pl.end,
                    phrase: phr
                        .iter()
                        .position(|ph| ph.notes.contains(&pl.index))
                        .unwrap_or(0),
                    articulated: true,
                })
                .collect();
            let joins = places
                .iter()
                .skip(1)
                .map(|pl| pl.onset - pl.preutter_ms / 1000.0)
                .collect();
            // Track mix: gain, pan (equal power), effects.
            let g = 10f64.powf(t.mix.gain_db / 20.0);
            let a = (t.mix.pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
            let (gl, gr) = (
                (g * a.cos() * std::f64::consts::SQRT_2) as f32,
                (g * a.sin() * std::f64::consts::SQRT_2) as f32,
            );
            let mut st = Stereo {
                l: mono.iter().map(|v| v * gl).collect(),
                r: mono.iter().map(|v| v * gr).collect(),
            };
            fx::process_chain(&t.effects, SAMPLE_RATE, &mut st);
            stereo_tracks.push(st);
            voices.push(Voice {
                track: t.id,
                mono,
                target: TargetPitch {
                    start_sec: curve.start_sec,
                    step_sec: curve.step_sec,
                    cents: curve.cents.clone(),
                    voiced: curve.voiced.clone(),
                },
                notes,
                joins,
            });
        }
        let len = stereo_tracks.iter().map(Stereo::len).max().unwrap_or(0);
        let mut samples = vec![0.0f32; len * 2];
        for st in &stereo_tracks {
            for i in 0..st.len() {
                samples[2 * i] += st.l[i];
                samples[2 * i + 1] += st.r[i];
            }
        }
        let inputs: Vec<VoiceInput<'_>> = voices
            .iter()
            .map(|v| VoiceInput {
                track: v.track,
                samples: &v.mono,
                sample_rate: SAMPLE_RATE,
                target: &v.target,
                notes: &v.notes,
                joins: &v.joins,
                reference: req.reference,
            })
            .collect();
        let mut report = render_report(&samples, 2, SAMPLE_RATE, &inputs);
        report.missing_aliases = missing;
        report.render_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Rendered {
            samples,
            report,
            diagnostics: diags,
            phrases_rendered: rendered,
            phrases_cached: cached,
        }
    }

    fn run_jobs(
        &self,
        jobs: &[(usize, PhraseJob)],
        rendered: &mut usize,
        cached: &mut usize,
        diags: &mut Vec<Diagnostic>,
    ) -> Vec<Arc<Vec<f32>>> {
        let keys: Vec<blake3::Hash> = jobs
            .iter()
            .map(|(_, j)| blake3::hash(&serde_json::to_vec(j).expect("phrase job serializes")))
            .collect();
        let mut out: Vec<Option<Arc<Vec<f32>>>> = {
            let c = self.cache.lock().expect("cache lock");
            keys.iter().map(|k| c.get(k).cloned()).collect()
        };
        let todo: Vec<usize> = (0..jobs.len()).filter(|&i| out[i].is_none()).collect();
        *cached += jobs.len() - todo.len();
        *rendered += todo.len();
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(todo.len().max(1));
        let results: Vec<(usize, Result<Vec<f32>, String>)> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|w| {
                    let todo = &todo;
                    s.spawn(move || {
                        todo.iter()
                            .skip(w)
                            .step_by(threads)
                            .map(|&i| (i, self.synth_phrase(&jobs[i].1)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("phrase thread"))
                .collect()
        });
        let mut c = self.cache.lock().expect("cache lock");
        if c.len() > CACHE_PHRASES {
            c.clear();
        }
        for (i, r) in results {
            let audio = Arc::new(r.unwrap_or_else(|e| {
                diags.push(Diagnostic::error("render.sample", e));
                vec![0.0; jobs[i].1.len]
            }));
            c.insert(keys[i], audio.clone());
            out[i] = Some(audio);
        }
        out.into_iter()
            .map(|a| a.expect("every phrase rendered"))
            .collect()
    }

    fn synth_phrase(&self, j: &PhraseJob) -> Result<Vec<f32>, String> {
        let mut buf = vec![0.0f32; j.len];
        for n in &j.notes {
            let a = self.analysis(&n.wav)?;
            let len = ((n.audio_end - n.start) * SR).max(1.0) as usize;
            let usable_end = if n.cutoff < 0.0 {
                n.offset - n.cutoff
            } else {
                a.secs() - n.cutoff
            };
            let fixed_out = n.consonant * n.k;
            let stretch_src = (usable_end - n.offset - n.consonant).max(0.01);
            let stretch_out = (len as f64 / SR - fixed_out).max(0.001);
            let ratio = (stretch_src / stretch_out).min(1.0);
            let grid = |v: &[f32], i: usize| {
                let x = i as f64 / SR / GRID;
                let a0 = (x.floor() as usize).min(v.len() - 1);
                let b0 = (a0 + 1).min(v.len() - 1);
                let u = x - a0 as f64;
                f64::from(v[a0]) * (1.0 - u) + f64::from(v[b0]) * u
            };
            let src_time = |i: usize| {
                let u = i as f64 / SR;
                if u < fixed_out {
                    n.offset + u / n.k
                } else {
                    n.offset + n.consonant + (u - fixed_out) * ratio
                }
            };
            let target = |i: usize| {
                let c = grid(&n.pitch, i);
                c.is_finite().then_some(c)
            };
            let formant = |i: usize| grid(&n.formant, i);
            let y = psola::synthesize(
                &a,
                &psola::Request {
                    len,
                    src_time: &src_time,
                    target_cents: &target,
                    modulation: n.modulation,
                    formant: &formant,
                },
            );
            let at = ((n.start - j.start) * SR).round().max(0.0) as usize;
            let fade_in = (n.fade_in * SR).max(1.0);
            let fade_out = (n.fade_out * SR).max(1.0);
            for (i, v) in y.iter().enumerate() {
                let Some(slot) = buf.get_mut(at + i) else {
                    break;
                };
                let env = (i as f64 / fade_in)
                    .min(1.0)
                    .min((len - i) as f64 / fade_out)
                    .max(0.0);
                let gain = n.vol * 10f64.powf(grid(&n.dyn_db, i) / 20.0);
                *slot += (f64::from(*v) * env * gain) as f32;
            }
        }
        Ok(buf)
    }
}

fn note_job(t: &Track, tempo: &TempoMap, curve: &PitchCurve, pl: &Placement) -> NoteJob {
    let e = pl.entry.as_ref().expect("placed notes have an entry");
    let n = &t.notes[pl.index];
    let start = pl.onset - pl.preutter_ms / 1000.0;
    let steps = ((pl.audio_end - start) / GRID).ceil() as usize + 2;
    let at = |i: usize| start + i as f64 * GRID;
    let curve_at = |id: &str, s: f64| {
        t.curves
            .get(id)
            .and_then(|c| c.value_at(tempo.sec_to_tick(s)))
            .unwrap_or(0.0)
    };
    let flag_g = parse_flag(&format!("{}{}", t.flags, n.flags), 'g').unwrap_or(0.0);
    NoteJob {
        wav: e.wav.clone(),
        offset: e.offset, // timing.offset_ms already moved the onset
        consonant: e.consonant.max(0.0),
        cutoff: e.cutoff,
        preutter: e.preutter,
        start,
        audio_end: pl.audio_end,
        fade_in: (pl.overlap_ms / 1000.0).max(0.005),
        fade_out: pl.fade_out,
        k: pl.k.max(0.05),
        modulation: (n.mod_ / 100.0).clamp(0.0, 1.0),
        vol: n.vol.clamp(0.0, 200.0) / 100.0,
        pitch: (0..steps)
            .map(|i| {
                let s = at(i);
                // Before the vowel the consonant keeps its own pitch.
                if s < pl.onset - 0.01 {
                    f32::NAN
                } else {
                    curve.at(s).map_or(f32::NAN, |c| c as f32)
                }
            })
            .collect(),
        dyn_db: (0..steps)
            .map(|i| curve_at(known::DYN, at(i)) as f32)
            .collect(),
        formant: (0..steps)
            .map(|i| {
                // + gen / + g = deeper: lower formants; ±100 ≈ ±3 semitones.
                let g = curve_at(known::GEN, at(i)) + flag_g;
                2f64.powf(-g / 100.0 * 0.25) as f32
            })
            .collect(),
    }
}

/// The number after a one-letter resampler flag ("g-5B40" → g = −5).
fn parse_flag(flags: &str, letter: char) -> Option<f64> {
    let i = flags.find(letter)?;
    let rest = &flags[i + 1..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+' || c == '.'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Diagnostics for what the synth does not render yet.
fn unsupported(t: &Track, diags: &mut Vec<Diagnostic>) {
    for id in [known::BRE, known::BRI, known::TONE] {
        if t.curves
            .get(id)
            .is_some_and(|c| c.points.iter().any(|p| p.1.abs() > 1e-9))
        {
            diags.push(
                Diagnostic::warning(
                    "param.not_rendered",
                    format!("The `{id}` curve is not rendered by the built-in resampler yet"),
                )
                .on_track(t.id),
            );
        }
    }
    let mut letters: Vec<char> = Vec::new();
    for f in std::iter::once(&t.flags).chain(t.notes.iter().map(|n| &n.flags)) {
        for c in f.chars().filter(|c| c.is_ascii_alphabetic()) {
            if c != 'g' && !letters.contains(&c) {
                letters.push(c);
            }
        }
    }
    if !letters.is_empty() {
        let l: String = letters.iter().collect();
        diags.push(
            Diagnostic::warning(
                "flags.unsupported",
                format!("Resampler flags `{l}` are ignored (only `g` is supported)"),
            )
            .on_track(t.id),
        );
    }
}

/// Phoneme placement of one track, for `GET /api/tracks/{id}/phonemes`.
pub fn phonemes(project: &Project, track: &Track, banks: &[Arc<Voicebank>]) -> Vec<Placement> {
    let bank = banks.iter().find(|b| b.id == track.singer).map(|b| &**b);
    place(track, &project.tempo_map(), bank, &mut Vec::new())
}

/// Encodes interleaved stereo f32 as a 16-bit PCM WAV.
pub fn wav_bytes(samples: &[f32], channels: u16) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).expect("in-memory wav");
        for s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
                .expect("in-memory wav");
        }
        w.finalize().expect("in-memory wav");
    }
    cur.into_inner()
}
