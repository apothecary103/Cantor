//! OpenUtau projects (`.ustx`, YAML): import and export.
//!
//! Import: each OpenUtau track becomes a track; the notes of its voice
//! parts land on it at absolute ticks (part position + note position).
//! Pitch points → portamento/bend points, vibrato → vibrato, phoneme
//! expressions `vel`/`vol`/`mod` of the first phoneme → note fields;
//! curves `pitd` (cents) → `pitd`, `dyn` (0.1 dB) → `dyn` (dB), `genc` →
//! `gen`, `brec` → `bre`, `tenc` → `bri`, `shfc` (cents) → `tone`
//! (semitones). Every track uses Cantor's UTAU resampler; DiffSinger and
//! other renderers are reported.
//!
//! Export writes USTX 0.6 with one voice part per track starting at 0.

use crate::common::{Diags, finish, rescale};
use crate::pitch::{self, Pt, Vbr};
use crate::{ExportError, Exported, ImportError, Imported};
use cantor_core::params::known;
use cantor_core::{
    Curve, Interp, Key, Mix, Note, NoteId, ParamId, Phonemizer, Project, Scale, TempoPoint, Tick,
    TimeSigPoint, Track, TrackId,
};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

const FORMAT: &str = "OpenUtau USTX";

/// Source ticks beyond this are rejected, so arithmetic never overflows.
const MAX_TICK: i64 = 1 << 40;

/// Curve mapping: OpenUtau abbreviation, Cantor parameter, scale
/// (Cantor = OpenUtau × scale).
const CURVES: &[(&str, &str, f64)] = &[
    ("pitd", known::PITD, 1.0),
    ("dyn", known::DYN, 0.1),
    ("genc", known::GEN, 1.0),
    ("brec", known::BRE, 1.0),
    ("tenc", known::BRI, 1.0),
    ("shfc", known::TONE, 0.01),
];

fn parse_err(message: impl Into<String>) -> ImportError {
    ImportError::Parse {
        format: FORMAT,
        message: message.into(),
    }
}

