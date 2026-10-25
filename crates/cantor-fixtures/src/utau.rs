//! A synthetic UTAU CV voicebank for tests.
//!
//! No small UTAU voicebank has a licence that clearly allows redistribution
//! and use in automated tests, so the tests generate one: a deterministic
//! source-filter voice (a band-limited glottal pulse through formant
//! resonators for vowels; shaped noise, bursts and murmurs for consonants)
//! singing every CV syllable of a small Japanese-style inventory at one
//! pitch, with an `oto.ini` describing each sample the way a human
//! voicebank author would.

use std::f64::consts::PI;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Sample rate of the generated samples.
pub const SAMPLE_RATE: u32 = 44_100;
/// Pitch every sample is sung at, in cents (C4).
pub const RECORDED_CENTS: f64 = 6000.0;
/// Silence before each sample's consonant, in seconds.
const LEAD_IN: f64 = 0.12;
/// Sustained vowel length, in seconds.
const VOWEL: f64 = 0.9;

/// Vowels and their first three formants (Hz) with bandwidths.
const VOWELS: &[(&str, [f64; 3])] = &[
    ("a", [800.0, 1250.0, 2600.0]),
    ("i", [300.0, 2250.0, 3000.0]),
    ("u", [350.0, 1250.0, 2350.0]),
    ("e", [480.0, 1850.0, 2550.0]),
    ("o", [480.0, 850.0, 2600.0]),
];

/// Consonants: name, kind.
const CONSONANTS: &[(&str, Consonant)] = &[
    ("k", Consonant::Stop { burst_hz: 2000.0 }),
    ("t", Consonant::Stop { burst_hz: 4500.0 }),
    ("s", Consonant::Fricative { low_hz: 4500.0 }),
    ("h", Consonant::Aspirate),
    ("n", Consonant::Nasal { formant: 280.0 }),
    ("m", Consonant::Nasal { formant: 250.0 }),
    ("r", Consonant::Flap),
];

#[derive(Clone, Copy)]
enum Consonant {
    Stop { burst_hz: f64 },
    Fricative { low_hz: f64 },
    Aspirate,
    Nasal { formant: f64 },
    Flap,
}

