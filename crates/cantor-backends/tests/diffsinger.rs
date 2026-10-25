//! M2 acceptance tests with a public DiffSinger voicebank (Qixuan v2.7.0)
//! and the PC-NSF-HiFiGAN vocoder, downloaded by checksum on first use.

use cantor_backends::DiffSinger;
use cantor_core::*;
use cantor_render::cache::DiskCache;
use cantor_render::render::{Outcome, Quality, RenderOutput, Renderer};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

fn backend() -> Arc<DiffSinger> {
    static B: OnceLock<Arc<DiffSinger>> = OnceLock::new();
    B.get_or_init(|| {
        let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN).expect("voicebank fixture");
        let voc =
            cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN).expect("vocoder fixture");
        Arc::new(DiffSinger::load(&vb, &voc).expect("load voicebank"))
    })
    .clone()
}

/// "Twinkle, twinkle" opening in pinyin syllables, two phrases.
fn project() -> Project {
    let melody: [(i64, f64, &str); 8] = [
        (0, 60.0, "yi"),
        (1, 60.0, "shan"),
        (2, 67.0, "yi"),
        (3, 67.0, "shan"),
        // one-beat rest, then the second phrase
        (5, 69.0, "liang"),
        (6, 69.0, "jing"),
        (7, 67.0, "jing"),
        (8, 67.0, "+"),
    ];
    let notes = melody
        .iter()
        .enumerate()
        .map(|(i, &(beat, midi, lyric))| Note {
            id: NoteId(100 + i as u64),
            start: beat * 1920,
            length: 1920,
            pitch_cents: midi * 100.0,
            lyric: lyric.into(),
            lang: None,
            phonemes: None,
            expr: NoteExpr::default(),
            modes: VocalModes::default(),
        })
        .collect();
    let mut p = Project::new();
    p.tracks.push(Track {
        id: TrackId(1),
        name: "Lead".into(),
        singer: SingerRef {
            id: "qixuan".into(),
            version: Some("2.7.0".into()),
        },
        backend: BackendRef("diffsinger".into()),
        mix: Mix::default(),
        effects: vec![],
        doubling: Doubling::default(),
        voice_mix: vec![],
        curves: BTreeMap::new(),
        parts: vec![Part {
            id: PartId(2),
            name: "Verse".into(),
            start: 0,
            length: 10 * 1920,
            notes,
        }],
    });
    p
}

fn cache(name: &str) -> DiskCache {
    let d = std::env::temp_dir().join(format!("cantor-ds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    DiskCache::new(d).unwrap()
}

fn rms(out: &RenderOutput) -> f64 {
    let s: f64 = out.samples.iter().map(|x| f64::from(*x).powi(2)).sum();
    (s / out.samples.len().max(1) as f64).sqrt()
}

#[test]
fn renders_deterministically_and_caches() {
    let p = project();
    let r = Renderer::new(backend(), cache("det"));
    let first = r.render(&p, Quality::Preview).unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.trace.phrases.len(), 2);
    assert!(
        first
            .trace
            .phrases
            .iter()
            .all(|t| t.outcome == Outcome::Full)
    );
    assert!(rms(&first) > 1e-3, "audible output, rms {}", rms(&first));
    let secs = first.samples.len() as f64 / 2.0 / f64::from(first.sample_rate);
    assert!(
        (4.0..6.5).contains(&secs),
        "about 4.5 s of audio, got {secs}"
    );

    let second = r.render(&p, Quality::Preview).unwrap();
    assert_eq!(second.trace.cache_hit_rate(), 1.0);
    assert_eq!(first.audio_hash, second.audio_hash);

    // A cold cache reproduces the same audio bit for bit.
    let cold = Renderer::new(backend(), cache("det-cold"))
        .render(&p, Quality::Preview)
        .unwrap();
    assert_eq!(cold.audio_hash, first.audio_hash);

    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("qixuan-preview.wav");
    cantor_render::wav::write_stereo_f32(&out, first.sample_rate, &first.samples).unwrap();
}

#[test]
fn pitch_only_edit_uses_vocoder_fast_path() {
    let p = project();
    let r = Renderer::new(backend(), cache("fast"));
    let before = r.render(&p, Quality::Preview).unwrap();
    let mut doc = Document::new(p);
    let res = doc.apply(Op::SetCurve {
        track: TrackId(1),
        param: ParamId::new("pitch"),
        range: TickRange {
            start: 5 * 1920,
            end: 7 * 1920,
        },
        points: vec![(5 * 1920, 0.0), (6 * 1920, 50.0), (7 * 1920 - 1, 0.0)],
        interp: Interp::MonotoneHermite,
    });
    assert!(res.applied);
    let after = r.render(doc.project(), Quality::Preview).unwrap();
    let outcomes: Vec<_> = after.trace.phrases.iter().map(|t| t.outcome).collect();
    assert_eq!(outcomes, [Outcome::CacheHit, Outcome::VocoderFastPath]);
    assert_ne!(after.audio_hash, before.audio_hash);

    let cold = Renderer::new(backend(), cache("fast-cold"))
        .render(doc.project(), Quality::Preview)
        .unwrap();
    assert_eq!(
        cold.audio_hash, after.audio_hash,
        "the fast path equals a cold render"
    );
}

fn rmvpe() -> Arc<cantor_backends::rmvpe::Rmvpe> {
    static R: OnceLock<Arc<cantor_backends::rmvpe::Rmvpe>> = OnceLock::new();
    R.get_or_init(|| {
        let p = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).expect("RMVPE fixture");
        Arc::new(cantor_backends::rmvpe::Rmvpe::load(&p).expect("load RMVPE"))
    })
    .clone()
}

