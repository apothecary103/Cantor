//! UTAU voicebanks: `oto.ini` (in the folder and its subfolders),
//! `prefix.map` and `character.txt`, and lyric → alias resolution.

use cantor_core::Phonemizer;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One line of `oto.ini`, in seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct OtoEntry {
    /// The recording.
    pub wav: PathBuf,
    /// The alias lyrics refer to.
    pub alias: String,
    /// Start of the usable region.
    pub offset: f64,
    /// Length of the fixed (unstretched) region from `offset`.
    pub consonant: f64,
    /// Negative: length of the usable region from `offset`; positive:
    /// silence to skip at the end of the file.
    pub cutoff: f64,
    /// Time from `offset` to the vowel onset.
    pub preutter: f64,
    /// Crossfade with the previous note, from `offset`.
    pub overlap: f64,
}

/// What kind of aliases a bank has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BankKind {
    /// One alias per mora ("か").
    Cv,
    /// Vowel-consonant-vowel strings ("a か").
    Vcv,
    /// CV plus vowel-consonant links ("a k").
    Cvvc,
    /// Several of the above.
    Mixed,
}

/// A loaded voicebank.
#[derive(Clone, Debug)]
pub struct Voicebank {
    /// Stable id: the folder name.
    pub id: String,
    /// Display name from `character.txt` (or the folder name).
    pub name: String,
    /// Author from `character.txt`.
    pub author: String,
    /// Portrait image, if `character.txt` names one that exists.
    pub image: Option<PathBuf>,
    /// The folder.
    pub root: PathBuf,
    /// Aliases (first definition wins, as in UTAU).
    pub entries: BTreeMap<String, OtoEntry>,
    /// MIDI key → (prefix, suffix).
    pub prefix_map: BTreeMap<u8, (String, String)>,
    /// Problems found while loading.
    pub warnings: Vec<String>,
    /// Changes when any oto.ini changes (part of render cache keys).
    pub version: String,
}

/// JSON summary for `GET /api/voicebanks` and `list_voicebanks`.
#[derive(Clone, Debug, Serialize)]
pub struct VoicebankInfo {
    /// Id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Author.
    pub author: String,
    /// Whether an image exists.
    pub image: bool,
    /// Number of aliases.
    pub aliases_count: usize,
    /// Alias style.
    pub kind: BankKind,
    /// Whether a non-empty prefix.map exists.
    pub prefix_map: bool,
    /// Folder.
    pub path: String,
}

/// Whether `dir` looks like a voicebank (has an oto.ini at the top or one
/// level down).
pub fn is_voicebank(dir: &Path) -> bool {
    !oto_files(dir).is_empty()
}

fn oto_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if dir.join("oto.ini").is_file() {
        out.push(dir.join("oto.ini"));
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut subs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subs.sort();
        for s in subs {
            if s.join("oto.ini").is_file() {
                out.push(s.join("oto.ini"));
            }
        }
    }
    out
}

/// Reads text as UTF-8, falling back to Shift-JIS (the UTAU default).
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => encoding_rs::SHIFT_JIS.decode(e.as_bytes()).0.into_owned(),
    })
}

/// Parses `oto.ini` text. Unreadable lines come back as warnings. An empty
/// alias defaults to the file stem.
pub fn parse_oto(text: &str, dir: &Path) -> (Vec<OtoEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let Some((file, rest)) = line.split_once('=') else {
            warnings.push(format!("oto.ini line {}: no `=`", i + 1));
            continue;
        };
        let fields: Vec<&str> = rest.split(',').collect();
        if fields.len() != 6 {
            warnings.push(format!("oto.ini line {}: expected 6 fields", i + 1));
            continue;
        }
        let num = |s: &str| s.trim().parse::<f64>().map(|ms| ms / 1000.0);
        let (Ok(offset), Ok(consonant), Ok(cutoff), Ok(preutter), Ok(overlap)) = (
            num(fields[1]),
            num(fields[2]),
            num(fields[3]),
            num(fields[4]),
            num(fields[5]),
        ) else {
            warnings.push(format!("oto.ini line {}: a number cannot be read", i + 1));
            continue;
        };
        let alias = if fields[0].trim().is_empty() {
            Path::new(file)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            fields[0].trim().to_owned()
        };
        out.push(OtoEntry {
            wav: dir.join(file.trim()),
            alias,
            offset,
            consonant,
            cutoff,
            preutter,
            overlap,
        });
    }
    (out, warnings)
}

