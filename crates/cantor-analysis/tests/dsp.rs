//! YIN accuracy, voicing, octave robustness, transcription, report.

use cantor_analysis::{
    F0Extractor, NoteSpan, ReferenceF0, TargetPitch, TranscribeOptions, VoiceInput, Yin,
    analyze_reference, cents_to_hz, hz_to_cents, render_report, transcribe_detailed,
};
use cantor_core::{NoteId, TempoMap, TrackId};
use std::f64::consts::TAU;

const SR: u32 = 44_100;

/// A voice-like harmonic tone following `cents(t)` (None = silence),
/// phase-continuous, with harmonic amplitudes `amps`.
fn tone(secs: f64, amps: &[f64], cents: impl Fn(f64) -> Option<f64>) -> Vec<f32> {
    let n = (secs * f64::from(SR)) as usize;
    let mut ph = 0.0f64;
    let mut env = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(SR);
            let c = cents(t);
            // 5 ms attack/release ramps.
            let goal = if c.is_some() { 1.0 } else { 0.0 };
            env += (goal - env).clamp(-1.0 / 220.0, 1.0 / 220.0);
            if let Some(c) = c {
                ph += TAU * cents_to_hz(c) / f64::from(SR);
            }
            let s: f64 = amps
                .iter()
                .enumerate()
                .map(|(k, a)| a * ((k + 1) as f64 * ph).sin())
                .sum();
            (0.2 * env * s) as f32
        })
        .collect()
}

const VOICE: [f64; 6] = [1.0, 0.6, 0.4, 0.25, 0.15, 0.1];

/// Median absolute error (cents) of voiced frames inside `[a, b)` s.
fn median_err(y: &Yin, x: &[f32], a: f64, b: f64, truth: impl Fn(f64) -> f64) -> f64 {
    let f0 = y.extract(x, SR).unwrap();
    let mut e: Vec<f64> = (0..f0.hz.len())
        .filter(|&i| f0.time(i) >= a && f0.time(i) < b)
        .map(|i| match f0.cents(i) {
            Some(c) => (c - truth(f0.time(i))).abs(),
            None => 10_000.0,
        })
        .collect();
    e.sort_by(f64::total_cmp);
    e[e.len() / 2]
}

#[test]
fn steady_tones_within_5_cents() {
    for hz in [65.0, 110.0, 220.0, 440.0, 880.0, 1050.0] {
        let c = hz_to_cents(hz);
        let x = tone(1.0, &VOICE, |_| Some(c));
        for y in [Yin::for_samples(), Yin::for_reference()] {
            let e = median_err(&y, &x, 0.1, 0.9, |_| c);
            assert!(e <= 5.0, "{hz} Hz: {e} cents");
        }
    }
}

#[test]
fn vibrato_and_glides_within_5_cents() {
    let vib = |t: f64| 6000.0 + 80.0 * (TAU * 5.5 * t).sin();
    let x = tone(2.0, &VOICE, |t| Some(vib(t)));
    let e = median_err(&Yin::for_reference(), &x, 0.1, 1.9, vib);
    assert!(e <= 5.0, "vibrato {e}");

    let glide = |t: f64| 5500.0 + 1200.0 * t;
    let x = tone(1.0, &VOICE, |t| Some(glide(t)));
    let e = median_err(&Yin::for_samples(), &x, 0.1, 0.9, glide);
    assert!(e <= 5.0, "glide {e}");
}

#[test]
fn noise_and_silence_are_unvoiced() {
    let y = Yin::for_reference();
    let silent = y.extract(&vec![0.0; SR as usize], SR).unwrap();
    assert!(silent.voiced.iter().all(|v| !v));
    let mut s = 0x1234_5678u64;
    let noise: Vec<f32> = (0..SR)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * 0.3
        })
        .collect();
    let f0 = y.extract(&noise, SR).unwrap();
    let ratio = f0.voiced.iter().filter(|v| **v).count() as f64 / f0.voiced.len() as f64;
    assert!(ratio < 0.05, "noise voiced ratio {ratio}");
}

#[test]
fn harmonic_rich_has_no_octave_errors() {
    // Weak fundamental, strong 2nd/3rd harmonics, at 48 kHz.
    let amps = [0.15, 1.0, 0.8, 0.5, 0.4, 0.3, 0.2, 0.2, 0.1, 0.1];
    for hz in [98.0, 196.0, 392.0] {
        let c = hz_to_cents(hz);
        let n = 48_000;
        let mut ph = 0.0f64;
        let x: Vec<f32> = (0..n)
            .map(|_| {
                ph += TAU * hz / 48_000.0;
                let s: f64 = amps
                    .iter()
                    .enumerate()
                    .map(|(k, a)| a * ((k + 1) as f64 * ph).sin())
                    .sum();
                (0.1 * s) as f32
            })
            .collect();
        let f0 = Yin::for_reference().extract(&x, 48_000).unwrap();
        for i in 5..f0.hz.len() - 5 {
            let e = (f0.cents(i).expect("voiced") - c).abs();
            assert!(e < 50.0, "{hz} Hz frame {i}: off by {e} cents");
        }
    }
}

#[test]
fn deterministic() {
    let x = tone(0.5, &VOICE, |t| Some(6000.0 + 100.0 * t));
    let a = Yin::for_samples().extract(&x, SR).unwrap();
    let b = Yin::for_samples().extract(&x, SR).unwrap();
    assert_eq!(a, b);
}

/// A sung-like melody: (start beat, beats, key); 120 BPM.
const MELODY: [(f64, f64, u8); 7] = [
    (0.0, 1.0, 60),
    (1.0, 0.5, 62),
    (1.5, 0.5, 64),
    (2.0, 1.0, 64), // repeated note: consonant gap
    (3.0, 2.0, 67), // long note with vibrato
    (5.0, 0.5, 65),
    (5.5, 1.5, 64),
];

