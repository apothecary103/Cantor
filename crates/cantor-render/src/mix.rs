//! Placing phrase audio on a track timeline with crossfades.

/// Crossfade length at phrase boundaries.
pub const CROSSFADE_SECS: f64 = 0.010;

/// One rendered phrase ready to mix.
#[derive(Clone, Debug)]
pub struct PlacedAudio<'a> {
    /// Time of the first sample (may be negative).
    pub start_sec: f64,
    /// Mono samples.
    pub samples: &'a [f32],
    /// Earliest time this phrase may sound: the boundary with the previous
    /// phrase (`None` for the first).
    pub lo_sec: Option<f64>,
    /// Latest time this phrase may sound: the boundary with the next phrase.
    pub hi_sec: Option<f64>,
}

/// Mixes phrases into a mono buffer of `len` samples starting at time 0.
/// Each phrase sounds between its boundaries, with equal-power fades of
/// [`CROSSFADE_SECS`] centered on each boundary and at its own edges.
pub fn mix(phrases: &[PlacedAudio<'_>], sample_rate: u32, len: usize) -> Vec<f32> {
    let sr = f64::from(sample_rate);
    let half = CROSSFADE_SECS / 2.0;
    let mut out = vec![0.0f32; len];
    for p in phrases {
        let own_lo = p.start_sec;
        let own_hi = p.start_sec + p.samples.len() as f64 / sr;
        // Fade in ending at lo + half, fade out starting at hi - half.
        let lo = p.lo_sec.map_or(own_lo + half, |b| b.max(own_lo + half));
        let hi = p.hi_sec.map_or(own_hi - half, |b| b.min(own_hi - half));
        for (i, &s) in p.samples.iter().enumerate() {
            let t = p.start_sec + i as f64 / sr;
            let gain = fade_in(t, lo, half) * fade_out(t, hi, half);
            if gain <= 0.0 {
                continue;
            }
            let j = (t * sr).round();
            if j >= 0.0 && (j as usize) < len {
                out[j as usize] += (f64::from(s) * gain) as f32;
            }
        }
    }
    out
}

/// Equal-power fade in over `[edge - half, edge + half]`.
fn fade_in(t: f64, edge: f64, half: f64) -> f64 {
    if t <= edge - half {
        0.0
    } else if t >= edge + half {
        1.0
    } else {
        let u = (t - (edge - half)) / (2.0 * half);
        (std::f64::consts::FRAC_PI_2 * u).sin()
    }
}

/// Equal-power fade out over `[edge - half, edge + half]`.
fn fade_out(t: f64, edge: f64, half: f64) -> f64 {
    if t <= edge - half {
        1.0
    } else if t >= edge + half {
        0.0
    } else {
        let u = (t - (edge - half)) / (2.0 * half);
        (std::f64::consts::FRAC_PI_2 * u).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_is_equal_power() {
        let sr = 1000;
        let ones = vec![1.0f32; 2000];
        let a = PlacedAudio {
            start_sec: 0.0,
            samples: &ones,
            lo_sec: None,
            hi_sec: Some(1.0),
        };
        let b = PlacedAudio {
            start_sec: 0.5,
            samples: &ones,
            lo_sec: Some(1.0),
            hi_sec: None,
        };
        let out = mix(std::slice::from_ref(&a), sr, 3000);
        let both = mix(&[a, b], sr, 3000);
        // Away from the boundary the phrases do not leak into each other.
        assert_eq!(out[900], 1.0);
        assert_eq!(both[1200], 1.0);
        // At the boundary each is at cos²+sin² = 1 in power; amplitude √2/2 each.
        assert!((both[1000] - std::f32::consts::SQRT_2).abs() < 0.01);
        assert_eq!(out[1100], 0.0);
    }
}
