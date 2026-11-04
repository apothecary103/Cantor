//! Properties of the final pitch curve (`cantor_core::pitch`).

use cantor_core::pitch::{GLIDE_GAP_SEC, PitchOptions, render_pitch_with};
use cantor_core::*;

/// 120 BPM: 480 ticks = 0.5 s.
fn track(notes: &[(Tick, Tick, u8, &str)]) -> Track {
    Track {
        id: TrackId(1),
        notes: notes
            .iter()
            .enumerate()
            .map(|(i, &(s, l, k, lyric))| Note {
                id: NoteId(i as u64 + 1),
                ..Note::new(s, l, k, lyric)
            })
            .collect(),
        ..Track::default()
    }
}

fn tempo() -> TempoMap {
    TempoMap::constant(120.0)
}

fn at(c: &PitchCurve, sec: f64) -> f64 {
    c.at(sec).unwrap()
}

#[test]
fn hard_step_without_portamento() {
    let mut t = track(&[(0, 480, 60, "a"), (480, 480, 64, "i")]);
    t.notes[1].pitch.portamento.length_ms = 0.0;
    let c = render_pitch(&t, &tempo(), None);
    assert!(c.cents[..100].iter().all(|&x| x == 6000.0));
    assert!(c.cents[100..].iter().all(|&x| x == 6400.0));
    assert!(c.voiced.iter().all(|&v| v));
}

#[test]
fn rests_step_and_are_unvoiced() {
    // A one-beat gap: no glide, unvoiced in between.
    let t = track(&[
        (0, 480, 60, "a"),
        (960, 480, 67, "i"),
        (1440, 240, 67, "br"),
    ]);
    let c = render_pitch(&t, &tempo(), Some((0.0, 1.0)));
    assert_eq!(at(&c, 0.49), 6000.0);
    assert_eq!(at(&c, 0.97), 6000.0, "holds the previous pitch in the gap");
    assert!(!c.voiced[110] && c.voiced[50]);
    let c = render_pitch(&t, &tempo(), Some((0.98, 1.6)));
    assert_eq!(at(&c, 1.0), 6700.0, "no glide across the rest");
    assert!(!c.voiced[c.len() - 2], "breath is unvoiced");
}

#[test]
fn portamento_reaches_target_and_is_monotonic_without_bends() {
    let t = track(&[(0, 480, 60, "a"), (480, 480, 67, "i")]);
    let c = render_pitch(&t, &tempo(), None);
    // Default glide: 80 ms starting 40 ms before the note.
    assert_eq!(at(&c, 0.455), 6000.0);
    assert!(
        (at(&c, 0.5) - 6350.0).abs() < 1.0,
        "midpoint at the boundary"
    );
    assert_eq!(at(&c, 0.545), 6700.0);
    let w: Vec<f32> = c.cents[90..110].to_vec();
    assert!(w.windows(2).all(|p| p[1] >= p[0]));
}

#[test]
fn overshoot_and_preparation() {
    let mut t = track(&[(0, 480, 60, "a"), (480, 480, 67, "i"), (960, 480, 60, "u")]);
    t.notes[1].pitch.portamento.overshoot_cents = 40.0;
    t.notes[2].pitch.portamento.preparation_cents = 20.0;
    let c = render_pitch(&t, &tempo(), None);
    let max = c.cents[100..140].iter().fold(0f32, |a, &b| a.max(b));
    assert!((max - 6740.0).abs() < 1.0, "overshoots by 40: {max}");
    assert_eq!(at(&c, 0.75), 6700.0, "settled");
    let peak = c.cents[180..196].iter().fold(0f32, |a, &b| a.max(b));
    assert!(
        (peak - 6720.0).abs() < 1.0,
        "lifts 20 before falling: {peak}"
    );
    assert_eq!(at(&c, 1.2), 6000.0);
}

#[test]
fn glide_only_across_small_gaps() {
    let gap = |ticks: Tick| {
        let t = track(&[(0, 480, 60, "a"), (480 + ticks, 480, 67, "i")]);
        render_pitch(&t, &tempo(), None)
    };
    // 20 ticks ≈ 21 ms < 50 ms: glides and stays voiced; the default
    // glide is centered on the note start (start_ms = -length/2).
    let c = gap(20);
    assert!(c.voiced[101] && (at(&c, 0.5 + 20.0 / 960.0) - 6350.0).abs() < 1.0);
    // 60 ticks ≈ 62 ms: steps.
    const { assert!(GLIDE_GAP_SEC < 0.0625) };
    let c = gap(60);
    assert!(!c.voiced[105]);
    // The note starts at 0.5625 s, between grid samples: the gap holds the
    // previous key and the first sample on the note has the new one.
    assert_eq!((c.cents[112], c.cents[113]), (6000.0, 6700.0)); // 0.56 s, 0.565 s
}

