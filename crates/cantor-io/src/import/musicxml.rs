//! MusicXML importer: uncompressed `score-partwise` files (MusicXML 3.x and
//! 4.0). The mapping and what is left out are recorded in
//! `docs/decisions/0007-musicxml-import.md`.
//!
//! The file is first read into a small element tree (MusicXML files are
//! modest in size), then each part is walked measure by measure. Positions
//! inside a measure are kept relative to the measure; measure starts are
//! shared by all parts, so parts stay aligned even if one is malformed.

use super::{Ids, ImportError, Imported, finish, rescale};
use cantor_core::{
    Diagnostic, EntityRef, KeyPoint, Meta, Note, PPQ, Part, Project, Scale, Severity, TempoPoint,
    Tick, TimeSigPoint, Track, TrackId,
};
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::{BTreeMap, BTreeSet};

const FORMAT: &str = "MusicXML";
/// Lyric for notes that have none and do not continue a sung note.
const DEFAULT_LYRIC: &str = "la";
/// Tempo used before the first tempo mark.
const DEFAULT_BPM: f64 = 120.0;
/// Deepest element nesting accepted; MusicXML needs fewer than ten levels.
const MAX_DEPTH: usize = 256;

/// Imports a MusicXML file.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    if bytes.starts_with(b"PK") {
        return Err(unsupported(
            "this is a compressed MusicXML archive (.mxl); unzip it and import the \
             .musicxml file inside",
        ));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(unsupported(
            "UTF-16 encoded files are not supported; save the score as UTF-8",
        ));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| parse_error(format!("not UTF-8: {e}")))?;
    let root = parse_xml(text)?;
    match root.name.as_str() {
        "score-partwise" => {}
        "score-timewise" => {
            return Err(unsupported(
                "score-timewise files are not supported; export the score as \
                 score-partwise (the default in notation programs)",
            ));
        }
        other => {
            return Err(parse_error(format!(
                "root element <{other}> is not a MusicXML score"
            )));
        }
    }
    convert(&root)
}

fn unsupported(message: &str) -> ImportError {
    ImportError::Unsupported {
        format: FORMAT,
        message: message.into(),
    }
}

fn parse_error(message: String) -> ImportError {
    ImportError::Parse {
        format: FORMAT,
        message,
    }
}

// ---------------------------------------------------------------------------
// Element tree

/// One XML element. Text directly inside the element is concatenated.
#[derive(Debug, Default)]
struct El {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<El>,
    text: String,
}

impl El {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.trim())
    }

    fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> {
        self.children.iter().filter(move |c| c.name == name)
    }

    fn has(&self, name: &str) -> bool {
        self.child(name).is_some()
    }

    /// Trimmed text of the first child called `name`.
    fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }
}

/// Reads XML into an element tree. The DOCTYPE is skipped (never fetched);
/// unknown entity references are kept literally.
fn parse_xml(text: &str) -> Result<El, ImportError> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<El> = Vec::new();
    let mut root: Option<El> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| parse_error(format!("at byte {}: {e}", reader.error_position())))?;
        match event {
            Event::Start(start) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(parse_error(format!(
                        "elements nested deeper than {MAX_DEPTH} levels"
                    )));
                }
                stack.push(element(&start)?);
            }
            Event::Empty(start) => {
                let el = element(&start)?;
                attach(&mut stack, &mut root, el)?;
            }
            Event::End(_) => {
                let el = stack
                    .pop()
                    .ok_or_else(|| parse_error("unexpected closing tag".into()))?;
                attach(&mut stack, &mut root, el)?;
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&c.into_inner());
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    match r.resolve_char_ref() {
                        Ok(Some(ch)) => top.text.push(ch),
                        Ok(None) => match resolve_predefined_entity(&r) {
                            Some(s) => top.text.push_str(s),
                            None => {
                                top.text.push('&');
                                top.text.push_str(&r);
                                top.text.push(';');
                            }
                        },
                        Err(e) => return Err(parse_error(format!("bad character reference: {e}"))),
                    }
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if let Some(open) = stack.last() {
        return Err(parse_error(format!("document ends inside <{}>", open.name)));
    }
    root.ok_or_else(|| parse_error("no root element".into()))
}

fn element(start: &BytesStart<'_>) -> Result<El, ImportError> {
    let mut el = El {
        name: start.local_name().as_ref().to_owned(),
        ..El::default()
    };
    for attr in start.attributes() {
        let attr = attr.map_err(|e| parse_error(format!("bad attribute: {e}")))?;
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| parse_error(format!("bad attribute value: {e}")))?;
        el.attrs.push((
            attr.key.local_name().as_ref().to_owned(),
            value.into_owned(),
        ));
    }
    Ok(el)
}

