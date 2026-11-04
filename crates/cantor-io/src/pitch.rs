//! UTAU/OpenUtau pitch points and vibrato ↔ Cantor's [`NotePitch`].
//!
//! UTAU Mode2 (`PBS/PBW/PBY/PBM`) and OpenUtau (`pitch.data`) describe a
//! note's pitch as points: x in ms from the note start, y in tenths of a
//! semitone from the note's key, and a shape per segment. Cantor names the
//! parts instead: a portamento (from the previous note, with overshoot),
//! scoop/fall and free bend points. Import recognizes the common glide
//! patterns and keeps everything else as bend points; export does the
//! reverse.

use crate::common::Diags;
use cantor_core::{Bend, Note, NotePitch, PitchPoint, Portamento, Shape, Vibrato};

/// One pitch point; `y` is relative to the note's own key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pt {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) shape: Shape,
}

/// UTAU/OpenUtau segment shape names (`io`/`l`/`i`/`o`; UST `PBM`
/// `""`/`s`/`j`/`r`).
pub(crate) fn shape_from(s: &str) -> Shape {
    match s {
        "l" | "s" => Shape::Linear,
        "i" | "j" => Shape::In,
        "o" | "r" => Shape::Out,
        _ => Shape::S,
    }
}

/// OpenUtau shape name.
pub(crate) fn shape_ustx(s: Shape) -> &'static str {
    match s {
        Shape::S => "io",
        Shape::Linear => "l",
        Shape::In => "i",
        Shape::Out => "o",
    }
}

/// UST `PBM` name.
pub(crate) fn shape_ust(s: Shape) -> &'static str {
    match s {
        Shape::S => "",
        Shape::Linear => "s",
        Shape::In => "j",
        Shape::Out => "r",
    }
}

/// Converts points to a portamento plus bend points. `prev` is the key of
/// the previous note when it ends exactly where this one starts. No points
/// means a flat note (no glide).
pub(crate) fn from_points(
    mut pts: Vec<Pt>,
    key: u8,
    prev: Option<u8>,
    d: &mut Diags,
    fmt: &str,
) -> (Portamento, Vec<PitchPoint>) {
    let code = format!("{fmt}.pitch_approximated");
    let flat = |start_ms: f64| Portamento {
        length_ms: 0.0,
        start_ms,
        ..Portamento::default()
    };
    let before = pts.len();
    pts.retain(|p| p.x.is_finite() && p.y.is_finite());
    if pts.len() != before {
        d.count_warn(&code, "non-finite pitch points were dropped");
    }
    if !pts.is_sorted_by(|a, b| a.x <= b.x) {
        pts.sort_by(|a, b| a.x.total_cmp(&b.x));
        d.count_warn(&code, "pitch points out of time order were sorted");
    }
    let Some(first) = pts.first().copied() else {
        return (flat(Portamento::default().start_ms), Vec::new());
    };
    let start_ms = d.clamp(&code, "portamento start (ms)", first.x, -1000.0, 1000.0);
    if pts.iter().all(|p| p.y == 0.0) {
        return (flat(start_ms), Vec::new());
    }
    let to_point = |p: &Pt, d: &mut Diags| PitchPoint {
        ms: p.x,
        cents: d.clamp(&code, "bend point (cents)", p.y * 10.0, -1200.0, 1200.0),
    };
    let delta = prev
        .map(|k| (f64::from(k) - f64::from(key)) * 10.0)
        .filter(|v| *v != 0.0);
    if let Some(dlt) = delta
        && (first.y - dlt).abs() < 0.05
        && let Some(k) = (1..pts.len()).find(|&i| pts[i].y == 0.0)
    {
        let mut overshoot = 0.0;
        if k == 2 && pts[1].y * dlt < 0.0 {
            overshoot = d.clamp(
                &code,
                "overshoot (cents)",
                pts[1].y.abs() * 10.0,
                0.0,
                200.0,
            );
        } else if k > 1 {
            d.count_info(
                &code,
                "glide points other than a single overshoot were simplified to a portamento",
            );
        }
        let porta = Portamento {
            length_ms: d.clamp(
                &code,
                "portamento length (ms)",
                pts[k].x - first.x,
                0.0,
                1000.0,
            ),
            start_ms,
            overshoot_cents: overshoot,
            preparation_cents: 0.0,
            shape: first.shape,
        };
        let rest = &pts[k..];
        if rest.len() > 1 && rest[..rest.len() - 1].iter().any(|p| p.shape != Shape::S) {
            d.count_info(&code, "segment shapes of bend points were smoothed");
        }
        let points = if rest.len() > 1 {
            rest.iter().map(|p| to_point(p, d)).collect()
        } else {
            Vec::new()
        };
        return (porta, points);
    }
    if pts[..pts.len() - 1].iter().any(|p| p.shape != Shape::S) {
        d.count_info(&code, "segment shapes of bend points were smoothed");
    }
    (flat(start_ms), pts.iter().map(|p| to_point(p, d)).collect())
}

