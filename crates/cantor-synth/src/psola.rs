//! Recording analysis and pitch-synchronous overlap-add (TD-PSOLA).
//!
//! A recording is analyzed once: mono at [`SAMPLE_RATE`], F0 from its UTAU
//! `*_wav.frq` file or YIN, and pitch marks one period apart, each placed on
//! the waveform peak near where the previous mark predicts (so successive
//! grains stay in phase). Unvoiced stretches get marks every 5 ms.
//!
//! Synthesis places output marks one *target* period apart. Each takes the
//! analysis mark nearest the mapped source time and overlap-adds its grain
//! (a Hann window two source periods wide). Grains are not resampled, so
//! formants stay where they are; a formant factor ≠ 1 reads grains faster
//! or slower (the `g` flag and the `gen` curve).

use cantor_analysis::Yin;
use std::path::Path;

/// Output sample rate.
pub const SAMPLE_RATE: u32 = 44_100;
const SR: f64 = SAMPLE_RATE as f64;
/// Mark spacing in unvoiced stretches, samples (5 ms).
const UNVOICED_PERIOD: f64 = SR * 0.005;

/// One pitch mark.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    /// Sample index.
    pub center: usize,
    /// Local period in samples.
    pub period: f64,
    /// Whether the region is voiced.
    pub voiced: bool,
}

/// An analyzed recording.
#[derive(Debug)]
pub struct Analysis {
    /// Mono samples at [`SAMPLE_RATE`].
    pub samples: Vec<f32>,
    /// Pitch marks in time order.
    pub marks: Vec<Mark>,
    /// Median voiced pitch in cents (MIDI × 100), if anything is voiced.
    pub median_cents: Option<f64>,
}

impl Analysis {
    /// Length in seconds.
    pub fn secs(&self) -> f64 {
        self.samples.len() as f64 / SR
    }

    /// Reads and analyzes a WAV file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let (raw, sr) = read_mono(path)?;
        let samples = if sr == SAMPLE_RATE {
            raw
        } else {
            resample(&raw, sr)
        };
        let hop = 0.005;
        let n = (samples.len() as f64 / (hop * SR)).ceil() as usize + 1;
        let times: Vec<f64> = (0..n).map(|i| i as f64 * hop).collect();
        let f0 = read_frq(path, &times).unwrap_or_else(|| {
            let t = Yin::for_samples().track(&samples, SAMPLE_RATE);
            times
                .iter()
                .map(|&s| match t.frame_at(s) {
                    Some(i) if t.voiced[i] => f64::from(t.hz[i]),
                    _ => 0.0,
                })
                .collect()
        });
        Ok(Self::from_f0(samples, &f0, hop))
    }

    /// Analysis from samples and an F0 track (Hz, 0 = unvoiced) at `hop` s.
    pub fn from_f0(samples: Vec<f32>, f0: &[f64], hop: f64) -> Self {
        let f0_at = |i: usize| {
            f0.get(((i as f64 / SR) / hop).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let mut marks = Vec::new();
        let mut voiced_cents = Vec::new();
        let mut pos = 0.0f64;
        let mut prev_voiced = false;
        while (pos as usize) < samples.len() {
            let i = pos as usize;
            let hz = f0_at(i);
            if hz > 40.0 {
                let period = SR / hz;
                // Snap to the waveform peak within ±¼ period (the first mark of
                // a voiced run searches a whole period).
                let reach = if prev_voiced {
                    period * 0.25
                } else {
                    period * 0.5
                };
                let lo = (pos - reach).max(0.0) as usize;
                let hi = ((pos + reach) as usize).min(samples.len() - 1);
                let center = (lo..=hi)
                    .max_by(|&a, &b| samples[a].total_cmp(&samples[b]))
                    .unwrap_or(i);
                marks.push(Mark {
                    center,
                    period,
                    voiced: true,
                });
                voiced_cents.push(1200.0 * (hz / 440.0).log2() + 6900.0);
                pos = center as f64 + period;
                prev_voiced = true;
            } else {
                marks.push(Mark {
                    center: i,
                    period: UNVOICED_PERIOD,
                    voiced: false,
                });
                pos += UNVOICED_PERIOD;
                prev_voiced = false;
            }
        }
        voiced_cents.sort_by(f64::total_cmp);
        let median_cents = voiced_cents.get(voiced_cents.len() / 2).copied();
        Self {
            samples,
            marks,
            median_cents,
        }
    }

    /// Index of the mark nearest sample position `x`.
    fn nearest(&self, x: f64) -> Option<usize> {
        if self.marks.is_empty() {
            return None;
        }
        let i = self.marks.partition_point(|m| (m.center as f64) < x);
        let cand = [i.saturating_sub(1), i.min(self.marks.len() - 1)];
        cand.into_iter().min_by(|&a, &b| {
            (self.marks[a].center as f64 - x)
                .abs()
                .total_cmp(&(self.marks[b].center as f64 - x).abs())
        })
    }
}

/// What to synthesize from one recording.
#[allow(missing_debug_implementations)] // closures
pub struct Request<'a> {
    /// Output length in samples.
    pub len: usize,
    /// Source time (s) for output sample `n`.
    pub src_time: &'a dyn Fn(usize) -> f64,
    /// Target pitch in cents for output sample `n` (`None`: keep the
    /// recording's own pitch).
    pub target_cents: &'a dyn Fn(usize) -> Option<f64>,
    /// Fraction (0..1) of the recording's own pitch wobble kept (`mod`).
    pub modulation: f64,
    /// Formant factor for output sample `n`: > 1 raises formants.
    pub formant: &'a dyn Fn(usize) -> f64,
}

