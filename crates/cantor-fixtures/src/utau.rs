//! A synthetic UTAU voicebank for tests (CC0, generated, never downloaded).
//!
//! No small UTAU voicebank has a licence that clearly allows redistribution
//! and use in automated tests, so the tests generate one: a deterministic
//! source-filter voice (a band-limited glottal pulse through formant
//! resonators for vowels; shaped noise, bursts and murmurs for consonants),
//! laid out the way a human voicebank author would:
//!
//! ```text
//! character.txt  readme.txt  icon.bmp  prefix.map
//! oto.ini        CV aliases (あ か き ... ん) and a breath (息), sung at C4,
//!                each with a UTAU `*_wav.frq` pitch file
//! vcv/oto.ini    VCV strings (`- か`, `a か`, `a き`, `i か`, ...) and
//!                releases (`a R`), sung at C4, without .frq files
//! high/oto.ini   CV aliases of the vowels and k-row with suffix `↑`, sung
//!                at G4; prefix.map maps G4 and above to `↑`
//! ```
//!
//! Everything is generated bit-identically on every run.

use std::f64::consts::PI;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Sample rate of the generated samples.
pub const SAMPLE_RATE: u32 = 44_100;
/// Pitch of the top-level CV and the VCV samples, in cents (C4).
pub const RECORDED_CENTS: f64 = 6000.0;
/// Pitch of the `high/` tier, in cents (G4).
pub const HIGH_CENTS: f64 = 6700.0;
/// Alias suffix of the `high/` tier (from `prefix.map`).
pub const HIGH_SUFFIX: &str = "↑";
/// Lowest MIDI key `prefix.map` maps to [`HIGH_SUFFIX`] (G4).
pub const HIGH_FROM_KEY: u8 = 67;
/// Changes whenever the generated files change.
pub const VERSION: &str = "cantor-synthetic-3";

/// Silence before each CV sample's consonant, in seconds.
const LEAD_IN: f64 = 0.12;
/// Sustained vowel length of CV samples, in seconds.
const CV_VOWEL: f64 = 0.9;
/// Vowel length of each mora in the VCV strings, in seconds.
const VCV_VOWEL: f64 = 0.42;
/// Frame hop of the `.frq` files, in samples.
const FRQ_HOP: usize = 256;

/// Vowels and their first three formants (Hz).
const VOWELS: [(&str, [f64; 3]); 5] = [
    ("a", [800.0, 1250.0, 2600.0]),
    ("i", [300.0, 2250.0, 3000.0]),
    ("u", [350.0, 1250.0, 2350.0]),
    ("e", [480.0, 1850.0, 2550.0]),
    ("o", [480.0, 850.0, 2600.0]),
];

/// Kana rows: consonant (romaji) and the kana for a, i, u, e, o.
pub const ROWS: [(&str, [&str; 5]); 8] = [
    ("", ["あ", "い", "う", "え", "お"]),
    ("k", ["か", "き", "く", "け", "こ"]),
    ("s", ["さ", "し", "す", "せ", "そ"]),
    ("t", ["た", "ち", "つ", "て", "と"]),
    ("n", ["な", "に", "ぬ", "ね", "の"]),
    ("h", ["は", "ひ", "ふ", "へ", "ほ"]),
    ("m", ["ま", "み", "む", "め", "も"]),
    ("r", ["ら", "り", "る", "れ", "ろ"]),
];

/// Vowel sequence of every VCV string: each vowel follows an `a` once.
const VCV_PATTERN: [usize; 10] = [0, 0, 1, 0, 2, 0, 3, 0, 4, 0];

#[derive(Clone, Copy)]
enum Consonant {
    Stop {
        burst_hz: f64,
    },
    Fricative {
        low_hz: f64,
    },
    Aspirate,
    Nasal {
        formant: f64,
    },
    Flap,
    /// A short dip between two vowels (VCV vowel-to-vowel).
    Glottal,
}

impl Consonant {
    fn for_row(c: &str) -> Option<Self> {
        Some(match c {
            "k" => Consonant::Stop { burst_hz: 2000.0 },
            "t" => Consonant::Stop { burst_hz: 4500.0 },
            "s" => Consonant::Fricative { low_hz: 4500.0 },
            "h" => Consonant::Aspirate,
            "n" => Consonant::Nasal { formant: 280.0 },
            "m" => Consonant::Nasal { formant: 250.0 },
            "r" => Consonant::Flap,
            _ => return None,
        })
    }