#[test]
fn render_returns_an_analysis_report() {
    let r = Renderer::new(backend(), cache("analysis")).with_analyzer(rmvpe());
    let out = r.render(&project(), Quality::Preview).unwrap();
    let a = out.analysis.expect("analysis report");
    let t = &a.tracks[0];
    for n in &t.notes {
        eprintln!(
            "{:>6} {:<6} mean {:>6.1} p95 {:>6.1} bias {:>+6.1} voiced {:.2} onset {:?} ms",
            n.id.to_string(),
            n.lyric,
            n.pitch_error_mean.unwrap_or(f64::NAN),
            n.pitch_error_p95.unwrap_or(f64::NAN),
            n.pitch_bias.unwrap_or(f64::NAN),
            n.voiced_ratio,
            n.onset_error_ms.map(|x| x.round())
        );
    }
    eprintln!(
        "track: {:?}\ntiming: {:?}\nloudness: {:?}\nartifacts: {:?}",
        t.pitch, t.timing, a.loudness.integrated_lufs, a.artifacts
    );
    assert_eq!(t.notes.len(), 8);
    // Every sung note is measured, close to the score, and voiced.
    let sung: Vec<_> = t.notes.iter().filter(|n| n.lyric != "+").collect();
    assert!(sung.iter().all(|n| n.voiced_ratio > 0.8), "voicing");
    assert!(
        t.pitch.p95_cents.unwrap() < 100.0,
        "p95 {:?}",
        t.pitch.p95_cents
    );
    assert_eq!(t.pitch.octave_errors, 0);
    assert!(t.timing.measured >= 6);
    let lufs = a.loudness.integrated_lufs.unwrap();
    assert!((-40.0..-6.0).contains(&lufs), "integrated {lufs} LUFS");
    assert!(a.loudness.true_peak_dbtp.unwrap() < 0.0);
    assert!(
        !a.artifacts
            .iter()
            .any(|x| x.kind == cantor_analysis::artifacts::ArtifactKind::Clipping)
    );
}

#[test]
fn xsampa_overrides_sing_the_same_phonemes_as_native_ones() {
    let with = |alphabet, symbols: &[&str]| {
        let mut p = project();
        let note = &mut p.tracks[0].parts[0].notes[1]; // "shan"
        note.phonemes = Some(
            symbols
                .iter()
                .map(|s| PhonemeOverride {
                    symbol: (*s).into(),
                    alphabet,
                    offset_ms: None,
                    length_ms: None,
                })
                .collect(),
        );
        p
    };
    let first_phrase = Some(TickRange {
        start: 0,
        end: 4 * 1920,
    });
    let r = Renderer::new(backend(), cache("xsampa"));
    let native = r
        .render_range(
            &with(PhonemeAlphabet::Native, &["sh", "an"]),
            Quality::Preview,
            first_phrase,
        )
        .unwrap();
    let xsampa = r
        .render_range(
            &with(PhonemeAlphabet::XSampa, &["s`", "an"]),
            Quality::Preview,
            first_phrase,
        )
        .unwrap();
    assert!(xsampa.diagnostics.is_empty(), "{:?}", xsampa.diagnostics);
    assert_eq!(native.audio_hash, xsampa.audio_hash);

    // A symbol outside the map is reported, and the lyric is sung instead.
    let unknown = r
        .render_range(
            &with(PhonemeAlphabet::XSampa, &["Q", "an"]),
            Quality::Preview,
            first_phrase,
        )
        .unwrap();
    assert!(
        unknown
            .diagnostics
            .iter()
            .any(|d| d.code == "phon.xsampa_unknown" && d.message.contains('Q')),
        "{:?}",
        unknown.diagnostics
    );
    let lyric = r
        .render_range(&project(), Quality::Preview, first_phrase)
        .unwrap();
    assert_eq!(unknown.audio_hash, lyric.audio_hash);
}
