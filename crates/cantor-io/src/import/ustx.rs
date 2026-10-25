//! OpenUtau USTX importer.
//!
//! USTX is YAML. This reads the fields OpenUtau writes in USTX versions
//! 0.5–0.9 into lenient private structs, then maps them onto a Cantor
//! [`Project`]. Mapping choices and unit conversions are recorded in
//! `docs/decisions/0005-ustx-import.md`. Everything that cannot be carried
//! over is reported as a diagnostic with a `ustx.` code.

use super::{Ids, ImportError, Imported, finish, rescale};
use cantor_core::params::known;
use cantor_core::{
    BackendRef, Curve, Diagnostic, EntityRef, Interp, KeyPoint, LayeredCurve, MAX_BPM, MIN_BPM,
    Meta, Mix, Note, NoteExpr, NoteId, ParamId, Part, PartId, Portamento, Project, Scale, Severity,
    SingerRef, TempoPoint, Tick, TimeSigPoint, Track, TrackId, Vibrato, VocalModes,
};
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use std::collections::BTreeMap;
use std::fmt;

const FORMAT: &str = "OpenUtau USTX";

/// OpenUtau's default resolution when a file does not name one.
const DEFAULT_RESOLUTION: i64 = 480;

/// Source tick values beyond this magnitude are rejected as malformed
/// (about 18 000 hours at 120 BPM and 480 PPQ), so arithmetic never overflows.
const MAX_SOURCE_TICK: i64 = 1 << 40;

/// Imports a OpenUtau USTX file.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|e| parse_error(format!("not UTF-8: {e}")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let doc: Ustx = serde_saphyr::from_str(text).map_err(|e| parse_error(e.to_string()))?;
    if doc.ustx_version.is_empty() && doc.tracks.is_empty() && doc.voice_parts.is_empty() {
        return Err(ImportError::Unsupported {
            format: FORMAT,
            message: "no `ustx_version`, tracks or voice parts; this does not look like a USTX \
                      project"
                .into(),
        });
    }
    let resolution = doc.resolution.unwrap_or(DEFAULT_RESOLUTION);
    let ppq = u32::try_from(resolution)
        .ok()
        .filter(|&r| r > 0)
        .ok_or_else(|| parse_error(format!("resolution {resolution} is not a positive integer")))?;

    let mut cx = Cx {
        ppq,
        inexact: 0,
        diags: Diags::default(),
    };
    let project = convert(&doc, &mut cx)?;
    if cx.inexact > 0 {
        cx.diags.push(Diagnostic::warning(
            "ustx.resolution_inexact",
            format!(
                "resolution {ppq} does not divide Cantor's {} ticks per quarter note evenly; \
                 {} tick values were rounded to the nearest tick",
                cantor_core::PPQ,
                cx.inexact
            ),
            None,
        ));
    }
    finish(project, cx.diags.into_vec())
}

