//! Loudness (ITU-R BS.1770 / EBU R128) and true peak.

use ebur128::{EbuR128, Mode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Loudness of a stereo mix.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Loudness {
    /// Integrated loudness in LUFS; `None` for silence or very short audio.
    pub integrated_lufs: Option<f64>,
    /// Loudest 3-second window in LUFS.
    pub short_term_max_lufs: Option<f64>,
    /// Short-term loudness every second, in LUFS.
    pub short_term_lufs: Vec<Option<f64>>,
    /// Loudness range in LU.
    pub range_lu: Option<f64>,
    /// True peak in dBTP (4× oversampled).
    pub true_peak_dbtp: Option<f64>,
    /// Sample peak in dBFS.
    pub sample_peak_dbfs: Option<f64>,
}

fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

/// Measures interleaved stereo samples.
pub fn measure(stereo: &[f32], sample_rate: u32) -> Loudness {
    let Ok(mut m) = EbuR128::new(
        2,
        sample_rate,
        Mode::I | Mode::S | Mode::LRA | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK,
    ) else {
        return Loudness::default();
    };
    let frames_per_sec = sample_rate as usize;
    let mut short_term = Vec::new();
    let mut max_st = f64::NEG_INFINITY;
    for chunk in stereo.chunks(frames_per_sec * 2) {
        if m.add_frames_f32(chunk).is_err() {
            return Loudness::default();
        }
        let st = m.loudness_shortterm().ok().and_then(finite);
        if let Some(s) = st {
            max_st = max_st.max(s);
        }
        short_term.push(st);
    }
    let peak = |f: &dyn Fn(u32) -> Result<f64, ebur128::Error>| {
        let p = f(0).unwrap_or(0.0).max(f(1).unwrap_or(0.0));
        (p > 0.0).then(|| 20.0 * p.log10())
    };
    Loudness {
        integrated_lufs: m.loudness_global().ok().and_then(finite),
        short_term_max_lufs: finite(max_st),
        short_term_lufs: short_term,
        range_lu: m.loudness_range().ok().and_then(finite),
        true_peak_dbtp: peak(&|c| m.true_peak(c)),
        sample_peak_dbfs: peak(&|c| m.sample_peak(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-scale 997 Hz sine in both channels reads about −3 LUFS
    /// (BS.1770 reference: 0 dBFS sine on one channel ≈ −3.01 LUFS; both
    /// channels add 3 dB).
    #[test]
    fn sine_reference_level() {
        let sr = 48_000;
        let amp = 10f32.powf(-20.0 / 20.0); // −20 dBFS
        let stereo: Vec<f32> = (0..sr * 5)
            .flat_map(|i| {
                let s = amp * (std::f32::consts::TAU * 997.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let l = measure(&stereo, sr);
        let i = l.integrated_lufs.unwrap();
        assert!((i - (-20.0)).abs() < 0.2, "integrated {i}");
        assert!((l.sample_peak_dbfs.unwrap() + 20.0).abs() < 0.1);
        assert_eq!(
            measure(&vec![0.0; 2 * sr as usize], sr).integrated_lufs,
            None
        );
    }
}