fn attach(stack: &mut [El], root: &mut Option<El>, el: El) -> Result<(), ImportError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(el);
    } else if root.is_none() {
        *root = Some(el);
    } else {
        return Err(parse_error("more than one root element".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Score walk

/// Markings Cantor does not import, reported once per kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Dropped {
    Dynamics,
    Wedges,
    Words,
    Articulations,
    Ornaments,
    Technical,
    Fermatas,
    Arpeggios,
    Glissandos,
    Harmony,
    FiguredBass,
    Repeats,
    MetricModulation,
    TransposeDouble,
}

impl Dropped {
    fn code(self) -> &'static str {
        match self {
            Self::Dynamics => "musicxml.dynamics",
            Self::Wedges => "musicxml.wedges",
            Self::Words => "musicxml.words",
            Self::Articulations => "musicxml.articulations",
            Self::Ornaments => "musicxml.ornaments",
            Self::Technical => "musicxml.technical",
            Self::Fermatas => "musicxml.fermatas",
            Self::Arpeggios => "musicxml.arpeggios",
            Self::Glissandos => "musicxml.glissandos",
            Self::Harmony => "musicxml.harmony",
            Self::FiguredBass => "musicxml.figured_bass",
            Self::Repeats => "musicxml.repeats",
            Self::MetricModulation => "musicxml.metric_modulation",
            Self::TransposeDouble => "musicxml.transpose_double",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Dynamics => "dynamics markings",
            Self::Wedges => "crescendo/diminuendo hairpins",
            Self::Words => "text directions (such as \"rit.\" or \"dolce\")",
            Self::Articulations => "articulations",
            Self::Ornaments => "ornaments",
            Self::Technical => "technical markings",
            Self::Fermatas => "fermatas",
            Self::Arpeggios => "arpeggio marks",
            Self::Glissandos => "glissandos and slides",
            Self::Harmony => "chord symbols",
            Self::FiguredBass => "figured-bass figures",
            Self::Repeats => {
                "repeat signs, endings and jumps (the music is imported as written, \
                 not unfolded)"
            }
            Self::MetricModulation => "metric-modulation metronome marks",
            Self::TransposeDouble => "octave-doubling transpositions",
        }
    }

    fn severity(self) -> Severity {
        match self {
            Self::Repeats => Severity::Warning,
            _ => Severity::Info,
        }
    }
}

/// Per-part counts of things not carried over.
#[derive(Debug, Default)]
struct PartStats {
    chord: usize,
    grace: usize,
    cue: usize,
    unpitched: usize,
    out_of_range: usize,
    too_short: usize,
    other_voices: BTreeMap<String, usize>,
    other_verses: BTreeMap<String, usize>,
    tie_unmatched: usize,
    tie_lyric: usize,
    backup_clamped: usize,
    divisions_missing: bool,
    transposed_cents: Option<f64>,
    default_lyric: usize,
}

/// Measure facts from one part.
#[derive(Debug)]
struct MeasureOut {
    number: String,
    /// Furthest position reached, relative to the measure start.
    len: Tick,
}

/// A note of the imported voice, before ties are merged.
#[derive(Debug)]
struct NoteEv {
    measure: usize,
    start: Tick,
    end: Tick,
    cents: f64,
    tie_start: bool,
    tie_stop: bool,
    lyric: Option<String>,
}

#[derive(Debug)]
struct TempoEv {
    measure: usize,
    rel: Tick,
    bpm: f64,
    from_sound: bool,
}

#[derive(Debug)]
struct KeyEv {
    measure: usize,
    rel: Tick,
    tonic: u8,
    scale: Scale,
}

#[derive(Debug)]
enum TimeEv {
    Sig(u8, u8),
    Unsupported(String),
}

/// What walking one part produces.
#[derive(Debug, Default)]
struct PartOut {
    measures: Vec<MeasureOut>,
    notes: Vec<NoteEv>,
    tempos: Vec<TempoEv>,
    keys: Vec<KeyEv>,
    unsupported_keys: usize,
    times: BTreeMap<usize, TimeEv>,
    stats: PartStats,
}

/// Counters shared by all parts.
#[derive(Debug, Default)]
struct Global {
    inexact: usize,
    dropped: BTreeMap<Dropped, usize>,
}

/// Walks one `<part>`.
struct Walker<'g> {
    global: &'g mut Global,
    out: PartOut,
    voice: Option<String>,
    verse: Option<String>,
    divisions: Option<u32>,
    /// Tick (relative to the measure) where `pos` is zero.
    base: Tick,
    /// Position in current divisions since `base`.
    pos: i64,
    measure: usize,
    measure_number: String,
    max: Tick,
    transpose_cents: f64,
    /// Index into `out.notes` of the last non-chord note, if it was kept.
    last_kept: Option<usize>,
}

