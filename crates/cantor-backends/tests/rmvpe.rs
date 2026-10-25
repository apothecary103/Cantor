//! RMVPE on synthetic and rendered voices.

use cantor_analysis::F0Extractor;
use cantor_backends::rmvpe::Rmvpe;

fn rmvpe() -> Rmvpe {
    let path = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).expect("RMVPE fixture");
    Rmvpe::load(&path).expect("load RMVPE")
}

/// A harmonic tone with a vowel-like spectrum.
fn tone(hz: f64, secs: f64, sr: u32) -> Vec<f32> {
    let n = (secs * f64::from(sr)) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(sr);
            let s: f64 = (1..=12)
                .map(|k| (std::f64::consts::TAU * hz * k as f64 * t).sin() / k as f64)
                .sum();
            (0.2 * s) as f32
        })
        .collect()
}

#[test]
fn tracks_a_harmonic_tone() {
    let r = rmvpe();
    let f0 = r.extract(&tone(261.63, 1.0, 44_100), 44_100).unwrap();
    assert!((f0.hop_secs - 0.01).abs() < 1e-12);
    let mid: Vec<f32> = (20..80)
        .filter(|&i| f0.voiced[i])
        .map(|i| f0.hz[i])
        .collect();
    assert!(mid.len() > 50, "voiced frames: {}", mid.len());
    let mean = mid.iter().sum::<f32>() / mid.len() as f32;
    let cents = 1200.0 * (f64::from(mean) / 261.63).log2();
    assert!(cents.abs() < 15.0, "mean {mean} Hz ({cents:+.1} cents)");
    // Deterministic.
    assert_eq!(f0, r.extract(&tone(261.63, 1.0, 44_100), 44_100).unwrap());
}

#[test]
fn silence_is_unvoiced() {
    let f0 = rmvpe().extract(&vec![0.0; 44_100], 44_100).unwrap();
    assert!(f0.voiced.iter().all(|v| !v));
}