/// TD-PSOLA synthesis into a new buffer of `req.len` samples.
pub fn synthesize(a: &Analysis, req: &Request<'_>) -> Vec<f32> {
    let mut out = vec![0.0f32; req.len];
    let mut t = 0.0f64;
    while (t as usize) < req.len {
        let n = t as usize;
        let src = (req.src_time)(n) * SR;
        let Some(mi) = a.nearest(src) else { break };
        let m = a.marks[mi];
        let p_src = m.period;
        let p_out = match (m.voiced, (req.target_cents)(n)) {
            (true, Some(c)) => {
                let own = 1200.0 * ((SR / p_src) / 440.0).log2() + 6900.0;
                let wobble = a
                    .median_cents
                    .map_or(0.0, |med| (own - med) * req.modulation);
                SR / (440.0 * 2f64.powf((c + wobble - 6900.0) / 1200.0))
            }
            _ => p_src,
        };
        let f = (req.formant)(n).clamp(0.5, 2.0);
        // Overlapping grains two source periods wide, spaced p_out apart,
        // sum to p_src / p_out; scale back to unity.
        let gain = (p_out / p_src).min(2.0) as f32;
        let half = (p_src / f).max(2.0);
        let lo = (t - half).ceil() as i64;
        let hi = (t + half).floor() as i64;
        for k in lo..=hi {
            if k < 0 || k as usize >= out.len() {
                continue;
            }
            let d = k as f64 - t;
            let w = 0.5 + 0.5 * (std::f64::consts::PI * d / half).cos();
            let s = sample_at(&a.samples, m.center as f64 + d * f);
            out[k as usize] += (w * s) as f32 * gain;
        }
        t += p_out.max(8.0);
    }
    out
}

/// Cubic (Catmull-Rom) interpolation; zero outside the buffer.
fn sample_at(x: &[f32], pos: f64) -> f64 {
    if pos < 0.0 || pos >= (x.len().saturating_sub(1)) as f64 {
        return 0.0;
    }
    let i = pos.floor() as usize;
    let u = pos - i as f64;
    let g = |j: isize| f64::from(x[(i as isize + j).clamp(0, x.len() as isize - 1) as usize]);
    let (y0, y1, y2, y3) = (g(-1), g(0), g(1), g(2));
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * u + c2) * u + c1) * u + y1
}

