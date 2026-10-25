//! The concatenative UTAU backend (docs/spec/40-algorithms.md §7).
//!
//! A UTAU voicebank is a folder of recorded syllables plus `oto.ini`, which
//! marks in each recording the offset, the fixed (consonant) region, the
//! cutoff, the preutterance (how far before the note onset the sample
//! starts) and the overlap (the crossfade with the previous note).
//!
//! Cantor maps the pipeline onto the [`Backend`] stages:
//! - `predict` resolves each lyric to an alias and places it: the vowel
//!   onset lands on the note onset; the consonant borrows time from the
//!   previous note, at most half of it (compressing preutterance and
//!   overlap proportionally when it does not fit).
//! - `acoustic` builds WORLD features for the phrase: each sample's
//!   spectral envelope (CheapTrick) and aperiodicity (D4C), analyzed with
//!   RMVPE F0, time-mapped (consonant kept or compressed, vowel stretched)
//!   and joined with equal-power crossfades, gA = cos(πu/2), gB = sin(πu/2),
//!   applied as gA², gB² to the power spectra. Energy, breathiness and
//!   gender curves act on these features.
//! - `vocode` is WORLD synthesis with the target F0, so pitch edits take
//!   the renderer's vocoder fast path, and `warp_timbre` shifts the
//!   envelope's frequency axis (a true formant shift) for doubling.

use cantor_analysis::F0Extractor;
use cantor_core::params::known;
use cantor_core::{Diagnostic, EntityRef};
use cantor_phon::{Lyric, parse_lyric};
use cantor_render::backend::{
    AcousticInput, Backend, BackendError, Grid, Phone, PhraseInput, Prediction,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Output sample rate.
pub const SAMPLE_RATE: u32 = 44_100;
/// Samples per frame (5.8 ms).
pub const HOP: usize = 256;
/// Spectral floor for silence (WORLD power spectrum units).
const SILENCE_SP: f64 = 1e-14;
/// Aperiodicity of silence.
const SILENCE_AP: f64 = 0.999;
/// Leading consonants may take at most this fraction of the previous note.
const MAX_BORROW: f64 = 0.5;
/// Separates an alias from its part (`c` consonant, `v` vowel) in phones.
const PART: char = '#';

/// One line of `oto.ini`, in seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct OtoEntry {
    /// The recording.
    pub wav: PathBuf,
    /// The alias lyrics refer to.
    pub alias: String,
    /// Start of the usable region.
    pub offset: f64,
    /// Length of the fixed (unstretched) region from `offset`.
    pub consonant: f64,
    /// Negative: length of the usable region from `offset`; positive:
    /// silence to skip at the end of the file.
    pub cutoff: f64,
    /// Time from `offset` to the vowel onset.
    pub preutter: f64,
    /// Crossfade with the previous note, from `offset`.
    pub overlap: f64,
}

/// Parses `oto.ini` text. Lines that cannot be read are returned as
/// warnings. Missing aliases default to the file stem.
pub fn parse_oto(text: &str, dir: &Path) -> (Vec<OtoEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let Some((file, rest)) = line.split_once('=') else {
            warnings.push(format!("oto.ini line {}: no `=`", i + 1));
            continue;
        };
        let fields: Vec<&str> = rest.split(',').collect();
        if fields.len() != 6 {
            warnings.push(format!("oto.ini line {}: expected 6 fields", i + 1));
            continue;
        }
        let num = |s: &str| s.trim().parse::<f64>().map(|ms| ms / 1000.0);
        let (Ok(offset), Ok(consonant), Ok(cutoff), Ok(preutter), Ok(overlap)) = (
            num(fields[1]),
            num(fields[2]),
            num(fields[3]),
            num(fields[4]),
            num(fields[5]),
        ) else {
            warnings.push(format!("oto.ini line {}: a number cannot be read", i + 1));
            continue;
        };
        let alias = if fields[0].trim().is_empty() {
            Path::new(file)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            fields[0].trim().to_owned()
        };
        out.push(OtoEntry {
            wav: dir.join(file.trim()),
            alias,
            offset,
            consonant,
            cutoff,
            preutter,
            overlap,
        });
    }
    (out, warnings)
}

