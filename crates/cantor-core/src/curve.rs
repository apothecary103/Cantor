//! Curve evaluation.

use crate::ids::Tick;
use crate::model::{Curve, Interp};

impl Curve {
    /// The curve's value at `t`, or `None` if it has no points. Before the
    /// first point and after the last, the nearest point's value holds.
    pub fn value_at(&self, t: Tick) -> Option<f64> {
        let pts = &self.points;
        let (first, last) = (pts.first()?, pts.last()?);
        if t <= first.0 {
            return Some(first.1);
        }
        if t >= last.0 {
            return Some(last.1);
        }
        // pts[i].0 <= t < pts[i + 1].0
        let i = pts.partition_point(|p| p.0 <= t) - 1;
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[i + 1];
        let h = (x1 - x0) as f64;
        let u = (t - x0) as f64 / h;
        Some(match self.interp {
            Interp::Step => y0,
            Interp::Linear => y0 + (y1 - y0) * u,
            Interp::MonotoneHermite => {
                let m0 = self.tangent(i);
                let m1 = self.tangent(i + 1);
                let (u2, u3) = (u * u, u * u * u);
                (2.0 * u3 - 3.0 * u2 + 1.0) * y0
                    + (u3 - 2.0 * u2 + u) * h * m0
                    + (-2.0 * u3 + 3.0 * u2) * y1
                    + (u3 - u2) * h * m1
            }
        })
    }

    /// The value at `t` if `t` lies within the curve's first and last
    /// points, else `None`. Edit layers use this, so an edit has no effect
    /// outside the span it covers.
    pub fn value_in_span(&self, t: Tick) -> Option<f64> {
        let (first, last) = (self.points.first()?, self.points.last()?);
        (first.0 <= t && t <= last.0)
            .then(|| self.value_at(t))
            .flatten()
    }

    /// Fritsch–Carlson tangent at point `i`, so the interpolant never
    /// overshoots between points.
    fn tangent(&self, i: usize) -> f64 {
        let pts = &self.points;
        let slope = |a: usize| (pts[a + 1].1 - pts[a].1) / (pts[a + 1].0 - pts[a].0) as f64;
        if i == 0 {
            return if pts.len() > 1 { slope(0) } else { 0.0 };
        }
        if i == pts.len() - 1 {
            return slope(i - 1);
        }
        let (d0, d1) = (slope(i - 1), slope(i));
        if d0 * d1 <= 0.0 {
            0.0
        } else {
            // Weighted harmonic mean (Fritsch–Butland), monotone-preserving.
            let (h0, h1) = (
                (pts[i].0 - pts[i - 1].0) as f64,
                (pts[i + 1].0 - pts[i].0) as f64,
            );
            let (w0, w1) = (2.0 * h1 + h0, h1 + 2.0 * h0);
            (w0 + w1) / (w0 / d0 + w1 / d1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn curve(points: Vec<(Tick, f64)>, interp: Interp) -> Curve {
        Curve { points, interp }
    }

    #[test]
    fn interpolation_modes() {
        let pts = vec![(0, 0.0), (100, 10.0)];
        assert_eq!(curve(pts.clone(), Interp::Step).value_at(50), Some(0.0));
        assert_eq!(curve(pts.clone(), Interp::Linear).value_at(25), Some(2.5));
        assert_eq!(curve(pts.clone(), Interp::Linear).value_at(-5), Some(0.0));
        assert_eq!(curve(pts, Interp::Linear).value_at(500), Some(10.0));
        assert_eq!(Curve::default().value_at(0), None);
        let c = curve(vec![(10, 1.0), (20, 3.0)], Interp::Linear);
        assert_eq!(c.value_in_span(15), Some(2.0));
        assert_eq!(c.value_in_span(9), None);
        assert_eq!(c.value_in_span(21), None);
    }

    proptest! {
        /// Monotone Hermite stays within the range of its two neighbors.
        #[test]
        fn hermite_never_overshoots(
            ys in prop::collection::vec(-100.0f64..100.0, 2..8),
            gaps in prop::collection::vec(1i64..500, 8),
            t in 0i64..4000,
        ) {
            let mut x = 0;
            let pts: Vec<_> = ys.iter().zip(&gaps).map(|(&y, &g)| { let p = (x, y); x += g; p }).collect();
            let c = curve(pts.clone(), Interp::MonotoneHermite);
            let v = c.value_at(t).unwrap();
            let i = pts.partition_point(|p| p.0 <= t).saturating_sub(1).min(pts.len() - 1);
            let j = (i + 1).min(pts.len() - 1);
            let (lo, hi) = (pts[i].1.min(pts[j].1), pts[i].1.max(pts[j].1));
            prop_assert!(v >= lo - 1e-9 && v <= hi + 1e-9, "{v} outside [{lo}, {hi}]");
        }
    }
}
