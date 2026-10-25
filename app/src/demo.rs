//! Deterministic demo projects for the UI benchmark and screenshots.

use cantor_core::{
    BackendRef, Curve, Doubling, Interp, LayeredCurve, Mix, Note, NoteExpr, NoteId, ParamId, Part,
    PartId, Project, Scale, SingerRef, Track, TrackId, Vibrato, VocalModes,
};
use std::collections::BTreeMap;

const LYRICS: [&str; 12] = [
    "la", "shan", "yi", "liang", "jing", "ming", "yue", "guang", "zai", "tian", "+", "a",
];

/// A song with `notes` notes spread over two tracks, with pitch and
/// breathiness curves, sections, a key and some vibrato. Deterministic.
pub fn project(notes: usize) -> Project {
    let mut p = Project::new();
    p.meta.title = format!("Demo ({notes} notes)");
    p.key_map.push(cantor_core::KeyPoint {
        tick: 0,
        tonic: 2,
        scale: Scale::Major,
    });
    let scale = [0, 2, 4, 5, 7, 9, 11];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut rand = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let per_track = notes / 2;
    let mut id = 10;
    for t in 0..2u64 {
        let base = if t == 0 { 6900 } else { 6200 };
        let mut part_notes = Vec::with_capacity(per_track);
        let mut tick = 0i64;
        let mut pitch_curve = Vec::new();
        let mut breath = Vec::new();
        for i in 0..per_track {
            let len = [480, 960, 960, 1920][(rand() % 4) as usize];
            let degree = (rand() % 7) as usize;
            let octave = (rand() % 2) as i64 * 1200 - 600;
            let cents = (base + octave + 200) as f64 + f64::from(scale[degree]) * 100.0;
            let gap = if i % 16 == 15 { 960 } else { 0 };
            let vib = (i % 5 == 0).then_some(Vibrato {
                depth_cents: 35.0,
                rate_hz: 5.5,
                start: 0.35,
                fade_in: 0.3,
                fade_out: 0.2,
                phase: 0.0,
                drift: 0.5,
            });
            part_notes.push(Note {
                id: NoteId(id),
                start: tick,
                length: len,
                pitch_cents: cents,
                lyric: LYRICS[i % LYRICS.len()].into(),
                lang: None,
                phonemes: None,
                expr: NoteExpr {
                    vibrato: vib,
                    ..NoteExpr::default()
                },
                modes: VocalModes::default(),
            });
            if i % 3 == 0 {
                pitch_curve.push((tick + len / 4, -20.0 + (rand() % 40) as f64));
                pitch_curve.push((tick + len / 2, 0.0));
            }
            if i % 4 == 0 {
                breath.push((tick, -((rand() % 12) as f64)));
            }
            id += 1;
            tick += len + gap;
        }
        let mut curves = BTreeMap::new();
        curves.insert(
            ParamId::new("pitch"),
            LayeredCurve {
                model_prediction: Curve {
                    points: pitch_curve
                        .iter()
                        .map(|(t, v)| (*t + 30, v * 0.5 + 6.0))
                        .collect(),
                    interp: Interp::MonotoneHermite,
                },
                user_delta: Curve {
                    points: pitch_curve,
                    interp: Interp::MonotoneHermite,
                },
            },
        );
        curves.insert(
            ParamId::new("breathiness"),
            LayeredCurve {
                model_prediction: Curve::default(),
                user_delta: Curve {
                    points: breath,
                    interp: Interp::Linear,
                },
            },
        );
        p.tracks.push(Track {
            id: TrackId(t + 1),
            name: if t == 0 {
                "Lead".into()
            } else {
                "Harmony".into()
            },
            singer: SingerRef {
                id: "Qixuan_v2.7.0_DiffSinger_OpenUtau".into(),
                version: None,
            },
            backend: BackendRef("diffsinger".into()),
            mix: Mix::default(),
            effects: vec![],
            doubling: Doubling::default(),
            voice_mix: vec![],
            curves,
            parts: vec![Part {
                id: PartId(1000 + t),
                name: "Song".into(),
                start: 0,
                length: tick + 1920,
                notes: part_notes,
            }],
        });
    }
    let end = p.tracks[0].parts[0].length;
    p.sections.push(cantor_core::Section {
        name: "verse".into(),
        start: 0,
        end: end / 2,
    });
    p.sections.push(cantor_core::Section {
        name: "chorus".into(),
        start: end / 2,
        end,
    });
    p.canonicalize();
    p
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_is_valid_and_deterministic() {
        let a = super::project(2000);
        assert!(
            a.validate().iter().all(|d| !d.is_error()),
            "{:?}",
            a.validate()
        );
        assert_eq!(a.notes().count(), 2000);
        assert_eq!(a, super::project(2000));
    }
}