fn parse_error(message: String) -> ImportError {
    ImportError::Parse {
        format: FORMAT,
        message,
    }
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

struct Cx {
    ppq: u32,
    inexact: usize,
    diags: Diags,
}

impl Cx {
    /// Converts a source tick value to Cantor ticks, counting inexact ones.
    fn tick(&mut self, src: i64) -> Result<Tick, ImportError> {
        if src.unsigned_abs() > MAX_SOURCE_TICK.unsigned_abs() {
            return Err(parse_error(format!("tick value {src} is out of range")));
        }
        let (t, exact) = rescale(src, self.ppq);
        if !exact {
            self.inexact += 1;
        }
        Ok(t)
    }

    /// Converts `a + b` source ticks, rejecting overflow.
    fn tick_sum(&mut self, a: i64, b: i64) -> Result<Tick, ImportError> {
        let sum = a
            .checked_add(b)
            .ok_or_else(|| parse_error(format!("tick value {a} + {b} overflows")))?;
        self.tick(sum)
    }
}

fn convert(doc: &Ustx, cx: &mut Cx) -> Result<Project, ImportError> {
    let mut project = Project::new();
    project.meta = Meta {
        title: doc.name.clone(),
        artist: String::new(),
        comment: doc.comment.clone(),
    };
    check_version(&doc.ustx_version, cx);
    project.tempo_map = tempo_map(doc, cx)?;
    project.time_sig_map = time_sig_map(doc, cx);
    if let Some(key) = doc.key {
        match u8::try_from(key).ok().filter(|k| *k <= 11) {
            Some(tonic) => project.key_map.push(KeyPoint {
                tick: 0,
                tonic,
                scale: Scale::Major,
            }),
            None => cx.diags.push(Diagnostic::warning(
                "ustx.key_invalid",
                format!("key {key} is not a pitch class 0..=11 and was ignored"),
                Some(EntityRef::KeyMap),
            )),
        }
    }

    let mut ids = Ids::new();
    let mut tracks: Vec<Track> = doc
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| track(t, i, ids.track(), cx))
        .collect();
    // Pitch-deviation points per track, keyed by absolute tick.
    let mut pitd: Vec<BTreeMap<Tick, f64>> = vec![BTreeMap::new(); tracks.len()];

    for vp in &doc.voice_parts {
        let Some(track) = usize::try_from(vp.track_no)
            .ok()
            .filter(|&i| i < tracks.len())
        else {
            cx.diags.push(Diagnostic::warning(
                "ustx.part_track_missing",
                format!(
                    "voice part `{}` is on track {} which does not exist; the part and its {} \
                     notes were not imported",
                    vp.name,
                    vp.track_no,
                    vp.notes.len()
                ),
                None,
            ));
            continue;
        };
        if let Some(part) = voice_part(vp, &mut ids, &mut pitd[track], tracks[track].id, cx)? {
            tracks[track].parts.push(part);
        }
    }

    for (track, points) in tracks.iter_mut().zip(pitd) {
        if points.is_empty() {
            continue;
        }
        track.curves.insert(
            ParamId::new(known::PITCH),
            LayeredCurve {
                model_prediction: Curve::default(),
                user_delta: Curve {
                    points: points.into_iter().collect(),
                    interp: Interp::Linear,
                },
            },
        );
    }

    for wp in &doc.wave_parts {
        let target = usize::try_from(wp.track_no)
            .ok()
            .and_then(|i| tracks.get(i))
            .map(|t| EntityRef::Track { id: t.id });
        cx.diags.agg(
            Severity::Warning,
            "ustx.wave_part_unsupported",
            "wave_part",
            "audio (wave) parts are not supported by Cantor and were not imported",
            "part",
            target,
        );
    }

    project.tracks = tracks;
    Ok(project)
}

fn check_version(version: &str, cx: &mut Cx) {
    if version.is_empty() {
        return;
    }
    let mut it = version.split('.').map(|s| s.trim().parse::<u32>());
    let parsed = match (it.next(), it.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => Some((major, minor)),
        (Some(Ok(major)), None) => Some((major, 0)),
        _ => None,
    };
    let note = match parsed {
        Some(v) if v > (0, 9) => {
            "is newer than the 0.5–0.9 range this importer knows; \
                                  fields it does not recognize were ignored"
        }
        Some(v) if v < (0, 5) => {
            "is older than the 0.5–0.9 range this importer knows; \
                                  legacy fields such as per-note phoneme lists were ignored"
        }
        Some(_) => return,
        None => "is not a recognizable version number; the file was read as USTX 0.5–0.9",
    };
    cx.diags.push(Diagnostic::info(
        "ustx.version",
        format!("USTX version `{version}` {note}"),
        None,
    ));
}

fn tempo_map(doc: &Ustx, cx: &mut Cx) -> Result<Vec<TempoPoint>, ImportError> {
    let mut points = Vec::new();
    if doc.tempos.is_empty() {
        if let Some(bpm) = doc.bpm {
            points.push((0, bpm));
        }
    } else {
        for t in &doc.tempos {
            points.push((cx.tick(t.position)?, t.bpm));
        }
    }
    let mut valid = Vec::with_capacity(points.len());
    for (tick, bpm) in points {
        if tick < 0 || !(MIN_BPM..=MAX_BPM).contains(&bpm) {
            cx.diags.push(Diagnostic::warning(
                "ustx.tempo_invalid",
                format!(
                    "tempo {bpm} BPM at tick {tick} is outside {MIN_BPM}..={MAX_BPM} BPM or \
                     before the start and was ignored"
                ),
                Some(EntityRef::TempoMap),
            ));
        } else {
            valid.push((tick, bpm));
        }
    }
    let (mut points, dups) = dedupe_keep_last(valid);
    if dups > 0 {
        cx.diags.push(Diagnostic::warning(
            "ustx.tempo_duplicate",
            format!(
                "{dups} tempo changes shared a tick with a later one; the last in file order \
                 was kept"
            ),
            Some(EntityRef::TempoMap),
        ));
    }
    match points.first() {
        None => {
            cx.diags.push(Diagnostic::warning(
                "ustx.tempo_missing",
                "the file has no usable tempo; 120 BPM was assumed",
                Some(EntityRef::TempoMap),
            ));
            points.push((0, 120.0));
        }
        Some(&(tick, bpm)) if tick != 0 => {
            cx.diags.push(Diagnostic::warning(
                "ustx.tempo_missing_start",
                format!("the first tempo is at tick {tick}; its {bpm} BPM was extended to tick 0"),
                Some(EntityRef::TempoMap),
            ));
            points.insert(0, (0, bpm));
        }
        Some(_) => {}
    }
    Ok(points
        .into_iter()
        .map(|(tick, bpm)| TempoPoint { tick, bpm })
        .collect())
}