/// Imports a USTX file.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|e| parse_err(format!("not UTF-8: {e}")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let doc: Ustx = serde_saphyr::from_str(text).map_err(|e| parse_err(e.to_string()))?;
    if doc.ustx_version.is_empty() && doc.tracks.is_empty() && doc.voice_parts.is_empty() {
        return Err(ImportError::Unsupported {
            format: FORMAT,
            message: "no `ustx_version`, tracks or voice parts; this is not a USTX project".into(),
        });
    }
    let res = doc.resolution.unwrap_or(480);
    let ppq = u32::try_from(res)
        .ok()
        .filter(|&r| r > 0)
        .ok_or_else(|| parse_err(format!("resolution {res} is not a positive integer")))?;
    let mut d = Diags::default();
    let mut inexact = 0;
    let mut tick = |src: i64| -> Result<Tick, ImportError> {
        if src.unsigned_abs() > MAX_TICK as u64 {
            return Err(parse_err(format!("tick value {src} is out of range")));
        }
        let (t, exact) = rescale(src, ppq);
        inexact += usize::from(!exact);
        Ok(t)
    };

    let mut p = Project::default();
    p.meta.title = doc.name.clone();
    p.meta.comment = doc.comment.clone();
    // Tempo.
    let mut tempos: Vec<(Tick, f64)> = Vec::new();
    if doc.tempos.is_empty() {
        tempos.extend(doc.bpm.map(|b| (0, b)));
    }
    for t in &doc.tempos {
        tempos.push((tick(t.position)?, t.bpm));
    }
    tempos.retain(|&(t, bpm)| {
        let ok = t >= 0 && bpm.is_finite() && (10.0..=1000.0).contains(&bpm);
        if !ok {
            d.count_warn(
                "ustx.tempo_invalid",
                "tempos outside 10..=1000 BPM or before the start were ignored",
            );
        }
        ok
    });
    tempos.sort_by_key(|t| t.0);
    p.tempo = tempos
        .iter()
        .map(|&(tick, bpm)| TempoPoint { tick, bpm })
        .collect();
    if p.tempo.is_empty() {
        d.warn(
            "ustx.tempo_missing",
            "the file has no usable tempo; 120 BPM was assumed",
        );
    }
    // Time signatures.
    let legacy = UTimeSig {
        bar_position: 0,
        beat_per_bar: doc.beat_per_bar.unwrap_or(4),
        beat_unit: doc.beat_unit.unwrap_or(4),
    };
    let sigs = if doc.time_signatures.is_empty() {
        std::slice::from_ref(&legacy)
    } else {
        &doc.time_signatures[..]
    };
    p.time_sig = Vec::new();
    for s in sigs {
        match (
            u32::try_from(s.bar_position),
            u8::try_from(s.beat_per_bar),
            u8::try_from(s.beat_unit),
        ) {
            (Ok(bar), Ok(num), Ok(den))
                if (1..=32).contains(&num) && [1, 2, 4, 8, 16, 32].contains(&den) =>
            {
                p.time_sig.push(TimeSigPoint { bar, num, den });
            }
            _ => d.count_warn(
                "ustx.time_sig_invalid",
                "invalid time signatures were ignored",
            ),
        }
    }
    if let Some(k) = doc.key {
        match u8::try_from(k).ok().filter(|k| *k < 12) {
            Some(tonic) => {
                p.key = Some(Key {
                    tonic,
                    scale: Scale::Major,
                })
            }
            None => d.warn(
                "ustx.key_invalid",
                format!("key {k} is not 0..=11 and was ignored"),
            ),
        }
    }

    // Tracks.
    let mut notes: Vec<Vec<Note>> = vec![Vec::new(); doc.tracks.len()];
    let mut curves: Vec<BTreeMap<&str, BTreeMap<Tick, f64>>> =
        vec![BTreeMap::new(); doc.tracks.len()];
    for (i, t) in doc.tracks.iter().enumerate() {
        p.tracks.push(track(t, i, &mut d));
    }
    let mut next_id = 1u64;
    for vp in &doc.voice_parts {
        let Some(ti) = usize::try_from(vp.track_no)
            .ok()
            .filter(|&i| i < p.tracks.len())
        else {
            d.warn(
                "ustx.part_track_missing",
                format!(
                    "voice part `{}` is on missing track {}; its {} notes were not imported",
                    vp.name,
                    vp.track_no,
                    vp.notes.len()
                ),
            );
            continue;
        };
        tick(vp.position)?;
        if !vp.comment.is_empty() {
            d.info(
                "ustx.part_comment",
                format!(
                    "voice part `{}` comment was not imported: {:?}",
                    vp.name, vp.comment
                ),
            );
        }
        let mut order: Vec<&UNote> = vp.notes.iter().collect();
        order.sort_by_key(|n| n.position);
        let mut prev: Option<(i64, u8)> = None; // (source end, key)
        for n in order {
            let start = tick(vp.position.saturating_add(n.position))?;
            let end = tick(
                vp.position
                    .saturating_add(n.position)
                    .saturating_add(n.duration),
            )?;
            let Some(key) = u8::try_from(n.tone).ok().filter(|k| *k <= 127) else {
                d.count_warn(
                    "ustx.note_invalid",
                    "notes with a tone outside 0..=127 were skipped",
                );
                continue;
            };
            if start < 0 || end <= start {
                d.count_warn(
                    "ustx.note_invalid",
                    "notes before the song start or without duration were skipped",
                );
                continue;
            }
            let lyric = if n.lyric.starts_with('+') && n.lyric != "+" {
                d.count_info(
                    "ustx.lyric_extension",
                    format!("extension lyric `{}` became `+`", n.lyric),
                );
                "+".to_owned()
            } else {
                n.lyric.clone()
            };
            let mut note = Note::new(start, end - start, key, lyric);
            note.id = NoteId(next_id);
            next_id += 1;
            let adj = prev.filter(|pv| pv.0 == n.position).map(|pv| pv.1);
            let pts: Vec<Pt> = n
                .pitch
                .data
                .iter()
                .enumerate()
                .map(|(i, q)| Pt {
                    x: q.x,
                    y: if i == 0 && n.pitch.snap_first {
                        adj.map_or(0.0, |k| (f64::from(k) - f64::from(key)) * 10.0)
                    } else {
                        q.y
                    },
                    shape: pitch::shape_from(&q.shape),
                })
                .collect();
            let (porta, points) = pitch::from_points(pts, key, adj, &mut d, "ustx");
            note.pitch.portamento = porta;
            note.pitch.points = points;
            let v = &n.vibrato;
            note.pitch.vibrato = pitch::vibrato_from(
                Vbr {
                    length: v.length,
                    period: v.period,
                    depth: v.depth,
                    fade_in: v.fade_in,
                    fade_out: v.fade_out,
                    phase: v.shift,
                    height: v.drift,
                },
                &mut d,
                "ustx",
            );
            if v.vol_link != 0.0 && note.pitch.vibrato.is_some() {
                d.count_warn(
                    "ustx.vibrato_vol_link",
                    "vibrato volume links (tremolo) were not imported",
                );
            }
            for e in &n.phoneme_expressions {
                let first = e.index.unwrap_or(0) == 0;
                match (e.abbr.as_str(), first) {
                    ("vel", true) => {
                        note.vel = d.clamp("ustx.value_clamped", "vel", e.value, 0.0, 200.0)
                    }
                    ("vol", true) => {
                        note.vol = d.clamp("ustx.value_clamped", "vol", e.value, 0.0, 200.0)
                    }
                    ("mod", true) => {
                        note.mod_ = d.clamp("ustx.value_clamped", "mod", e.value, 0.0, 100.0)
                    }
                    (a, _) => d.count_warn(
                        "ustx.expression_unsupported",
                        format!(
                            "phoneme expression `{a}` (or one on a later phoneme) was not imported"
                        ),
                    ),
                }
            }
            if !n.phoneme_overrides.is_empty() {
                d.count_warn(
                    "ustx.phoneme_override",
                    "per-phoneme overrides (symbol, offset, preutterance, overlap) were not imported",
                );
            }
            prev = Some((n.position.saturating_add(n.duration), key));
            notes[ti].push(note);
        }
        for c in &vp.curves {
            if c.xs.len() != c.ys.len() {
                d.warn(
                    "ustx.curve_malformed",
                    format!(
                        "curve `{}` has {} xs but {} ys; extra values were ignored",
                        c.abbr,
                        c.xs.len(),
                        c.ys.len()
                    ),
                );
            }
            let Some(&(_, param, scale)) = CURVES.iter().find(|m| m.0 == c.abbr) else {
                if !c.xs.is_empty() {
                    d.count_warn(
                        "ustx.curve_unsupported",
                        format!(
                            "curve `{}` has no Cantor equivalent and was not imported",
                            c.abbr
                        ),
                    );
                }
                continue;
            };
            let desc = ParamId::new(param).descriptor().expect("known curve");
            for (&x, &y) in c.xs.iter().zip(&c.ys) {
                let t = tick(vp.position.saturating_add(x))?;
                let v = d.clamp(
                    "ustx.curve_clamped",
                    &format!("{} value", c.abbr),
                    y * scale,
                    desc.min,
                    desc.max,
                );
                if curves[ti]
                    .entry(param)
                    .or_default()
                    .insert(t.max(0), v)
                    .is_some_and(|old| old != v)
                {
                    d.count_warn(
                        "ustx.curve_duplicate",
                        format!("`{}` curve points sharing a tick were merged; the last in file order was kept", c.abbr),
                    );
                }
            }
        }
    }
    for (t, (mut ns, cs)) in p.tracks.iter_mut().zip(notes.into_iter().zip(curves)) {
        ns.sort_by_key(|n| (n.start, n.id));
        let mut kept: Vec<Note> = Vec::with_capacity(ns.len());
        for n in ns {
            if let Some(prev) = kept.last_mut() {
                if prev.start == n.start {
                    d.count_warn(
                        "ustx.overlap",
                        "notes starting together with another (overlapping parts) were dropped",
                    );
                    continue;
                }
                if prev.end() > n.start {
                    prev.length = n.start - prev.start;
                    d.count_warn(
                        "ustx.overlap",
                        "overlapping notes were shortened to end where the next starts",
                    );
                }
            }
            kept.push(n);
        }
        t.notes = kept;
        for (param, pts) in cs {
            let points: Vec<_> = pts.into_iter().collect();
            if points.iter().any(|p| p.1 != 0.0) {
                t.curves.insert(
                    ParamId::new(param),
                    Curve {
                        points,
                        interp: Interp::Linear,
                    },
                );
            }
        }
    }
    if !doc.wave_parts.is_empty() {
        d.warn(
            "ustx.wave_part",
            format!(
                "{} audio (wave) parts were not imported; set the reference audio instead",
                doc.wave_parts.len()
            ),
        );
    }
    if inexact > 0 {
        d.warn(
            "ustx.resolution_inexact",
            format!("resolution {ppq}: {inexact} tick values were rounded to 480 per quarter"),
        );
    }
    finish(p, d)
}

