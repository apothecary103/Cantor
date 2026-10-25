//! Tempo and meter math.
//!
//! `sec(t) = Σ over tempo segments k up to t of (Δticks_k / PPQ) · 60 / bpm_k`.

use crate::ids::{PPQ, Tick};
use crate::model::{TempoPoint, TimeSigPoint};

/// Converts between ticks and seconds for a tempo map.
///
/// Built from a canonical tempo map (sorted, unique ticks, first point at
/// tick 0). Ticks before 0 extrapolate with the first tempo.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoMap {
    /// Segment start ticks, ascending; `ticks[0] == 0`.
    ticks: Vec<Tick>,
    /// Segment tempo in BPM.
    bpm: Vec<f64>,
    /// Seconds at each segment start.
    secs: Vec<f64>,
}

impl TempoMap {
    /// Builds a map from tempo points. Points must be sorted by tick with
    /// unique ticks, start at tick 0, and have finite positive BPM; an empty
    /// slice means 120 BPM.
    pub fn new(points: &[TempoPoint]) -> Self {
        if points.is_empty() {
            return Self::constant(120.0);
        }
        debug_assert_eq!(points[0].tick, 0, "tempo map must start at tick 0");
        let mut ticks = Vec::with_capacity(points.len());
        let mut bpm = Vec::with_capacity(points.len());
        let mut secs = Vec::with_capacity(points.len());
        let mut acc = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                acc += seg_secs(p.tick - points[i - 1].tick, points[i - 1].bpm);
            }
            ticks.push(p.tick);
            bpm.push(p.bpm);
            secs.push(acc);
        }
        Self { ticks, bpm, secs }
    }

    /// A map with one tempo.
    pub fn constant(bpm: f64) -> Self {
        Self {
            ticks: vec![0],
            bpm: vec![bpm],
            secs: vec![0.0],
        }
    }

    /// Seconds at `tick`.
    pub fn tick_to_sec(&self, tick: Tick) -> f64 {
        let k = self.ticks.partition_point(|&t| t <= tick).saturating_sub(1);
        self.secs[k] + seg_secs(tick - self.ticks[k], self.bpm[k])
    }

    /// Fractional tick at `sec`. Round it to get the nearest whole tick.
    pub fn sec_to_tick_f64(&self, sec: f64) -> f64 {
        let k = self.secs.partition_point(|&s| s <= sec).saturating_sub(1);
        self.ticks[k] as f64 + (sec - self.secs[k]) * self.bpm[k] * f64::from(PPQ) / 60.0
    }

    /// Nearest whole tick at `sec`.
    pub fn sec_to_tick(&self, sec: f64) -> Tick {
        self.sec_to_tick_f64(sec).round() as Tick
    }

    /// Tempo in BPM at `tick`.
    pub fn bpm_at(&self, tick: Tick) -> f64 {
        let k = self.ticks.partition_point(|&t| t <= tick).saturating_sub(1);
        self.bpm[k]
    }
}

fn seg_secs(dticks: Tick, bpm: f64) -> f64 {
    dticks as f64 / f64::from(PPQ) * 60.0 / bpm
}

/// Converts between bars and ticks for a time-signature map.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeSigMap {
    /// (start bar, start tick, ticks per bar), ascending.
    segs: Vec<(u32, Tick, Tick)>,
}

impl TimeSigMap {
    /// Builds a map from time-signature points, sorted by bar with unique
    /// bars and the first at bar 0; an empty slice means 4/4.
    pub fn new(points: &[TimeSigPoint]) -> Self {
        if points.is_empty() {
            return Self {
                segs: vec![(0, 0, bar_ticks(4, 4))],
            };
        }
        let mut segs: Vec<(u32, Tick, Tick)> = Vec::with_capacity(points.len());
        for p in points {
            let start = match segs.last() {
                Some(&(bar, tick, len)) => tick + Tick::from(p.bar - bar) * len,
                None => 0,
            };
            segs.push((p.bar, start, bar_ticks(p.numerator, p.denominator)));
        }
        Self { segs }
    }

    /// Tick at the start of `bar` (0-based).
    pub fn bar_to_tick(&self, bar: u32) -> Tick {
        let k = self.segs.partition_point(|s| s.0 <= bar).saturating_sub(1);
        let (b, t, len) = self.segs[k];
        t + Tick::from(bar - b) * len
    }

    /// The 0-based bar containing `tick` and the offset into that bar.
    /// Negative ticks are clamped to bar 0.
    pub fn tick_to_bar(&self, tick: Tick) -> (u32, Tick) {
        let tick = tick.max(0);
        let k = self.segs.partition_point(|s| s.1 <= tick).saturating_sub(1);
        let (b, t, len) = self.segs[k];
        let bars = (tick - t) / len;
        (b + bars as u32, tick - t - bars * len)
    }
}

