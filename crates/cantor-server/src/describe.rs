//! Compact, token-efficient text descriptions of projects for agents.

use cantor_core::{Curve, PhraseId, Project, Revision, TickRange, Track, TrackId, phrase_spans};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// How much a range description includes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// Notes only.
    Notes,
    /// Notes and curve summaries.
    #[default]
    Curves,
    /// Notes, curve summaries and every curve point.
    Full,
}

type Phonemizer<'a> = &'a dyn Fn(&Track) -> Option<crate::engine::Voice>;

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// "C4", "A#3+12" (cents above the nearest semitone below 50).
pub fn pitch_name(cents: f64) -> String {
    let semis = (cents / 100.0).round();
    let off = (cents - semis * 100.0).round() as i64;
    let midi = semis as i64;
    let name = format!(
        "{}{}",
        NAMES[midi.rem_euclid(12) as usize],
        midi.div_euclid(12) - 1
    );
    match off {
        0 => name,
        o => format!("{name}{o:+}"),
    }
}

/// The whole project.
pub fn project(p: &Project, rev: Revision, phon: Phonemizer<'_>) -> String {
    let tempo = p.tempo();
    let end_tick = p
        .notes()
        .map(|(_, part, n)| part.start + n.start + n.length)
        .max()
        .unwrap_or(0);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Project \"{}\" rev {} · {} · {} · {:.1} s",
        p.meta.title,
        rev.0,
        tempo_text(p),
        meter_text(p),
        tempo.tick_to_sec(end_tick)
    );
    if !p.sections.is_empty() {
        let secs: Vec<String> = p
            .sections
            .iter()
            .map(|x| {
                format!(
                    "{} {}–{} ({:.1}–{:.1}s)",
                    x.name,
                    x.start,
                    x.end,
                    tempo.tick_to_sec(x.start),
                    tempo.tick_to_sec(x.end)
                )
            })
            .collect();
        let _ = writeln!(s, "Sections: {}", secs.join(", "));
    }
    for t in &p.tracks {
        track_text(&mut s, p, t, None, Detail::Curves, phon);
    }
    s
}

/// A tick range, optionally one track.
pub fn range(
    p: &Project,
    rev: Revision,
    track: Option<TrackId>,
    range: TickRange,
    detail: Detail,
    phon: Phonemizer<'_>,
) -> String {
    let tempo = p.tempo();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Range {}–{} ({:.2}–{:.2}s) rev {} · {}",
        range.start,
        range.end,
        tempo.tick_to_sec(range.start),
        tempo.tick_to_sec(range.end),
        rev.0,
        tempo_text(p)
    );
    for t in p
        .tracks
        .iter()
        .filter(|t| track.is_none_or(|id| id == t.id))
    {
        track_text(&mut s, p, t, Some(range), detail, phon);
    }
    s
}

fn tempo_text(p: &Project) -> String {
    match p.tempo_map.as_slice() {
        [one] => format!("{} BPM", one.bpm),
        many => format!(
            "{} BPM (+{} changes)",
            many.first().map_or(120.0, |t| t.bpm),
            many.len() - 1
        ),
    }
}

fn meter_text(p: &Project) -> String {
    p.time_sig_map.first().map_or("4/4".into(), |t| {
        format!("{}/{}", t.numerator, t.denominator)
    })
}