/// Reads `oto.ini` as UTF-8, falling back to Shift-JIS (the UTAU default).
fn read_oto(path: &Path) -> Result<String, BackendError> {
    let bytes =
        std::fs::read(path).map_err(|e| BackendError(format!("{}: {e}", path.display())))?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(s),
        Err(e) => {
            let (text, _, _) = encoding_rs::SHIFT_JIS.decode(e.as_bytes());
            Ok(text.into_owned())
        }
    }
}

/// WORLD analysis of one recording on the backend's frame grid.
#[derive(Debug)]
struct Analysis {
    sp: Vec<Vec<f64>>,
    ap: Vec<Vec<f64>>,
    /// Length of the recording in seconds.
    secs: f64,
}

impl Analysis {
    /// Linearly interpolated features at `t` seconds into the recording.
    fn at(&self, t: f64, frame_secs: f64) -> (Vec<f64>, Vec<f64>) {
        let x = (t / frame_secs).clamp(0.0, (self.sp.len() - 1) as f64);
        let i = (x.floor() as usize).min(self.sp.len().saturating_sub(2));
        let u = x - i as f64;
        let j = (i + 1).min(self.sp.len() - 1);
        let lerp = |a: &[f64], b: &[f64]| -> Vec<f64> {
            a.iter()
                .zip(b)
                .map(|(&a, &b)| a * (1.0 - u) + b * u)
                .collect()
        };
        (
            lerp(&self.sp[i], &self.sp[j]),
            lerp(&self.ap[i], &self.ap[j]),
        )
    }
}

/// A UTAU voicebank.
pub struct Utau {
    root: PathBuf,
    id: String,
    version: String,
    grid: Grid,
    fft_size: usize,
    entries: BTreeMap<String, OtoEntry>,
    f0: Arc<dyn F0Extractor>,
    analyses: Mutex<HashMap<PathBuf, Arc<Analysis>>>,
}

impl std::fmt::Debug for Utau {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Utau")
            .field("root", &self.root)
            .field("aliases", &self.entries.len())
            .finish_non_exhaustive()
    }
}

/// Whether a directory looks like a UTAU voicebank: `oto.ini` at the top
/// or in an immediate subdirectory (multi-pitch banks).
pub fn is_voicebank(dir: &Path) -> bool {
    oto_files(dir).next().is_some()
}

fn oto_files(dir: &Path) -> impl Iterator<Item = PathBuf> {
    let top = dir.join("oto.ini");
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path().join("oto.ini")))
        .filter(|p| p.is_file())
        .collect();
    subs.sort();
    top.is_file().then_some(top).into_iter().chain(subs)
}

