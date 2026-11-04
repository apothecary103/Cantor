//! Classic UTAU sequence files (`.ust`): import (Shift-JIS or UTF-8) and
//! export (Shift-JIS, UST 1.2, Mode2 pitch).
//!
//! Mapping: `Length`/`NoteNum`/`Lyric` → notes (rests `R` become gaps);
//! `Tempo` → tempo points; `Velocity` → `vel`, `Intensity` → `vol`,
//! `Modulation` → `mod`, `Flags` → `flags`, `PreUtterance`/`VoiceOverlap`/
//! `StartPoint` → `timing`; Mode2 `PBS/PBW/PBY/PBM` → portamento and bend
//! points (see `pitch.rs`; the first point's y is relative to the previous
//! note when there is one, else to the note); `VBR` → vibrato. VCV-style
//! lyrics such as `a か` become lyric `か` with that exact alias.

use crate::common::{Diags, Enc, decode, finish, num};
use crate::pitch::{self, Pt, Vbr};
use crate::{ExportError, Exported, ImportError, Imported};
use cantor_core::{Note, NoteId, Project, TempoPoint, Tick, Timing, Track, TrackId};

const FORMAT: &str = "UST";

fn parse_err(message: impl Into<String>) -> ImportError {
    ImportError::Parse {
        format: FORMAT,
        message: message.into(),
    }
}

/// Splits UST text into `[#SECTION]`s of `key=value` pairs.
fn sections(text: &str) -> Vec<(&str, Vec<(&str, &str)>)> {
    let mut out: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix("[#").and_then(|l| l.strip_suffix(']')) {
            out.push((name, Vec::new()));
        } else if let Some((_, kv)) = out.last_mut() {
            match line.split_once('=') {
                Some((k, v)) => kv.push((k.trim(), v.trim())),
                None if !line.trim().is_empty() => kv.push((line.trim(), "")),
                None => {}
            }
        }
    }
    out
}

fn f(v: &str) -> Option<f64> {
    v.trim()
        .replace(',', ".")
        .parse()
        .ok()
        .filter(|x: &f64| x.is_finite())
}