fn track(t: &UTrack, i: usize, d: &mut Diags) -> Track {
    let name = if t.track_name.is_empty() {
        format!("Track {}", i + 1)
    } else {
        t.track_name.clone()
    };
    let settings = t.renderer_settings.clone().unwrap_or_default();
    let renderer = if settings.renderer.is_empty() {
        &t.renderer
    } else {
        &settings.renderer
    };
    match renderer.to_ascii_uppercase().as_str() {
        "" | "CLASSIC" | "WORLDLINE-R" => {}
        other => d.warn(
            "ustx.renderer",
            format!(
                "track `{name}` uses renderer {other}; Cantor renders it with its UTAU resampler"
            ),
        ),
    }
    if !settings.resampler.is_empty() || !settings.wavtool.is_empty() {
        d.info(
            "ustx.resampler",
            format!("track `{name}`: resampler/wavtool settings are not used"),
        );
    }
    let ph = &t.phonemizer;
    let phonemizer = if ph.contains("JapaneseCVVC") {
        Phonemizer::JpCvvc
    } else if ph.contains("JapaneseVCV") {
        Phonemizer::JpVcv
    } else if ph.contains("JapaneseCV") || ph.contains("JapanesePresamp") {
        Phonemizer::JpCv
    } else if ph.contains("Arpasing") {
        Phonemizer::EnArpasing
    } else if ph.is_empty() || ph.ends_with("DefaultPhonemizer") {
        Phonemizer::Raw
    } else {
        d.info(
            "ustx.phonemizer",
            format!("track `{name}`: phonemizer `{ph}` has no Cantor equivalent; auto is used"),
        );
        Phonemizer::Auto
    };
    Track {
        id: TrackId(i as u64 + 1),
        name,
        singer: t.singer.clone(),
        phonemizer,
        mix: Mix {
            gain_db: d.clamp(
                "ustx.mix_clamped",
                "track volume (dB)",
                t.volume,
                -60.0,
                12.0,
            ),
            pan: d.clamp("ustx.mix_clamped", "track pan", t.pan / 100.0, -1.0, 1.0),
            mute: t.mute,
            solo: t.solo,
        },
        ..Track::default()
    }
}

