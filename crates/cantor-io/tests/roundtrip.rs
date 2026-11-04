//! Export → import round trips (UST, USTX), UST import, lyrics.

use cantor_core::{Note, NoteId, Project, TempoPoint, Track, TrackId, Vibrato, apply};
use cantor_io::lyrics::{self, Lang, LyricsOpts};
use cantor_io::{Kind, ust, ustx};

/// A small song: a tempo change at a rest, kana/romaji/extension lyrics,
/// a glide, a vibrato and note expression.
fn song() -> Project {
    let mut p = Project {
        tempo: vec![
            TempoPoint {
                tick: 0,
                bpm: 128.0,
            },
            TempoPoint {
                tick: 2400,
                bpm: 96.5,
            },
        ],
        ..Project::default()
    };
    p.meta.title = "Round trip".into();
    let mut t = Track {
        id: TrackId(1),
        name: "Lead".into(),
        singer: "uta".into(),
        ..Track::default()
    };
    let notes = [
        (0, 480, 60, "さ"),
        (480, 480, 62, "く"),
        (960, 960, 64, "ら"),
        (1920, 240, 64, "+"),
        (2880, 480, 67, "ka"),
        (3360, 960, 65, "ze"),
    ];
    for (i, (start, len, key, lyric)) in notes.into_iter().enumerate() {
        let mut n = Note::new(start, len, key, lyric);
        n.id = NoteId(i as u64 + 1);
        t.notes.push(n);
    }
    t.notes[1].pitch.portamento.overshoot_cents = 30.0;
    t.notes[2].vel = 140.0;
    t.notes[2].vol = 80.0;
    t.notes[5].pitch.vibrato = Some(Vibrato::default());
    p.tracks.push(t);
    p
}

fn summary(p: &Project) -> Vec<(i64, i64, u8, String)> {
    p.tracks[0]
        .notes
        .iter()
        .map(|n| (n.start, n.length, n.key, n.lyric.clone()))
        .collect()
}

#[test]
fn ust_export_import_preserves_notes_lyrics_tempo() {
    let p = song();
    let out = ust::export(&p, None).unwrap();
    let back = ust::import(&out.bytes).unwrap().project;
    assert_eq!(summary(&back), summary(&p));
    assert_eq!(back.tempo, p.tempo);
    let (a, b) = (&p.tracks[0].notes, &back.tracks[0].notes);
    assert_eq!((b[2].vel, b[2].vol), (140.0, 80.0));
    assert_eq!(b[1].pitch.portamento, a[1].pitch.portamento);
    assert_eq!(b[5].pitch.vibrato, a[5].pitch.vibrato);
    assert_eq!(back.tracks[0].singer, "uta");
    // Shift-JIS on disk.
    assert!(std::str::from_utf8(&out.bytes).is_err());
}

#[test]
fn ustx_export_import_preserves_notes_lyrics_tempo() {
    let p = song();
    let out = ustx::export(&p).unwrap();
    let back = ustx::import(&out.bytes).unwrap().project;
    assert_eq!(summary(&back), summary(&p));
    assert_eq!(back.tempo, p.tempo);
    let (a, b) = (&p.tracks[0].notes, &back.tracks[0].notes);
    assert_eq!((b[2].vel, b[2].vol), (140.0, 80.0));
    assert_eq!(b[1].pitch.portamento, a[1].pitch.portamento);
    assert_eq!(b[5].pitch.vibrato, a[5].pitch.vibrato);
}