impl Walker<'_> {
    fn walk(mut self, part: &El) -> Result<PartOut, ImportError> {
        for (i, m) in part.children_named("measure").enumerate() {
            self.measure = i;
            self.measure_number = m.attr("number").unwrap_or("?").to_owned();
            self.base = 0;
            self.pos = 0;
            self.max = 0;
            self.last_kept = None;
            for item in &m.children {
                match item.name.as_str() {
                    "attributes" => self.attributes(item)?,
                    "note" => self.note(item)?,
                    "backup" => {
                        let d = self.duration(item)?;
                        self.pos -= d;
                        if self.tick(self.pos) < 0 {
                            self.out.stats.backup_clamped += 1;
                            self.base = 0;
                            self.pos = 0;
                        }
                    }
                    "forward" => {
                        let d = self.duration(item)?;
                        self.pos += d;
                        let t = self.tick(self.pos);
                        self.max = self.max.max(t);
                    }
                    "direction" => self.direction(item)?,
                    "sound" => self.sound(item, 0),
                    "barline" => {
                        if item.has("repeat") || item.has("ending") {
                            self.drop_kind(Dropped::Repeats);
                        }
                    }
                    "harmony" => self.drop_kind(Dropped::Harmony),
                    "figured-bass" => self.drop_kind(Dropped::FiguredBass),
                    _ => {}
                }
            }
            self.out.measures.push(MeasureOut {
                number: self.measure_number.clone(),
                len: self.max,
            });
        }
        Ok(self.out)
    }

    fn drop_kind(&mut self, kind: Dropped) {
        *self.global.dropped.entry(kind).or_default() += 1;
    }

    fn divisions(&mut self) -> u32 {
        match self.divisions {
            Some(d) => d,
            None => {
                self.out.stats.divisions_missing = true;
                self.divisions = Some(1);
                1
            }
        }
    }

    /// Converts a position in current divisions to a measure-relative tick.
    fn tick(&mut self, pos: i64) -> Tick {
        let divisions = self.divisions();
        let (t, exact) = rescale(pos, divisions);
        if !exact {
            self.global.inexact += 1;
        }
        self.base + t
    }

    /// Parses a divisions count, rounding fractional values.
    fn divs(&mut self, text: &str, what: &str) -> Result<i64, ImportError> {
        let v: f64 = text
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| {
                parse_error(format!(
                    "invalid <{what}> \"{text}\" in measure {}",
                    self.measure_number
                ))
            })?;
        if v.fract() != 0.0 {
            self.global.inexact += 1;
        }
        Ok(v.round() as i64)
    }

    fn duration(&mut self, el: &El) -> Result<i64, ImportError> {
        let text = el.text_of("duration").ok_or_else(|| {
            parse_error(format!(
                "<{}> without <duration> in measure {}",
                el.name, self.measure_number
            ))
        })?;
        self.divs(text, "duration")
    }

    fn attributes(&mut self, a: &El) -> Result<(), ImportError> {
        if let Some(text) = a.text_of("divisions") {
            let d = self.divs(text, "divisions")?;
            let d = u32::try_from(d).ok().filter(|d| *d > 0).ok_or_else(|| {
                parse_error(format!(
                    "invalid <divisions> \"{text}\" in measure {}",
                    self.measure_number
                ))
            })?;
            if self.divisions != Some(d) {
                if self.divisions.is_some() {
                    self.base = self.tick(self.pos);
                    self.pos = 0;
                }
                self.divisions = Some(d);
            }
        }
        if let Some(t) = a.child("transpose") {
            let chromatic: f64 = t
                .text_of("chromatic")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let octaves: f64 = t
                .text_of("octave-change")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            self.transpose_cents = (chromatic + 12.0 * octaves) * 100.0;
            if self.transpose_cents != 0.0 {
                self.out.stats.transposed_cents = Some(self.transpose_cents);
            }
            if t.has("double") {
                self.drop_kind(Dropped::TransposeDouble);
            }
        }
        if let Some(k) = a.child("key") {
            let rel = self.tick(self.pos);
            let fifths = k.text_of("fifths").and_then(|f| f.parse::<i32>().ok());
            match fifths.and_then(|f| key_from_fifths(f, k.text_of("mode"))) {
                Some((tonic, scale)) => {
                    let shift = (self.transpose_cents / 100.0).round() as i64;
                    let tonic = (i64::from(tonic) + shift).rem_euclid(12) as u8;
                    self.out.keys.push(KeyEv {
                        measure: self.measure,
                        rel,
                        tonic,
                        scale,
                    });
                }
                None => self.out.unsupported_keys += 1,
            }
        }
        if let Some(t) = a.child("time") {
            let ev = parse_time(t);
            self.out.times.entry(self.measure).or_insert(ev);
        }
        Ok(())
    }

    fn note(&mut self, n: &El) -> Result<(), ImportError> {
        let voice = n.text_of("voice").unwrap_or("1").to_owned();
        let in_voice = self.voice.as_deref() == Some(voice.as_str());
        let rest = n.has("rest");
        if n.has("grace") {
            if !rest {
                self.out.stats.grace += 1;
            }
            return Ok(());
        }
        let dur = self.duration(n)?;
        if n.has("chord") {
            // Starts with the previous note and does not move the cursor.
            // Tracks are monophonic: the first note of a chord is kept.
            if rest {
                return Ok(());
            }
            if !in_voice {
                *self.out.stats.other_voices.entry(voice).or_default() += 1;
                return Ok(());
            }
            self.out.stats.chord += 1;
            let lyric = self.lyric(n);
            if let Some(i) = self.last_kept
                && let Some(head) = self.out.notes.get_mut(i)
                && head.lyric.is_none()
            {
                head.lyric = lyric;
            }
            return Ok(());
        }
        self.last_kept = None;
        let start = self.tick(self.pos);
        self.pos += dur;
        let end = self.tick(self.pos);
        self.max = self.max.max(end);
        if rest {
            return Ok(());
        }
        if !in_voice {
            *self.out.stats.other_voices.entry(voice).or_default() += 1;
            return Ok(());
        }
        if n.has("cue") {
            self.out.stats.cue += 1;
            return Ok(());
        }
        let Some(cents) = n.child("pitch").and_then(pitch_cents) else {
            if n.has("unpitched") {
                self.out.stats.unpitched += 1;
                return Ok(());
            }
            return Err(parse_error(format!(
                "note without a valid <pitch>, <unpitched> or <rest> in measure {}",
                self.measure_number
            )));
        };
        let cents = cents + self.transpose_cents;
        if !(0.0..=12_800.0).contains(&cents) {
            self.out.stats.out_of_range += 1;
            return Ok(());
        }
        if end <= start {
            self.out.stats.too_short += 1;
            return Ok(());
        }
        let mut tie_start = false;
        let mut tie_stop = false;
        let tied = n
            .children_named("notations")
            .flat_map(|nt| nt.children_named("tied"));
        for t in n.children_named("tie").chain(tied) {
            match t.attr("type") {
                Some("start") => tie_start = true,
                Some("stop") => tie_stop = true,
                Some("continue") => {
                    tie_start = true;
                    tie_stop = true;
                }
                _ => {}
            }
        }
        if n.attr("dynamics").is_some() {
            self.drop_kind(Dropped::Dynamics);
        }
        for nt in n.children_named("notations") {
            for c in &nt.children {
                let kind = match c.name.as_str() {
                    "articulations" => Dropped::Articulations,
                    "ornaments" => Dropped::Ornaments,
                    "technical" => Dropped::Technical,
                    "fermata" => Dropped::Fermatas,
                    "arpeggiate" | "non-arpeggiate" => Dropped::Arpeggios,
                    "glissando" | "slide" => Dropped::Glissandos,
                    "dynamics" => Dropped::Dynamics,
                    _ => continue,
                };
                self.drop_kind(kind);
            }
        }
        let lyric = self.lyric(n);
        self.last_kept = Some(self.out.notes.len());
        self.out.notes.push(NoteEv {
            measure: self.measure,
            start,
            end,
            cents,
            tie_start,
            tie_stop,
            lyric,
        });
        Ok(())
    }

    /// The note's lyric in the chosen verse, hyphenated: syllables that
    /// continue a word (`begin`, `middle`) get a trailing `-`. Elided
    /// syllables are joined with a space. A lyric with no text (only an
    /// `<extend>`) counts as no lyric.
    fn lyric(&mut self, n: &El) -> Option<String> {
        let mut chosen = None;
        for (i, l) in n.children_named("lyric").enumerate() {
            let key = lyric_key(l, i);
            if self.verse.as_deref() == Some(key.as_str()) {
                chosen.get_or_insert(l);
            } else if l.children_named("text").any(|t| !t.text.trim().is_empty()) {
                *self.out.stats.other_verses.entry(key).or_default() += 1;
            }
        }
        let l = chosen?;
        let mut texts = Vec::new();
        let mut last_syllabic = None;
        for c in &l.children {
            match c.name.as_str() {
                "syllabic" => last_syllabic = Some(c.text.trim()),
                "text" if !c.text.trim().is_empty() => texts.push(c.text.trim()),
                _ => {}
            }
        }
        if texts.is_empty() {
            return None;
        }
        let mut text = texts.join(" ");
        if matches!(last_syllabic, Some("begin" | "middle")) && !text.ends_with('-') {
            text.push('-');
        }
        Some(text)
    }

    fn direction(&mut self, d: &El) -> Result<(), ImportError> {
        let offset = match d.child("offset") {
            Some(o) if o.attr("sound") == Some("yes") => self.divs(o.text.trim(), "offset")?,
            _ => 0,
        };
        for dt in d.children_named("direction-type") {
            for c in &dt.children {
                match c.name.as_str() {
                    "metronome" => self.metronome(c, offset),
                    "dynamics" => self.drop_kind(Dropped::Dynamics),
                    "wedge" if c.attr("type") != Some("stop") => self.drop_kind(Dropped::Wedges),
                    "words" if !c.text.trim().is_empty() => self.drop_kind(Dropped::Words),
                    "segno" | "coda" => self.drop_kind(Dropped::Repeats),
                    _ => {}
                }
            }
        }
        if let Some(s) = d.child("sound") {
            self.sound(s, offset);
        }
        Ok(())
    }

    fn sound(&mut self, s: &El, offset: i64) {
        if let Some(bpm) = s.attr("tempo").and_then(|t| t.parse::<f64>().ok()) {
            let rel = self.tick(self.pos + offset);
            self.out.tempos.push(TempoEv {
                measure: self.measure,
                rel,
                bpm,
                from_sound: true,
            });
        }
        if s.attr("dynamics").is_some() {
            self.drop_kind(Dropped::Dynamics);
        }
        let jumps = ["dacapo", "dalsegno", "fine", "tocoda", "segno", "coda"];
        if jumps.iter().any(|j| s.attr(j).is_some()) {
            self.drop_kind(Dropped::Repeats);
        }
    }

    /// `<metronome>`: beat unit (with dots) = per-minute, as quarter BPM.
    fn metronome(&mut self, m: &El, offset: i64) {
        let units: Vec<&str> = m
            .children_named("beat-unit")
            .map(|u| u.text.trim())
            .collect();
        let &[unit] = units.as_slice() else {
            if units.len() > 1 {
                self.drop_kind(Dropped::MetricModulation);
            }
            return;
        };
        let Some(per_minute) = m.text_of("per-minute").and_then(leading_number) else {
            return;
        };
        let Some(quarters) = beat_unit_quarters(unit) else {
            return;
        };
        let dots = m.children_named("beat-unit-dot").count() as i32;
        let dotted = 2.0 - 0.5f64.powi(dots);
        let rel = self.tick(self.pos + offset);
        self.out.tempos.push(TempoEv {
            measure: self.measure,
            rel,
            bpm: per_minute * quarters * dotted,
            from_sound: false,
        });
    }
}