/// Imports a UST file.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    let (text, enc) = decode(bytes);
    let secs = sections(&text);
    if !secs
        .iter()
        .any(|(n, _)| *n == "SETTING" || n.parse::<u32>().is_ok())
    {
        return Err(parse_err(
            "no [#SETTING] or note sections; this is not a UST file",
        ));
    }
    let mut d = Diags::default();
    if enc == Enc::Lossy {
        d.warn(
            "ust.encoding",
            "the file is neither valid UTF-8 nor Shift-JIS; undecodable bytes became U+FFFD",
        );
    }
    let mut p = Project::default();
    let mut track = Track {
        id: TrackId(1),
        name: "UST".into(),
        ..Track::default()
    };
    let mut bpm0 = None;
    let mut mode2 = false;
    for (name, kv) in &secs {
        match *name {
            "VERSION" | "TRACKEND" => {}
            "SETTING" => {
                for &(k, v) in kv {
                    match k {
                        "Tempo" => bpm0 = f(v),
                        "ProjectName" if !v.is_empty() => {
                            p.meta.title = v.into();
                            track.name = v.into();
                        }
                        "VoiceDir" => {
                            let dir = v.trim_end_matches(['/', '\\']);
                            track.singer = dir
                                .rsplit(['/', '\\', '%'])
                                .next()
                                .unwrap_or_default()
                                .to_owned();
                            if !track.singer.is_empty() {
                                d.info(
                                    "ust.singer",
                                    format!(
                                        "VoiceDir `{v}` became singer `{}`; check it matches a \
                                         voicebank id",
                                        track.singer
                                    ),
                                );
                            }
                        }
                        "Flags" => track.flags = v.into(),
                        "Mode2" => mode2 = v.eq_ignore_ascii_case("true"),
                        "Tool1" | "Tool2" if !v.is_empty() => d.info(
                            "ust.tools",
                            format!(
                                "{k}={v} is not used; Cantor renders with its built-in resampler"
                            ),
                        ),
                        "Tracks" | "OutFile" | "CacheDir" | "Tool1" | "Tool2" | "ProjectName" => {}
                        _ => d.count_info(
                            "ust.setting_ignored",
                            format!("setting `{k}` was ignored"),
                        ),
                    }
                }
            }
            "PREV" | "NEXT" | "INSERT" | "DELETE" => d.count_info(
                "ust.plugin_section",
                format!("plugin section [#{name}] was ignored"),
            ),
            _ if name.parse::<u32>().is_ok() => {}
            _ => d.count_info(
                "ust.section_ignored",
                format!("section [#{name}] was ignored"),
            ),
        }
    }
    let bpm0 = bpm0.unwrap_or_else(|| {
        d.warn(
            "ust.tempo_missing",
            "no Tempo in [#SETTING]; 120 BPM was assumed",
        );
        120.0
    });
    p.tempo = vec![TempoPoint {
        tick: 0,
        bpm: d.clamp("ust.tempo_clamped", "tempo (BPM)", bpm0, 10.0, 1000.0),
    }];

    let mut tick: Tick = 0;
    let mut prev_key: Option<u8> = None;
    let mut next_id = 1;
    for (_, kv) in secs.iter().filter(|(n, _)| n.parse::<u32>().is_ok()) {
        let get = |key: &str| kv.iter().rev().find(|(k, _)| *k == key).map(|(_, v)| *v);
        if let Some(bpm) = get("Tempo").and_then(f) {
            let bpm = d.clamp("ust.tempo_clamped", "tempo (BPM)", bpm, 10.0, 1000.0);
            p.tempo.retain(|t| t.tick != tick);
            p.tempo.push(TempoPoint { tick, bpm });
        }
        let length = get("Length")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|l| l.is_finite() && *l > 0.0)
            .map(|l| l.round() as Tick);
        let Some(length) = length else {
            d.count_warn(
                "ust.note_invalid",
                "notes without a positive Length were skipped",
            );
            continue;
        };
        let start = tick;
        tick += length;
        let lyric = get("Lyric").unwrap_or_default();
        if matches!(lyric, "R" | "r" | "") {
            prev_key = None;
            continue;
        }
        let Some(key) = get("NoteNum")
            .and_then(|v| v.trim().parse::<u8>().ok())
            .filter(|k| *k <= 127)
        else {
            d.count_warn(
                "ust.note_invalid",
                "notes without a NoteNum in 0..=127 were skipped",
            );
            prev_key = None;
            continue;
        };
        let mut n = Note::new(start, length, key, lyric);
        n.id = NoteId(next_id);
        next_id += 1;
        if let Some((_, cv)) = lyric.rsplit_once(' ')
            && !cv.is_empty()
        {
            n.alias = Some(lyric.into());
            n.lyric = cv.into();
            d.count_info(
                "ust.vcv_lyric",
                "VCV-style lyrics like `a か` became the syllable with that exact alias",
            );
        }
        let mut num_field = |key: &str, what: &str, lo: f64, hi: f64| {
            get(key)
                .and_then(f)
                .map(|v| d.clamp("ust.value_clamped", what, v, lo, hi))
        };
        n.vel = num_field("Velocity", "Velocity", 0.0, 200.0).unwrap_or(100.0);
        n.vol = num_field("Intensity", "Intensity", 0.0, 200.0).unwrap_or(100.0);
        n.mod_ = num_field("Modulation", "Modulation", 0.0, 100.0)
            .or_else(|| num_field("Moduration", "Modulation", 0.0, 100.0))
            .unwrap_or(0.0);
        n.timing = Timing {
            preutter_ms: num_field("PreUtterance", "PreUtterance (ms)", 0.0, 2000.0),
            overlap_ms: num_field("VoiceOverlap", "VoiceOverlap (ms)", -1000.0, 2000.0),
            offset_ms: num_field("StartPoint", "StartPoint (ms)", -1000.0, 1000.0).unwrap_or(0.0),
        };
        n.flags = get("Flags").unwrap_or_default().into();

        let mut pts = Vec::new();
        if let Some(pbs) = get("PBS").filter(|v| !v.is_empty()) {
            let mut it = pbs.split([';', ',']).map(f);
            let x0 = it.next().flatten().unwrap_or(0.0);
            let y0 = it.next().flatten().unwrap_or(0.0)
                + prev_key.map_or(0.0, |pk| (f64::from(pk) - f64::from(key)) * 10.0);
            let list = |k: &str| -> Vec<&str> {
                get(k)
                    .filter(|v| !v.is_empty())
                    .map(|v| v.split(',').collect())
                    .unwrap_or_default()
            };
            let (w, y, m) = (list("PBW"), list("PBY"), list("PBM"));
            let shape = |i: usize| pitch::shape_from(m.get(i).copied().unwrap_or("").trim());
            pts.push(Pt {
                x: x0,
                y: y0,
                shape: shape(0),
            });
            let mut x = x0;
            for (i, wi) in w.iter().enumerate() {
                x += f(wi).unwrap_or(0.0);
                let yi = y.get(i).and_then(|v| f(v)).unwrap_or(0.0);
                pts.push(Pt {
                    x,
                    y: yi,
                    shape: shape(i + 1),
                });
            }
        } else if !mode2 && get("PitchBend").is_some_and(|v| !v.is_empty()) {
            d.count_warn(
                "ust.mode1_pitch",
                "Mode1 pitch bends (PitchBend/PBStart) are not imported; the notes are flat",
            );
        }
        let (porta, points) = pitch::from_points(pts, key, prev_key, &mut d, "ust");
        n.pitch.portamento = porta;
        n.pitch.points = points;
        if let Some(vbr) = get("VBR").filter(|v| !v.is_empty()) {
            let v: Vec<f64> = vbr.split(',').map(|s| f(s).unwrap_or(0.0)).collect();
            let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
            n.pitch.vibrato = pitch::vibrato_from(
                Vbr {
                    length: at(0),
                    period: at(1),
                    depth: at(2),
                    fade_in: at(3),
                    fade_out: at(4),
                    phase: at(5),
                    height: at(6),
                },
                &mut d,
                "ust",
            );
        }
        for &(k, v) in kv {
            match k {
                "Length" | "Lyric" | "NoteNum" | "Tempo" | "Velocity" | "Intensity"
                | "Modulation" | "Moduration" | "PreUtterance" | "VoiceOverlap" | "StartPoint"
                | "Flags" | "PBS" | "PBW" | "PBY" | "PBM" | "VBR" | "PBType" | "PBStart"
                | "PitchBend" => {}
                _ if k.starts_with('@') => {} // UTAU's cached oto values
                "Envelope" => {
                    d.count_info("ust.envelope", "note volume envelopes were not imported")
                }
                "Label" if v.is_empty() => {}
                _ => d.count_info(
                    "ust.key_ignored",
                    format!("note field `{k}` was not imported"),
                ),
            }
        }
        track.notes.push(n);
        prev_key = Some(key);
    }
    p.tracks.push(track);
    finish(p, d)
}