impl Utau {
    /// Loads a voicebank's `oto.ini` files. Recordings are analyzed lazily,
    /// with `f0` for pitch (RMVPE in Cantor).
    pub fn load(dir: &Path, f0: Arc<dyn F0Extractor>) -> Result<Self, BackendError> {
        let mut entries = BTreeMap::new();
        let mut hasher = blake3::Hasher::new();
        for oto in oto_files(dir) {
            let text = read_oto(&oto)?;
            hasher.update(text.as_bytes());
            let parent = oto.parent().unwrap_or(dir);
            let (list, warnings) = parse_oto(&text, parent);
            for w in warnings {
                tracing::warn!("{}: {w}", oto.display());
            }
            for e in list {
                // The first entry for an alias wins (top level before
                // subdirectories), as in UTAU.
                entries.entry(e.alias.clone()).or_insert(e);
            }
        }
        if entries.is_empty() {
            return Err(BackendError(format!(
                "{}: no oto.ini entries found",
                dir.display()
            )));
        }
        let option = world_rs::cheaptrick::initialize_cheaptrick_option(f64::from(SAMPLE_RATE));
        let fft_size = option.fft_size as usize;
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            root: dir.to_owned(),
            id: format!("utau/{name}"),
            version: format!(
                "oto:{}+world-rs:0.1.0+{}",
                &hasher.finalize().to_hex()[..16],
                f0.id()
            ),
            grid: Grid {
                sample_rate: SAMPLE_RATE,
                hop: HOP,
                mel_bins: 2 * (fft_size / 2 + 1),
            },
            fft_size,
            entries,
            f0,
            analyses: Mutex::new(HashMap::new()),
        })
    }

    /// The voicebank directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every alias.
    pub fn aliases(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The `oto.ini` entry a lyric sings, trying the lyric as typed and
    /// then lowercased.
    pub fn entry_for(&self, lyric: &str) -> Option<&OtoEntry> {
        let t = lyric.trim().trim_matches('-');
        self.entries
            .get(t)
            .or_else(|| self.entries.get(&t.to_lowercase()))
    }

    fn bins(&self) -> usize {
        self.fft_size / 2 + 1
    }

    fn frame_secs(&self) -> f64 {
        self.grid.frame_secs()
    }

    /// WORLD analysis of a recording, cached for the process.
    fn analysis(&self, wav: &Path) -> Result<Arc<Analysis>, BackendError> {
        if let Some(a) = self.analyses.lock().expect("analysis lock").get(wav) {
            return Ok(a.clone());
        }
        let err = |m: String| BackendError(format!("{}: {m}", wav.display()));
        let (x, sr) = read_mono(wav).map_err(err)?;
        let x: Vec<f64> = if sr == SAMPLE_RATE {
            x.iter().map(|&s| f64::from(s)).collect()
        } else {
            world_rs::resample::resample(&x, sr, SAMPLE_RATE)
                .into_iter()
                .map(f64::from)
                .collect()
        };
        let x32: Vec<f32> = x.iter().map(|&s| s as f32).collect();
        let track = self.f0.extract(&x32, SAMPLE_RATE).map_err(err)?;
        let fs = f64::from(SAMPLE_RATE);
        let frame_secs = self.frame_secs();
        let frames = (x.len() as f64 / fs / frame_secs).ceil() as usize + 1;
        let positions: Vec<f64> = (0..frames).map(|i| i as f64 * frame_secs).collect();
        let f0: Vec<f64> = positions
            .iter()
            .map(|&t| match track.frame_at(t) {
                Some(i) if track.voiced[i] => f64::from(track.hz[i]),
                _ => 0.0,
            })
            .collect();
        let option = world_rs::cheaptrick::initialize_cheaptrick_option(fs);
        let sp = world_rs::cheaptrick::cheaptrick(&x, fs, &positions, &f0, &option)
            .map_err(|e| err(format!("CheapTrick: {e:?}")))?;
        let ap = world_rs::d4c::d4c(
            &x,
            fs,
            &positions,
            &f0,
            self.fft_size as i32,
            &world_rs::d4c::initialize_d4c_option(),
        )
        .map_err(|e| err(format!("D4C: {e:?}")))?;
        let a = Arc::new(Analysis {
            sp,
            ap,
            secs: x.len() as f64 / fs,
        });
        self.analyses
            .lock()
            .expect("analysis lock")
            .insert(wav.to_owned(), a.clone());
        Ok(a)
    }
}

/// Reads a WAV as mono f32 (first channel).
fn read_mono(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let ch = usize::from(spec.channels.max(1));
    let all: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 2f32.powi(i32::from(spec.bits_per_sample) - 1);
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    Ok((all.into_iter().step_by(ch).collect(), spec.sample_rate))
}