/// The key of a lyric for verse selection: its `number`, or its 1-based
/// position on the note.
fn lyric_key(l: &El, index: usize) -> String {
    l.attr("number")
        .map_or_else(|| (index + 1).to_string(), str::to_owned)
}

fn pitch_cents(p: &El) -> Option<f64> {
    let step = match p.text_of("step")? {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let octave: i32 = p.text_of("octave")?.parse().ok()?;
    let alter: f64 = match p.text_of("alter") {
        Some(a) => a.parse().ok().filter(|a: &f64| a.is_finite())?,
        None => 0.0,
    };
    let midi = f64::from((octave + 1) * 12 + step);
    Some((midi + alter) * 100.0)
}

/// Tonic pitch class and scale from a traditional key signature.
fn key_from_fifths(fifths: i32, mode: Option<&str>) -> Option<(u8, Scale)> {
    let major = (fifths * 7).rem_euclid(12);
    let (offset, scale) = match mode.unwrap_or("major") {
        "major" | "ionian" | "" => (0, Scale::Major),
        "minor" | "aeolian" => (9, Scale::Minor),
        "dorian" => (2, Scale::Dorian),
        "phrygian" => (4, Scale::Phrygian),
        "lydian" => (5, Scale::Lydian),
        "mixolydian" => (7, Scale::Mixolydian),
        "locrian" => (11, Scale::Locrian),
        "none" => (0, Scale::Chromatic),
        _ => return None,
    };
    Some(((major + offset).rem_euclid(12) as u8, scale))
}

fn parse_time(t: &El) -> TimeEv {
    if t.has("senza-misura") {
        return TimeEv::Unsupported("senza misura".into());
    }
    let beats: Vec<&str> = t.children_named("beats").map(|b| b.text.trim()).collect();
    let types: Vec<&str> = t
        .children_named("beat-type")
        .map(|b| b.text.trim())
        .collect();
    let shown = beats
        .iter()
        .zip(&types)
        .map(|(b, t)| format!("{b}/{t}"))
        .collect::<Vec<_>>()
        .join(" + ");
    if beats.len() != types.len() || types.is_empty() || types.iter().any(|t| *t != types[0]) {
        return TimeEv::Unsupported(shown);
    }
    let mut num: u32 = 0;
    for b in &beats {
        for part in b.split('+') {
            match part.trim().parse::<u32>() {
                Ok(n) => num += n,
                Err(_) => return TimeEv::Unsupported(shown),
            }
        }
    }
    match (u8::try_from(num), types[0].parse::<u8>()) {
        (Ok(n), Ok(d)) if n >= 1 && d.is_power_of_two() && d <= 64 => TimeEv::Sig(n, d),
        _ => TimeEv::Unsupported(shown),
    }
}

fn beat_unit_quarters(unit: &str) -> Option<f64> {
    Some(match unit {
        "maxima" => 32.0,
        "long" => 16.0,
        "breve" => 8.0,
        "whole" => 4.0,
        "half" => 2.0,
        "quarter" => 1.0,
        "eighth" => 0.5,
        "16th" => 0.25,
        "32nd" => 0.125,
        "64th" => 0.0625,
        "128th" => 0.031_25,
        _ => return None,
    })
}

/// Parses the first number in text like `"120"`, `"72-76"` or `"c. 60"`.
fn leading_number(text: &str) -> Option<f64> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok().filter(|v: &f64| *v > 0.0)
}