fn time_sig_map(doc: &Ustx, cx: &mut Cx) -> Vec<TimeSigPoint> {
    let legacy = (doc.beat_per_bar.is_some() || doc.beat_unit.is_some()).then(|| UTimeSig {
        bar_position: 0,
        beat_per_bar: doc.beat_per_bar.unwrap_or(4),
        beat_unit: doc.beat_unit.unwrap_or(4),
    });
    let source: Vec<&UTimeSig> = if doc.time_signatures.is_empty() {
        legacy.iter().collect()
    } else {
        doc.time_signatures.iter().collect()
    };
    let mut valid = Vec::new();
    for ts in source {
        let bar = u32::try_from(ts.bar_position).ok();
        let num = u8::try_from(ts.beat_per_bar).ok().filter(|n| *n >= 1);
        let den = u8::try_from(ts.beat_unit)
            .ok()
            .filter(|d| d.is_power_of_two() && *d <= 64);
        match (bar, num, den) {
            (Some(bar), Some(numerator), Some(denominator)) => valid.push((
                bar,
                TimeSigPoint {
                    bar,
                    numerator,
                    denominator,
                },
            )),
            _ => cx.diags.push(Diagnostic::warning(
                "ustx.time_sig_invalid",
                format!(
                    "time signature {}/{} at bar {} is not valid and was ignored",
                    ts.beat_per_bar, ts.beat_unit, ts.bar_position
                ),
                Some(EntityRef::TimeSigMap),
            )),
        }
    }
    let (mut points, dups) = dedupe_keep_last(valid);
    if dups > 0 {
        cx.diags.push(Diagnostic::warning(
            "ustx.time_sig_duplicate",
            format!(
                "{dups} time signatures shared a bar with a later one; the last in file order \
                 was kept"
            ),
            Some(EntityRef::TimeSigMap),
        ));
    }
    if points.first().is_none_or(|p| p.0 != 0) {
        let missing = if points.is_empty() {
            "the file has no usable time signature"
        } else {
            "the first time signature is not at bar 0"
        };
        cx.diags.push(Diagnostic::warning(
            "ustx.time_sig_missing_start",
            format!("{missing}; 4/4 was assumed from bar 0"),
            Some(EntityRef::TimeSigMap),
        ));
        points.insert(
            0,
            (
                0,
                TimeSigPoint {
                    bar: 0,
                    numerator: 4,
                    denominator: 4,
                },
            ),
        );
    }
    points.into_iter().map(|(_, p)| p).collect()
}

/// Sorts by key (stably) and keeps the last entry of each key in input
/// order. Returns the entries and how many were dropped.
fn dedupe_keep_last<K: Ord + Copy, V>(mut v: Vec<(K, V)>) -> (Vec<(K, V)>, usize) {
    v.sort_by_key(|e| e.0);
    let before = v.len();
    let mut out: Vec<(K, V)> = Vec::with_capacity(v.len());
    for e in v {
        match out.last_mut() {
            Some(last) if last.0 == e.0 => *last = e,
            _ => out.push(e),
        }
    }
    let dropped = before - out.len();
    (out, dropped)
}