/// One placed note: where its sample starts, where its vowel lands, and
/// how much of the preutterance and overlap survived compression.
#[derive(Clone, Debug)]
struct Placed {
    alias: String,
    /// Frame where the sample (its consonant) starts.
    start: i64,
    /// Frame of the note onset (vowel onset).
    onset: i64,
    /// Frame where the note's own phones end.
    end: i64,
}

impl Backend for Utau {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn grid(&self) -> Grid {
        self.grid
    }

    fn params(&self) -> &[&'static str] {
        &[
            known::PITCH,
            known::ENERGY,
            known::BREATHINESS,
            known::GENDER,
        ]
    }

    fn param_default(&self, _: &str) -> f32 {
        0.0
    }

    fn param_range(&self, param: &str) -> (f32, f32) {
        cantor_core::params::lookup(param).map_or((0.0, 0.0), |d| (d.min as f32, d.max as f32))
    }

    fn predict(&self, input: &PhraseInput<'_>) -> Result<Prediction, BackendError> {
        let fs = self.frame_secs();
        let frame = |t: f64| ((t - input.t0) / fs).round() as i64;
        let mut diagnostics = Vec::new();
        let mut placed: Vec<Placed> = Vec::new();
        let mut prev_end_sec = input.t0;
        let mut prev_len: Option<f64> = None;
        for n in &input.phrase.notes {
            let target = Some(EntityRef::Note { id: n.id });
            if n.phonemes.is_some() {
                diagnostics.push(Diagnostic::warning(
                    "utau.phonemes_unsupported",
                    "UTAU voicebanks sing recorded aliases; phoneme overrides are ignored",
                    target.clone(),
                ));
            }
            let alias = match parse_lyric(&n.lyric) {
                Lyric::Continue => {
                    if let Some(last) = placed.last_mut() {
                        last.end = frame(n.end_sec);
                    }
                    prev_end_sec = n.end_sec;
                    continue;
                }
                Lyric::Rest => None,
                Lyric::Breath => {
                    let e = self.entry_for("br").or_else(|| self.entry_for("息"));
                    if e.is_none() {
                        diagnostics.push(Diagnostic::warning(
                            "utau.no_breath",
                            "the voicebank has no breath sample (`br` or `息`); the note is silent",
                            target.clone(),
                        ));
                    }
                    e
                }
                Lyric::Syllable(_) => {
                    let e = self.entry_for(&n.lyric);
                    if e.is_none() {
                        diagnostics.push(Diagnostic::warning(
                            "phon.unknown_lyric",
                            format!(
                                "`{}` is not an alias in the voicebank; the note is silent",
                                n.lyric
                            ),
                            target.clone(),
                        ));
                    }
                    e
                }
            };
            let len = n.end_sec - n.start_sec;
            if let Some(e) = alias {
                // The consonant borrows from the previous note (at most
                // half of it) or from the silence before.
                let gap = n.start_sec - prev_end_sec;
                let room = match prev_len {
                    Some(l) if gap < 1e-6 => MAX_BORROW * l,
                    _ => gap.max(0.0),
                };
                let pre = e.preutter.min(room).max(0.0);
                if pre + 1e-9 < e.preutter && room < 0.010 {
                    diagnostics.push(Diagnostic::warning(
                        "phon.collision",
                        format!(
                            "`{}` needs {:.0} ms before its onset; only {:.0} ms are free",
                            e.alias,
                            e.preutter * 1000.0,
                            room * 1000.0
                        ),
                        target,
                    ));
                }
                placed.push(Placed {
                    alias: e.alias.clone(),
                    start: frame(n.start_sec - pre),
                    onset: frame(n.start_sec),
                    end: frame(n.end_sec),
                });
            }
            prev_end_sec = n.end_sec;
            prev_len = alias.map(|_| len);
        }

        // Phones cover every frame: SP, then per note `alias#c` (consonant,
        // possibly empty) and `alias#v` up to the next note's start.
        let total = input.frames as i64;
        let mut phones = Vec::new();
        let mut cursor = 0i64;
        let push = |phones: &mut Vec<Phone>, symbol: String, frames: i64| {
            if frames > 0 {
                phones.push(Phone { symbol, frames });
            }
        };
        for (i, p) in placed.iter().enumerate() {
            let start = p.start.clamp(cursor, total);
            push(&mut phones, "SP".into(), start - cursor);
            let onset = p.onset.clamp(start, total);
            let next_start = placed
                .get(i + 1)
                .map_or(total, |n| n.start.clamp(onset, total));
            let end = p.end.clamp(onset, next_start);
            push(&mut phones, format!("{}{PART}c", p.alias), onset - start);
            push(&mut phones, format!("{}{PART}v", p.alias), end - onset);
            cursor = end;
        }
        push(&mut phones, "SP".into(), total - cursor);
        Ok(Prediction {
            phones,
            pitch: None,
            variance: BTreeMap::new(),
            diagnostics,
        })
    }

