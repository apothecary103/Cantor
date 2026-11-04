//! Standard MIDI file import (format 0 and 1, metrical timing).
//!
//! The melody is one (SMF track, channel) group, imported as one track:
//! picked with [`MidiOpts`] or automatically (lyrics, a vocal-sounding
//! track name, monophony, note count, vocal range). Tempo, time and key
//! signatures come from all tracks. Overlapping notes are made monophonic
//! (of notes starting together the highest wins; earlier notes are cut
//! where the next starts). Lyric meta events (Text events if there are
//! none) in UTF-8 or Shift-JIS become lyrics; a note without one that
//! follows a sung note directly becomes `+`, others sing [`DEFAULT_LYRIC`].
//! Pitch bends on the melody channel become the `pitd` curve.

use crate::common::{Diags, Enc, decode, finish, rescale};
use crate::{ImportError, Imported};
use cantor_core::params::known;
use cantor_core::{
    Curve, Interp, Key, Note, NoteId, ParamId, Project, Scale, TempoPoint, Tick, TimeSigMap,
    TimeSigPoint, Track, TrackId,
};
use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::collections::{BTreeMap, HashMap};

const FORMAT: &str = "MIDI";

/// Lyric for notes that have none (and do not continue a sung note).
pub const DEFAULT_LYRIC: &str = "a";

/// The General MIDI drum channel (0-based).
const DRUMS: u8 = 9;

/// Words in track names that suggest the vocal line.
const VOCAL_WORDS: &[&str] = &[
    "vocal",
    "voice",
    "vox",
    "melody",
    "melo",
    "lead",
    "sing",
    "lyric",
    "歌",
    "ボーカル",
    "メロ",
    "主旋律",
    "ヴォーカル",
];

/// Which part of the file holds the melody.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiOpts {
    /// 0-based SMF track index; `None` = choose automatically.
    pub track: Option<usize>,
    /// 0-based MIDI channel (0..=15); `None` = any (best) channel.
    pub channel: Option<u8>,
}

fn err(unsupported: bool, message: impl Into<String>) -> ImportError {
    let message = message.into();
    if unsupported {
        ImportError::Unsupported {
            format: FORMAT,
            message,
        }
    } else {
        ImportError::Parse {
            format: FORMAT,
            message,
        }
    }
}

/// One (track, channel) group of notes, in source ticks.
#[derive(Debug, Default)]
struct Group {
    notes: Vec<(u64, u64, u8)>,
    bends: Vec<(u64, f64)>,
}

#[derive(Debug, Default)]
struct RawTrack {
    name: String,
    groups: BTreeMap<u8, Group>,
    lyrics: Vec<(u64, String)>,
    texts: Vec<(u64, String)>,
}

