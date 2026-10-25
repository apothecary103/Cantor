//! Phonemization: dictionaries, lyric parsing and phoneme placement.
//!
//! Dictionary lookup (the OpenUtau `dsdict.yaml` format used by DiffSinger
//! voicebanks), X-SAMPA mapping ([`xsampa`]) and the sandboxed WASM
//! phonemizer host ([`wasm`]).

#![deny(unsafe_code)]

pub mod place;
pub mod wasm;
pub mod xsampa;

use serde::Deserialize;
use std::collections::HashMap;

/// What kind of sound a phoneme is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneKind {
    /// Vowels, and the silence/breath symbols SP and AP: syllable nuclei.
    Vowel,
    /// Everything else.
    Consonant,
}

/// A grapheme → phonemes dictionary with phoneme kinds.
#[derive(Clone, Debug, Default)]
pub struct Dictionary {
    kinds: HashMap<String, PhoneKind>,
    entries: HashMap<String, Vec<String>>,
}

/// Why a dictionary could not be read.
#[derive(Debug, thiserror::Error)]
#[error("cannot read phoneme dictionary: {0}")]
pub struct DictError(String);

#[derive(Deserialize)]
struct RawDict {
    #[serde(default)]
    symbols: Vec<RawSymbol>,
    #[serde(default)]
    entries: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawSymbol {
    symbol: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct RawEntry {
    grapheme: String,
    phonemes: Vec<String>,
}

impl Dictionary {
    /// Parses a `dsdict.yaml`.
    pub fn from_yaml(text: &str) -> Result<Self, DictError> {
        // Voicebank dictionaries hold 100k+ entries, far above the parser's
        // default anti-DoS budget. They are local files the user installed.
        let mut budget = serde_saphyr::Budget::default();
        budget.max_nodes = 50_000_000;
        budget.max_events = 200_000_000;
        budget.max_total_scalar_bytes = 1 << 30;
        budget.max_reader_input_bytes = Some(1 << 30);
        let mut options = serde_saphyr::Options::default();
        options.budget = Some(budget);
        let raw: RawDict = serde_saphyr::from_str_with_options(text, options)
            .map_err(|e| DictError(e.to_string()))?;
        let mut d = Self::default();
        for s in raw.symbols {
            let kind = if s.kind == "vowel" {
                PhoneKind::Vowel
            } else {
                PhoneKind::Consonant
            };
            d.kinds.insert(s.symbol, kind);
        }
        for e in raw.entries {
            d.entries.insert(e.grapheme, e.phonemes);
        }
        Ok(d)
    }

    /// Adds entries from another dictionary; existing entries win.
    pub fn merge(&mut self, other: Dictionary) {
        for (k, v) in other.kinds {
            self.kinds.entry(k).or_insert(v);
        }
        for (k, v) in other.entries {
            self.entries.entry(k).or_insert(v);
        }
    }

    /// Phonemes for a grapheme.
    pub fn lookup(&self, grapheme: &str) -> Option<&[String]> {
        self.entries.get(grapheme).map(Vec::as_slice)
    }

    /// The kind of a phoneme; unknown symbols count as consonants.
    pub fn kind(&self, phoneme: &str) -> PhoneKind {
        self.kinds
            .get(phoneme)
            .copied()
            .unwrap_or(PhoneKind::Consonant)
    }

    /// Whether the dictionary declares this phoneme.
    pub fn has_symbol(&self, phoneme: &str) -> bool {
        self.kinds.contains_key(phoneme)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// How a lyric relates to its note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lyric {
    /// `+`: continue the previous note's vowel.
    Continue,
    /// `SP`, `R`, `-` or empty: silence.
    Rest,
    /// `AP` or `br`: a breath.
    Breath,
    /// A syllable to look up, with hyphens and case normalized.
    Syllable(String),
}

/// Parses a note lyric. Leading/trailing `-` mark syllables of one word and
/// are not part of the grapheme.
pub fn parse_lyric(lyric: &str) -> Lyric {
    let t = lyric.trim();
    match t {
        "+" | "+~" | "+*" => Lyric::Continue,
        "" | "-" | "SP" | "R" | "r" => Lyric::Rest,
        "AP" | "br" | "breath" => Lyric::Breath,
        _ if t.starts_with('+') => Lyric::Continue,
        _ => Lyric::Syllable(t.trim_matches('-').to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICT: &str = "\
symbols:
- symbol: SP
  type: vowel
- symbol: zh/l
  type: liquid
- symbol: zh/a
  type: vowel
entries:
- grapheme: la
  phonemes:
  - zh/l
  - zh/a
";

    #[test]
    fn parses_dsdict() {
        let d = Dictionary::from_yaml(DICT).unwrap();
        assert_eq!(d.lookup("la").unwrap(), ["zh/l", "zh/a"]);
        assert_eq!(d.kind("zh/a"), PhoneKind::Vowel);
        assert_eq!(d.kind("zh/l"), PhoneKind::Consonant);
        assert!(d.lookup("xyz").is_none());
    }

    #[test]
    fn lyrics() {
        assert_eq!(parse_lyric("+"), Lyric::Continue);
        assert_eq!(parse_lyric("hel-"), Lyric::Syllable("hel".into()));
        assert_eq!(parse_lyric("-LO"), Lyric::Syllable("lo".into()));
        assert_eq!(parse_lyric("AP"), Lyric::Breath);
        assert_eq!(parse_lyric(""), Lyric::Rest);
    }
}