    /// Length in seconds.
    fn secs(self) -> f64 {
        match self {
            Consonant::Stop { .. } => 0.07,
            Consonant::Fricative { .. } => 0.13,
            Consonant::Aspirate => 0.08,
            Consonant::Nasal { .. } => 0.07,
            Consonant::Flap => 0.03,
            Consonant::Glottal => 0.04,
        }
    }

    fn is_stop(self) -> bool {
        matches!(self, Consonant::Stop { .. })
    }
}

#[derive(Clone, Copy)]
enum Seg {
    Silence(f64),
    Cons(Consonant),
    /// Vowel index and length.
    Vowel(usize, f64),
    /// Syllabic nasal (ん).
    N(f64),
    /// An inhale.
    Breath(f64),
}

impl Seg {
    fn secs(self) -> f64 {
        match self {
            Seg::Silence(s) | Seg::Vowel(_, s) | Seg::N(s) | Seg::Breath(s) => s,
            Seg::Cons(c) => c.secs(),
        }
    }
}

/// A tiny deterministic noise source (xorshift).
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

/// Two-pole resonator (constant peak gain bandpass).
#[derive(Clone, Copy)]
struct Resonator {
    a1: f64,
    a2: f64,
    g: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn new(freq: f64, bandwidth: f64) -> Self {
        let mut r = Self {
            a1: 0.0,
            a2: 0.0,
            g: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        r.tune(freq, bandwidth);
        r
    }

    fn tune(&mut self, freq: f64, bandwidth: f64) {
        let sr = f64::from(SAMPLE_RATE);
        let r = (-PI * bandwidth / sr).exp();
        let theta = 2.0 * PI * freq / sr;
        self.a1 = 2.0 * r * theta.cos();
        self.a2 = -r * r;
        self.g = 1.0 - r;
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

const BANDWIDTHS: [f64; 3] = [80.0, 100.0, 140.0];

/// One period of a band-limited glottal pulse (harmonics with a
/// −12 dB/octave tilt), as a wavetable.
fn glottal_table(f0: f64) -> Vec<f64> {
    const N: usize = 4096;
    let nyquist = f64::from(SAMPLE_RATE) / 2.0;
    let harmonics = ((nyquist * 0.9) / (f0 * 1.003)).floor() as usize;
    (0..N)
        .map(|i| {
            let phase = 2.0 * PI * i as f64 / N as f64;
            (1..=harmonics)
                .map(|k| (k as f64 * phase).sin() / (k * k) as f64)
                .sum()
        })
        .collect()
}

/// A rendered utterance: samples, each segment's `(start, end)` in seconds
/// and the voiced F0 per [`FRQ_HOP`] frame (0 = unvoiced).
struct Utterance {
    samples: Vec<f32>,
    spans: Vec<(f64, f64)>,
    f0: Vec<f64>,
}

/// Sings a sequence of segments at `f0` Hz.
fn sing(segs: &[Seg], f0: f64, seed: u64) -> Utterance {
    let sr = f64::from(SAMPLE_RATE);
    let mut spans = Vec::with_capacity(segs.len());
    let mut t = 0.0;
    for s in segs {
        spans.push((t, t + s.secs()));
        t += s.secs();
    }
    let n = (t * sr).ceil() as usize;
    let table = glottal_table(f0);
    let mut noise = Noise(seed | 1);
    // Formant targets: the current vowel, else the next one, else the last.
    let vowel_after = |i: usize| -> usize {
        segs[i..]
            .iter()
            .find_map(|s| match s {
                Seg::Vowel(v, _) => Some(*v),
                Seg::N(_) => Some(2),
                _ => None,
            })
            .or_else(|| {
                segs[..i].iter().rev().find_map(|s| match s {
                    Seg::Vowel(v, _) => Some(*v),
                    _ => None,
                })
            })
            .unwrap_or(0)
    };
    let first = VOWELS[vowel_after(0)].1;
    let mut formants = first;
    let mut bank: Vec<Resonator> = first
        .iter()
        .zip(BANDWIDTHS)
        .map(|(&f, bw)| Resonator::new(f, bw))
        .collect();
    let mut burst = Resonator::new(2000.0, 1500.0);
    let mut fric = Resonator::new(6000.0, 3000.0);
    let mut nasal = Resonator::new(260.0, 60.0);
    let (mut amp, mut nasal_amp) = (0.0f64, 0.0f64);
    let amp_coeff = (-1.0 / (0.010 * sr)).exp();
    let formant_coeff = (-1.0 / (0.025 * sr)).exp();
    let mut phase = 0.0f64;
    let mut seg = 0usize;
    let mut out = Vec::with_capacity(n);
    let mut voiced_amp = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / sr;
        while seg + 1 < segs.len() && t >= spans[seg].1 {
            seg += 1;
        }
        let (s0, s1) = spans[seg];
        let u = ((t - s0) / (s1 - s0)).clamp(0.0, 1.0);
        let target = VOWELS[vowel_after(seg)].1;
        // Voiced amplitude, nasal murmur amplitude, noise into the vowel
        // bank, and noise output that bypasses it.
        let (amp_target, nasal_target, bank_noise, direct) = match segs[seg] {
            Seg::Silence(_) => (0.0, 0.0, 0.0, 0.0),
            Seg::Vowel(..) => (1.0, 0.0, 0.0, 0.0),
            Seg::N(_) => (0.12, 1.0, 0.0, 0.0),
            Seg::Breath(_) => (0.0, 0.0, 0.35 * (PI * u).sin(), 0.0),
            Seg::Cons(c) => match c {
                Consonant::Stop { burst_hz } => {
                    if s1 - t < 0.015 {
                        burst.tune(burst_hz, 1500.0);
                        (0.0, 0.0, 0.0, 1.5)
                    } else {
                        (0.0, 0.0, 0.0, 0.0)
                    }
                }
                Consonant::Fricative { low_hz } => {
                    fric.tune(low_hz + 1500.0, 3000.0);
                    (0.0, 0.0, 0.0, 1.2 * (PI * u).sin())
                }
                Consonant::Aspirate => (0.0, 0.0, 0.8 * (PI * u).sin(), 0.0),
                Consonant::Nasal { formant } => {
                    nasal.tune(formant, 60.0);
                    (0.0, (u / 0.2).min(1.0), 0.0, 0.0)
                }
                Consonant::Flap => (0.45, 0.0, 0.0, 0.0),
                Consonant::Glottal => (0.3, 0.0, 0.0, 0.0),
            },
        };
        amp = amp_target + amp_coeff * (amp - amp_target);
        nasal_amp = nasal_target + amp_coeff * (nasal_amp - nasal_target);
        if i % 32 == 0 {
            for (k, r) in bank.iter_mut().enumerate() {
                formants[k] = target[k] + formant_coeff.powi(32) * (formants[k] - target[k]);
                r.tune(formants[k], BANDWIDTHS[k]);
            }
        }
        // Slight natural jitter in the source keeps pitch trackers honest.
        let f = f0 * (1.0 + 0.002 * (2.0 * PI * 5.0 * t).sin());
        phase = (phase + f / sr).fract();
        let x = phase * table.len() as f64;
        let k = x as usize;
        let frac = x - k as f64;
        let source = table[k] * (1.0 - frac) + table[(k + 1) % table.len()] * frac;
        let wn = noise.next();
        let excitation = source * amp + wn * bank_noise;
        let mut s: f64 = bank.iter_mut().map(|r| r.tick(excitation)).sum::<f64>() * 3.0;
        if direct > 0.0 {
            let r = match segs[seg] {
                Seg::Cons(Consonant::Fricative { .. }) => &mut fric,
                _ => &mut burst,
            };
            s += r.tick(wn) * direct;
        }
        if nasal_amp > 1e-4 {
            s += nasal.tick(source) * nasal_amp * 0.3;
        }
        s += wn * 0.002 * amp;
        out.push((s * 0.3) as f32);
        voiced_amp.push(amp.max(nasal_amp * 0.5));
    }
    let frames = n.div_ceil(FRQ_HOP);
    let f0_frames = (0..frames)
        .map(|j| {
            let c = (j * FRQ_HOP + FRQ_HOP / 2).min(n - 1);
            if voiced_amp[c] > 0.08 { f0 } else { 0.0 }
        })
        .collect();
    Utterance {
        samples: out,
        spans,
        f0: f0_frames,
    }
}

fn cents_to_hz(cents: f64) -> f64 {
    440.0 * 2f64.powf((cents / 100.0 - 69.0) / 12.0)
}

/// Next seed of the deterministic sequence.
fn next_seed(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed
}

/// One oto.ini line, all times in seconds (written in ms).
#[allow(clippy::too_many_arguments)] // one parameter per oto field
fn oto_line(
    out: &mut String,
    file: &str,
    alias: &str,
    offset: f64,
    consonant: f64,
    cutoff: f64,
    pre: f64,
    overlap: f64,
) {
    let ms = |s: f64| s * 1000.0;
    let _ = writeln!(
        out,
        "{file}={alias},{:.1},{:.1},{:.1},{:.1},{:.1}",
        ms(offset),
        ms(consonant),
        ms(cutoff),
        ms(pre),
        ms(overlap)
    );
}

/// Writes a CV sample and its `.frq`, and appends its oto line.
#[allow(clippy::too_many_arguments)]
fn write_cv(
    dir: &Path,
    oto: &mut String,
    file_stem: &str,
    alias: &str,
    row: &str,
    vowel: usize,
    f0: f64,
    seed: u64,
) -> Result<(), String> {
    let cons = Consonant::for_row(row);
    let mut segs = vec![Seg::Silence(LEAD_IN)];
    segs.extend(cons.map(Seg::Cons));
    segs.push(Seg::Vowel(vowel, CV_VOWEL));
    segs.push(Seg::Silence(0.15));
    let u = sing(&segs, f0, seed);
    write_wav(&dir.join(format!("{file_stem}.wav")), &u.samples)?;
    write_frq(&dir.join(format!("{file_stem}_wav.frq")), &u.f0, f0)?;
    let onset = LEAD_IN + cons.map_or(0.0, Consonant::secs);
    let offset = LEAD_IN - 0.03;
    let pre = onset - offset;
    let overlap = if cons.is_some_and(Consonant::is_stop) {
        0.010
    } else {
        0.025
    };
    oto_line(
        oto,
        &format!("{file_stem}.wav"),
        alias,
        offset,
        pre + 0.06,
        -(pre + CV_VOWEL - 0.02),
        pre,
        overlap,
    );
    Ok(())
}

/// Writes the voicebank into `dir` (created if needed) and returns it.
/// See the module docs for the layout. Overwrites existing files.
pub fn write_synthetic_voicebank(dir: &Path) -> Result<PathBuf, String> {
    let mk = |d: &Path| std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()));
    mk(dir)?;
    mk(&dir.join("vcv"))?;
    mk(&dir.join("high"))?;
    let low = cents_to_hz(RECORDED_CENTS);
    let high = cents_to_hz(HIGH_CENTS);
    let mut seed = 0x5eed_u64;