    fn acoustic(&self, input: &AcousticInput<'_>) -> Result<Vec<f32>, BackendError> {
        let bins = self.bins();
        let fs = self.frame_secs();
        let frames: usize = input.phones.iter().map(|p| p.frames.max(0) as usize).sum();

        // Segments: (alias, consonant frames, start, onset, vowel end).
        struct Seg<'a> {
            entry: &'a OtoEntry,
            start: usize,
            onset: usize,
            end: usize,
        }
        let mut segs: Vec<Seg<'_>> = Vec::new();
        let mut at = 0usize;
        for p in input.phones {
            let n = p.frames.max(0) as usize;
            if let Some((alias, part)) = p.symbol.rsplit_once(PART) {
                let entry = self
                    .entries
                    .get(alias)
                    .ok_or_else(|| BackendError(format!("unknown alias `{alias}`")))?;
                match part {
                    "c" => segs.push(Seg {
                        entry,
                        start: at,
                        onset: at + n,
                        end: at + n,
                    }),
                    _ => match segs.last_mut() {
                        Some(s) if s.onset == at && s.entry.alias == alias => s.end = at + n,
                        _ => segs.push(Seg {
                            entry,
                            start: at,
                            onset: at,
                            end: at + n,
                        }),
                    },
                }
            }
            at += n;
        }

        let mut sp_acc = vec![0.0f64; frames * bins];
        let mut ap_acc = vec![0.0f64; frames * bins];
        let mut w_acc = vec![0.0f64; frames];
        for (k, s) in segs.iter().enumerate() {
            let e = s.entry;
            let a = self.analysis(&e.wav)?;
            // Compression of the consonant when it had to be shortened.
            let pre_out = (s.onset - s.start) as f64 * fs;
            let c_scale = if pre_out > 0.0 && e.preutter > 0.0 {
                e.preutter / pre_out
            } else {
                1.0
            };
            let fade_in = ((e.overlap / c_scale) / fs).round() as usize;
            // This note keeps sounding under the next note's overlap.
            let next = segs.get(k + 1).filter(|n| n.start <= s.end + 1);
            let fade_out = next.map_or(0, |n| {
                let ns = if n.onset > n.start && n.entry.preutter > 0.0 {
                    n.entry.preutter / ((n.onset - n.start) as f64 * fs)
                } else {
                    1.0
                };
                ((n.entry.overlap / ns) / fs).round() as usize
            });
            let stop = (s.end + fade_out).min(frames);
            let usable_end = if e.cutoff < 0.0 {
                e.offset - e.cutoff
            } else {
                a.secs - e.cutoff
            };
            let fixed_after = (e.consonant - e.preutter).max(0.0);
            let stretch_src = (usable_end - (e.offset + e.consonant)).max(fs);
            let vowel_out = (stop - s.onset) as f64 * fs;
            // `j` indexes several per-frame arrays at once.
            #[allow(clippy::needless_range_loop)]
            for j in s.start..stop {
                let tau = (j - s.start) as f64 * fs;
                let t_src = if j < s.onset {
                    e.offset + tau * c_scale
                } else {
                    let tv = (j - s.onset) as f64 * fs;
                    if tv < fixed_after || vowel_out <= fixed_after {
                        e.offset + e.preutter + tv.min(fixed_after)
                    } else {
                        let u = (tv - fixed_after) / (vowel_out - fixed_after).max(fs);
                        e.offset + e.consonant + u * stretch_src
                    }
                };
                let (sp, ap) = a.at(t_src, fs);
                let u_in = if fade_in > 0 && j < s.start + fade_in {
                    (j - s.start) as f64 / fade_in as f64
                } else {
                    1.0
                };
                let u_out = if fade_out > 0 && j >= stop - fade_out {
                    (stop - j) as f64 / fade_out as f64
                } else {
                    1.0
                };
                // Equal-power gains, applied to power spectra as g².
                let g_in = (std::f64::consts::FRAC_PI_2 * u_in).sin();
                let g_out = (std::f64::consts::FRAC_PI_2 * (1.0 - u_out)).cos();
                let w = (g_in * g_out).powi(2);
                let row = j * bins;
                for b in 0..bins {
                    sp_acc[row + b] += w * sp[b];
                    ap_acc[row + b] += w * ap[b];
                }
                w_acc[j] += w;
            }
        }