fn track_text(
    s: &mut String,
    p: &Project,
    t: &Track,
    range: Option<TickRange>,
    detail: Detail,
    phon: Phonemizer<'_>,
) {
    let tempo = p.tempo();
    let _ = writeln!(
        s,
        "Track {} \"{}\" singer={} backend={}{}{}",
        t.id.0,
        t.name,
        t.singer.id,
        t.backend.0,
        if t.mix.mute { " muted" } else { "" },
        if t.mix.gain_db != 0.0 {
            format!(" gain {:+.1} dB", t.mix.gain_db)
        } else {
            String::new()
        }
    );
    let backend = phon(t);
    let notes: std::collections::HashMap<_, _> = t
        .parts
        .iter()
        .flat_map(|part| part.notes.iter().map(move |n| (n.id, (part.start, n))))
        .collect();
    for span in phrase_spans(p, t) {
        if range.is_some_and(|r| span.end <= r.start || span.start >= r.end) {
            continue;
        }
        let lane = p.takes.get(&span.id);
        let _ = writeln!(
            s,
            "  Phrase {} ({:.2}–{:.2}s){}{}",
            span.id.0,
            tempo.tick_to_sec(span.start),
            tempo.tick_to_sec(span.end),
            p.seeds
                .get(&span.id)
                .map_or(String::new(), |x| format!(" seed {x}")),
            lane.map_or(String::new(), |l| format!(
                " takes {} selected {}",
                l.takes.len(),
                l.selected.map_or("original".into(), |x| x.0.to_string())
            ))
        );
        let mut items = Vec::new();
        for id in &span.notes {
            let (pstart, n) = notes[id];
            let start = pstart + n.start;
            if range.is_some_and(|r| start + n.length <= r.start || start >= r.end) {
                continue;
            }
            let phonemes = match &n.phonemes {
                Some(ov) => Some(ov.iter().map(|x| x.symbol.clone()).collect::<Vec<_>>()),
                None => backend
                    .as_ref()
                    .and_then(|b| b.phonemes_for(&n.lyric, n.lang.as_ref().map(|l| l.0.as_str()))),
            };
            let mut item = format!(
                "#{} {} \"{}\"{} {:.2}s+{:.2} [{}+{}]",
                n.id.0,
                pitch_name(n.pitch_cents),
                n.lyric,
                phonemes.map_or(String::new(), |ph| format!(" /{}/", ph.join(" "))),
                tempo.tick_to_sec(start),
                tempo.tick_to_sec(start + n.length) - tempo.tick_to_sec(start),
                start,
                n.length
            );
            if let Some(v) = &n.expr.vibrato {
                let _ = write!(item, " vib {:.0}c@{:.1}Hz", v.depth_cents, v.rate_hz);
            }
            items.push(item);
        }
        for chunk in items.chunks(4) {
            let _ = writeln!(s, "    {}", chunk.join(" · "));
        }
    }
    if detail != Detail::Notes {
        for (param, c) in &t.curves {
            for (layer, curve) in [("user Δ", &c.user_delta), ("model", &c.model_prediction)] {
                if let Some(text) = curve_text(curve, range, detail == Detail::Full) {
                    let _ = writeln!(s, "  Curve {param} {layer}: {text}");
                }
            }
        }
    }
    let _ = PhraseId(0);
}

/// Key points (first, last, extremes) and statistics of a curve in range.
fn curve_text(c: &Curve, range: Option<TickRange>, all: bool) -> Option<String> {
    let pts: Vec<(i64, f64)> = c
        .points
        .iter()
        .copied()
        .filter(|(t, _)| range.is_none_or(|r| r.contains(*t)))
        .collect();
    if pts.is_empty() {
        return None;
    }
    let (min, max) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), (_, v)| {
        (a.min(*v), b.max(*v))
    });
    let mean = pts.iter().map(|(_, v)| v).sum::<f64>() / pts.len() as f64;
    let mut s = format!(
        "{} pts {:?} min {:.1} max {:.1} mean {:.1}",
        pts.len(),
        c.interp,
        min,
        max,
        mean
    );
    if all {
        let list: Vec<String> = pts.iter().map(|(t, v)| format!("{t}:{v:.1}")).collect();
        let _ = write!(s, " [{}]", list.join(" "));
    } else {
        let (f, l) = (pts[0], pts[pts.len() - 1]);
        let _ = write!(s, " first {}:{:.1} last {}:{:.1}", f.0, f.1, l.0, l.1);
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_names() {
        assert_eq!(pitch_name(6000.0), "C4");
        assert_eq!(pitch_name(6912.0), "A4+12");
        assert_eq!(pitch_name(6950.0), "A#4-50");
    }
}