impl Consonant {
    /// Length in seconds.
    fn secs(self) -> f64 {
        match self {
            Consonant::Stop { .. } => 0.07,
            Consonant::Fricative { .. } => 0.13,
            Consonant::Aspirate => 0.08,
            Consonant::Nasal { .. } => 0.07,
            Consonant::Flap => 0.03,
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
struct Resonator {
    a1: f64,
    a2: f64,
    g: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn new(freq: f64, bandwidth: f64) -> Self {
        let sr = f64::from(SAMPLE_RATE);
        let r = (-PI * bandwidth / sr).exp();
        let theta = 2.0 * PI * freq / sr;
        Self {
            a1: 2.0 * r * theta.cos(),
            a2: -r * r,
            g: 1.0 - r,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn formant_bank(formants: [f64; 3]) -> Vec<Resonator> {
    formants
        .iter()
        .zip([80.0, 100.0, 140.0])
        .map(|(&f, bw)| Resonator::new(f, bw))
        .collect()
}

/// Band-limited glottal source: harmonics with a −12 dB/octave tilt.
fn glottal(phase: f64, f0: f64) -> f64 {
    let nyquist = f64::from(SAMPLE_RATE) / 2.0;
    let mut s = 0.0;
    let mut k = 1.0;
    while k * f0 < nyquist * 0.9 {
        s += (k * phase).sin() / (k * k);
        k += 1.0;
    }
    s
}

/// Synthesizes one CV (or V) sample, returning samples and the vowel onset
/// in seconds.
fn syllable(consonant: Option<Consonant>, formants: [f64; 3], seed: u64) -> (Vec<f32>, f64) {
    let sr = f64::from(SAMPLE_RATE);
    let f0 = 440.0 * 2f64.powf((RECORDED_CENTS / 100.0 - 69.0) / 12.0);
    let c_secs = consonant.map_or(0.0, Consonant::secs);
    let onset = LEAD_IN + c_secs;
    let total = onset + VOWEL + 0.15;
    let n = (total * sr) as usize;
    let mut noise = Noise(seed | 1);
    let mut vowel_bank = formant_bank(formants);
    let mut cons_res = match consonant {
        Some(Consonant::Stop { burst_hz }) => Some(Resonator::new(burst_hz, 1500.0)),
        Some(Consonant::Fricative { low_hz }) => Some(Resonator::new(low_hz + 1500.0, 3000.0)),
        Some(Consonant::Nasal { formant }) => Some(Resonator::new(formant, 60.0)),
        _ => None,
    };
    let mut phase = 0.0;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / sr;
        // Slight natural jitter in the source keeps RMVPE honest.
        let f = f0 * (1.0 + 0.002 * (2.0 * PI * 5.0 * t).sin());
        phase += 2.0 * PI * f / sr;
        let source = glottal(phase, f);
        let mut s = 0.0;
        if t >= onset && t < onset + VOWEL {
            // Vowel with 20 ms attack and 40 ms release.
            let env = ((t - onset) / 0.02).min(1.0) * ((onset + VOWEL - t) / 0.04).min(1.0);
            let v: f64 = vowel_bank.iter_mut().map(|r| r.tick(source)).sum();
            s = v * env * 3.0 + noise.next() * 0.002 * env;
        } else if let Some(c) = consonant
            && t >= LEAD_IN
            && t < onset
        {
            let u = (t - LEAD_IN) / c_secs;
            s = match c {
                Consonant::Stop { .. } => {
                    // Closure (silence) then a 15 ms burst before the vowel.
                    if t > onset - 0.015 {
                        cons_res
                            .as_mut()
                            .expect("stop resonator")
                            .tick(noise.next())
                            * 1.5
                    } else {
                        0.0
                    }
                }
                Consonant::Fricative { .. } => {
                    let env = (PI * u).sin();
                    cons_res
                        .as_mut()
                        .expect("fricative resonator")
                        .tick(noise.next())
                        * env
                        * 1.2
                }
                Consonant::Aspirate => {
                    let env = (PI * u).sin() * 0.8;
                    let v: f64 = vowel_bank.iter_mut().map(|r| r.tick(noise.next())).sum();
                    v * env
                }
                Consonant::Nasal { .. } => {
                    let env = (u / 0.2).min(1.0);
                    cons_res.as_mut().expect("nasal resonator").tick(source) * env * 1.5
                }
                Consonant::Flap => {
                    let v: f64 = vowel_bank.iter_mut().map(|r| r.tick(source)).sum();
                    v * 0.5 * (PI * u).sin()
                }
            };
        }
        out.push((s * 0.3) as f32);
    }
    (out, onset)
}

/// Writes the voicebank into `dir` (created if needed) and returns it: one
/// WAV per syllable, `oto.ini`, `character.txt` and a `readme.txt` with the
/// licence (CC0; generated).
pub fn write_synthetic_voicebank(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut oto = String::new();
    let mut seed = 0x5eed_u64;
    let consonants = std::iter::once(None).chain(CONSONANTS.iter().map(|c| Some(*c)));
    for c in consonants {
        for &(v, formants) in VOWELS {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let alias = format!("{}{v}", c.map_or("", |c| c.0));
            let (samples, onset) = syllable(c.map(|c| c.1), formants, seed);
            let path = dir.join(format!("{alias}.wav"));
            write_wav(&path, &samples)?;
            // oto.ini (ms): offset, consonant (fixed region), cutoff
            // (negative: length from offset), preutterance, overlap.
            let offset = (LEAD_IN - 0.03) * 1000.0;
            let pre = onset * 1000.0 - offset;
            let fixed = pre + 60.0;
            let cutoff = -(pre + VOWEL * 1000.0 - 20.0);
            let overlap = if c.is_some_and(|c| matches!(c.1, Consonant::Stop { .. })) {
                10.0
            } else {
                25.0
            };
            let _ = writeln!(
                oto,
                "{alias}.wav={alias},{offset:.1},{fixed:.1},{cutoff:.1},{pre:.1},{overlap:.1}"
            );
        }
    }
    std::fs::write(dir.join("oto.ini"), oto).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("character.txt"),
        "name=Synthetic CV\nauthor=Cantor tests\nweb=https://github.com/apothecary103/Cantor\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("readme.txt"),
        "Generated by cantor-fixtures for tests. CC0 1.0: no rights reserved.\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(dir.to_owned())
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
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voicebank_is_complete_and_deterministic() {
        let a = std::env::temp_dir().join(format!("cantor-utau-a-{}", std::process::id()));
        let b = std::env::temp_dir().join(format!("cantor-utau-b-{}", std::process::id()));
        write_synthetic_voicebank(&a).unwrap();
        write_synthetic_voicebank(&b).unwrap();
        let oto = std::fs::read_to_string(a.join("oto.ini")).unwrap();
        assert_eq!(oto.lines().count(), 5 * (1 + CONSONANTS.len()));
        assert!(oto.contains("ka.wav=ka,"));
        for f in ["oto.ini", "ka.wav", "sa.wav", "o.wav"] {
            assert_eq!(
                std::fs::read(a.join(f)).unwrap(),
                std::fs::read(b.join(f)).unwrap()
            );
        }
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}