    // Top level: CV at C4, ん, and a breath.
    let mut oto = String::new();
    for (row, kana) in ROWS {
        for (v, k) in kana.iter().enumerate() {
            write_cv(dir, &mut oto, k, k, row, v, low, next_seed(&mut seed))?;
        }
    }
    {
        let u = sing(
            &[Seg::Silence(LEAD_IN), Seg::N(CV_VOWEL), Seg::Silence(0.15)],
            low,
            next_seed(&mut seed),
        );
        write_wav(&dir.join("ん.wav"), &u.samples)?;
        write_frq(&dir.join("ん_wav.frq"), &u.f0, low)?;
        let offset = LEAD_IN - 0.03;
        oto_line(
            &mut oto,
            "ん.wav",
            "ん",
            offset,
            0.09,
            -(CV_VOWEL + 0.01),
            0.04,
            0.02,
        );
    }
    {
        let u = sing(
            &[Seg::Silence(0.05), Seg::Breath(0.4), Seg::Silence(0.1)],
            low,
            next_seed(&mut seed),
        );
        write_wav(&dir.join("息.wav"), &u.samples)?;
        oto_line(&mut oto, "息.wav", "息", 0.03, 0.12, -0.45, 0.07, 0.02);
    }
    write(&dir.join("oto.ini"), oto.as_bytes())?;