fn track(t: &UTrack, index: usize, id: TrackId, cx: &mut Cx) -> Track {
    let target = Some(EntityRef::Track { id });
    let name = if t.track_name.is_empty() {
        format!("Track {}", index + 1)
    } else {
        t.track_name.clone()
    };

    let settings = t.renderer_settings.clone().unwrap_or_default();
    let renderer = if settings.renderer.is_empty() {
        t.renderer.clone()
    } else {
        settings.renderer.clone()
    };
    let backend = match renderer.to_ascii_uppercase().as_str() {
        "" => {
            cx.diags.push(Diagnostic::info(
                "ustx.renderer_missing",
                format!("track `{name}` names no renderer; OpenUtau's classic renderer (Cantor backend `utau`) was assumed"),
                target.clone(),
            ));
            "utau".to_owned()
        }
        "CLASSIC" | "WORLDLINE-R" => "utau".to_owned(),
        "DIFFSINGER" => "diffsinger".to_owned(),
        other => {
            let backend = other.to_ascii_lowercase();
            cx.diags.push(Diagnostic::warning(
                "ustx.renderer_unsupported",
                format!(
                    "track `{name}` uses renderer `{renderer}`, which has no Cantor equivalent; \
                     the backend was set to `{backend}` and must be changed before rendering"
                ),
                target.clone(),
            ));
            backend
        }
    };
    let engine: Vec<String> = [
        ("resampler", &settings.resampler),
        ("wavtool", &settings.wavtool),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| format!("{k} `{v}`"))
    .collect();
    if !engine.is_empty() {
        cx.diags.push(Diagnostic::info(
            "ustx.renderer_settings",
            format!(
                "track `{name}`: {} not carried over; Cantor's `{backend}` backend synthesizes \
                 on its own",
                engine.join(" and ")
            ),
            target.clone(),
        ));
    }
    if !t.phonemizer.is_empty() {
        cx.diags.push(Diagnostic::info(
            "ustx.phonemizer",
            format!(
                "track `{name}`: phonemizer `{}` not carried over; Cantor picks the phonemizer \
                 from the singer",
                t.phonemizer
            ),
            target.clone(),
        ));
    }

    let mut gain_db = t.volume;
    if !gain_db.is_finite() {
        cx.diags.push(Diagnostic::warning(
            "ustx.mix_invalid",
            format!("track `{name}` has volume {gain_db}; 0 dB was used"),
            target.clone(),
        ));
        gain_db = 0.0;
    }
    let mut pan = t.pan / 100.0;
    if !(-1.0..=1.0).contains(&pan) {
        let clamped = if pan.is_nan() {
            0.0
        } else {
            pan.clamp(-1.0, 1.0)
        };
        cx.diags.push(Diagnostic::warning(
            "ustx.mix_invalid",
            format!(
                "track `{name}` has pan {} outside -100..=100; {} was used",
                t.pan,
                clamped * 100.0
            ),
            target.clone(),
        ));
        pan = clamped;
    }

    Track {
        id,
        name,
        singer: SingerRef {
            id: t.singer.clone(),
            version: None,
        },
        backend: BackendRef(backend),
        mix: Mix {
            gain_db,
            pan,
            mute: t.mute,
            solo: t.solo,
        },
        effects: Vec::new(),
        doubling: Default::default(),
        voice_mix: Vec::new(),
        curves: BTreeMap::new(),
        parts: Vec::new(),
    }
}

fn voice_part(
    vp: &UVoicePart,
    ids: &mut Ids,
    pitd: &mut BTreeMap<Tick, f64>,
    track: TrackId,
    cx: &mut Cx,
) -> Result<Option<Part>, ImportError> {
    let start = cx.tick(vp.position)?;
    if start < 0 {
        cx.diags.push(Diagnostic::warning(
            "ustx.part_invalid",
            format!(
                "voice part `{}` starts before the song (position {}); the part and its {} \
                 notes were not imported",
                vp.name,
                vp.position,
                vp.notes.len()
            ),
            Some(EntityRef::Track { id: track }),
        ));
        return Ok(None);
    }
    let id = ids.part();
    let part_ref = Some(EntityRef::Part { id });
    if !vp.comment.is_empty() {
        cx.diags.push(Diagnostic::info(
            "ustx.part_comment",
            format!(
                "voice part `{}` has a comment, which Cantor parts cannot hold: {:?}",
                vp.name, vp.comment
            ),
            part_ref.clone(),
        ));
    }

    // Notes in position order, to find each note's predecessor.
    let mut order: Vec<&UNote> = vp.notes.iter().collect();
    order.sort_by_key(|n| n.position);
    let mut notes = Vec::with_capacity(order.len());
    let mut prev: Option<&UNote> = None;
    let mut notes_end: Tick = 0;
    for n in order {
        let note_id = ids.note();
        let prev_tone = prev
            .filter(|p| p.position.saturating_add(p.duration) == n.position)
            .map(|p| p.tone);
        if let Some(note) = note(n, note_id, start, vp.position, prev_tone, cx)? {
            notes_end = notes_end.max(note.start + note.length);
            notes.push(note);
            prev = Some(n);
        }
    }

    let declared = match vp.duration {
        Some(d) if d > 0 => Some(cx.tick_sum(vp.position, d)? - start),
        _ => None,
    };
    let length = match declared {
        Some(len) if len >= notes_end => len.max(1),
        Some(len) => {
            cx.diags.push(Diagnostic::info(
                "ustx.part_extended",
                format!(
                    "voice part `{}` was lengthened from {len} to {notes_end} ticks to cover \
                     its notes",
                    vp.name
                ),
                part_ref.clone(),
            ));
            notes_end
        }
        None => {
            let len = if notes_end > 0 {
                notes_end
            } else {
                Tick::from(cantor_core::PPQ)
            };
            cx.diags.push(Diagnostic::info(
                "ustx.part_length_derived",
                format!(
                    "voice part `{}` has no duration; its length was set to {len} ticks from \
                     its notes",
                    vp.name
                ),
                part_ref.clone(),
            ));
            len
        }
    };

    for curve in &vp.curves {
        part_curve(curve, vp, id, track, pitd, cx)?;
    }

    Ok(Some(Part {
        id,
        name: vp.name.clone(),
        start,
        length,
        notes,
    }))
}

