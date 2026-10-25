//! WORLD feature helpers shared by the backends whose acoustic features
//! are WORLD spectral envelopes and aperiodicities (UTAU, svs.io): each
//! frame is `[sp; bins]` followed by `[ap; bins]`, `bins = fft_size/2 + 1`.

use cantor_render::backend::BackendError;

/// WORLD synthesis of `[sp | ap]` frames with `f0_hz` per frame.
pub fn synthesize(
    features: &[f32],
    fft_size: usize,
    f0_hz: &[f32],
    hop: usize,
    sample_rate: u32,
) -> Result<Vec<f32>, BackendError> {
    let bins = fft_size / 2 + 1;
    let frames = features.len() / (2 * bins);
    if frames != f0_hz.len() {
        return Err(BackendError(format!(
            "internal: {frames} feature frames for {} f0 frames",
            f0_hz.len()
        )));
    }
    let (mut sp, mut ap) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    for row in features.chunks_exact(2 * bins) {
        sp.push(
            row[..bins]
                .iter()
                .map(|&v| f64::from(v))
                .collect::<Vec<_>>(),
        );
        ap.push(
            row[bins..]
                .iter()
                .map(|&v| f64::from(v))
                .collect::<Vec<_>>(),
        );
    }
    let f0: Vec<f64> = f0_hz.iter().map(|&f| f64::from(f)).collect();
    let fs = f64::from(sample_rate);
    let y = world_rs::synthesis::synthesis(
        &f0,
        frames,
        &sp,
        &ap,
        fft_size,
        hop as f64 / fs * 1000.0,
        fs,
        frames * hop,
    )
    .map_err(|e| BackendError(format!("WORLD synthesis: {e:?}")))?;
    Ok(y.into_iter().map(|s| s as f32).collect())
}

/// Shifts every frame's spectral envelope up in frequency by `factor`
/// (a formant shift); aperiodicity is unchanged.
pub fn warp_features(features: &[f32], fft_size: usize, factor: f64) -> Vec<f32> {
    let bins = fft_size / 2 + 1;
    let mut out = features.to_vec();
    for row in out.chunks_exact_mut(2 * bins) {
        let sp: Vec<f64> = row[..bins].iter().map(|&v| f64::from(v)).collect();
        for (d, s) in row[..bins].iter_mut().zip(warp(&sp, factor)) {
            *d = s as f32;
        }
    }
    out
}

/// Moves spectral features up in frequency by `factor` (formants rise when
/// `factor > 1`), interpolating linearly between bins.
pub fn warp(sp: &[f64], factor: f64) -> Vec<f64> {
    let n = sp.len();
    (0..n)
        .map(|b| {
            let x = (b as f64 / factor).clamp(0.0, (n - 1) as f64);
            let i = (x.floor() as usize).min(n - 2);
            let u = x - i as f64;
            sp[i] * (1.0 - u) + sp[i + 1] * u
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warp_moves_peaks_up() {
        let mut sp = vec![0.0; 100];
        sp[20] = 1.0;
        let w = warp(&sp, 1.5);
        let peak = w
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 30);
    }

    #[test]
    fn synthesis_is_deterministic_and_pitched() {
        let fft = 2048;
        let bins = fft / 2 + 1;
        let frames = 80;
        let mut feats = Vec::new();
        for _ in 0..frames {
            // A flat-ish envelope with a formant near 800 Hz, mostly periodic.
            feats.extend((0..bins).map(|b| {
                let hz = b as f64 * 44100.0 / fft as f64;
                (1e-4 * (1.0 + 5.0 * (-((hz - 800.0) / 200.0).powi(2)).exp())) as f32
            }));
            feats.extend(std::iter::repeat_n(0.01f32, bins));
        }
        let f0 = vec![220.0f32; frames];
        let a = synthesize(&feats, fft, &f0, 256, 44_100).unwrap();
        let b = synthesize(&feats, fft, &f0, 256, 44_100).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), frames * 256);
        // The autocorrelation of the steady middle peaks at the period.
        let mid = &a[a.len() / 4..3 * a.len() / 4];
        let ac = |lag: usize| -> f64 {
            mid.iter()
                .zip(&mid[lag..])
                .map(|(&x, &y)| f64::from(x) * f64::from(y))
                .sum()
        };
        let lag = (110..400).max_by(|&a, &b| ac(a).total_cmp(&ac(b))).unwrap();
        let hz = 44_100.0 / lag as f64;
        assert!((hz - 220.0).abs() < 5.0, "{hz} Hz");
    }
}