#[test]
fn ust_mode2_pitch_and_fields() {
    let text = "[#VERSION]\r\nUST Version1.2\r\n[#SETTING]\r\nTempo=150.00\r\nVoiceDir=%VOICE%uta\r\nMode2=True\r\n\
[#0000]\r\nLength=480\r\nLyric=R\r\nNoteNum=60\r\n\
[#0001]\r\nLength=480\r\nLyric=- あ\r\nNoteNum=60\r\nVelocity=120\r\nPreUtterance=30\r\nPBS=-20;5\r\nPBW=40\r\nPBY=0\r\n\
[#0002]\r\nLength=960\r\nLyric=い\r\nNoteNum=64\r\nIntensity=90\r\nModulation=20\r\nFlags=g-5\r\nPBS=-40;0\r\nPBW=80\r\nVBR=65,180,35,20,20,0,0,0\r\nEnvelope=0,5,35,0,100,100,0\r\n\
[#TRACKEND]\r\n";
    let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode(text);
    let imp = ust::import(&sjis).unwrap();
    let n = &imp.project.tracks[0].notes;
    assert_eq!(imp.project.tempo[0].bpm, 150.0);
    assert_eq!(
        (n[0].start, n[0].lyric.as_str(), n[0].alias.as_deref()),
        (480, "あ", Some("- あ"))
    );
    assert_eq!((n[0].vel, n[0].timing.preutter_ms), (120.0, Some(30.0)));
    // No previous note: a free bend from +50 cents.
    assert_eq!(n[0].pitch.points.len(), 2);
    assert_eq!(n[0].pitch.points[0].cents, 50.0);
    // A glide from the previous note.
    assert_eq!(
        (
            n[1].pitch.portamento.start_ms,
            n[1].pitch.portamento.length_ms
        ),
        (-40.0, 80.0)
    );
    assert_eq!(
        (n[1].vol, n[1].mod_, n[1].flags.as_str()),
        (90.0, 20.0, "g-5")
    );
    let v = n[1].pitch.vibrato.unwrap();
    assert_eq!((v.length, v.period_ms, v.depth_cents), (65.0, 180.0, 35.0));
    assert!(imp.diagnostics.iter().any(|d| d.code == "ust.envelope"));
}

#[test]
fn kind_from_extension() {
    use std::path::Path;
    assert_eq!(Kind::from_path(Path::new("a/song.MID")), Some(Kind::Midi));
    assert_eq!(Kind::from_path(Path::new("x.ustx")), Some(Kind::Ustx));
    assert_eq!(Kind::from_path(Path::new("x.cantor")), Some(Kind::Cantor));
    assert_eq!(Kind::from_path(Path::new("x.wav")), None);
}

fn parse(text: &str, lang: Lang) -> Vec<String> {
    lyrics::parse(
        text,
        LyricsOpts {
            lang,
            ..LyricsOpts::default()
        },
    )
}

#[test]
fn lyrics_kana() {
    assert_eq!(
        parse("きょうは　いい てんき", Lang::Auto),
        ["きょ", "う", "は", "い", "い", "て", "ん", "き"]
    );
    assert_eq!(
        parse("ずっと\n\nラーメン、", Lang::Auto),
        ["ずっ", "と", "ラ", "+", "メ", "ン"]
    );
    assert_eq!(parse("あ + い", Lang::Auto), ["あ", "+", "い"]);
}

#[test]
fn lyrics_romaji() {
    assert_eq!(
        parse("sakura sakura", Lang::Auto),
        ["sa", "ku", "ra", "sa", "ku", "ra"]
    );
    assert_eq!(
        parse("Konnichiwa\nkyou mo", Lang::Auto),
        ["ko", "n", "ni", "chi", "wa", "kyo", "u", "mo"]
    );
    assert_eq!(parse("shinjite", Lang::Romaji), ["shi", "n", "ji", "te"]);
}

#[test]
fn lyrics_english() {
    assert_eq!(
        parse("Hello, beau-ti-ful world!", Lang::Auto),
        ["Hello", "beau", "ti", "ful", "world"]
    );
    assert_eq!(
        parse("don't stop + believing", Lang::English),
        ["don't", "stop", "+", "believing"]
    );
}

#[test]
fn lyrics_assign_skips_extensions_and_reports_mismatch() {
    let mut p = song();
    let track = &p.tracks[0];
    let syl = parse("a i u e o x y", Lang::Romaji);
    let (op, diags) = lyrics::assign(track, &syl, None);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "lyrics.count_mismatch");
    apply(&mut p, &op).unwrap();
    let lyr: Vec<_> = p.tracks[0].notes.iter().map(|n| n.lyric.as_str()).collect();
    assert_eq!(lyr, ["a", "i", "u", "+", "e", "o"]);
    let (_, diags) = lyrics::assign(&p.tracks[0], &syl[..2], Some((0, 960)));
    assert!(diags.is_empty());
}