fn note(
    n: &UNote,
    id: NoteId,
    part_start: Tick,
    part_position: i64,
    prev_tone: Option<i64>,
    cx: &mut Cx,
) -> Result<Option<Note>, ImportError> {
    let target = Some(EntityRef::Note { id });
    let abs_start = cx.tick_sum(part_position, n.position)?;
    let abs_end = cx.tick_sum(part_position, n.position.saturating_add(n.duration))?;
    let start = abs_start - part_start;
    let length = abs_end - abs_start;
    if n.position < 0 || start < 0 || n.duration <= 0 || length < 1 || !(0..=128).contains(&n.tone)
    {
        cx.diags.agg(
            Severity::Warning,
            "ustx.note_invalid",
            "note_invalid",
            "notes before their part's start, with no duration, or with a tone outside \
             0..=128 were not imported",
            "note",
            None,
        );
        return Ok(None);
    }

    let lyric = if n.lyric.starts_with('+') {
        if n.lyric != "+" {
            cx.diags.agg(
                Severity::Info,
                "ustx.lyric_extension",
                format!("lyric:{}", n.lyric),
                format!(
                    "OpenUtau extension lyric `{}` became Cantor's `+` (continue the previous \
                     syllable); its variant meaning was not kept",
                    n.lyric
                ),
                "note",
                target.clone(),
            );
        }
        "+".to_owned()
    } else {
        n.lyric.clone()
    };

    let mut expr = NoteExpr {
        vibrato: vibrato(&n.vibrato, &target, cx),
        ..NoteExpr::default()
    };
    if let Some(p) = portamento(&n.pitch, n.tone, prev_tone, &target, cx) {
        expr.portamento = p;
    }

    if !n.phoneme_overrides.is_empty() {
        cx.diags.agg(
            Severity::Warning,
            "ustx.phoneme_override_unsupported",
            "phoneme_override",
            "per-phoneme overrides (symbol, offset, preutterance, overlap) patch the output of \
             OpenUtau's phonemizer, which is not known at import time, and were not imported",
            "note",
            target.clone(),
        );
    }
    for pe in &n.phoneme_expressions {
        cx.diags.agg(
            Severity::Warning,
            "ustx.phoneme_expression_unsupported",
            format!("phoneme_expression:{}", pe.abbr),
            format!(
                "per-phoneme expression `{}` has no Cantor equivalent and was not imported",
                pe.abbr
            ),
            "value",
            target.clone(),
        );
    }

    Ok(Some(Note {
        id,
        start,
        length,
        pitch_cents: n.tone as f64 * 100.0,
        lyric,
        lang: None,
        phonemes: None,
        expr,
        modes: VocalModes::default(),
    }))
}

