#![recursion_limit = "256"]
//! Cantor v1 projects open, converted, with honest diagnostics.

use cantor_io::project_file::from_bytes;
use serde_json::json;

#[test]
fn converts_a_v1_project() {
    let v1 = json!({ "format": "cantor-project", "version": 2, "project": {
        "meta": { "title": "Song", "artist": "", "comment": "" },
        "tempo_map": [{ "tick": 0, "bpm": 138.0 }],
        "time_sig_map": [{ "bar": 0, "numerator": 4, "denominator": 4 }],
        "key_map": [{ "tick": 0, "tonic": 3, "scale": "minor" }],
        "sections": [{ "name": "Verse", "start": 3840, "end": 7680 }],
        "seeds": {}, "takes": {},
        "tracks": [{ "id": 1, "name": "Vocal", "singer": { "id": "Sugar" }, "backend": "utau",
            "mix": { "gain_db": -2.0, "pan": 0.0, "mute": false, "solo": false },
            "effects": [{ "kind": "reverb", "enabled": true, "params": { "mix": 0.2 } }],
            "doubling": { "layers": 2, "spacing": 0.0, "detune_cents": 0.0, "jitter_ms": 0.0, "timbre": 0.0, "seed": 0 },
            "voice_mix": [],
            "curves": {
                "energy": { "model_prediction": { "points": [], "interp": "monotone_hermite" },
                            "user_delta": { "points": [[0, -4.0], [3840, 30.0]], "interp": "monotone_hermite" } },
                "gender": { "model_prediction": { "points": [], "interp": "linear" },
                            "user_delta": { "points": [[0, 0.6]], "interp": "linear" } } },
            "parts": [{ "id": 1, "name": "P", "start": 1920, "length": 9600, "notes": [
                { "id": 101, "start": 1920, "length": 960, "pitch_cents": 6300.0, "lyric": "- なD#4",
                  "expr": { "portamento": { "length_ms": 80.0, "before": 0.5, "overshoot_cents": 5.0, "preparation_cents": 0.0 } },
                  "modes": { "pitch": {}, "timbre": {}, "pronunciation": {} } },
                { "id": 102, "start": 2880, "length": 1920, "pitch_cents": 6550.0, "lyric": "a にD#4",
                  "expr": { "portamento": { "before": 0.6, "overshoot_cents": 0.0, "preparation_cents": 0.0 },
                            "vibrato": { "depth_cents": 26.0, "rate_hz": 5.0, "start": 0.4, "fade_in": 0.3, "fade_out": 0.2, "phase": 0.0, "drift": 0.4 } },
                  "modes": { "pitch": {}, "timbre": {}, "pronunciation": {} } } ] }] }] } });
    let l = from_bytes(&serde_json::to_vec(&v1).unwrap()).expect("converts");
    assert!(l.from_v1);
    let p = &l.project;
    assert_eq!(p.tempo[0].bpm, 138.0);
    assert_eq!((p.sections[0].start, p.sections[0].end), (960, 1920));
    let t = &p.tracks[0];
    assert_eq!(t.singer, "Sugar");
    assert_eq!(t.effects.len(), 1);
    let (a, b) = (&t.notes[0], &t.notes[1]);
    assert_eq!(
        (a.start, a.length, a.key, a.lyric.as_str()),
        (960, 240, 63, "- なD#4")
    );
    assert_eq!(a.pitch.portamento.start_ms, -40.0);
    assert_eq!(a.pitch.portamento.overshoot_cents, 5.0);
    assert_eq!((b.start, b.key), (1200, 66)); // 65.5 rounds up; the -50¢ is a bend
    assert_eq!(b.pitch.points.len(), 1);
    let v = b.pitch.vibrato.as_ref().expect("vibrato");
    assert_eq!((v.length.round(), v.period_ms), (60.0, 200.0));
    assert_eq!(
        t.curves.get("dyn").unwrap().points,
        vec![(0, -4.0), (960, 12.0)]
    );
    assert!((t.curves.get("gen").unwrap().points[0].1 + 20.0).abs() < 1e-9);
    let codes: Vec<&str> = l.diagnostics.iter().map(|d| d.code.as_str()).collect();
    for c in [
        "v1.converted",
        "v1.doubling",
        "v1.microtonal",
        "v1.curve_clamped",
    ] {
        assert!(codes.contains(&c), "{c} missing from {codes:?}");
    }
}
