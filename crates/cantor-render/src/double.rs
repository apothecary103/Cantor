//! Voice doubling (docs/spec/40-algorithms.md §10): up to 16 layers, each a
//! re-vocoded copy of the lead with seeded micro-variations in pitch,
//! timing and timbre, spread across the stereo field.
//!
//! Layers reuse the lead's cached mel, so doubling costs one vocoder pass
//! per layer, not a full synthesis.

use crate::key::KeyBuilder;
use crate::rng::Rng;
use cantor_core::{Doubling, PhraseId};

/// Seconds between the control points of a layer's pitch drift.
pub const DRIFT_STEP_SECS: f64 = 0.25;
/// Largest formant shift at `timbre = 1`, as a fraction.
pub const MAX_FORMANT_SHIFT: f64 = 0.04;
/// Combined power of all doubles relative to the lead.
pub const DOUBLES_POWER: f64 = 0.5;

/// One extra layer of a phrase.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    /// 1-based layer number (0 is the lead).
    pub index: u8,
    /// Pitch offset in cents for each frame of the phrase.
    pub detune_cents: Vec<f64>,
    /// Formant factor for [`Backend::warp_timbre`](crate::backend::Backend::warp_timbre).
    pub formant: f32,
    /// Time offset of the layer in seconds.
    pub offset_sec: f64,
    /// Stereo position, -1 (left) … 1 (right).
    pub pan: f64,
    /// Linear gain.
    pub gain: f64,
}

/// The extra layers of a phrase: `layers - 1` of them, none when doubling is
/// off. Deterministic in the doubling settings, the phrase and the frame grid.
pub fn layers(d: &Doubling, phrase: PhraseId, frames: usize, frame_secs: f64) -> Vec<Layer> {
    let n = usize::from(d.layers.clamp(1, 16));
    if n == 1 {
        return Vec::new();
    }
    let doubles = n - 1;
    let max_rank = doubles.div_ceil(2) as f64;
    let gain = (DOUBLES_POWER / doubles as f64).sqrt();
    (1..n)
        .map(|k| {
            let seed = KeyBuilder::new("doubling")
                .u64("seed", d.seed)
                .u64("phrase", phrase.0)
                .u64("layer", k as u64)
                .finish();
            let mut rng = Rng::new(u64::from_le_bytes(seed.0[..8].try_into().expect("8 bytes")));
            let mut sym = || 2.0 * rng.next_f64() - 1.0;
            let offset_sec = d.jitter_ms / 1000.0 * sym();
            let formant = (1.0 + MAX_FORMANT_SHIFT * d.timbre * sym()) as f32;
            let constant = d.detune_cents * sym();
            // Slow drift: Gaussian control points, linearly interpolated.
            let steps = (frames as f64 * frame_secs / DRIFT_STEP_SECS).ceil() as usize + 2;
            let points: Vec<f64> = (0..steps)
                .map(|_| rng.normal() * d.detune_cents / 3.0)
                .collect();
            let detune_cents = (0..frames)
                .map(|i| {
                    let x = i as f64 * frame_secs / DRIFT_STEP_SECS;
                    let j = (x.floor() as usize).min(steps - 2);
                    let u = x - j as f64;
                    constant + points[j] * (1.0 - u) + points[j + 1] * u
                })
                .collect();
            // Alternate sides, widening outwards: L1, R1, L2, R2, …
            let side = if k % 2 == 1 { -1.0 } else { 1.0 };
            let rank = k.div_ceil(2) as f64;
            Layer {
                index: k as u8,
                detune_cents,
                formant,
                offset_sec,
                pan: side * d.spacing * rank / max_rank,
                gain,
            }
        })
        .collect()
}

/// Per-channel gains for a layer at `pan`, normalized so the centre is 1 on
/// both channels (the track's own pan law applies afterwards).
pub fn pan_gains(pan: f64) -> (f64, f64) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
    let k = std::f64::consts::SQRT_2;
    (k * angle.cos(), k * angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doubling(layers: u8) -> Doubling {
        Doubling {
            layers,
            ..Doubling::default()
        }
    }

    #[test]
    fn layers_are_seeded_bounded_and_spread() {
        let d = doubling(5);
        let a = layers(&d, PhraseId(7), 200, 0.01);
        assert_eq!(a.len(), 4);
        assert_eq!(a, layers(&d, PhraseId(7), 200, 0.01), "deterministic");
        assert_ne!(a, layers(&d, PhraseId(8), 200, 0.01), "varies by phrase");
        for l in &a {
            assert!(l.offset_sec.abs() <= d.jitter_ms / 1000.0);
            assert!((l.formant - 1.0).abs() <= MAX_FORMANT_SHIFT as f32 * d.timbre as f32);
            let mean = l.detune_cents.iter().sum::<f64>() / l.detune_cents.len() as f64;
            assert!(mean.abs() < 3.0 * d.detune_cents, "mean detune {mean}");
        }
        let pans: Vec<f64> = a.iter().map(|l| l.pan).collect();
        assert_eq!(pans, [-0.25, 0.25, -0.5, 0.5]);
        let power: f64 = a.iter().map(|l| l.gain * l.gain).sum();
        assert!((power - DOUBLES_POWER).abs() < 1e-12);
        assert!(layers(&doubling(1), PhraseId(7), 200, 0.01).is_empty());
    }

    #[test]
    fn centre_pan_is_unity() {
        let (l, r) = pan_gains(0.0);
        assert!((l - 1.0).abs() < 1e-12 && (r - 1.0).abs() < 1e-12);
        let (l, r) = pan_gains(-1.0);
        assert!((l - std::f64::consts::SQRT_2).abs() < 1e-12 && r.abs() < 1e-12);
    }
}