#[test]
fn scoop_fall_and_points() {
    let mut t = track(&[(0, 960, 60, "a")]);
    let p = &mut t.notes[0].pitch;
    p.scoop = Some(Bend {
        cents: -150.0,
        length_ms: 100.0,
    });
    p.fall = Some(Bend {
        cents: -300.0,
        length_ms: 200.0,
    });
    p.points = vec![
        PitchPoint {
            ms: 400.0,
            cents: 0.0,
        },
        PitchPoint {
            ms: 500.0,
            cents: 50.0,
        },
        PitchPoint {
            ms: 600.0,
            cents: 0.0,
        },
    ];
    let c = render_pitch(&t, &tempo(), None);
    assert_eq!(at(&c, 0.0), 5850.0);
    assert_eq!(at(&c, 0.1), 6000.0);
    assert_eq!(at(&c, 0.5), 6050.0);
    assert_eq!(at(&c, 0.7), 6000.0);
    assert_eq!(at(&c, 0.8), 6000.0);
    // The fall eases in over the last 200 ms: cents·(1 − cos(πu/2)).
    let fall =
        |sec: f64| 6000.0 - 300.0 * (1.0 - (std::f64::consts::FRAC_PI_2 * (sec - 0.8) / 0.2).cos());
    assert!((at(&c, 0.9) - fall(0.9)).abs() < 0.01);
    assert!((at(&c, 0.995) - fall(0.995)).abs() < 0.01);
    assert!(at(&c, 0.995) < 5715.0, "nearly the full fall by the end");
    assert!(
        c.cents[160..].windows(2).all(|p| p[1] <= p[0]),
        "falls monotonically"
    );
}

#[test]
fn vibrato_window_depth_and_determinism() {
    let mut t = track(&[(0, 1920, 60, "a")]); // 2 s
    t.notes[0].pitch.vibrato = Some(Vibrato {
        length: 50.0,
        depth_cents: 40.0,
        drift: 1.0,
        ..Vibrato::default()
    });
    let c = render_pitch(&t, &tempo(), None);
    assert!(
        c.cents[..200].iter().all(|&x| x == 6000.0),
        "zero before the window"
    );
    let dev: Vec<f32> = c.cents[200..].iter().map(|x| x - 6000.0).collect();
    assert!(dev.iter().all(|d| d.abs() <= 40.0 * 1.2 + 1e-3));
    assert!(dev.iter().any(|d| d.abs() > 25.0));
    assert_eq!(c, render_pitch(&t, &tempo(), None));
    let no_vib = PitchOptions {
        vibrato: false,
        ..PitchOptions::default()
    };
    let flat = render_pitch_with(&t, &tempo(), None, &no_vib);
    assert!(flat.cents.iter().all(|&x| x == 6000.0));
    // Another note id drifts differently.
    let mut u = t.clone();
    u.notes[0].id = NoteId(99);
    assert_ne!(c, render_pitch(&u, &tempo(), None));
}

#[test]
fn pitd_is_added_and_base_ignores_it() {
    let mut t = track(&[(0, 960, 60, "a")]);
    t.curves.insert(
        ParamId::new("pitd"),
        Curve {
            points: vec![(0, 0.0), (960, 100.0)],
            interp: Interp::Linear,
        },
    );
    let c = render_pitch(&t, &tempo(), None);
    assert!((at(&c, 0.5) - 6050.0).abs() < 0.6);
    assert_eq!(at(&base_pitch(&t, &tempo(), None), 0.5), 6000.0);
}

#[test]
fn ranges_align_to_the_grid() {
    let t = track(&[(0, 480, 60, "a"), (480, 480, 67, "i")]);
    let full = render_pitch(&t, &tempo(), None);
    let part = render_pitch(&t, &tempo(), Some((0.4512, 0.6)));
    assert_eq!(part.start_sec, 0.45);
    assert_eq!(part.cents[..], full.cents[90..90 + part.len()]);
}

#[test]
fn four_minute_track_is_fast() {
    // 960 eighth notes at 120 BPM = 4 minutes, with vibrato and pitd.
    let notes: Vec<_> = (0..960)
        .map(|i| (i * 240, 240, 55 + (i % 12) as u8, "a"))
        .collect();
    let mut t = track(&notes);
    for n in &mut t.notes {
        n.pitch.vibrato = Some(Vibrato::default());
    }
    t.curves.insert(
        ParamId::new("pitd"),
        Curve {
            points: (0..2000).map(|i| (i * 115, (i % 7) as f64)).collect(),
            interp: Interp::Smooth,
        },
    );
    let start = std::time::Instant::now();
    let c = render_pitch(&t, &tempo(), None);
    let el = start.elapsed();
    assert_eq!(c.len(), 48_000);
    // Generous bound for unoptimized test builds.
    assert!(el.as_millis() < 500, "{el:?}");
}