/// Imports a MIDI file.
pub fn import(bytes: &[u8], opts: &MidiOpts) -> Result<Imported, ImportError> {
    let smf = Smf::parse(bytes).map_err(|e| err(false, e.to_string()))?;
    if smf.header.format == Format::Sequential {
        return Err(err(true, "format 2 (sequential tracks) is not supported"));
    }
    let ppq = match smf.header.timing {
        Timing::Metrical(t) if t.as_int() > 0 => u32::from(t.as_int()),
        Timing::Metrical(_) => return Err(err(false, "the header's ticks per quarter note is 0")),
        Timing::Timecode(..) => {
            return Err(err(
                true,
                "SMPTE timecode timing is not supported; only PPQ timing",
            ));
        }
    };
    let mut d = Diags::default();
    let mut inexact = 0usize;
    let mut tick = |src: u64| {
        let (t, exact) = rescale(src as i64, ppq);
        inexact += usize::from(!exact);
        t
    };
    let text = |d: &mut Diags, b: &[u8]| {
        let (s, enc) = decode(b);
        if enc == Enc::Lossy {
            d.count_warn(
                "midi.text_encoding",
                "texts that are neither UTF-8 nor Shift-JIS were decoded lossily",
            );
        }
        s
    };

    let (mut tempos, mut sigs, mut keys) = (Vec::new(), Vec::new(), Vec::new());
    let mut raws = Vec::new();
    let mut ignored: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (ti, events) in smf.tracks.iter().enumerate() {
        let mut raw = RawTrack::default();
        let mut open: HashMap<(u8, u8), Vec<u64>> = HashMap::new();
        let mut rpn = [[127u8; 2]; 16];
        let mut range = [2.0f64; 16];
        let mut now = 0u64;
        for ev in events {
            now += u64::from(ev.delta.as_int());
            match ev.kind {
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            open.entry((ch, key.as_int())).or_default().push(now);
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            let k = key.as_int();
                            if let Some(starts) = open.get_mut(&(ch, k))
                                && !starts.is_empty()
                            {
                                let s = starts.remove(0);
                                if s < now {
                                    raw.groups.entry(ch).or_default().notes.push((s, now, k));
                                }
                            }
                        }
                        MidiMessage::PitchBend { bend } => {
                            let cents = bend.as_f64() * range[usize::from(ch)] * 100.0;
                            raw.groups.entry(ch).or_default().bends.push((now, cents));
                        }
                        MidiMessage::Controller { controller, value } => {
                            let (c, v) = (controller.as_int(), value.as_int());
                            let r = &mut rpn[usize::from(ch)];
                            match c {
                                101 => r[0] = v,
                                100 => r[1] = v,
                                6 if *r == [0, 0] => range[usize::from(ch)] = f64::from(v),
                                _ => *ignored.entry("control changes").or_default() += 1,
                            }
                        }
                        _ => *ignored.entry("aftertouch and program changes").or_default() += 1,
                    }
                }
                TrackEventKind::Meta(m) => match m {
                    MetaMessage::Tempo(us) => tempos.push((tick(now), us.as_int())),
                    MetaMessage::TimeSignature(n, p, ..) => sigs.push((tick(now), n, p)),
                    MetaMessage::KeySignature(sf, minor) => keys.push((tick(now), sf, minor)),
                    MetaMessage::TrackName(b) if raw.name.is_empty() => {
                        raw.name = text(&mut d, b).trim().to_owned();
                    }
                    MetaMessage::Lyric(b) => raw.lyrics.push((now, text(&mut d, b))),
                    MetaMessage::Text(b) => raw.texts.push((now, text(&mut d, b))),
                    MetaMessage::EndOfTrack => {}
                    _ => *ignored.entry("other meta events").or_default() += 1,
                },
                _ => *ignored.entry("system-exclusive events").or_default() += 1,
            }
        }
        // Notes still sounding end with the track.
        for ((ch, k), starts) in open {
            for s in starts.into_iter().filter(|&s| s < now) {
                raw.groups.entry(ch).or_default().notes.push((s, now, k));
                d.count_warn(
                    "midi.note_unterminated",
                    "notes without a note-off end at the end of their track",
                );
            }
        }
        for g in raw.groups.values_mut() {
            g.notes.sort_unstable();
        }
        raws.push(raw);
        let _ = ti;
    }

    let mut p = Project {
        tempo: tempo_map(&mut tempos, &mut d),
        time_sig: time_sigs(&mut sigs, &mut d),
        ..Project::default()
    };
    keys.sort_by_key(|k| k.0);
    if let Some(&(_, sf, minor)) = keys.first() {
        if (-7..=7).contains(&sf) {
            let major = (i32::from(sf) * 7).rem_euclid(12) as u8;
            p.key = Some(if minor {
                Key {
                    tonic: (major + 9) % 12,
                    scale: Scale::Minor,
                }
            } else {
                Key {
                    tonic: major,
                    scale: Scale::Major,
                }
            });
        } else {
            d.warn(
                "midi.key_invalid",
                format!("key signature with {sf} sharps/flats is not valid and was ignored"),
            );
        }
        if keys.iter().any(|k| (k.1, k.2) != (sf, minor)) {
            d.warn(
                "midi.key_change",
                "the key signature changes; Cantor keeps only the first key",
            );
        }
    }
    if let (Some(first), true) = (raws.first(), smf.header.format == Format::Parallel)
        && first.groups.is_empty()
    {
        p.meta.title = first.name.clone();
    }

    // Candidates for the melody.
    let mut cands: Vec<(f64, usize, u8, String)> = Vec::new();
    for (ti, raw) in raws.iter().enumerate() {
        for (&ch, g) in &raw.groups {
            if opts.track.is_some_and(|t| t != ti) || opts.channel.is_some_and(|c| c != ch) {
                continue;
            }
            if ch == DRUMS && opts.channel != Some(DRUMS) {
                continue;
            }
            let n = g.notes.len() as f64;
            let poly = g.notes.windows(2).filter(|w| w[1].0 < w[0].1).count() as f64;
            let mut keys: Vec<u8> = g.notes.iter().map(|n| n.2).collect();
            keys.sort_unstable();
            let median = keys.get(keys.len() / 2).copied().unwrap_or(60);
            let lname = raw.name.to_lowercase();
            let mut score = n * (1.0 - poly / n.max(1.0)).powi(2);
            if !(45..=84).contains(&median) {
                score *= 0.3;
            }
            if VOCAL_WORDS.iter().any(|w| lname.contains(w)) {
                score += 500.0;
            }
            score += 10.0 * raw.lyrics.len().min(g.notes.len()) as f64;
            cands.push((
                score,
                ti,
                ch,
                format!(
                    "track {ti} `{}` channel {ch}: {} notes, {} overlapping, {} lyrics",
                    raw.name,
                    g.notes.len(),
                    poly,
                    raw.lyrics.len()
                ),
            ));
        }
    }
    cands.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    let Some((_, ti, ch, _)) = cands.first().cloned() else {
        if opts.track.is_some() || opts.channel.is_some() {
            return Err(err(
                true,
                format!("no notes on the requested track/channel ({opts:?})"),
            ));
        }
        d.info("midi.no_notes", "the file has no (non-drum) notes");
        report_ignored(&mut d, inexact, ppq, &ignored);
        return finish(p, d);
    };
    let others: Vec<&str> = cands[1..].iter().take(8).map(|c| c.3.as_str()).collect();
    d.info(
        "midi.melody",
        format!(
            "imported the melody from {} (0-based indices){}",
            cands[0].3,
            if others.is_empty() {
                String::new()
            } else {
                format!(
                    "; other candidates: {}; pick one with the track/channel options",
                    others.join("; ")
                )
            }
        ),
    );
    let raw = &raws[ti];
    let g = &raw.groups[&ch];
    let mut track = Track {
        id: TrackId(1),
        name: if raw.name.is_empty() {
            format!("Track {ti}")
        } else {
            raw.name.clone()
        },
        ..Track::default()
    };

    // Monophony.
    let mut scaled: Vec<(Tick, Tick, u8)> = g
        .notes
        .iter()
        .map(|&(s, e, k)| {
            let s = tick(s);
            (s, tick(e).max(s + 1), k)
        })
        .collect();
    scaled.sort_by_key(|n| (n.0, std::cmp::Reverse(n.2)));
    let mut kept: Vec<(Tick, Tick, u8)> = Vec::new();
    let (mut dropped, mut trimmed) = (0, 0);
    for n in scaled {
        if let Some(prev) = kept.last_mut() {
            if prev.0 == n.0 {
                dropped += 1;
                continue;
            }
            if prev.1 > n.0 {
                prev.1 = n.0;
                trimmed += 1;
            }
        }
        kept.push(n);
    }
    if dropped > 0 {
        d.warn("midi.chord_dropped", format!("dropped {dropped} notes starting together with a higher note (tracks are monophonic)"));
    }
    if trimmed > 0 {
        d.warn(
            "midi.overlap_trimmed",
            format!("shortened {trimmed} overlapping notes to end where the next starts"),
        );
    }

    // Lyrics.
    let source = if raw.lyrics.is_empty() {
        if !raw.texts.is_empty() {
            d.info(
                "midi.text_as_lyrics",
                "no Lyric events; Text events were used as lyrics",
            );
        }
        &raw.texts
    } else {
        &raw.lyrics
    };
    let mut lyrics: Vec<Option<String>> = vec![None; kept.len()];
    let tolerance = 60;
    for (t, s) in source {
        let s: String = s.chars().filter(|c| !"\r\n/\\^<>".contains(*c)).collect();
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        let t = tick(*t);
        let i = kept.partition_point(|n| n.0 < t - tolerance);
        let near = (i..kept.len())
            .take_while(|&j| kept[j].0 <= t + tolerance)
            .find(|&j| lyrics[j].is_none());
        let containing = kept.iter().rposition(|n| n.0 <= t && t < n.1);
        match (near, containing) {
            (Some(j), _) => lyrics[j] = Some(s.to_owned()),
            (None, Some(j)) => {
                lyrics[j].get_or_insert_default().push_str(s);
                d.count_info(
                    "midi.lyric_merged",
                    "lyrics landing on an already-sung note were appended to its lyric",
                );
            }
            (None, None) => d.count_warn(
                "midi.lyric_unmatched",
                format!("lyrics with no note at their tick were dropped (e.g. {s:?} at tick {t})"),
            ),
        }
    }
    let has_lyrics = lyrics.iter().any(Option::is_some);
    let mut prev_end = None;
    let mut defaulted = 0;
    for (i, ((s, e, k), lyric)) in kept.iter().zip(lyrics).enumerate() {
        let lyric = lyric.unwrap_or_else(|| {
            if has_lyrics && prev_end == Some(*s) {
                "+".into()
            } else {
                defaulted += 1;
                DEFAULT_LYRIC.into()
            }
        });
        prev_end = Some(*e);
        let mut n = Note::new(*s, e - s, (*k).min(127), lyric);
        n.id = NoteId(i as u64 + 1);
        track.notes.push(n);
    }
    if defaulted > 0 {
        d.info("midi.lyric_default", format!("{defaulted} notes have no lyric and sing {DEFAULT_LYRIC:?}; set lyrics with lyrics::assign"));
    }

    // Pitch bends → pitd.
    let mut pts: Vec<(Tick, f64)> = Vec::new();
    let mut clamped = false;
    for &(t, c) in &g.bends {
        let v = c.clamp(-1200.0, 1200.0);
        clamped |= v != c;
        let t = tick(t);
        if pts.last().is_some_and(|l| l.0 == t) {
            pts.pop();
        }
        if pts.last().is_none_or(|l| l.1 != v) {
            pts.push((t, v));
        }
    }
    if clamped {
        d.warn(
            "midi.bend_clamped",
            "pitch bends beyond ±1200 cents were clamped",
        );
    }
    if pts.iter().any(|p| p.1 != 0.0) {
        d.info(
            "midi.pitch_bend",
            "pitch bends on the melody channel became the pitd curve",
        );
        track.curves.insert(
            ParamId::new(known::PITD),
            Curve {
                points: pts,
                interp: Interp::Step,
            },
        );
    }
    let other_tracks: usize = raws.iter().map(|r| r.groups.len()).sum::<usize>() - 1;
    if other_tracks > 0 {
        d.info(
            "midi.other_parts",
            format!("{other_tracks} other track/channel groups were not imported"),
        );
    }
    p.tracks.push(track);
    report_ignored(&mut d, inexact, ppq, &ignored);
    finish(p, d)
}