/// Exports the project as an OpenUtau USTX (0.6) file.
pub fn export(project: &Project) -> Result<Exported, ExportError> {
    let mut d = Diags::default();
    let tempo = project.tempo_map();
    let sigs = project.time_sig_map();
    if project.key.is_some_and(|k| k.scale != Scale::Major) {
        d.info(
            "ustx.scale",
            "OpenUtau keys have no scale; only the tonic was written",
        );
    }
    let mut doc = Ustx {
        name: project.meta.title.clone(),
        comment: project.meta.comment.clone(),
        output_dir: "Vocal".into(),
        cache_dir: "UCache".into(),
        ustx_version: "0.6".into(),
        resolution: Some(480),
        bpm: Some(project.tempo[0].bpm),
        beat_per_bar: Some(i64::from(project.time_sig[0].num)),
        beat_unit: Some(i64::from(project.time_sig[0].den)),
        key: project.key.map(|k| i64::from(k.tonic)),
        time_signatures: project
            .time_sig
            .iter()
            .map(|s| UTimeSig {
                bar_position: i64::from(s.bar),
                beat_per_bar: i64::from(s.num),
                beat_unit: i64::from(s.den),
            })
            .collect(),
        tempos: project
            .tempo
            .iter()
            .map(|t| UTempo {
                position: t.tick,
                bpm: t.bpm,
            })
            .collect(),
        ..Ustx::default()
    };
    for (ti, t) in project.tracks.iter().enumerate() {
        let phonemizer = match t.phonemizer {
            Phonemizer::JpCv => "OpenUtau.Plugin.Builtin.JapaneseCVPhonemizer",
            Phonemizer::JpVcv => "OpenUtau.Plugin.Builtin.JapaneseVCVPhonemizer",
            Phonemizer::JpCvvc => "OpenUtau.Plugin.Builtin.JapaneseCVVCPhonemizer",
            Phonemizer::EnArpasing => "OpenUtau.Plugin.Builtin.ArpasingPhonemizer",
            Phonemizer::Raw | Phonemizer::Auto => "OpenUtau.Core.DefaultPhonemizer",
        };
        if !t.effects.is_empty() {
            d.info(
                "ustx.effects",
                format!("track `{}`: effects were left out", t.name),
            );
        }
        if !t.flags.is_empty() {
            d.info(
                "ustx.flags",
                format!(
                    "track `{}`: resampler flags `{}` were left out",
                    t.name, t.flags
                ),
            );
        }
        doc.tracks.push(UTrack {
            singer: t.singer.clone(),
            phonemizer: phonemizer.into(),
            renderer_settings: Some(URendererSettings {
                renderer: "CLASSIC".into(),
                ..Default::default()
            }),
            track_name: t.name.clone(),
            mute: t.mix.mute,
            solo: t.mix.solo,
            volume: t.mix.gain_db,
            pan: t.mix.pan * 100.0,
            ..UTrack::default()
        });
        let mut notes = Vec::new();
        let mut prev: Option<&Note> = None;
        for n in &t.notes {
            let adj = prev.filter(|p| p.end() == n.start).map(|p| p.key);
            let dur_ms = (tempo.tick_to_sec(n.end()) - tempo.tick_to_sec(n.start)) * 1000.0;
            let pts = pitch::to_points(n, adj, dur_ms, &mut d, "ustx");
            let snap = adj.is_some()
                && pts.first().is_some_and(|p| {
                    Some(p.y) == adj.map(|k| (f64::from(k) - f64::from(n.key)) * 10.0)
                });
            let v = pitch::vibrato_to(n.pitch.vibrato.as_ref(), &mut d, "ustx");
            let mut exprs = Vec::new();
            for (abbr, value, default) in [
                ("vel", n.vel, 100.0),
                ("vol", n.vol, 100.0),
                ("mod", n.mod_, 0.0),
            ] {
                if value != default {
                    exprs.push(UPhonemeExpression {
                        index: Some(0),
                        abbr: abbr.into(),
                        value,
                    });
                }
            }
            if n.alias.is_some() || !n.flags.is_empty() || !n.timing.is_default() {
                d.count_info("ustx.note_fields", "note aliases, flags and timing overrides were left out (OpenUtau has no direct equivalent)");
            }
            notes.push(UNote {
                position: n.start,
                duration: n.length,
                tone: i64::from(n.key),
                lyric: n.lyric.clone(),
                pitch: UPitch {
                    data: pts
                        .iter()
                        .map(|q| UPitchPoint {
                            x: q.x,
                            y: q.y,
                            shape: pitch::shape_ustx(q.shape).into(),
                        })
                        .collect(),
                    snap_first: snap,
                },
                vibrato: match v {
                    Some(v) => UVibrato {
                        length: v.length,
                        period: v.period,
                        depth: v.depth,
                        fade_in: v.fade_in,
                        fade_out: v.fade_out,
                        shift: v.phase,
                        drift: v.height,
                        vol_link: 0.0,
                    },
                    None => UVibrato {
                        period: 175.0,
                        depth: 25.0,
                        fade_in: 10.0,
                        fade_out: 10.0,
                        ..UVibrato::default()
                    },
                },
                phoneme_expressions: exprs,
                phoneme_overrides: Vec::new(),
            });
            prev = Some(n);
        }
        let mut ucurves = Vec::new();
        for (param, c) in &t.curves {
            let Some(&(abbr, _, scale)) = CURVES.iter().find(|m| m.1 == param.as_str()) else {
                d.warn(
                    "ustx.curve",
                    format!("curve `{param}` has no OpenUtau equivalent"),
                );
                continue;
            };
            if c.interp != Interp::Linear {
                d.count_info(
                    "ustx.curve_interp",
                    "OpenUtau curves are linear; step/smooth curves were sampled every 5 ticks",
                );
            }
            let pts: Vec<(Tick, f64)> = if c.interp == Interp::Linear || c.points.len() < 2 {
                c.points.clone()
            } else {
                let (a, b) = (c.points[0].0, c.points[c.points.len() - 1].0);
                (a..=b)
                    .step_by(5)
                    .map(|x| (x, c.value_at(x).unwrap_or(0.0)))
                    .collect()
            };
            ucurves.push(UCurve {
                abbr: abbr.into(),
                xs: pts.iter().map(|p| p.0).collect(),
                ys: pts.iter().map(|p| (p.1 / scale).round()).collect(),
            });
        }
        let end = t.notes.iter().map(Note::end).max().unwrap_or(0);
        let (bar, off) = sigs.tick_to_bar(end);
        let duration = if off == 0 {
            end
        } else {
            sigs.bar_to_tick(bar + 1)
        }
        .max(1920);
        doc.voice_parts.push(UVoicePart {
            name: t.name.clone(),
            comment: String::new(),
            track_no: ti as i64,
            position: 0,
            duration: Some(duration),
            notes,
            curves: ucurves,
        });
    }
    let yaml = serde_saphyr::to_string(&doc).map_err(|e| ExportError::Write {
        format: FORMAT,
        message: e.to_string(),
    })?;
    Ok(Exported {
        bytes: yaml.into_bytes(),
        diagnostics: d.into_vec(),
    })
}