impl Voicebank {
    /// Loads the voicebank in `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let files = oto_files(dir);
        if files.is_empty() {
            return Err(format!("{}: no oto.ini", dir.display()));
        }
        let mut entries = BTreeMap::new();
        let mut warnings = Vec::new();
        let mut hasher = blake3::Hasher::new();
        for f in &files {
            let text = read_text(f).map_err(|e| format!("{}: {e}", f.display()))?;
            hasher.update(f.to_string_lossy().as_bytes());
            hasher.update(text.as_bytes());
            let (list, w) = parse_oto(&text, f.parent().unwrap_or(dir));
            warnings.extend(w.into_iter().map(|w| format!("{}: {w}", f.display())));
            for e in list {
                entries.entry(e.alias.clone()).or_insert(e);
            }
        }
        let folder = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (mut name, mut author, mut image) = (folder.clone(), String::new(), None);
        if let Ok(text) = read_text(&dir.join("character.txt")) {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                let v = v.trim();
                match k.trim().to_ascii_lowercase().as_str() {
                    "name" if !v.is_empty() => name = v.to_owned(),
                    "author" => author = v.to_owned(),
                    "image" if !v.is_empty() => {
                        let p = dir.join(v);
                        if p.is_file() {
                            image = Some(p);
                        }
                    }
                    _ => {}
                }
            }
        }
        let prefix_map = read_prefix_map(&dir.join("prefix.map"));
        Ok(Self {
            id: folder,
            name,
            author,
            image,
            root: dir.to_owned(),
            entries,
            prefix_map,
            warnings,
            version: hasher.finalize().to_hex()[..16].to_owned(),
        })
    }

    /// Alias style, from the alias names.
    pub fn kind(&self) -> BankKind {
        let (mut cv, mut vcv, mut vc) = (0, 0, 0);
        for a in self.entries.keys() {
            let a = strip_pitch_suffix(a);
            match a.split_whitespace().collect::<Vec<_>>().as_slice() {
                [v, m] if is_vowel_token(v) && !m.is_ascii() => vcv += 1,
                [v, c] if is_vowel_token(v) && c.is_ascii() => vc += 1,
                ["-", _] => vcv += 1,
                _ => cv += 1,
            }
        }
        let n = self.entries.len().max(1);
        let big = |x: usize| x * 5 >= n;
        match (big(cv), big(vcv), big(vc)) {
            (_, true, false) if !big(cv) => BankKind::Vcv,
            (true, false, true) => BankKind::Cvvc,
            (true, false, false) => BankKind::Cv,
            (false, true, false) => BankKind::Vcv,
            _ => BankKind::Mixed,
        }
    }

    /// Summary for listings.
    pub fn info(&self) -> VoicebankInfo {
        VoicebankInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            author: self.author.clone(),
            image: self.image.is_some(),
            aliases_count: self.entries.len(),
            kind: self.kind(),
            prefix_map: !self.prefix_map.is_empty(),
            path: self.root.display().to_string(),
        }
    }

    /// The oto entry for a lyric sung at `key`, given the previous sung
    /// note's lyric (for VCV), or `None` when no alias fits.
    pub fn resolve(
        &self,
        lyric: &str,
        key: u8,
        prev: Option<&str>,
        phonemizer: Phonemizer,
    ) -> Option<&OtoEntry> {
        let (pfx, sfx) = self.prefix_map.get(&key).cloned().unwrap_or_default();
        let lookup = |stem: &str| {
            let tiered = format!("{pfx}{stem}{sfx}");
            self.entries.get(&tiered).or_else(|| self.entries.get(stem))
        };
        if phonemizer == Phonemizer::Raw {
            return lookup(lyric.trim());
        }
        let prev_vowel = prev.and_then(trailing_vowel_of);
        let stems = candidate_stems(lyric, prev_vowel, phonemizer);
        stems.iter().find_map(|s| lookup(s))
    }
}

fn is_vowel_token(s: &str) -> bool {
    matches!(s, "a" | "i" | "u" | "e" | "o" | "n" | "N")
}