fn bar_ticks(n: u8, d: u8) -> Tick {
    cantor_core::time::bar_ticks(n, d)
}

/// A time signature lasting exactly `len` ticks, preferring `prefer_den`.
fn sig_for_len(len: Tick, prefer_den: u8) -> Option<(u8, u8)> {
    let whole = 4 * Tick::from(PPQ);
    let with_den = |d: u8| {
        let scaled = len * Tick::from(d);
        if scaled % whole != 0 {
            return None;
        }
        u8::try_from(scaled / whole)
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| (n, d))
    };
    with_den(prefer_den).or_else(|| [1, 2, 4, 8, 16, 32, 64].into_iter().find_map(with_den))
}

// ---------------------------------------------------------------------------
// Assembly

fn convert(root: &El) -> Result<Imported, ImportError> {
    let mut diags = Vec::new();
    let mut project = Project::new();
    project.meta = meta(root);
    project.tempo_map.clear();
    project.time_sig_map.clear();

    let names: BTreeMap<&str, &str> = root
        .child("part-list")
        .map(|pl| {
            pl.children_named("score-part")
                .filter_map(|sp| {
                    let id = sp.attr("id")?;
                    let name = sp
                        .text_of("part-name")
                        .filter(|n| !n.is_empty())
                        .or_else(|| sp.text_of("part-abbreviation").filter(|n| !n.is_empty()))
                        .unwrap_or(id);
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default();

    // Walk every part.
    let mut global = Global::default();
    let parts: Vec<&El> = root.children_named("part").collect();
    let mut outs = Vec::with_capacity(parts.len());
    for part in &parts {
        let voice = choose_voice(part);
        let verse = voice.as_deref().and_then(|v| choose_verse(part, v));
        let walker = Walker {
            global: &mut global,
            out: PartOut::default(),
            voice,
            verse,
            divisions: None,
            base: 0,
            pos: 0,
            measure: 0,
            measure_number: String::new(),
            max: 0,
            transpose_cents: 0.0,
            last_kept: None,
        };
        outs.push(walker.walk(part)?);
    }

    // Time signatures come from the first part that has any; measure
    // lengths are the longest any part reaches.
    let master = outs.iter().position(|o| !o.times.is_empty());
    let measure_count = outs.iter().map(|o| o.measures.len()).max().unwrap_or(0);
    let mut nominal = (4u8, 4u8);
    if master.is_none_or(|m| !outs[m].times.contains_key(&0)) {
        diags.push(Diagnostic::info(
            "musicxml.time_sig_default",
            "the score has no time signature at its start; assuming 4/4",
            Some(EntityRef::TimeSigMap),
        ));
    }
    let mut starts = Vec::with_capacity(measure_count + 1);
    let mut tick: Tick = 0;
    for i in 0..measure_count {
        if let Some(m) = master {
            match outs[m].times.get(&i) {
                Some(TimeEv::Sig(n, d)) => nominal = (*n, *d),
                Some(TimeEv::Unsupported(shown)) => diags.push(Diagnostic::warning(
                    "musicxml.time_sig_unsupported",
                    format!(
                        "time signature {shown} in measure {} cannot be represented; \
                         keeping {}/{}",
                        outs[m].measures[i].number, nominal.0, nominal.1
                    ),
                    Some(EntityRef::TimeSigMap),
                )),
                None => {}
            }
            for (pi, o) in outs.iter().enumerate() {
                if pi != m
                    && let Some(TimeEv::Sig(n, d)) = o.times.get(&i)
                    && (*n, *d) != nominal
                {
                    diags.push(Diagnostic::warning(
                        "musicxml.time_sig_conflict",
                        format!(
                            "part {} has {n}/{d} in measure {} where the first part has \
                             {}/{}; the first part's time signature is used",
                            pi + 1,
                            o.measures[i].number,
                            nominal.0,
                            nominal.1
                        ),
                        Some(EntityRef::TimeSigMap),
                    ));
                }
            }
        }
        let nominal_len = bar_ticks(nominal.0, nominal.1);
        let reached = outs
            .iter()
            .filter_map(|o| o.measures.get(i))
            .map(|m| m.len)
            .max()
            .unwrap_or(0);
        let number = outs
            .iter()
            .find_map(|o| o.measures.get(i))
            .map_or("?", |m| m.number.as_str());
        let (len, sig) = if reached == 0 || reached == nominal_len {
            (nominal_len, nominal)
        } else if let Some(sig) = sig_for_len(reached, nominal.1) {
            diags.push(Diagnostic::info(
                "musicxml.irregular_measure",
                format!(
                    "measure {number} lasts {reached} ticks instead of {nominal_len} \
                     ({}/{}); it is imported as a bar of {}/{}",
                    nominal.0, nominal.1, sig.0, sig.1
                ),
                Some(EntityRef::TimeSigMap),
            ));
            (reached, sig)
        } else {
            diags.push(Diagnostic::warning(
                "musicxml.irregular_measure",
                format!(
                    "measure {number} lasts {reached} ticks, which no time signature \
                     expresses; notes keep their timing but later bar lines are shifted"
                ),
                Some(EntityRef::TimeSigMap),
            ));
            (reached, nominal)
        };
        if project
            .time_sig_map
            .last()
            .is_none_or(|p| (p.numerator, p.denominator) != sig)
        {
            project.time_sig_map.push(TimeSigPoint {
                bar: i as u32,
                numerator: sig.0,
                denominator: sig.1,
            });
        }
        starts.push(tick);
        tick += len;
    }
    let score_len = tick;
    if project.time_sig_map.is_empty() {
        project.time_sig_map.push(TimeSigPoint {
            bar: 0,
            numerator: nominal.0,
            denominator: nominal.1,
        });
    }
    let at = |measure: usize, rel: Tick| starts.get(measure).copied().unwrap_or(0) + rel;

    // Tracks.
    let mut ids = Ids::new();
    let mut track_ids = Vec::with_capacity(outs.len());
    for (part, out) in parts.iter().zip(&mut outs) {
        let id = part.attr("id").unwrap_or("");
        let name = names.get(id).copied().unwrap_or(id).to_owned();
        let track = build_track(&mut ids, name, out, &at, score_len, &mut diags);
        track_ids.push(track.id);
        project.tracks.push(track);
    }

    // Keys from the first part that has any.
    if let Some(src) = outs
        .iter()
        .position(|o| !o.keys.is_empty() || o.unsupported_keys > 0)
    {
        let o = &outs[src];
        if o.unsupported_keys > 0 {
            diags.push(Diagnostic::warning(
                "musicxml.key_unsupported",
                format!(
                    "{} non-traditional or unknown-mode key signatures were not imported",
                    o.unsupported_keys
                ),
                Some(EntityRef::KeyMap),
            ));
        }
        let mut keys: BTreeMap<Tick, (u8, Scale)> = BTreeMap::new();
        for k in &o.keys {
            keys.insert(at(k.measure, k.rel), (k.tonic, k.scale));
        }
        for (tick, (tonic, scale)) in keys {
            if project
                .key_map
                .last()
                .is_none_or(|p| (p.tonic, p.scale) != (tonic, scale))
            {
                project.key_map.push(KeyPoint { tick, tonic, scale });
            }
        }
    }

    // Tempo: merged from all parts. At one tick, a <sound tempo> wins over a
    // <metronome>, and earlier parts win over later ones.
    let mut tempos: BTreeMap<Tick, (Option<f64>, Option<f64>)> = BTreeMap::new();
    let mut conflicts = 0;
    for o in &outs {
        for t in &o.tempos {
            let entry = tempos.entry(at(t.measure, t.rel)).or_default();
            let slot = if t.from_sound {
                &mut entry.0
            } else {
                &mut entry.1
            };
            match slot {
                Some(bpm) if t.from_sound && *bpm != t.bpm => conflicts += 1,
                Some(_) => {}
                None => *slot = Some(t.bpm),
            }
        }
    }
    if conflicts > 0 {
        diags.push(Diagnostic::warning(
            "musicxml.tempo_conflict",
            format!(
                "{conflicts} tempo marks disagree with an earlier part at the same \
                 position; the earlier part's tempo is used"
            ),
            Some(EntityRef::TempoMap),
        ));
    }
    for (tick, (sound, metronome)) in tempos {
        let Some(bpm) = sound.or(metronome) else {
            continue;
        };
        if !(cantor_core::MIN_BPM..=cantor_core::MAX_BPM).contains(&bpm) {
            diags.push(Diagnostic::warning(
                "musicxml.tempo_invalid",
                format!("tempo {bpm} at tick {tick} is out of range and was skipped"),
                Some(EntityRef::TempoMap),
            ));
            continue;
        }
        if project.tempo_map.last().is_none_or(|p| p.bpm != bpm) {
            project.tempo_map.push(TempoPoint { tick, bpm });
        }
    }
    if project.tempo_map.first().is_none_or(|p| p.tick != 0) {
        diags.push(Diagnostic::info(
            "musicxml.tempo_default",
            format!("the score has no tempo at its start; using {DEFAULT_BPM} BPM"),
            Some(EntityRef::TempoMap),
        ));
        project.tempo_map.insert(
            0,
            TempoPoint {
                tick: 0,
                bpm: DEFAULT_BPM,
            },
        );
    }

    if global.inexact > 0 {
        diags.push(Diagnostic::warning(
            "musicxml.rounding",
            format!(
                "{} positions or durations did not fall on a tick ({PPQ} per quarter) \
                 and were rounded",
                global.inexact
            ),
            None,
        ));
    }
    for (kind, count) in &global.dropped {
        diags.push(Diagnostic::new(
            kind.severity(),
            kind.code(),
            format!("{count} {} were not imported", kind.describe()),
            None,
        ));
    }
    finish(project, diags)
}

fn meta(root: &El) -> Meta {
    let nonempty = |s: &&str| !s.is_empty();
    let work = root
        .child("work")
        .and_then(|w| w.text_of("work-title"))
        .filter(nonempty);
    let movement = root.text_of("movement-title").filter(nonempty);
    let mut comment = Vec::new();
    if let (Some(w), Some(m)) = (work, movement)
        && w != m
    {
        comment.push(format!("Movement: {m}"));
    }
    let mut artists: Vec<&str> = Vec::new();
    if let Some(ident) = root.child("identification") {
        for c in ident.children_named("creator") {
            let text = c.text.trim();
            if text.is_empty() {
                continue;
            }
            match c.attr("type") {
                Some("composer" | "lyricist") => {
                    if !artists.contains(&text) {
                        artists.push(text);
                    }
                }
                Some(other) => comment.push(format!("{}: {text}", capitalize(other))),
                None => comment.push(format!("Creator: {text}")),
            }
        }
        for r in ident.children_named("rights") {
            if !r.text.trim().is_empty() {
                comment.push(format!("Rights: {}", r.text.trim()));
            }
        }
    }
    Meta {
        title: work.or(movement).unwrap_or_default().to_owned(),
        artist: artists.join(", "),
        comment: comment.join("\n"),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The voice of the part's first pitched, sounding note.
fn choose_voice(part: &El) -> Option<String> {
    part.children_named("measure")
        .flat_map(|m| m.children_named("note"))
        .find(|n| n.has("pitch") && !n.has("grace") && !n.has("cue"))
        .map(|n| n.text_of("voice").unwrap_or("1").to_owned())
}

/// Verse `1` if the voice has one, else the first verse seen.
fn choose_verse(part: &El, voice: &str) -> Option<String> {
    let mut first = None;
    let notes = part
        .children_named("measure")
        .flat_map(|m| m.children_named("note"))
        .filter(|n| n.text_of("voice").unwrap_or("1") == voice && !n.has("grace"));
    for n in notes {
        for (i, l) in n.children_named("lyric").enumerate() {
            let key = lyric_key(l, i);
            if key == "1" {
                return Some(key);
            }
            first.get_or_insert(key);
        }
    }
    first
}

fn build_track(
    ids: &mut Ids,
    name: String,
    out: &mut PartOut,
    at: &impl Fn(usize, Tick) -> Tick,
    score_len: Tick,
    diags: &mut Vec<Diagnostic>,
) -> Track {
    let track_id = ids.track();
    let part_id = ids.part();
    let stats = &mut out.stats;

    // Merge ties.
    let mut evs: Vec<NoteEv> = std::mem::take(&mut out.notes);
    for e in &mut evs {
        e.start = at(e.measure, e.start);
        e.end = at(e.measure, e.end);
    }
    evs.sort_by_key(|e| e.start);
    let mut merged: Vec<NoteEv> = Vec::with_capacity(evs.len());
    let mut open: Option<usize> = None;
    for e in evs {
        if e.tie_stop {
            if let Some(i) = open
                && merged[i].cents == e.cents
                && merged[i].end == e.start
            {
                merged[i].end = e.end;
                if e.lyric.is_some() {
                    stats.tie_lyric += 1;
                }
                if !e.tie_start {
                    open = None;
                }
                continue;
            }
            stats.tie_unmatched += 1;
        }
        if open.take().is_some() {
            stats.tie_unmatched += 1;
        }
        if e.tie_start {
            open = Some(merged.len());
        }
        merged.push(e);
    }
    if open.is_some() {
        stats.tie_unmatched += 1;
    }

    // Lyrics: a note without one continues the previous note ("+") when it
    // starts exactly where a sung note ends; otherwise it sings the default.
    let has_lyrics = merged.iter().any(|e| e.lyric.is_some());
    let mut prev_end = None;
    let mut notes = Vec::with_capacity(merged.len());
    for e in merged {
        let lyric = match e.lyric {
            Some(text) => text,
            None if has_lyrics && prev_end == Some(e.start) => "+".to_owned(),
            None => {
                stats.default_lyric += 1;
                DEFAULT_LYRIC.to_owned()
            }
        };
        prev_end = Some(e.end);
        notes.push(Note {
            id: ids.note(),
            start: e.start,
            length: e.end - e.start,
            pitch_cents: e.cents,
            lyric,
            lang: None,
            phonemes: None,
            expr: Default::default(),
            modes: Default::default(),
        });
    }
    let end = notes.iter().map(|n| n.start + n.length).max().unwrap_or(0);
    part_diags(track_id, &name, stats, notes.is_empty(), has_lyrics, diags);

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
            start: 0,
            length: score_len.max(end).max(1),
            notes,
        }],
    }
}

fn part_diags(
    track: TrackId,
    name: &str,
    s: &PartStats,
    empty: bool,
    has_lyrics: bool,
    diags: &mut Vec<Diagnostic>,
) {
    let target = || Some(EntityRef::Track { id: track });
    let mut push = |severity, code: &str, message: String| {
        diags.push(Diagnostic::new(
            severity,
            code,
            format!("{name}: {message}"),
            target(),
        ));
    };
    if s.divisions_missing {
        push(
            Severity::Warning,
            "musicxml.divisions_missing",
            "durations appear before <divisions>; assuming 1 division per quarter".into(),
        );
    }
    if empty {
        push(
            Severity::Info,
            "musicxml.empty_part",
            "the part has no pitched notes to import".into(),
        );
    }
    if !s.other_voices.is_empty() {
        let voices: BTreeSet<&str> = s.other_voices.keys().map(String::as_str).collect();
        let count: usize = s.other_voices.values().sum();
        push(
            Severity::Warning,
            "musicxml.voice",
            format!(
                "tracks are monophonic, so only the first voice was imported; {count} notes \
                 in voice(s) {} were not",
                voices.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }
    if s.chord > 0 {
        push(
            Severity::Warning,
            "musicxml.chord",
            format!(
                "tracks are monophonic, so each chord keeps its first note; {} chord notes \
                 were not imported",
                s.chord
            ),
        );
    }
    let skipped = [
        (s.grace, Severity::Info, "musicxml.grace", "grace notes"),
        (s.cue, Severity::Info, "musicxml.cue", "cue notes"),
        (
            s.unpitched,
            Severity::Warning,
            "musicxml.unpitched",
            "unpitched (percussion) notes",
        ),
        (
            s.out_of_range,
            Severity::Warning,
            "musicxml.pitch_out_of_range",
            "notes outside the MIDI pitch range",
        ),
        (
            s.too_short,
            Severity::Warning,
            "musicxml.note_too_short",
            "notes shorter than one tick",
        ),
    ];
    for (count, severity, code, what) in skipped {
        if count > 0 {
            push(severity, code, format!("{count} {what} were not imported"));
        }
    }
    if let Some(cents) = s.transposed_cents {
        push(
            Severity::Info,
            "musicxml.transpose",
            format!(
                "the part is written transposed; notes were moved {cents} cents to sounding pitch"
            ),
        );
    }
    if !s.other_verses.is_empty() {
        let verses: Vec<&str> = s.other_verses.keys().map(String::as_str).collect();
        let count: usize = s.other_verses.values().sum();
        push(
            Severity::Info,
            "musicxml.lyric_verse",
            format!(
                "only one verse is imported; {count} lyrics of verse(s) {} were not",
                verses.join(", ")
            ),
        );
    }
    if s.tie_lyric > 0 {
        push(
            Severity::Warning,
            "musicxml.tie_lyric",
            format!(
                "{} lyrics on tied continuation notes were dropped when the ties were merged",
                s.tie_lyric
            ),
        );
    }
    if s.tie_unmatched > 0 {
        push(
            Severity::Info,
            "musicxml.tie_unmatched",
            format!(
                "{} ties had no matching note to tie to and were ignored",
                s.tie_unmatched
            ),
        );
    }
    if s.backup_clamped > 0 {
        push(
            Severity::Warning,
            "musicxml.backup",
            format!(
                "{} <backup> elements went before the start of their measure and were clamped",
                s.backup_clamped
            ),
        );
    }
    if s.default_lyric > 0 {
        let message = if has_lyrics {
            format!(
                "{} notes have no lyric and do not continue a sung note; they sing \"{DEFAULT_LYRIC}\"",
                s.default_lyric
            )
        } else {
            format!(
                "the part has no lyrics; its {} notes sing \"{DEFAULT_LYRIC}\"",
                s.default_lyric
            )
        };
        push(Severity::Info, "musicxml.lyric_default", message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_fifths() {
        assert_eq!(key_from_fifths(0, None), Some((0, Scale::Major)));
        assert_eq!(key_from_fifths(1, Some("major")), Some((7, Scale::Major)));
        assert_eq!(key_from_fifths(-3, Some("minor")), Some((0, Scale::Minor)));
        assert_eq!(key_from_fifths(2, Some("dorian")), Some((4, Scale::Dorian)));
        assert_eq!(key_from_fifths(0, Some("weird")), None);
    }

    #[test]
    fn irregular_bars() {
        assert_eq!(sig_for_len(1920, 4), Some((1, 4)));
        assert_eq!(sig_for_len(960, 4), Some((1, 8)));
        assert_eq!(sig_for_len(1920, 8), Some((2, 8)));
        assert_eq!(sig_for_len(7, 4), None);
    }

    #[test]
    fn metronome_numbers() {
        assert_eq!(leading_number("120"), Some(120.0));
        assert_eq!(leading_number("72-76"), Some(72.0));
        assert_eq!(leading_number("c. 60"), Some(60.0));
        assert_eq!(leading_number("fast"), None);
    }
}