fn vibrato(v: &UVibrato, target: &Option<EntityRef>, cx: &mut Cx) -> Option<Vibrato> {
    if !(v.length > 0.0 && v.depth > 0.0) {
        return None;
    }
    let fields = [v.length, v.period, v.depth, v.fade_in, v.fade_out, v.shift];
    if v.period <= 0.0 || fields.iter().any(|x| !x.is_finite()) {
        cx.diags.agg(
            Severity::Warning,
            "ustx.vibrato_invalid",
            "vibrato_invalid",
            "vibratos with a non-positive period or non-finite values were not imported",
            "note",
            target.clone(),
        );
        return None;
    }
    let frac = |pct: f64| pct / 100.0;
    let raw = [
        frac(100.0 - v.length),
        frac(v.fade_in),
        frac(v.fade_out),
        frac(v.shift),
    ];
    let [start, fade_in, fade_out, phase] = raw.map(|x| x.clamp(0.0, 1.0));
    if raw != [start, fade_in, fade_out, phase] {
        cx.diags.agg(
            Severity::Warning,
            "ustx.vibrato_clamped",
            "vibrato_clamped",
            "vibrato length, fade-in, fade-out or phase shift outside 0..=100% was clamped",
            "note",
            target.clone(),
        );
    }
    if v.drift != 0.0 {
        cx.diags.agg(
            Severity::Warning,
            "ustx.vibrato_drift",
            "vibrato_drift",
            "OpenUtau vibrato drift offsets the vibrato's center pitch, which Cantor's vibrato \
             cannot express (Cantor's drift is rate jitter); it was not imported",
            "note",
            target.clone(),
        );
    }
    if v.vol_link != 0.0 {
        cx.diags.agg(
            Severity::Warning,
            "ustx.vibrato_vol_link",
            "vibrato_vol_link",
            "OpenUtau vibrato volume link (tremolo) has no Cantor equivalent and was not imported",
            "note",
            target.clone(),
        );
    }
    Some(Vibrato {
        depth_cents: v.depth,
        rate_hz: 1000.0 / v.period,
        start,
        fade_in,
        fade_out,
        phase,
        drift: 0.0,
    })
}

/// Maps a note's pitch points (x in ms from the note start, y in tenths of a
/// semitone from the note's tone) to a portamento. Returns `None` to keep
/// Cantor's default.
fn portamento(
    pitch: &UPitch,
    tone: i64,
    prev_tone: Option<i64>,
    target: &Option<EntityRef>,
    cx: &mut Cx,
) -> Option<Portamento> {
    let pts = &pitch.data;
    if pts.is_empty() {
        return None;
    }
    let well_formed = pts.len() >= 2
        && pts.iter().all(|p| p.x.is_finite() && p.y.is_finite())
        && pts.windows(2).all(|w| w[0].x < w[1].x);
    if !well_formed {
        cx.diags.agg(
            Severity::Warning,
            "ustx.portamento_unsupported",
            "portamento_unsupported",
            "pitch points with fewer than two points or non-increasing times could not be \
             mapped to a portamento; Cantor's automatic portamento is used",
            "note",
            target.clone(),
        );
        return None;
    }
    let mut approx = |key: &str, what: &str| {
        cx.diags.agg(
            Severity::Info,
            "ustx.portamento_approximated",
            format!("portamento:{key}"),
            format!("pitch points approximated by Cantor's portamento: {what}"),
            "note",
            target.clone(),
        );
    };

    // Snap-free and flat: the pitch never leaves the note's tone.
    if !pitch.snap_first && pts.iter().all(|p| p.y == 0.0) {
        return Some(Portamento {
            length_ms: Some(0.0),
            ..Portamento::default()
        });
    }

    let (first, last) = (&pts[0], &pts[pts.len() - 1]);
    let length = last.x - first.x;
    // `+ 0.0` turns -0 into 0 so the project serializes cleanly.
    let raw_before = -first.x / length + 0.0;
    let before = raw_before.clamp(0.0, 1.0);
    if raw_before != before {
        approx(
            "window",
            "transitions that do not span the note boundary were moved to span it",
        );
    }
    if last.y != 0.0 {
        approx(
            "end_offset",
            "transitions ending off the note's pitch were made to end on it",
        );
    }
    if !pitch.snap_first && first.y != 0.0 {
        approx(
            "fixed_start",
            "transitions from a fixed offset (snap_first off) start from the previous note \
             instead",
        );
    }
    if pts[..pts.len() - 1]
        .iter()
        .any(|p| !matches!(p.shape.as_str(), "" | "io"))
    {
        approx(
            "shape",
            "linear, ease-in and ease-out segments use Cantor's cosine (ease in-out) curve",
        );
    }

    let interior = &pts[1..pts.len() - 1];
    let mut overshoot_cents = 0.0;
    match (interior, prev_tone) {
        ([], _) => {}
        ([mid], Some(prev)) if pitch.snap_first && prev != tone => {
            let direction = (tone - prev).signum() as f64;
            let cents = mid.y * 10.0;
            if cents * direction > 0.0 {
                overshoot_cents = cents.abs();
            } else if cents != 0.0 {
                approx(
                    "interior",
                    "intermediate points other than a single overshoot were dropped",
                );
            }
        }
        _ => approx(
            "interior",
            "intermediate points other than a single overshoot were dropped",
        ),
    }

    Some(Portamento {
        length_ms: Some(length),
        before,
        overshoot_cents,
        preparation_cents: 0.0,
    })
}