/// Parses `prefix.map`: MIDI key → (prefix, suffix).
fn read_prefix_map(path: &Path) -> BTreeMap<u8, (String, String)> {
    let mut out = BTreeMap::new();
    let Ok(text) = read_text(path) else {
        return out;
    };
    for line in text.lines() {
        let line = line.trim_matches(['\r', '\n', '\u{feff}']);
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let Some(midi) = parse_note_name(cols[0].trim()) else {
            continue;
        };
        if !cols[1].is_empty() || !cols[2].is_empty() {
            out.insert(midi, (cols[1].to_owned(), cols[2].to_owned()));
        }
    }
    out
}

fn parse_note_name(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    if b.len() < 2 {
        return None;
    }
    let (pc, rest) = match (b[0].to_ascii_uppercase(), b.get(1).copied()) {
        (b'C', Some(b'#')) => (1i32, &s[2..]),
        (b'D', Some(b'#')) => (3, &s[2..]),
        (b'F', Some(b'#')) => (6, &s[2..]),
        (b'G', Some(b'#')) => (8, &s[2..]),
        (b'A', Some(b'#')) => (10, &s[2..]),
        (b'C', _) => (0, &s[1..]),
        (b'D', _) => (2, &s[1..]),
        (b'E', _) => (4, &s[1..]),
        (b'F', _) => (5, &s[1..]),
        (b'G', _) => (7, &s[1..]),
        (b'A', _) => (9, &s[1..]),
        (b'B', _) => (11, &s[1..]),
        _ => return None,
    };
    let oct: i32 = rest.parse().ok()?;
    let midi = (oct + 1) * 12 + pc;
    (0..=127).contains(&midi).then_some(midi as u8)
}

/// Strips a trailing pitch-tier suffix (`D#4`, ` F4`, `EB3`, `_LB3`).
fn strip_pitch_suffix(s: &str) -> &str {
    let t = s.trim();
    let b = t.as_bytes();
    let n = b.len();
    if n >= 2 && b[n - 1].is_ascii_digit() {
        let mut cut = n - 1;
        if cut >= 1 && b[cut - 1] == b'#' {
            cut -= 1;
        }
        if cut >= 1 && matches!(b[cut - 1], b'A'..=b'G' | b'a'..=b'g') {
            cut -= 1;
            if cut >= 1 && matches!(b[cut - 1], b'E' | b'e') {
                cut -= 1;
            } else if cut >= 2 && &b[cut - 2..cut] == b"_L" {
                cut -= 2;
            }
            let base = t[..cut].trim_end();
            if !base.is_empty() {
                return base;
            }
        }
    }
    t
}

/// Candidate alias stems for a lyric, best first.
fn candidate_stems(lyric: &str, prev_vowel: Option<&str>, phonemizer: Phonemizer) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    let raw = lyric.trim();
    let stripped = strip_pitch_suffix(raw);
    let parts: Vec<&str> = stripped.split_whitespace().collect();
    let (explicit_v, mora_raw) = match parts.as_slice() {
        [v, m] if matches!(*v, "-" | "a" | "i" | "u" | "e" | "o" | "n") => (Some(*v), *m),
        [m] => (None, m.trim_matches('-')),
        _ => (None, stripped),
    };
    let mora = mora_raw.trim_end_matches('2');
    let kana = romaji_to_hiragana(mora).unwrap_or(mora);
    let vcv = |push: &mut dyn FnMut(String), v: &str| {
        push(format!("{v} {kana}"));
        push(format!("{v} {kana}2"));
    };
    push(raw.to_owned());
    push(stripped.to_owned());
    if let Some(v) = explicit_v {
        vcv(&mut push, v);
    }
    let cv_first = matches!(phonemizer, Phonemizer::JpCv | Phonemizer::JpCvvc);
    if cv_first {
        push(kana.to_owned());
        push(format!("- {kana}"));
    }
    if let Some(pv) = prev_vowel {
        vcv(&mut push, pv);
    }
    push(format!("- {kana}"));
    push(format!("- {kana}2"));
    push(kana.to_owned());
    push(format!("{kana}_L"));
    push(format!("_{kana}"));
    if prev_vowel.is_none() && !cv_first {
        push(format!("a {kana}"));
    }
    out
}