        let curve = |param: &str, j: usize| -> f64 {
            input
                .variance
                .get(param)
                .and_then(|v| v.get(j))
                .map_or(0.0, |&x| f64::from(x))
        };
        let mut out = vec![0.0f32; frames * 2 * bins];
        let mut sp_row = vec![0.0f64; bins];
        for j in 0..frames {
            let w = w_acc[j].min(1.0);
            let norm = if w_acc[j] > 1.0 { 1.0 / w_acc[j] } else { 1.0 };
            for b in 0..bins {
                sp_row[b] = sp_acc[j * bins + b] * norm + (1.0 - w) * SILENCE_SP;
            }
            let gender = curve(known::GENDER, j);
            if gender != 0.0 {
                sp_row = crate::world::warp(&sp_row, 2f64.powf(gender / 12.0));
            }
            let energy = 10f64.powf(curve(known::ENERGY, j) / 10.0);
            let breath = 10f64.powf(curve(known::BREATHINESS, j) / 20.0);
            let row = &mut out[j * 2 * bins..(j + 1) * 2 * bins];
            for b in 0..bins {
                row[b] = (sp_row[b] * energy).max(SILENCE_SP) as f32;
                let ap = ap_acc[j * bins + b] * norm + (1.0 - w) * SILENCE_AP;
                row[bins + b] = (ap * breath).clamp(0.001, SILENCE_AP) as f32;
            }
        }
        Ok(out)
    }

    fn vocode(&self, features: &[f32], f0_hz: &[f32]) -> Result<Vec<f32>, BackendError> {
        crate::world::synthesize(features, self.fft_size, f0_hz, HOP, SAMPLE_RATE)
    }

    fn warp_timbre(&self, features: &[f32], factor: f32) -> Option<Vec<f32>> {
        Some(crate::world::warp_features(
            features,
            self.fft_size,
            f64::from(factor),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oto_lines() {
        let (e, w) = parse_oto(
            "ka.wav=ka,90,120,-950,60,10\n_a.wav=,80,100,-900,30,15\nbad line\n",
            Path::new("/vb"),
        );
        assert_eq!(w.len(), 1);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].alias, "ka");
        assert!((e[0].preutter - 0.060).abs() < 1e-12);
        assert_eq!(e[1].alias, "_a", "empty alias uses the file stem");
        assert_eq!(e[1].wav, Path::new("/vb/_a.wav"));
    }
}