fn resample(x: &[f32], from: u32) -> Vec<f32> {
    let ratio = f64::from(from) / SR;
    let n = (x.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| sample_at(x, i as f64 * ratio) as f32)
        .collect()
}

/// Reads a WAV file as mono f32 (first channel) and its sample rate.
pub fn read_mono(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let (all, ch, sr) = read_wav(path)?;
    Ok((all.into_iter().step_by(ch).collect(), sr))
}

/// Reads a WAV file: interleaved samples, channels, sample rate.
pub fn read_wav(path: &Path) -> Result<(Vec<f32>, usize, u32), String> {
    let err = |e: hound::Error| format!("{}: {e}", path.display());
    let mut r = hound::WavReader::open(path).map_err(err)?;
    let spec = r.spec();
    let all: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().map_err(err)?,
        hound::SampleFormat::Int => {
            let scale = 2f32.powi(i32::from(spec.bits_per_sample) - 1);
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(err)?
        }
    };
    Ok((all, usize::from(spec.channels.max(1)), spec.sample_rate))
}

/// Reads a UTAU `FREQ0003` file (`<stem>_wav.frq`) next to `wav`, sampled
/// at `times` (Hz, 0 where unvoiced).
fn read_frq(wav: &Path, times: &[f64]) -> Option<Vec<f64>> {
    let stem = wav.file_stem()?.to_str()?;
    let bytes = std::fs::read(wav.with_file_name(format!("{stem}_wav.frq"))).ok()?;
    if bytes.len() < 40 || &bytes[0..8] != b"FREQ0003" {
        return None;
    }
    let hop = f64::from(u32::from_le_bytes(bytes[8..12].try_into().ok()?).max(1)) / SR;
    let n = u32::from_le_bytes(bytes[36..40].try_into().ok()?) as usize;
    if n == 0 || bytes.len() < 40 + n * 16 {
        return None;
    }
    let frame =
        |i: usize| f64::from_le_bytes(bytes[40 + i * 16..48 + i * 16].try_into().unwrap_or([0; 8]));
    let mut f: Vec<f64> = times
        .iter()
        .map(|&t| {
            let v = frame(((t / hop).round() as usize).min(n - 1));
            if v > 55.0 { v } else { 0.0 }
        })
        .collect();
    // Bridge one-frame dropouts inside voiced stretches.
    for i in 1..f.len().saturating_sub(1) {
        if f[i] == 0.0 && f[i - 1] > 0.0 && f[i + 1] > 0.0 {
            f[i] = 0.5 * (f[i - 1] + f[i + 1]);
        }
    }
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_analysis::Yin;

    fn tone(hz: f64, secs: f64) -> Vec<f32> {
        (0..(secs * SR) as usize)
            .map(|i| {
                let ph = std::f64::consts::TAU * hz * i as f64 / SR;
                (0.3 * (ph.sin() + 0.5 * (2.0 * ph).sin() + 0.25 * (3.0 * ph).sin())) as f32
            })
            .collect()
    }

    #[test]
    fn shifts_pitch_and_stretches_time() {
        let x = tone(261.63, 1.0);
        let a = Analysis::from_f0(x, &vec![261.63; 300], 0.005);
        let target = 6700.0; // G4
        let len = (2.0 * SR) as usize;
        let y = synthesize(
            &a,
            &Request {
                len,
                src_time: &|n| 0.1 + 0.8 * n as f64 / len as f64,
                target_cents: &|_| Some(target),
                modulation: 0.0,
                formant: &|_| 1.0,
            },
        );
        assert_eq!(y.len(), len);
        let t = Yin::for_reference().track(&y, SAMPLE_RATE);
        let mid: Vec<f64> = (50..150).filter_map(|i| t.cents(i)).collect();
        let med = mid[mid.len() / 2];
        assert!((med - target).abs() < 15.0, "got {med}");
        let rms = (y.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / len as f64).sqrt();
        assert!(rms > 0.1 && rms < 0.4, "rms {rms}");
    }
}