/// Trailing vowel ("a", "i", "u", "e", "o", "n") of a Japanese lyric.
pub fn trailing_vowel_of(s: &str) -> Option<&'static str> {
    let base = strip_pitch_suffix(s)
        .trim_end_matches("_L")
        .trim_end_matches('2');
    let token = base.split_whitespace().last()?.trim_matches(['-', '_']);
    let kana = romaji_to_hiragana(token).unwrap_or(token);
    match kana.chars().last()? {
        'あ' | 'ぁ' | 'か' | 'さ' | 'た' | 'な' | 'は' | 'ま' | 'や' | 'ゃ' | 'ら' | 'わ'
        | 'が' | 'ざ' | 'だ' | 'ば' | 'ぱ' | 'a' => Some("a"),
        'い' | 'ぃ' | 'き' | 'し' | 'ち' | 'に' | 'ひ' | 'み' | 'り' | 'ぎ' | 'じ' | 'ぢ'
        | 'び' | 'ぴ' | 'i' => Some("i"),
        'う' | 'ぅ' | 'く' | 'す' | 'つ' | 'ぬ' | 'ふ' | 'む' | 'ゆ' | 'ゅ' | 'る' | 'ぐ'
        | 'ず' | 'づ' | 'ぶ' | 'ぷ' | 'u' => Some("u"),
        'え' | 'ぇ' | 'け' | 'せ' | 'て' | 'ね' | 'へ' | 'め' | 'れ' | 'げ' | 'ぜ' | 'で'
        | 'べ' | 'ぺ' | 'e' => Some("e"),
        'お' | 'ぉ' | 'こ' | 'そ' | 'と' | 'の' | 'ほ' | 'も' | 'よ' | 'ょ' | 'ろ' | 'を'
        | 'ご' | 'ぞ' | 'ど' | 'ぼ' | 'ぽ' | 'o' => Some("o"),
        'ん' | 'n' => Some("n"),
        _ => None,
    }
}

/// Romaji syllable → hiragana (Hepburn, Kunrei, and `l*` for `r*`).
pub fn romaji_to_hiragana(s: &str) -> Option<&'static str> {
    Some(match s.to_ascii_lowercase().as_str() {
        "a" => "あ",
        "i" => "い",
        "u" => "う",
        "e" => "え",
        "o" => "お",
        "n" | "nn" => "ん",
        "ka" => "か",
        "ki" => "き",
        "ku" => "く",
        "ke" => "け",
        "ko" => "こ",
        "kya" => "きゃ",
        "kyu" => "きゅ",
        "kyo" => "きょ",
        "sa" => "さ",
        "shi" | "si" => "し",
        "su" => "す",
        "se" => "せ",
        "so" => "そ",
        "sha" | "sya" => "しゃ",
        "shu" | "syu" => "しゅ",
        "sho" | "syo" => "しょ",
        "ta" => "た",
        "chi" | "ti" => "ち",
        "tsu" | "tu" => "つ",
        "te" => "て",
        "to" => "と",
        "cha" | "tya" => "ちゃ",
        "chu" | "tyu" => "ちゅ",
        "cho" | "tyo" => "ちょ",
        "na" => "な",
        "ni" => "に",
        "nu" => "ぬ",
        "ne" => "ね",
        "no" => "の",
        "nya" => "にゃ",
        "nyu" => "にゅ",
        "nyo" => "にょ",
        "ha" => "は",
        "hi" => "ひ",
        "fu" | "hu" => "ふ",
        "he" => "へ",
        "ho" => "ほ",
        "hya" => "ひゃ",
        "hyu" => "ひゅ",
        "hyo" => "ひょ",
        "ma" => "ま",
        "mi" => "み",
        "mu" => "む",
        "me" => "め",
        "mo" => "も",
        "mya" => "みゃ",
        "myu" => "みゅ",
        "myo" => "みょ",
        "ya" => "や",
        "yu" => "ゆ",
        "yo" => "よ",
        "ra" | "la" => "ら",
        "ri" | "li" => "り",
        "ru" | "lu" => "る",
        "re" | "le" => "れ",
        "ro" | "lo" => "ろ",
        "rya" | "lya" => "りゃ",
        "ryu" | "lyu" => "りゅ",
        "ryo" | "lyo" => "りょ",
        "wa" => "わ",
        "wo" => "を",
        "ga" => "が",
        "gi" => "ぎ",
        "gu" => "ぐ",
        "ge" => "げ",
        "go" => "ご",
        "gya" => "ぎゃ",
        "gyu" => "ぎゅ",
        "gyo" => "ぎょ",
        "za" => "ざ",
        "ji" | "zi" | "di" => "じ",
        "zu" | "du" => "ず",
        "ze" => "ぜ",
        "zo" => "ぞ",
        "ja" | "jya" | "zya" => "じゃ",
        "ju" | "jyu" | "zyu" => "じゅ",
        "jo" | "jyo" | "zyo" => "じょ",
        "da" => "だ",
        "de" => "で",
        "do" => "ど",
        "ba" => "ば",
        "bi" => "び",
        "bu" => "ぶ",
        "be" => "べ",
        "bo" => "ぼ",
        "bya" => "びゃ",
        "byu" => "びゅ",
        "byo" => "びょ",
        "pa" => "ぱ",
        "pi" => "ぴ",
        "pu" => "ぷ",
        "pe" => "ぺ",
        "po" => "ぽ",
        "pya" => "ぴゃ",
        "pyu" => "ぴゅ",
        "pyo" => "ぴょ",
        _ => return None,
    })
}