// ---------------------------------------------------------------------------
// USTX structs: lenient on read (unknown fields ignored, missing ones
// defaulted, null lists empty), in OpenUtau's field order on write.

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Ustx {
    #[serde(deserialize_with = "text")]
    name: String,
    #[serde(deserialize_with = "text")]
    comment: String,
    #[serde(deserialize_with = "text")]
    output_dir: String,
    #[serde(deserialize_with = "text")]
    cache_dir: String,
    #[serde(deserialize_with = "text")]
    ustx_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    beat_per_bar: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    beat_unit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<i64>,
    #[serde(deserialize_with = "list")]
    time_signatures: Vec<UTimeSig>,
    #[serde(deserialize_with = "list")]
    tempos: Vec<UTempo>,
    #[serde(deserialize_with = "list")]
    tracks: Vec<UTrack>,
    #[serde(deserialize_with = "list")]
    voice_parts: Vec<UVoicePart>,
    #[serde(deserialize_with = "list", skip_serializing)]
    wave_parts: Vec<de::IgnoredAny>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UTempo {
    position: i64,
    bpm: f64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UTimeSig {
    bar_position: i64,
    beat_per_bar: i64,
    beat_unit: i64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UTrack {
    #[serde(deserialize_with = "text")]
    singer: String,
    #[serde(deserialize_with = "text")]
    phonemizer: String,
    #[serde(deserialize_with = "text", skip_serializing)]
    renderer: String,
    renderer_settings: Option<URendererSettings>,
    #[serde(deserialize_with = "text")]
    track_name: String,
    mute: bool,
    solo: bool,
    volume: f64,
    pan: f64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct URendererSettings {
    #[serde(deserialize_with = "text")]
    renderer: String,
    #[serde(deserialize_with = "text", skip_serializing_if = "String::is_empty")]
    resampler: String,
    #[serde(deserialize_with = "text", skip_serializing_if = "String::is_empty")]
    wavtool: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UVoicePart {
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<i64>,
    #[serde(deserialize_with = "text")]
    name: String,
    #[serde(deserialize_with = "text")]
    comment: String,
    track_no: i64,
    position: i64,
    #[serde(deserialize_with = "list")]
    notes: Vec<UNote>,
    #[serde(deserialize_with = "list")]
    curves: Vec<UCurve>,
}

#[derive(Serialize, Deserialize, Default)]
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
    #[serde(deserialize_with = "list", skip_serializing)]
    phoneme_overrides: Vec<de::IgnoredAny>,
}

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UPitchPoint {
    x: f64,
    y: f64,
    #[serde(deserialize_with = "text")]
    shape: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UVibrato {
    length: f64,
    period: f64,
    depth: f64,
    #[serde(rename = "in")]
    fade_in: f64,
    #[serde(rename = "out")]
    fade_out: f64,
    shift: f64,
    drift: f64,
    vol_link: f64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UPhonemeExpression {
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<i64>,
    #[serde(deserialize_with = "text")]
    abbr: String,
    value: f64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct UCurve {
    #[serde(deserialize_with = "list")]
    xs: Vec<i64>,
    #[serde(deserialize_with = "list")]
    ys: Vec<f64>,
    #[serde(deserialize_with = "text")]
    abbr: String,
}

/// A sequence, with `null` read as empty.
fn list<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// Any scalar as text (YAML reads lyric `1` or version `0.6` as numbers),
/// with `null` read as empty.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    struct Text;
    impl<'de> Visitor<'de> for Text {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a scalar")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_owned())
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
