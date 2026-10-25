//! Standard MIDI File importer (SMF format 0 and 1, metrical timing).
//!
//! Mapping, in short (see `docs/decisions/0006-midi-import.md`):
//! - every tick is rescaled from the file's PPQ to [`PPQ`];
//! - tempo, time-signature and key-signature meta events from any track
//!   build the project's maps;
//! - each SMF track with notes becomes one track holding one part that runs
//!   from the start of the first note's bar to the end of the last note's
//!   bar;
//! - tracks are monophonic: of notes starting together the highest is kept,
//!   and a note overlapping the next one is trimmed to end where it starts;
//! - lyrics come from Lyric meta events (Text events if a track has none)
//!   at a note's start tick; notes without one sing [`DEFAULT_LYRIC`].
//!
//! Anything not carried over is reported as a diagnostic.

use super::{Ids, ImportError, Imported, finish, rescale};
use cantor_core::{
    Diagnostic, EntityRef, KeyPoint, Note, PPQ, Part, Project, Scale, TempoPoint, Tick, TimeSigMap,
    TimeSigPoint, Track,
};
use midly::{Format, MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::collections::{BTreeMap, HashMap, VecDeque};

const FORMAT: &str = "MIDI";

/// Lyric given to notes that have none in the file.
pub const DEFAULT_LYRIC: &str = "la";

/// MIDI channel 10 (0-based 9) is the General MIDI drum channel.
const DRUM_CHANNEL: u8 = 9;

/// Imports a MIDI file.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    let smf = Smf::parse(bytes).map_err(|e| ImportError::Parse {
        format: FORMAT,
        message: e.to_string(),
    })?;
    if smf.header.format == Format::Sequential {
        return Err(ImportError::Unsupported {
            format: FORMAT,
            message: "format 2 (sequential tracks) is not supported".into(),
        });
    }
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => u32::from(t.as_int()),
        Timing::Timecode(..) => {
            return Err(ImportError::Unsupported {
                format: FORMAT,
                message: "SMPTE timecode timing is not supported; only metrical (PPQ) timing"
                    .into(),
            });
        }
    };
    if ppq == 0 {
        return Err(ImportError::Parse {
            format: FORMAT,
            message: "the header's ticks per quarter note is 0".into(),
        });
    }
    let first_is_conductor = smf.header.format == Format::Parallel;
    Importer::new(ppq).run(&smf.tracks, first_is_conductor)
}

/// A note as read from the file, in source ticks.
#[derive(Debug)]
struct RawNote {
    start: u64,
    end: u64,
    key: u8,
}

/// A text meta event in source ticks.
#[derive(Debug)]
struct RawText<'a> {
    tick: u64,
    bytes: &'a [u8],
}

/// What one SMF track contributes besides the global maps.
#[derive(Debug, Default)]
struct RawTrack<'a> {
    name: Option<&'a [u8]>,
    notes: Vec<RawNote>,
    lyrics: Vec<RawText<'a>>,
    texts: Vec<RawText<'a>>,
    unterminated: usize,
    zero_length: usize,
}

/// A note after rescaling, before ids are assigned.
#[derive(Debug)]
struct ScaledNote {
    start: Tick,
    end: Tick,
    key: u8,
}

/// A map event with its sort key: (tick, track index, event index).
type Keyed<T> = ((Tick, usize, usize), T);

#[derive(Debug)]
struct Importer {
    ppq: u32,
    inexact: usize,
    diags: Vec<Diagnostic>,
    /// Ignored event kinds → count; reported once per kind at the end.
    ignored: BTreeMap<&'static str, usize>,
    invalid_utf8: usize,
    drum_notes: usize,
    unmatched_note_offs: usize,
    default_lyrics: usize,
}

impl Importer {
    fn new(ppq: u32) -> Self {
        Self {
            ppq,
            inexact: 0,
            diags: Vec::new(),
            ignored: BTreeMap::new(),
            invalid_utf8: 0,
            drum_notes: 0,
            unmatched_note_offs: 0,
            default_lyrics: 0,
        }
    }