/// Finds voicebanks: each directory in `paths` that is a voicebank, and
/// each voicebank directly inside one. Later duplicates of an id are
/// skipped with a warning.
pub fn scan(paths: &[PathBuf]) -> (Vec<Voicebank>, Vec<String>) {
    let mut out: Vec<Voicebank> = Vec::new();
    let mut warnings = Vec::new();
    let add =
        |dir: &Path, out: &mut Vec<Voicebank>, warnings: &mut Vec<String>| match Voicebank::load(
            dir,
        ) {
            Ok(vb) if out.iter().any(|v| v.id == vb.id) => {
                warnings.push(format!(
                    "{}: a voicebank named `{}` was already found",
                    dir.display(),
                    vb.id
                ));
            }
            Ok(vb) => out.push(vb),
            Err(e) => warnings.push(e),
        };
    for p in paths {
        if !p.is_dir() {
            continue;
        }
        if p.join("oto.ini").is_file() {
            add(p, &mut out, &mut warnings);
            continue;
        }
        let Ok(rd) = std::fs::read_dir(p) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|d| d.is_dir())
            .collect();
        dirs.sort();
        for d in dirs.iter().filter(|d| is_voicebank(d)) {
            add(d, &mut out, &mut warnings);
        }
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank(lines: &str) -> Voicebank {
        let (list, _) = parse_oto(lines, Path::new("/vb"));
        Voicebank {
            id: "t".into(),
            name: "t".into(),
            author: String::new(),
            image: None,
            root: "/vb".into(),
            entries: list.into_iter().map(|e| (e.alias.clone(), e)).collect(),
            prefix_map: BTreeMap::from([(67, (String::new(), "↑".into()))]),
            warnings: Vec::new(),
            version: String::new(),
        }
    }

    #[test]
    fn parses_oto_lines() {
        let (e, w) = parse_oto(
            "ka.wav=ka,90,120,-950,60,10\n_a.wav=,80,100,-900,30,15\nbad line\n",
            Path::new("/vb"),
        );
        assert_eq!(w.len(), 1);
        assert_eq!(e[0].alias, "ka");
        assert!((e[0].preutter - 0.060).abs() < 1e-12);
        assert_eq!(e[1].alias, "_a");
    }

    #[test]
    fn resolves_cv_vcv_romaji_and_tiers() {
        let b = bank(
            "k.wav=か,0,1,-1,1,1\nk2.wav=a か,0,1,-1,1,1\nk3.wav=か↑,0,1,-1,1,1\nr.wav=- ら,0,1,-1,1,1\n",
        );
        let alias = |l, k, p, ph| b.resolve(l, k, p, ph).map(|e| e.alias.as_str());
        assert_eq!(alias("か", 60, None, Phonemizer::JpCv), Some("か"));
        assert_eq!(alias("ka", 60, Some("さ"), Phonemizer::JpVcv), Some("a か"));
        assert_eq!(alias("か", 67, None, Phonemizer::Auto), Some("か↑"));
        assert_eq!(alias("la", 60, None, Phonemizer::Auto), Some("- ら"));
        assert_eq!(alias("ka", 60, None, Phonemizer::Raw), None);
        assert_eq!(alias("ぬ", 60, None, Phonemizer::Auto), None);
    }
}