fn report_ignored(d: &mut Diags, inexact: usize, ppq: u32, ignored: &BTreeMap<&str, usize>) {
    if inexact > 0 {
        d.warn(
            "midi.ticks_rounded",
            format!("{inexact} positions were rounded converting {ppq} to 480 ticks per quarter"),
        );
    }
    for (what, n) in ignored {
        d.info("midi.ignored", format!("ignored {n} {what}"));
    }
}

fn tempo_map(events: &mut [(Tick, u32)], d: &mut Diags) -> Vec<TempoPoint> {
    events.sort_by_key(|e| e.0);
    let mut out: Vec<TempoPoint> = Vec::new();
    for &(tick, us) in events.iter() {
        let raw = if us == 0 {
            1000.0
        } else {
            60e6 / f64::from(us)
        };
        let bpm = d.clamp("midi.tempo_clamped", "tempo (BPM)", raw, 10.0, 1000.0);
        if out.last().is_some_and(|l| l.tick == tick) {
            out.pop();
        }
        if out.last().is_none_or(|l| l.bpm != bpm) {
            out.push(TempoPoint { tick, bpm });
        }
    }
    if out.first().is_none_or(|p| p.tick != 0) {
        d.info(
            "midi.tempo_default",
            "no tempo at the start; 120 BPM until the first tempo event",
        );
        out.insert(0, TempoPoint::default());
    }
    out
}