    // vcv/: one string per row, "- Ca", "a Ca", "a Ci", "i Ca", ... "a R".
    let mut oto = String::new();
    for (row, kana) in ROWS {
        let cons = Consonant::for_row(row);
        let mut segs = vec![Seg::Silence(0.15)];
        // Index into segs of each mora's (consonant, vowel).
        let mut morae = Vec::new();
        for (m, &v) in VCV_PATTERN.iter().enumerate() {
            let c = match cons {
                Some(c) => Some(c),
                None if m > 0 => Some(Consonant::Glottal),
                None => None,
            };
            let ci = c.map(|c| {
                segs.push(Seg::Cons(c));
                segs.len() - 1
            });
            segs.push(Seg::Vowel(v, VCV_VOWEL));
            morae.push((ci, segs.len() - 1, v));
        }
        segs.push(Seg::Silence(0.3));
        let u = sing(&segs, low, next_seed(&mut seed));
        let file = format!("_{}.wav", if row.is_empty() { "a" } else { row });
        write_wav(&dir.join("vcv").join(&file), &u.samples)?;
        let mut seen = std::collections::BTreeSet::new();
        let mut prev_v: Option<usize> = None;
        for &(ci, vi, v) in &morae {
            let (v_on, v_end) = u.spans[vi];
            let c_start = ci.map_or(v_on, |ci| u.spans[ci].0);
            let alias = match prev_v {
                None => format!("- {}", kana[v]),
                Some(p) => format!("{} {}", VOWELS[p].0, kana[v]),
            };
            if seen.insert(alias.clone()) {
                let (lead, overlap) = if prev_v.is_none() {
                    (0.06, 0.03)
                } else {
                    (0.12, 0.05)
                };
                let offset = c_start - lead;
                let pre = v_on - offset;
                oto_line(
                    &mut oto,
                    &file,
                    &alias,
                    offset,
                    pre + 0.08,
                    -((v_end - 0.04) - offset),
                    pre,
                    overlap,
                );
            }
            prev_v = Some(v);
        }
        // Release of the last vowel into silence.
        let (_, v_end) = u.spans[morae.last().expect("morae").1];
        let alias = format!("{} R", VOWELS[prev_v.unwrap_or(0)].0);
        if seen.insert(alias.clone()) {
            let offset = v_end - 0.15;
            oto_line(&mut oto, &file, &alias, offset, 0.15, -0.35, 0.10, 0.04);
        }
    }
    write(&dir.join("vcv").join("oto.ini"), oto.as_bytes())?;