/// Converts a note's pitch expression to points (inverse of
/// [`from_points`]). `dur_ms` is the note's duration, for the fall.
pub(crate) fn to_points(
    n: &Note,
    prev: Option<u8>,
    dur_ms: f64,
    d: &mut Diags,
    fmt: &str,
) -> Vec<Pt> {
    let code = format!("{fmt}.pitch_approximated");
    let p: &NotePitch = &n.pitch;
    let po = p.portamento;
    let s = |x: f64, y: f64| Pt {
        x,
        y,
        shape: Shape::S,
    };
    let mut pts = Vec::new();
    let delta = prev
        .map(|k| (f64::from(k) - f64::from(n.key)) * 10.0)
        .filter(|v| *v != 0.0);
    match delta {
        Some(dlt) if po.length_ms > 0.0 => {
            pts.push(Pt {
                x: po.start_ms,
                y: dlt,
                shape: po.shape,
            });
            if po.overshoot_cents > 0.0 {
                pts.push(s(
                    po.start_ms + po.length_ms * 0.6,
                    -dlt.signum() * po.overshoot_cents / 10.0,
                ));
            }
            pts.push(s(po.start_ms + po.length_ms, 0.0));
            if po.preparation_cents > 0.0 {
                d.count_info(
                    &code,
                    "portamento preparation has no UTAU equivalent and was left out",
                );
            }
            if p.scoop.is_some() {
                d.count_info(
                    &code,
                    "a scoop on a note that glides from the previous one was left out",
                );
            }
        }
        _ => {
            if let Some(Bend { cents, length_ms }) = p.scoop {
                pts.push(s(0.0, cents / 10.0));
                pts.push(s(length_ms, 0.0));
            }
        }
    }
    let mut add = |pts: &mut Vec<Pt>, x: f64, y: f64| {
        if pts.last().is_none_or(|l: &Pt| x > l.x) {
            pts.push(s(x, y));
        } else {
            d.count_info(
                &code,
                "bend points overlapping the portamento or scoop were left out",
            );
        }
    };
    for q in &p.points {
        add(&mut pts, q.ms, q.cents / 10.0);
    }
    if let Some(Bend { cents, length_ms }) = p.fall {
        add(&mut pts, (dur_ms - length_ms).max(0.0), 0.0);
        add(&mut pts, dur_ms, cents / 10.0);
    }
    pts
}

/// UTAU `VBR` / OpenUtau vibrato fields, in their units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Vbr {
    /// % of the note, from its end.
    pub(crate) length: f64,
    /// ms per cycle.
    pub(crate) period: f64,
    /// cents.
    pub(crate) depth: f64,
    /// % of the vibrato.
    pub(crate) fade_in: f64,
    /// % of the vibrato.
    pub(crate) fade_out: f64,
    /// Phase, % of a cycle.
    pub(crate) phase: f64,
    /// Center offset (UTAU height, OpenUtau drift), % of depth.
    pub(crate) height: f64,
}

/// A vibrato from UTAU fields; `None` when it is off.
pub(crate) fn vibrato_from(v: Vbr, d: &mut Diags, fmt: &str) -> Option<Vibrato> {
    if !(v.length > 0.0 && v.depth > 0.0) {
        return None;
    }
    let code = format!("{fmt}.vibrato_clamped");
    let mut c = |what: &str, v: f64, lo: f64, hi: f64| d.clamp(&code, what, v, lo, hi);
    Some(Vibrato {
        length: c("vibrato length (%)", v.length, 0.0, 100.0),
        period_ms: c("vibrato period (ms)", v.period, 50.0, 1000.0),
        depth_cents: c("vibrato depth (cents)", v.depth, 0.0, 200.0),
        fade_in: c("vibrato fade-in (%)", v.fade_in, 0.0, 100.0),
        fade_out: c("vibrato fade-out (%)", v.fade_out, 0.0, 100.0),
        phase: c("vibrato phase (%)", v.phase, 0.0, 100.0) / 100.0,
        shift: c("vibrato height (%)", v.height, -100.0, 100.0),
        drift: 0.0,
    })
}

/// UTAU fields for a vibrato (all zero when there is none).
pub(crate) fn vibrato_to(v: Option<&Vibrato>, d: &mut Diags, fmt: &str) -> Option<Vbr> {
    let v = v?;
    if v.drift != 0.0 {
        d.count_info(
            &format!("{fmt}.vibrato_drift"),
            "vibrato drift (random wander) has no UTAU equivalent and was left out",
        );
    }
    Some(Vbr {
        length: v.length,
        period: v.period_ms,
        depth: v.depth_cents,
        fade_in: v.fade_in,
        fade_out: v.fade_out,
        phase: v.phase * 100.0,
        height: v.shift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Pt {
        Pt {
            x,
            y,
            shape: Shape::S,
        }
    }

    #[test]
    fn glide_with_overshoot_round_trips() {
        let mut d = Diags::default();
        let (porta, points) = from_points(
            vec![pt(-30.0, -30.0), pt(20.0, 3.0), pt(60.0, 0.0)],
            67,
            Some(64),
            &mut d,
            "t",
        );
        assert_eq!(
            (porta.start_ms, porta.length_ms, porta.overshoot_cents),
            (-30.0, 90.0, 30.0)
        );
        assert!(points.is_empty());
        let mut n = Note::new(0, 480, 67, "a");
        n.pitch.portamento = porta;
        let back = to_points(&n, Some(64), 500.0, &mut d, "t");
        let (again, _) = from_points(back, 67, Some(64), &mut d, "t");
        assert_eq!(again, porta);
        assert!(d.into_vec().is_empty());
    }

    #[test]
    fn free_bends_become_points() {
        let mut d = Diags::default();
        let (porta, points) =
            from_points(vec![pt(0.0, -5.0), pt(50.0, 0.0)], 64, None, &mut d, "t");
        assert_eq!(porta.length_ms, 0.0);
        assert_eq!(
            points,
            vec![
                PitchPoint {
                    ms: 0.0,
                    cents: -50.0
                },
                PitchPoint {
                    ms: 50.0,
                    cents: 0.0
                }
            ]
        );
        let (flat, none) = from_points(
            vec![pt(-40.0, 0.0), pt(40.0, 0.0)],
            64,
            Some(64),
            &mut d,
            "t",
        );
        assert_eq!((flat.length_ms, none.len()), (0.0, 0));
    }
}