/// Exports one track (default: the first) as a Shift-JIS UST 1.2 file.
pub fn export(project: &Project, track: Option<TrackId>) -> Result<Exported, ExportError> {
    let t = match track {
        Some(id) => project.track(id),
        None => project.tracks.first(),
    }
    .ok_or_else(|| ExportError::NoTrack(track.map_or("(no tracks)".into(), |t| t.to_string())))?;
    let mut d = Diags::default();
    let tempo = project.tempo_map();
    let head = vec![
        "[#VERSION]".to_owned(),
        "UST Version1.2".into(),
        "[#SETTING]".into(),
        format!("Tempo={:.2}", project.tempo[0].bpm),
        "Tracks=1".into(),
        format!("ProjectName={}", project.meta.title),
        format!("VoiceDir=%VOICE%{}", t.singer),
        "OutFile=".into(),
        "CacheDir=".into(),
        "Tool1=wavtool.exe".into(),
        "Tool2=resampler.exe".into(),
        "Mode2=True".into(),
        format!("Flags={}", t.flags),
    ];
    if project.time_sig.iter().any(|s| (s.num, s.den) != (4, 4)) {
        d.info(
            "ust.time_sig",
            "UST has no time signatures; they were left out",
        );
    }
    if !t.curves.is_empty() {
        let names: Vec<_> = t.curves.keys().map(|k| k.as_str()).collect();
        d.warn(
            "ust.curves",
            format!(
                "UST has no expression curves; {} were left out",
                names.join(", ")
            ),
        );
    }

    // Entries: (start, length, fields). Rests fill gaps, split at tempo
    // changes so those stay exact.
    let changes: Vec<Tick> = project.tempo[1..].iter().map(|p| p.tick).collect();
    let mut entries: Vec<(Tick, Tick, Vec<String>)> = Vec::new();
    let mut tick: Tick = 0;
    let mut prev_key: Option<u8> = None;
    for n in &t.notes {
        while tick < n.start {
            let end = changes
                .iter()
                .copied()
                .find(|&x| x > tick && x < n.start)
                .unwrap_or(n.start);
            entries.push((
                tick,
                end - tick,
                vec!["Lyric=R".into(), format!("NoteNum={}", n.key)],
            ));
            tick = end;
            prev_key = None;
        }
        let dur_ms = (tempo.tick_to_sec(n.end()) - tempo.tick_to_sec(n.start)) * 1000.0;
        let pts = pitch::to_points(n, prev_key, dur_ms, &mut d, "ust");
        if n.is_extension() {
            d.count_info(
                "ust.extension_lyric",
                "extension lyrics (`+`) were kept; classic UTAU needs a real lyric there",
            );
        }
        let opt = |v: Option<f64>| v.map(num).unwrap_or_default();
        let mut fields = vec![
            format!("Lyric={}", n.alias.as_deref().unwrap_or(&n.lyric)),
            format!("NoteNum={}", n.key),
            format!("PreUtterance={}", opt(n.timing.preutter_ms)),
            format!("VoiceOverlap={}", opt(n.timing.overlap_ms)),
            format!("Intensity={}", num(n.vol)),
            format!("Modulation={}", num(n.mod_)),
            format!("Velocity={}", num(n.vel)),
            format!("StartPoint={}", num(n.timing.offset_ms)),
            format!("Flags={}", n.flags),
        ];
        if let Some(first) = pts.first() {
            let rel = prev_key.map_or(0.0, |pk| (f64::from(pk) - f64::from(n.key)) * 10.0);
            let w: Vec<_> = pts.windows(2).map(|w| num(w[1].x - w[0].x)).collect();
            let y: Vec<_> = pts[1..].iter().map(|p| num(p.y)).collect();
            let m: Vec<_> = pts.iter().map(|p| pitch::shape_ust(p.shape)).collect();
            fields.push(format!("PBS={};{}", num(first.x), num(first.y - rel)));
            fields.push(format!("PBW={}", w.join(",")));
            fields.push(format!("PBY={}", y.join(",")));
            fields.push(format!("PBM={}", m.join(",")));
        }
        if let Some(v) = pitch::vibrato_to(n.pitch.vibrato.as_ref(), &mut d, "ust") {
            let vals = [
                v.length, v.period, v.depth, v.fade_in, v.fade_out, v.phase, v.height, 0.0,
            ];
            fields.push(format!("VBR={}", vals.map(num).join(",")));
        }
        entries.push((n.start, n.length, fields));
        tick = n.end();
        prev_key = Some(n.key);
    }
    let mut out = head;
    let mut pending = project.tempo[1..].iter().peekable();
    for (i, (start, len, fields)) in entries.into_iter().enumerate() {
        out.push(format!("[#{i:04}]"));
        out.push(format!("Length={len}"));
        out.extend(fields);
        let mut bpm = None;
        while let Some(p) = pending.next_if(|p| p.tick < start + len) {
            if p.tick != start {
                d.count_warn(
                    "ust.tempo_moved",
                    "UST changes tempo only at note starts; tempo changes inside notes were \
                     moved to the note's start",
                );
            }
            bpm = Some(p.bpm);
        }
        if let Some(bpm) = bpm {
            out.push(format!("Tempo={bpm:.2}"));
        }
    }
    if pending.next().is_some() {
        d.info(
            "ust.tempo_after_end",
            "tempo changes after the last note were left out",
        );
    }
    out.push("[#TRACKEND]".into());
    let mut text = out.join("\r\n");
    text.push_str("\r\n");
    let (bytes, _, unmappable) = encoding_rs::SHIFT_JIS.encode(&text);
    if unmappable {
        d.warn(
            "ust.encoding",
            "some characters have no Shift-JIS form and were written as &#NNNN; references",
        );
    }
    Ok(Exported {
        bytes: bytes.into_owned(),
        diagnostics: d.into_vec(),
    })
}