fn sung_melody(offset_cents: f64) -> Vec<f32> {
    let beat = 0.5;
    let pitch = move |t: f64| -> Option<f64> {
        let lead = 0.25;
        let t = t - lead;
        let k = MELODY
            .iter()
            .position(|&(s, l, _)| t >= s * beat && t < (s + l) * beat)?;
        let (s, l, key) = MELODY[k];
        let (st, en) = (s * beat, (s + l) * beat);
        // 60 ms unvoiced gap before the repeated note and before the end.
        if (k == 3 && t < st + 0.06) || (k + 1 == MELODY.len() && t > en - 0.05) {
            return None;
        }
        let mut c = f64::from(key) * 100.0 + offset_cents;
        // 60 ms glide from the previous key.
        if k > 0 && k != 3 && t - st < 0.06 {
            let prev = f64::from(MELODY[k - 1].2) * 100.0 + offset_cents;
            let f = (t - st) / 0.06;
            c = prev + (c - prev) * (0.5 - 0.5 * (std::f64::consts::PI * f).cos());
        }
        if en - st >= 0.9 && t - st > 0.25 {
            c += 70.0 * (TAU * 5.5 * (t - st)).sin();
        }
        Some(c)
    };
    tone(4.0, &VOICE, pitch)
}

#[test]
fn transcribes_a_sung_melody() {
    let x = sung_melody(30.0);
    let r = analyze_reference(&x, SR);
    let tempo = TempoMap::constant(120.0);
    let t = transcribe_detailed(&r, &tempo, &TranscribeOptions::default());
    let keys: Vec<u8> = t.notes.iter().map(|n| n.key).collect();
    let want: Vec<u8> = MELODY.iter().map(|m| m.2).collect();
    assert_eq!(keys, want, "{:?}", t.notes);
    assert!(
        (t.tuning_offset_cents - 30.0).abs() < 10.0,
        "{}",
        t.tuning_offset_cents
    );
    // 0.25 s lead-in = half a beat = 240 ticks.
    for (n, m) in t.notes.iter().zip(&MELODY) {
        let start = 240 + (m.0 * 480.0) as i64;
        assert!((n.start - start).abs() <= 120, "{n:?} vs {m:?}");
        assert_eq!(n.lyric, "a");
    }
    for w in t.notes.windows(2) {
        assert!(w[0].end() <= w[1].start);
    }
}

#[test]
fn report_finds_the_detuned_note() {
    // Two phrases; the second note of phrase 1 is sung 40 cents sharp.
    let notes = [
        (0.2, 0.6, 6000.0, 0usize),
        (0.6, 1.0, 6200.0, 0),
        (1.4, 2.0, 6400.0, 1),
    ];
    let sung = |t: f64| {
        notes
            .iter()
            .find(|n| t >= n.0 && t < n.1)
            .map(|n| if n.2 == 6200.0 { n.2 + 40.0 } else { n.2 })
    };
    let voice = tone(2.3, &VOICE, sung);
    let step = 0.005;
    let frames = (2.3 / step) as usize;
    let target = TargetPitch {
        start_sec: 0.0,
        step_sec: step,
        cents: (0..frames)
            .map(|i| {
                let t = i as f64 * step;
                notes
                    .iter()
                    .find(|n| t >= n.0 && t < n.1)
                    .map_or(0.0, |n| n.2) as f32
            })
            .collect(),
        voiced: (0..frames)
            .map(|i| {
                notes
                    .iter()
                    .any(|n| i as f64 * step >= n.0 && i as f64 * step < n.1)
            })
            .collect(),
    };
    let spans: Vec<NoteSpan> = notes
        .iter()
        .enumerate()
        .map(|(k, n)| NoteSpan {
            note: NoteId(k as u64 + 1),
            lyric: "a".into(),
            start_sec: n.0,
            end_sec: n.1,
            phrase: n.3,
            articulated: true,
        })
        .collect();
    let reference = ReferenceF0::from_track(&Yin::for_reference().track(&voice, SR));
    let mix: Vec<f32> = voice.iter().flat_map(|s| [*s, *s]).collect();
    let rep = render_report(
        &mix,
        2,
        SR,
        &[VoiceInput {
            track: TrackId(1),
            samples: &voice,
            sample_rate: SR,
            target: &target,
            notes: &spans,
            joins: &[],
            reference: Some(&reference),
        }],
    );
    assert_eq!(rep.phrases.len(), 2);
    let w = rep.phrases[0].worst_note.as_ref().unwrap();
    assert_eq!(w.note, NoteId(2));
    assert!((w.bias - 40.0).abs() < 8.0, "{w:?}");
    assert!(
        rep.phrases[1].pitch_p95.unwrap() < 10.0,
        "{:?}",
        rep.phrases[1]
    );
    assert!(rep.phrases[0].voiced_ratio > 0.9);
    let r = rep.ref_pitch_error.as_ref().unwrap();
    assert!(r.p95 < 5.0, "{r:?}");
    let o = rep.onset_error_ms.as_ref().unwrap();
    assert!(o.p50 < 25.0, "{o:?}");
    assert_eq!(rep.clipped, 0);
    assert!(rep.lufs.is_some() && rep.true_peak_db.is_some());
    let json = serde_json::to_value(&rep).unwrap();
    for key in [
        "render_ms",
        "lufs",
        "true_peak_db",
        "clipped",
        "pitch_error",
        "ref_pitch_error",
        "missing_aliases",
        "phrases",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
}