fn time_sigs(events: &mut [(Tick, u8, u8)], d: &mut Diags) -> Vec<TimeSigPoint> {
    events.sort_by_key(|e| e.0);
    let mut out: Vec<TimeSigPoint> = vec![TimeSigPoint::default()];
    let mut seen_start = false;
    for &(tick, num, pow) in events.iter() {
        if !(1..=32).contains(&num) || pow > 5 {
            d.count_warn(
                "midi.time_sig_invalid",
                format!("time signature {num}/2^{pow} is invalid and was ignored"),
            );
            continue;
        }
        let (mut bar, off) = TimeSigMap::new(&out).tick_to_bar(tick);
        if off != 0 {
            bar += 1;
            d.count_warn(
                "midi.time_sig_moved",
                "time signatures off a bar line were moved to the next bar",
            );
        }
        seen_start |= bar == 0;
        let pt = TimeSigPoint {
            bar,
            num,
            den: 1 << pow,
        };
        out.retain(|p| p.bar < bar);
        if out.last().is_none_or(|l| (l.num, l.den) != (num, pt.den)) {
            out.push(pt);
        }
        if out.is_empty() {
            out.push(pt);
        }
    }
    if out.first().is_none_or(|p| p.bar != 0) {
        out.insert(0, TimeSigPoint::default());
    }
    if !seen_start {
        d.info(
            "midi.time_sig_default",
            "no time signature at the start; 4/4 was assumed",
        );
    }
    out
}