    // high/: vowels and the k-row at G4 with the ↑ suffix.
    let mut oto = String::new();
    for (row, kana) in &ROWS[..2] {
        for (v, k) in kana.iter().enumerate() {
            let alias = format!("{k}{HIGH_SUFFIX}");
            write_cv(
                &dir.join("high"),
                &mut oto,
                k,
                &alias,
                row,
                v,
                high,
                next_seed(&mut seed),
            )?;
        }
    }
    write(&dir.join("high").join("oto.ini"), oto.as_bytes())?;

    // prefix.map: every key from C1 to B7; G4 and above sing the ↑ tier.
    let mut map = String::new();
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    for key in (24u8..=107).rev() {
        let name = format!(
            "{}{}",
            NAMES[usize::from(key % 12)],
            i32::from(key / 12) - 1
        );
        let suffix = if key >= HIGH_FROM_KEY {
            HIGH_SUFFIX
        } else {
            ""
        };
        let _ = writeln!(map, "{name}\t\t{suffix}");
    }
    write(&dir.join("prefix.map"), map.as_bytes())?;

    write(
        &dir.join("character.txt"),
        b"name=Cantor Synthetic\nimage=icon.bmp\nauthor=Cantor tests\nweb=https://github.com/apothecary103/Cantor\n",
    )?;
    write(
        &dir.join("readme.txt"),
        b"Cantor Synthetic: generated by cantor-fixtures for tests.\nCC0 1.0: no rights reserved.\n",
    )?;
    write(&dir.join("icon.bmp"), &bmp_2x2())?;
    Ok(dir.to_owned())
}