    /// Rescales a source tick, counting inexact conversions.
    fn tick(&mut self, src: u64) -> Tick {
        let (t, exact) = rescale(src as i64, self.ppq);
        if !exact {
            self.inexact += 1;
        }
        t
    }

    fn ignore(&mut self, kind: &'static str) {
        *self.ignored.entry(kind).or_default() += 1;
    }

    fn text(&mut self, bytes: &[u8]) -> String {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                self.invalid_utf8 += 1;
                String::from_utf8_lossy(bytes).into_owned()
            }
        }
    }

    fn run(
        mut self,
        tracks: &[Vec<midly::TrackEvent<'_>>],
        first_is_conductor: bool,
    ) -> Result<Imported, ImportError> {
        let mut tempos: Vec<Keyed<u32>> = Vec::new();
        let mut sigs: Vec<Keyed<(u8, u8)>> = Vec::new();
        let mut keys: Vec<Keyed<(i8, bool)>> = Vec::new();
        let mut raws = Vec::with_capacity(tracks.len());

        for (ti, events) in tracks.iter().enumerate() {
            let mut raw = RawTrack::default();
            let mut open: HashMap<(u8, u8), VecDeque<u64>> = HashMap::new();
            let mut now: u64 = 0;
            for (ei, ev) in events.iter().enumerate() {
                now += u64::from(ev.delta.as_int());
                match ev.kind {
                    TrackEventKind::Midi { channel, message } => {
                        let ch = channel.as_int();
                        match message {
                            MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                                if ch == DRUM_CHANNEL {
                                    self.drum_notes += 1;
                                } else {
                                    open.entry((ch, key.as_int())).or_default().push_back(now);
                                }
                            }
                            MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                                if ch == DRUM_CHANNEL {
                                    continue;
                                }
                                let k = key.as_int();
                                match open.get_mut(&(ch, k)).and_then(VecDeque::pop_front) {
                                    Some(start) if start == now => raw.zero_length += 1,
                                    Some(start) => raw.notes.push(RawNote {
                                        start,
                                        end: now,
                                        key: k,
                                    }),
                                    None => self.unmatched_note_offs += 1,
                                }
                            }
                            MidiMessage::Aftertouch { .. } => self.ignore("poly_aftertouch"),
                            MidiMessage::Controller { .. } => self.ignore("controller"),
                            MidiMessage::ProgramChange { .. } => self.ignore("program_change"),
                            MidiMessage::ChannelAftertouch { .. } => {
                                self.ignore("channel_aftertouch")
                            }
                            MidiMessage::PitchBend { .. } => self.ignore("pitch_bend"),
                        }
                    }
                    TrackEventKind::SysEx(_) | TrackEventKind::Escape(_) => self.ignore("sysex"),
                    TrackEventKind::Meta(meta) => match meta {
                        MetaMessage::Tempo(us) => {
                            tempos.push(((self.tick(now), ti, ei), us.as_int()));
                        }
                        MetaMessage::TimeSignature(n, d, _, _) => {
                            sigs.push(((self.tick(now), ti, ei), (n, d)));
                        }
                        MetaMessage::KeySignature(sf, minor) => {
                            keys.push(((self.tick(now), ti, ei), (sf, minor)));
                        }
                        MetaMessage::TrackName(name) if raw.name.is_none() => {
                            raw.name = Some(name);
                        }
                        MetaMessage::TrackName(_) => self.ignore("track_name"),
                        MetaMessage::Lyric(bytes) => {
                            raw.lyrics.push(RawText { tick: now, bytes });
                        }
                        MetaMessage::Text(bytes) => {
                            raw.texts.push(RawText { tick: now, bytes });
                        }
                        MetaMessage::EndOfTrack => {}
                        MetaMessage::Copyright(_) => self.ignore("copyright"),
                        MetaMessage::InstrumentName(_) => self.ignore("instrument_name"),
                        MetaMessage::Marker(_) => self.ignore("marker"),
                        MetaMessage::CuePoint(_) => self.ignore("cue_point"),
                        MetaMessage::ProgramName(_) => self.ignore("program_name"),
                        MetaMessage::DeviceName(_) => self.ignore("device_name"),
                        MetaMessage::MidiChannel(_) => self.ignore("midi_channel"),
                        MetaMessage::MidiPort(_) => self.ignore("midi_port"),
                        MetaMessage::TrackNumber(_) => self.ignore("track_number"),
                        MetaMessage::SmpteOffset(_) => self.ignore("smpte_offset"),
                        MetaMessage::SequencerSpecific(_) => self.ignore("sequencer_specific"),
                        MetaMessage::Unknown(..) => self.ignore("unknown_meta"),
                    },
                }
            }
            // Notes still sounding end with the track.
            let mut left: Vec<_> = open
                .into_iter()
                .flat_map(|((_, key), starts)| starts.into_iter().map(move |s| (s, key)))
                .collect();
            left.sort_unstable();
            for (start, key) in left {
                if start == now {
                    raw.zero_length += 1;
                } else {
                    raw.unterminated += 1;
                    raw.notes.push(RawNote {
                        start,
                        end: now,
                        key,
                    });
                }
            }
            raws.push(raw);
        }

        let mut project = Project::new();
        project.tempo_map = self.tempo_map(tempos);
        project.time_sig_map = self.time_sig_map(sigs);
        project.key_map = self.key_map(keys);
        let meter = project.meter();

        let mut ids = Ids::new();
        for (ti, raw) in raws.into_iter().enumerate() {
            if raw.notes.is_empty() {
                if let Some(name) = raw.name {
                    if ti == 0 && first_is_conductor {
                        project.meta.title = self.text(name).trim().to_owned();
                    } else {
                        self.ignore("track_name");
                    }
                }
                let texts = raw.lyrics.len() + raw.texts.len();
                for _ in 0..texts {
                    self.ignore("text");
                }
                if raw.zero_length > 0 {
                    self.diags.push(Diagnostic::warning(
                        "midi.note_zero_length",
                        format!(
                            "SMF track {}: dropped {} zero-length notes",
                            ti + 1,
                            raw.zero_length
                        ),
                        None,
                    ));
                }
                continue;
            }
            let track = self.track(ti, raw, &meter, &mut ids);
            project.tracks.push(track);
        }

        self.report_globals();
        finish(project, self.diags)
    }

    fn tempo_map(&mut self, mut events: Vec<Keyed<u32>>) -> Vec<TempoPoint> {
        events.sort_by_key(|e| e.0);
        let mut points: Vec<TempoPoint> = Vec::new();
        for ((tick, _, _), us) in events {
            let mut bpm = if us == 0 {
                f64::INFINITY
            } else {
                60e6 / f64::from(us)
            };
            if !(cantor_core::MIN_BPM..=cantor_core::MAX_BPM).contains(&bpm) {
                let clamped = bpm.clamp(cantor_core::MIN_BPM, cantor_core::MAX_BPM);
                self.diags.push(Diagnostic::warning(
                    "midi.tempo_clamped",
                    format!("tempo {bpm} BPM at tick {tick} clamped to {clamped} BPM"),
                    Some(EntityRef::TempoMap),
                ));
                bpm = clamped;
            }
            let point = TempoPoint { tick, bpm };
            match points.last_mut() {
                Some(last) if last.tick == tick => {
                    if last.bpm != bpm {
                        self.diags.push(Diagnostic::warning(
                            "midi.tempo_conflict",
                            format!(
                                "tempos {} and {bpm} BPM both at tick {tick}; kept the later {bpm}",
                                last.bpm
                            ),
                            Some(EntityRef::TempoMap),
                        ));
                    }
                    *last = point;
                }
                _ => points.push(point),
            }
        }
        if points.first().is_none_or(|p| p.tick != 0) {
            self.diags.push(Diagnostic::info(
                "midi.tempo_default",
                "no tempo at the start of the file; using 120 BPM until the first tempo event",
                Some(EntityRef::TempoMap),
            ));
            points.insert(
                0,
                TempoPoint {
                    tick: 0,
                    bpm: 120.0,
                },
            );
        }
        points
    }

    fn time_sig_map(&mut self, mut events: Vec<Keyed<(u8, u8)>>) -> Vec<TimeSigPoint> {
        events.sort_by_key(|e| e.0);
        let four_four = TimeSigPoint {
            bar: 0,
            numerator: 4,
            denominator: 4,
        };
        let mut points: Vec<TimeSigPoint> = Vec::new();
        for ((tick, _, _), (numerator, pow)) in events {
            if numerator == 0 || pow > 6 {
                self.diags.push(Diagnostic::warning(
                    "midi.time_sig_invalid",
                    format!(
                        "time signature {numerator}/2^{pow} at tick {tick} is invalid; ignored"
                    ),
                    Some(EntityRef::TimeSigMap),
                ));
                continue;
            }
            let denominator = 1u8 << pow;
            let (mut bar, offset) = TimeSigMap::new(&points).tick_to_bar(tick);
            if offset != 0 {
                bar += 1;
                self.diags.push(Diagnostic::warning(
                    "midi.time_sig_moved",
                    format!(
                        "time signature {numerator}/{denominator} at tick {tick} is not on a bar \
                         line; moved to the start of bar {}",
                        bar + 1
                    ),
                    Some(EntityRef::TimeSigMap),
                ));
            }
            if points.is_empty() && bar > 0 {
                self.diags.push(Diagnostic::info(
                    "midi.time_sig_default",
                    "no time signature at the start of the file; using 4/4 until the first one",
                    Some(EntityRef::TimeSigMap),
                ));
                points.push(four_four);
            }
            let point = TimeSigPoint {
                bar,
                numerator,
                denominator,
            };
            match points.last_mut() {
                Some(last) if last.bar == bar => {
                    if *last != point {
                        self.diags.push(Diagnostic::warning(
                            "midi.time_sig_conflict",
                            format!(
                                "time signatures {}/{} and {numerator}/{denominator} both at bar \
                                 {}; kept the later",
                                last.numerator,
                                last.denominator,
                                bar + 1
                            ),
                            Some(EntityRef::TimeSigMap),
                        ));
                    }
                    *last = point;
                }
                _ => points.push(point),
            }
        }
        if points.is_empty() {
            self.diags.push(Diagnostic::info(
                "midi.time_sig_default",
                "the file has no time signature; using 4/4",
                Some(EntityRef::TimeSigMap),
            ));
            points.push(four_four);
        }
        points
    }

    fn key_map(&mut self, mut events: Vec<Keyed<(i8, bool)>>) -> Vec<KeyPoint> {
        events.sort_by_key(|e| e.0);
        let mut points: Vec<KeyPoint> = Vec::new();
        for ((tick, _, _), (sf, minor)) in events {
            if !(-7..=7).contains(&sf) {
                self.diags.push(Diagnostic::warning(
                    "midi.key_invalid",
                    format!("key signature with {sf} sharps at tick {tick} is invalid; ignored"),
                    Some(EntityRef::KeyMap),
                ));
                continue;
            }
            // Each sharp moves the major tonic a fifth up (7 semitones);
            // the relative minor is a minor third below (9 up).
            let major = (i32::from(sf) * 7).rem_euclid(12);
            let (tonic, scale) = if minor {
                ((major + 9) % 12, Scale::Minor)
            } else {
                (major, Scale::Major)
            };
            let point = KeyPoint {
                tick,
                tonic: tonic as u8,
                scale,
            };
            match points.last_mut() {
                Some(last) if last.tick == tick => {
                    if *last != point {
                        self.diags.push(Diagnostic::warning(
                            "midi.key_conflict",
                            format!("two different key signatures at tick {tick}; kept the later"),
                            Some(EntityRef::KeyMap),
                        ));
                    }
                    *last = point;
                }
                _ => points.push(point),
            }
        }
        points
    }

    fn track(&mut self, ti: usize, raw: RawTrack<'_>, meter: &TimeSigMap, ids: &mut Ids) -> Track {
        let name = match raw.name {
            Some(bytes) => self.text(bytes).trim().to_owned(),
            None => String::new(),
        };
        let name = if name.is_empty() {
            format!("Track {}", ti + 1)
        } else {
            name
        };
        let track_id = ids.track();
        let part_id = ids.part();
        let target = Some(EntityRef::Track { id: track_id });

        if raw.zero_length > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.note_zero_length",
                format!("{name}: dropped {} zero-length notes", raw.zero_length),
                target.clone(),
            ));
        }
        if raw.unterminated > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.note_unterminated",
                format!(
                    "{name}: {} notes have no note-off; they end at the end of the track",
                    raw.unterminated
                ),
                target.clone(),
            ));
        }

        // Rescale; a note that rounds to nothing keeps one tick.
        let mut scaled: Vec<ScaledNote> = Vec::with_capacity(raw.notes.len());
        for n in &raw.notes {
            let start = self.tick(n.start);
            let end = self.tick(n.end).max(start + 1);
            scaled.push(ScaledNote {
                start,
                end,
                key: n.key,
            });
        }
        // Highest pitch first among notes that start together.
        scaled.sort_by_key(|n| (n.start, std::cmp::Reverse(n.key), n.end));

        // Monophony: keep the highest of simultaneous notes, trim overlaps.
        let mut kept: Vec<ScaledNote> = Vec::with_capacity(scaled.len());
        let (mut dropped, mut trimmed) = (0usize, 0usize);
        for n in scaled {
            if let Some(prev) = kept.last_mut() {
                if prev.start == n.start {
                    dropped += 1;
                    continue;
                }
                if prev.end > n.start {
                    prev.end = n.start;
                    trimmed += 1;
                }
            }
            kept.push(n);
        }
        if dropped > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.note_chord_dropped",
                format!(
                    "{name}: dropped {dropped} notes that start together with a higher note \
                     (tracks are monophonic)"
                ),
                target.clone(),
            ));
        }
        if trimmed > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.note_overlap_trimmed",
                format!(
                    "{name}: shortened {trimmed} notes to end where the next note starts \
                     (tracks are monophonic)"
                ),
                target.clone(),
            ));
        }

        // Lyrics: Lyric events, or Text events if the track has none.
        let (sources, ignored_texts) = if raw.lyrics.is_empty() {
            if !raw.texts.is_empty() {
                self.diags.push(Diagnostic::info(
                    "midi.text_as_lyrics",
                    format!("{name}: no Lyric events; using Text events as lyrics"),
                    target.clone(),
                ));
            }
            (raw.texts, 0)
        } else {
            let n = raw.texts.len();
            (raw.lyrics, n)
        };
        for _ in 0..ignored_texts {
            self.ignore("text");
        }
        let mut lyrics: Vec<Option<String>> = vec![None; kept.len()];
        let (mut unmatched, mut merged) = (Vec::new(), 0usize);
        for t in sources {
            let text = self.text(t.bytes).trim().to_owned();
            if text.is_empty() {
                continue;
            }
            let (tick, _) = rescale(t.tick as i64, self.ppq);
            match kept.binary_search_by_key(&tick, |n| n.start) {
                Ok(i) => match &mut lyrics[i] {
                    Some(l) => {
                        l.push_str(&text);
                        merged += 1;
                    }
                    slot => *slot = Some(text),
                },
                Err(_) => unmatched.push((tick, text)),
            }
        }
        if merged > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.lyric_merged",
                format!("{name}: {merged} extra lyrics at a note's tick were appended to it"),
                target.clone(),
            ));
        }
        if let Some((tick, text)) = unmatched.first() {
            self.diags.push(Diagnostic::warning(
                "midi.lyric_unmatched",
                format!(
                    "{name}: {} lyrics have no note starting at their tick and were dropped \
                     (first: {text:?} at tick {tick})",
                    unmatched.len()
                ),
                target.clone(),
            ));
        }

        // The part spans whole bars, from the first note's bar to the end
        // of the last note's bar.
        let first = kept.first().map_or(0, |n| n.start);
        let last_end = kept.iter().map(|n| n.end).max().unwrap_or(1);
        let part_start = meter.bar_to_tick(meter.tick_to_bar(first).0);
        let (end_bar, offset) = meter.tick_to_bar(last_end);
        let part_end = if offset == 0 {
            last_end
        } else {
            meter.bar_to_tick(end_bar + 1)
        };

        let mut notes = Vec::with_capacity(kept.len());
        for (n, lyric) in kept.into_iter().zip(lyrics) {
            let lyric = lyric.unwrap_or_else(|| {
                self.default_lyrics += 1;
                DEFAULT_LYRIC.to_owned()
            });
            self.ignore("velocity");
            notes.push(Note {
                id: ids.note(),
                start: n.start - part_start,
                length: n.end - n.start,
                pitch_cents: f64::from(n.key) * 100.0,
                lyric,
                lang: None,
                phonemes: None,
                expr: Default::default(),
                modes: Default::default(),
            });
        }

        Track {
            id: track_id,
            name: name.clone(),
            singer: Default::default(),
            backend: Default::default(),
            mix: Default::default(),
            effects: Vec::new(),
            doubling: Default::default(),
            voice_mix: Vec::new(),
            curves: Default::default(),
            parts: vec![Part {
                id: part_id,
                name,
                start: part_start,
                length: (part_end - part_start).max(1),
                notes,
            }],
        }
    }

    fn report_globals(&mut self) {
        if self.inexact > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.ticks_rounded",
                format!(
                    "{} positions did not land on whole ticks when converting from {} to {PPQ} \
                     ticks per quarter note and were rounded to the nearest tick",
                    self.inexact, self.ppq
                ),
                None,
            ));
        }
        if self.drum_notes > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.drum_notes_skipped",
                format!(
                    "skipped {} notes on MIDI channel 10 (drums)",
                    self.drum_notes
                ),
                None,
            ));
        }
        if self.unmatched_note_offs > 0 {
            self.diags.push(Diagnostic::info(
                "midi.note_off_unmatched",
                format!(
                    "ignored {} note-offs without a sounding note",
                    self.unmatched_note_offs
                ),
                None,
            ));
        }
        if self.default_lyrics > 0 {
            self.diags.push(Diagnostic::info(
                "midi.lyric_default",
                format!(
                    "{} notes have no lyric and sing {DEFAULT_LYRIC:?}",
                    self.default_lyrics
                ),
                None,
            ));
        }
        if self.invalid_utf8 > 0 {
            self.diags.push(Diagnostic::warning(
                "midi.invalid_utf8",
                format!(
                    "{} texts are not valid UTF-8; invalid bytes were replaced with U+FFFD",
                    self.invalid_utf8
                ),
                None,
            ));
        }
        for (&kind, &count) in &self.ignored {
            let (severity, what) = match kind {
                "pitch_bend" => (true, "pitch-bend events"),
                "controller" => (true, "control-change events"),
                "poly_aftertouch" => (true, "polyphonic aftertouch events"),
                "channel_aftertouch" => (true, "channel aftertouch events"),
                "program_change" => (false, "program changes"),
                "sysex" => (false, "system-exclusive events"),
                "velocity" => (false, "note velocities"),
                "text" => (false, "text events not used as lyrics"),
                "track_name" => (
                    false,
                    "track names of tracks without notes or repeated names",
                ),
                _ => (false, "meta events of this kind"),
            };
            let message = format!("ignored {count} {what} ({kind})");
            let code = format!("midi.ignored.{kind}");
            self.diags.push(if severity {
                Diagnostic::warning(code, message, None)
            } else {
                Diagnostic::info(code, message, None)
            });
        }
    }
}