/// Ticks in one bar of `numerator`/`denominator`.
pub fn bar_ticks(numerator: u8, denominator: u8) -> Tick {
    Tick::from(numerator) * Tick::from(PPQ) * 4 / Tick::from(denominator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tempo(points: &[(Tick, f64)]) -> TempoMap {
        let pts: Vec<_> = points
            .iter()
            .map(|&(tick, bpm)| TempoPoint { tick, bpm })
            .collect();
        TempoMap::new(&pts)
    }

    #[test]
    fn constant_tempo() {
        let m = tempo(&[(0, 120.0)]);
        assert_eq!(m.tick_to_sec(Tick::from(PPQ)), 0.5);
        assert_eq!(m.sec_to_tick(1.0), 2 * Tick::from(PPQ));
    }

    #[test]
    fn tempo_change() {
        // One beat at 60 BPM (1 s), then beats at 120 BPM (0.5 s each).
        let q = Tick::from(PPQ);
        let m = tempo(&[(0, 60.0), (q, 120.0)]);
        assert_eq!(m.tick_to_sec(q), 1.0);
        assert_eq!(m.tick_to_sec(3 * q), 2.0);
        assert_eq!(m.sec_to_tick(2.0), 3 * q);
        assert_eq!(m.bpm_at(q - 1), 60.0);
        assert_eq!(m.bpm_at(q), 120.0);
    }

    #[test]
    fn time_signatures() {
        let m = TimeSigMap::new(&[
            TimeSigPoint {
                bar: 0,
                numerator: 4,
                denominator: 4,
            },
            TimeSigPoint {
                bar: 2,
                numerator: 3,
                denominator: 8,
            },
        ]);
        let q = Tick::from(PPQ);
        assert_eq!(m.bar_to_tick(2), 8 * q);
        assert_eq!(m.bar_to_tick(3), 8 * q + 3 * q / 2);
        assert_eq!(m.tick_to_bar(8 * q + 2 * q), (3, q / 2));
    }

    /// Random canonical tempo maps: first point at 0, strictly increasing
    /// ticks, BPM in a musically wide range with fractional values.
    fn tempo_maps() -> impl Strategy<Value = Vec<TempoPoint>> {
        (
            10.0f64..=999.0,
            prop::collection::vec((1..=400_000i64, 10.0f64..=999.0), 0..24),
        )
            .prop_map(|(first, rest)| {
                let mut pts = vec![TempoPoint {
                    tick: 0,
                    bpm: first,
                }];
                let mut tick = 0;
                for (gap, bpm) in rest {
                    tick += gap;
                    pts.push(TempoPoint { tick, bpm });
                }
                pts
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: if cfg!(miri) { 8 } else { 2048 },
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        /// tick → sec → tick is the identity on whole ticks.
        #[test]
        fn tick_sec_tick_round_trips(points in tempo_maps(), tick in -1_000_000i64..=2_000_000_000) {
            let m = TempoMap::new(&points);
            let sec = m.tick_to_sec(tick);
            let back = m.sec_to_tick_f64(sec);
            prop_assert!((back - tick as f64).abs() < 1e-3, "tick {tick} -> {sec}s -> {back}");
            prop_assert_eq!(m.sec_to_tick(sec), tick);
        }

        /// Seconds are strictly increasing in ticks.
        #[test]
        fn tick_to_sec_is_monotonic(points in tempo_maps(), a in 0i64..=10_000_000, d in 1i64..=10_000) {
            let m = TempoMap::new(&points);
            prop_assert!(m.tick_to_sec(a) < m.tick_to_sec(a + d));
        }

        /// bar → tick → bar round-trips.
        #[test]
        fn bar_tick_bar_round_trips(
            sigs in prop::collection::vec((1u32..=8, 1u8..=15, prop::sample::select(vec![1u8, 2, 4, 8, 16, 32])), 0..8),
            bar in 0u32..=500,
        ) {
            let mut pts = vec![TimeSigPoint { bar: 0, numerator: 4, denominator: 4 }];
            let mut b = 0;
            for (gap, numerator, denominator) in sigs {
                b += gap;
                pts.push(TimeSigPoint { bar: b, numerator, denominator });
            }
            let m = TimeSigMap::new(&pts);
            prop_assert_eq!(m.tick_to_bar(m.bar_to_tick(bar)), (bar, 0));
        }
    }
}
