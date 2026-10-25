//! A small, versioned, deterministic random number generator.
//!
//! Diffusion noise must be identical for the same seed forever, so Cantor
//! owns its generator instead of depending on a crate whose output may
//! change between versions: xoshiro256** seeded through SplitMix64, with
//! Box–Muller for Gaussian samples.

/// Deterministic RNG. Changing its output is a breaking change to every
/// project's audio and must bump [`Rng::VERSION`].
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
    spare: Option<f64>,
}

impl Rng {
    /// Part of every cache key that depends on random numbers.
    pub const VERSION: u32 = 1;

    /// Seeds the generator.
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let mut next = || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self {
            s: [next(), next(), next(), next()],
            spare: None,
        }
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Uniform in (0, 1].
    pub fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) + 1) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal sample.
    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        let (u1, u2) = (self.next_f64(), self.next_f64());
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }

    /// `n` standard normal samples as f32.
    pub fn normals_f32(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.normal() as f32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the generator's output; if this fails, every rendered project
    /// changes. Bump `Rng::VERSION` and update the values deliberately.
    #[test]
    fn output_is_pinned() {
        let mut r = Rng::new(42);
        let a: Vec<u64> = (0..3).map(|_| r.next_u64()).collect();
        let mut r2 = Rng::new(42);
        let b: Vec<u64> = (0..3).map(|_| r2.next_u64()).collect();
        assert_eq!(a, b);
        assert_eq!(a, PINNED);
    }

    const PINNED: [u64; 3] = [
        1546998764402558742,
        6990951692964543102,
        12544586762248559009,
    ];

    #[test]
    fn normals_look_normal() {
        let mut r = Rng::new(7);
        let xs: Vec<f64> = (0..100_000).map(|_| r.normal()).collect();
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((var - 1.0).abs() < 0.02, "var {var}");
    }
}