fn part_curve(
    curve: &UCurve,
    vp: &UVoicePart,
    part: PartId,
    track: TrackId,
    pitd: &mut BTreeMap<Tick, f64>,
    cx: &mut Cx,
) -> Result<(), ImportError> {
    let part_ref = Some(EntityRef::Part { id: part });
    if curve.xs.len() != curve.ys.len() {
        cx.diags.push(Diagnostic::warning(
            "ustx.curve_malformed",
            format!(
                "curve `{}` in voice part `{}` has {} xs but {} ys; the extra values were \
                 ignored",
                curve.abbr,
                vp.name,
                curve.xs.len(),
                curve.ys.len()
            ),
            part_ref.clone(),
        ));
    }
    if curve.xs.is_empty() || curve.ys.is_empty() {
        return Ok(());
    }
    if curve.abbr != "pitd" {
        cx.diags.agg(
            Severity::Warning,
            "ustx.curve_unsupported",
            format!("curve:{}", curve.abbr),
            format!(
                "curve `{}` has no Cantor parameter with the same meaning and units and was \
                 not imported",
                curve.abbr
            ),
            "part",
            part_ref,
        );
        return Ok(());
    }

    let param = ParamId::new(known::PITCH);
    let curve_ref = Some(EntityRef::Curve {
        track,
        param: param.clone(),
    });
    let max = param.descriptor().map_or(f64::INFINITY, |d| d.max_delta());
    for (&x, &y) in curve.xs.iter().zip(&curve.ys) {
        let tick = cx.tick_sum(vp.position, x)?;
        if !y.is_finite() {
            cx.diags.agg(
                Severity::Warning,
                "ustx.curve_non_finite",
                "curve_non_finite",
                "non-finite pitch-deviation points were not imported",
                "point",
                curve_ref.clone(),
            );
            continue;
        }
        let value = y.clamp(-max, max);
        if value != y {
            cx.diags.agg(
                Severity::Warning,
                "ustx.curve_out_of_range",
                "curve_out_of_range:pitd",
                format!("pitch-deviation (pitd) values beyond ±{max} cents were clamped"),
                "point",
                curve_ref.clone(),
            );
        }
        if pitd.insert(tick, value).is_some() {
            cx.diags.agg(
                Severity::Warning,
                "ustx.curve_duplicate_tick",
                "curve_duplicate_tick:pitd",
                "pitch-deviation (pitd) points landing on the same tick of a track were merged; \
                 the last in file order was kept",
                "point",
                curve_ref.clone(),
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Diagnostics with aggregation
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Diags {
    out: Vec<Diagnostic>,
    aggs: Vec<Agg>,
}

/// One aggregated diagnostic: repeated occurrences are counted, and the
/// target is the first occurrence.
struct Agg {
    key: String,
    severity: Severity,
    code: &'static str,
    what: String,
    unit: &'static str,
    target: Option<EntityRef>,
    count: usize,
}

impl Diags {
    fn push(&mut self, d: Diagnostic) {
        self.out.push(d);
    }

    fn agg(
        &mut self,
        severity: Severity,
        code: &'static str,
        key: impl Into<String>,
        what: impl Into<String>,
        unit: &'static str,
        target: Option<EntityRef>,
    ) {
        let key = key.into();
        if let Some(a) = self.aggs.iter_mut().find(|a| a.key == key) {
            a.count += 1;
            return;
        }
        self.aggs.push(Agg {
            key,
            severity,
            code,
            what: what.into(),
            unit,
            target,
            count: 1,
        });
    }

    fn into_vec(self) -> Vec<Diagnostic> {
        let mut out = self.out;
        out.extend(self.aggs.into_iter().map(|a| {
            let plural = if a.count == 1 { "" } else { "s" };
            Diagnostic::new(
                a.severity,
                a.code,
                format!("{} ({} {}{plural})", a.what, a.count, a.unit),
                a.target,
            )
        }));
        out
    }
}

// ---------------------------------------------------------------------------
// USTX structs (lenient: unknown fields ignored, missing fields defaulted)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct Ustx {
    #[serde(deserialize_with = "text")]
    name: String,
    #[serde(deserialize_with = "text")]
    comment: String,
    #[serde(deserialize_with = "text")]
    ustx_version: String,
    resolution: Option<i64>,
    bpm: Option<f64>,
    beat_per_bar: Option<i64>,
    beat_unit: Option<i64>,
    key: Option<i64>,
    #[serde(deserialize_with = "list")]
    tempos: Vec<UTempo>,
    #[serde(deserialize_with = "list")]
    time_signatures: Vec<UTimeSig>,
    #[serde(deserialize_with = "list")]
    tracks: Vec<UTrack>,
    #[serde(deserialize_with = "list")]
    voice_parts: Vec<UVoicePart>,
    #[serde(deserialize_with = "list")]
    wave_parts: Vec<UWavePart>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UTempo {
    position: i64,
    bpm: f64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UTimeSig {
    bar_position: i64,
    beat_per_bar: i64,
    beat_unit: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UTrack {
    #[serde(deserialize_with = "text")]
    singer: String,
    #[serde(deserialize_with = "text")]
    phonemizer: String,
    /// Legacy (before `renderer_settings`).
    #[serde(deserialize_with = "text")]
    renderer: String,
    renderer_settings: Option<URendererSettings>,
    #[serde(deserialize_with = "text")]
    track_name: String,
    mute: bool,
    solo: bool,
    volume: f64,
    pan: f64,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct URendererSettings {
    #[serde(deserialize_with = "text")]
    renderer: String,
    #[serde(deserialize_with = "text")]
    resampler: String,
    #[serde(deserialize_with = "text")]
    wavtool: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UVoicePart {
    #[serde(deserialize_with = "text")]
    name: String,
    #[serde(deserialize_with = "text")]
    comment: String,
    track_no: i64,
    position: i64,
    duration: Option<i64>,
    #[serde(deserialize_with = "list")]
    notes: Vec<UNote>,
    #[serde(deserialize_with = "list")]
    curves: Vec<UCurve>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UWavePart {
    track_no: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UNote {
    position: i64,
    duration: i64,
    tone: i64,
    #[serde(deserialize_with = "text")]
    lyric: String,
    pitch: UPitch,
    vibrato: UVibrato,
    #[serde(deserialize_with = "list")]
    phoneme_expressions: Vec<UPhonemeExpression>,
    #[serde(deserialize_with = "list")]
    phoneme_overrides: Vec<de::IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(default)]
struct UPitch {
    #[serde(deserialize_with = "list")]
    data: Vec<UPitchPoint>,
    snap_first: bool,
}

impl Default for UPitch {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            snap_first: true,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UPitchPoint {
    x: f64,
    y: f64,
    #[serde(deserialize_with = "text")]
    shape: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UVibrato {
    /// Percent of the note, counted from its end.
    length: f64,
    /// Milliseconds per cycle.
    period: f64,
    /// Cents.
    depth: f64,
    /// Percent of the vibrato.
    #[serde(rename = "in")]
    fade_in: f64,
    /// Percent of the vibrato.
    #[serde(rename = "out")]
    fade_out: f64,
    /// Phase, percent of a cycle.
    shift: f64,
    /// Center offset, percent of depth.
    drift: f64,
    /// Tremolo amount, percent.
    vol_link: f64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UPhonemeExpression {
    #[serde(deserialize_with = "text")]
    abbr: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UCurve {
    #[serde(deserialize_with = "text")]
    abbr: String,
    #[serde(deserialize_with = "list")]
    xs: Vec<i64>,
    #[serde(deserialize_with = "list")]
    ys: Vec<f64>,
}

/// A sequence, with `null` read as empty.
fn list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// Any scalar as text, with `null` read as empty. YAML reads an unquoted
/// lyric such as `1` or a version such as `0.6` as a number.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    struct Text;
    impl<'de> Visitor<'de> for Text {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a string or other scalar")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_owned())
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_unit<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }
        fn visit_none<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }
        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
            text(d)
        }
    }
    d.deserialize_any(Text)
}