/// Writes the voicebank into `dir` unless a complete copy of this
/// [`VERSION`] is already there. Safe to call from parallel processes: the
/// bank is generated next to `dir` and renamed into place.
pub fn ensure_synthetic_voicebank(dir: &Path) -> Result<PathBuf, String> {
    let marker = dir.join(".cantor-fixture");
    if std::fs::read_to_string(&marker).is_ok_and(|v| v == VERSION) {
        return Ok(dir.to_owned());
    }
    let tmp = dir.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    write_synthetic_voicebank(&tmp)?;
    write(&tmp.join(".cantor-fixture"), VERSION.as_bytes())?;
    if dir.exists() {
        // An outdated (or concurrently written) copy: replace it unless
        // someone else just finished an up-to-date one.
        if std::fs::read_to_string(&marker).is_ok_and(|v| v == VERSION) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Ok(dir.to_owned());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
    if std::fs::rename(&tmp, dir).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        if !std::fs::read_to_string(&marker).is_ok_and(|v| v == VERSION) {
            return Err(format!(
                "{}: could not install the voicebank",
                dir.display()
            ));
        }
    }
    Ok(dir.to_owned())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// 16-bit mono PCM WAV.
fn write_wav(path: &Path, samples: &[f32]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    write(path, &bytes)
}

/// A UTAU `FREQ0003` pitch file: header, then (f0, amplitude) f64 pairs
/// per [`FRQ_HOP`] samples.
fn write_frq(path: &Path, f0: &[f64], average: f64) -> Result<(), String> {
    let mut b = Vec::with_capacity(40 + f0.len() * 16);
    b.extend_from_slice(b"FREQ0003");
    b.extend_from_slice(&(FRQ_HOP as i32).to_le_bytes());
    b.extend_from_slice(&average.to_le_bytes());
    b.extend_from_slice(&[0u8; 16]);
    b.extend_from_slice(&(f0.len() as i32).to_le_bytes());
    for &f in f0 {
        b.extend_from_slice(&f.to_le_bytes());
        b.extend_from_slice(&(if f > 0.0 { 1000.0f64 } else { 0.0 }).to_le_bytes());
    }
    write(path, &b)
}

/// A 2×2 24-bit BMP (the voicebank's icon).
fn bmp_2x2() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&70u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&2i32.to_le_bytes());
    b.extend_from_slice(&2i32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0u8; 24]);
    // Two rows of two BGR pixels, each row padded to 4 bytes.
    b.extend_from_slice(&[0x40, 0x80, 0xe0, 0x20, 0x20, 0x20, 0, 0]);
    b.extend_from_slice(&[0x20, 0x20, 0x20, 0x40, 0x80, 0xe0, 0, 0]);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voicebank_is_complete_and_deterministic() {
        let base = std::env::temp_dir().join(format!("cantor-fixture-test-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));
        write_synthetic_voicebank(&a).unwrap();
        ensure_synthetic_voicebank(&b).unwrap();
        ensure_synthetic_voicebank(&b).unwrap();
        let oto = std::fs::read_to_string(a.join("oto.ini")).unwrap();
        assert_eq!(oto.lines().count(), 5 * ROWS.len() + 2);
        assert!(oto.contains("か.wav=か,"));
        let vcv = std::fs::read_to_string(a.join("vcv/oto.ini")).unwrap();
        for alias in ["=- か,", "=a か,", "=a き,", "=i か,", "=a R,", "=o あ,"] {
            assert!(vcv.contains(alias), "{alias} in {vcv}");
        }
        let high = std::fs::read_to_string(a.join("high/oto.ini")).unwrap();
        assert!(high.contains("か.wav=か↑,"));
        let map = std::fs::read_to_string(a.join("prefix.map")).unwrap();
        assert!(map.contains("G4\t\t↑\n") && map.contains("F#4\t\t\n"));
        for f in [
            "oto.ini",
            "か.wav",
            "か_wav.frq",
            "vcv/_k.wav",
            "vcv/oto.ini",
            "high/こ.wav",
        ] {
            assert_eq!(
                std::fs::read(a.join(f)).unwrap(),
                std::fs::read(b.join(f)).unwrap(),
                "{f}"
            );
        }
        let _ = std::fs::remove_dir_all(base);
    }
}
