//! Base pitch from notes: cosine-eased portamento and procedural vibrato
//! (docs/spec/40-algorithms.md §3).

use crate::phrase::PhraseNote;
use crate::rng::Rng;

/// Portamento length when a note does not set one.
pub const DEFAULT_PORTAMENTO_SECS: f64 = 0.12;

/// Notes further apart than this jump instead of gliding.
const MAX_GLIDE_GAP_SECS: f64 = 0.05;

/// Base pitch in cents at each frame time `t0 + i · frame_secs`.
pub fn base_pitch(notes: &[PhraseNote], t0: f64, frame_secs: f64, n: usize, seed: u64) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    let drift: Vec<DriftNoise> = notes
        .iter()
        .enumerate()
        .map(|(i, _)| DriftNoise::new(seed ^ (i as u64).wrapping_mul(0x9E37_79B9)))
        .collect();
    for i in 0..n {
        let t = t0 + i as f64 * frame_secs;
        out.push(pitch_at(notes, t, &drift));
    }
    out
}

fn pitch_at(notes: &[PhraseNote], t: f64, drift: &[DriftNoise]) -> f64 {
    // The note sounding at t: the last note starting at or before t, or the
    // first note before the phrase starts.
    let k = notes
        .partition_point(|n| n.start_sec <= t)
        .saturating_sub(1);
    let note = &notes[k];
    let mut p = note.pitch_cents;

    // Portamento into this note from the previous one, and into the next
    // note if its transition starts before that note begins.
    for (a, b) in [(k.checked_sub(1), k), (Some(k), k + 1)] {
        let (Some(a), Some(nb)) = (a, notes.get(b)) else {
            continue;
        };
        let na = &notes[a];
        if nb.start_sec - na.end_sec > MAX_GLIDE_GAP_SECS {
            continue;
        }
        let port = &nb.expr.portamento;
        let w = port
            .length_ms
            .map_or(DEFAULT_PORTAMENTO_SECS, |ms| ms / 1000.0)
            .max(1e-6);
        let start = nb.start_sec - port.before.clamp(0.0, 1.0) * w;
        if t >= start && t < start + w {
            let u = ((t - start) / w).clamp(0.0, 1.0);
            let (pa, pb) = (na.pitch_cents, nb.pitch_cents);
            p = pa + (pb - pa) * (1.0 - (std::f64::consts::PI * u).cos()) / 2.0;
        }
    }

    if let Some(v) = &note.expr.vibrato
        && t >= note.start_sec
        && t < note.end_sec
    {
        let dur = note.end_sec - note.start_sec;
        let vs = note.start_sec + v.start.clamp(0.0, 1.0) * dur;
        if t >= vs && v.depth_cents != 0.0 && v.rate_hz > 0.0 {
            let len = (note.end_sec - vs).max(1e-6);
            let x = (t - vs) / len;
            let fade_in = v.fade_in.clamp(0.0, 1.0);
            let fade_out = v.fade_out.clamp(0.0, 1.0);
            let mut env = 1.0;
            if fade_in > 0.0 {
                env = f64::min(env, x / fade_in);
            }
            if fade_out > 0.0 {
                env = f64::min(env, (1.0 - x) / fade_out);
            }
            // ∫ f(τ)dτ with f = rate · (1 + 5% · drift · noise(τ)).
            let tau = t - vs;
            let phase_int = v.rate_hz * (tau + 0.05 * v.drift * drift[k].integral(tau));
            let phase = 2.0 * std::f64::consts::PI * (phase_int + v.phase);
            p += v.depth_cents * env.clamp(0.0, 1.0) * phase.sin();
        }
    }
    p
}

/// Smooth noise in [-1, 1] as a sum of slow seeded sinusoids, with a
/// closed-form integral so vibrato phase needs no numeric integration.
#[derive(Clone, Debug)]
struct DriftNoise {
    parts: [(f64, f64); 3],
}

impl DriftNoise {
    fn new(seed: u64) -> Self {
        let mut r = Rng::new(seed);
        let freqs = [0.31, 0.53, 0.97];
        let parts = freqs.map(|f| (f, r.next_f64() * std::f64::consts::TAU));
        Self { parts }
    }

    /// ∫₀ᵗ noise(τ) dτ, where noise = mean of sin(2π f τ + φ).
    fn integral(&self, t: f64) -> f64 {
        self.parts
            .iter()
            .map(|&(f, phi)| {
                let w = std::f64::consts::TAU * f;
                (phi.cos() - (w * t + phi).cos()) / w
            })
            .sum::<f64>()
            / self.parts.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_core::{NoteExpr, NoteId, Tick, Vibrato};

    fn pn(id: u64, s: f64, e: f64, cents: f64) -> PhraseNote {
        PhraseNote {
            id: NoteId(id),
            start: (s * 1000.0) as Tick,
            end: (e * 1000.0) as Tick,
            start_sec: s,
            end_sec: e,
            pitch_cents: cents,
            lyric: "a".into(),
            lang: None,
            phonemes: None,
            expr: NoteExpr::default(),
        }
    }

    #[test]
    fn glides_with_cosine_easing() {
        let notes = [pn(1, 0.0, 1.0, 6000.0), pn(2, 1.0, 2.0, 6200.0)];
        let w = DEFAULT_PORTAMENTO_SECS;
        let at = |t: f64| base_pitch(&notes, t, 1.0, 1, 0)[0];
        assert_eq!(at(0.5), 6000.0);
        assert_eq!(at(1.5), 6200.0);
        // Halfway through the window (60% before the boundary) is the midpoint.
        let mid = 1.0 - 0.6 * w + 0.5 * w;
        assert!((at(mid) - 6100.0).abs() < 1e-9);
        assert!(at(1.0 - 0.6 * w + 0.1 * w) < 6100.0);
    }

    #[test]
    fn jumps_across_rests() {
        let notes = [pn(1, 0.0, 1.0, 6000.0), pn(2, 1.1, 2.0, 6200.0)];
        assert_eq!(base_pitch(&notes, 1.05, 1.0, 1, 0)[0], 6000.0);
        assert_eq!(base_pitch(&notes, 1.1, 1.0, 1, 0)[0], 6200.0);
    }

    #[test]
    fn vibrato_stays_within_depth_and_is_deterministic() {
        let mut n = pn(1, 0.0, 2.0, 6000.0);
        n.expr.vibrato = Some(Vibrato {
            depth_cents: 50.0,
            rate_hz: 5.5,
            start: 0.25,
            fade_in: 0.2,
            fade_out: 0.2,
            phase: 0.0,
            drift: 1.0,
        });
        let notes = [n];
        let a = base_pitch(&notes, 0.0, 0.01, 200, 7);
        assert_eq!(a, base_pitch(&notes, 0.0, 0.01, 200, 7));
        assert!(a.iter().all(|p| (p - 6000.0).abs() <= 50.0 + 1e-9));
        assert!(
            a[..50].iter().all(|&p| p == 6000.0),
            "no vibrato before start"
        );
        assert!(
            a.iter().any(|p| (p - 6000.0).abs() > 30.0),
            "vibrato present"
        );
    }
}
