//! A Japanese phonemizer for Cantor: kana or romaji lyrics → romaji-style
//! phonemes (`k a`, `sh i`, `N`, `cl`), the symbol set DiffSinger
//! voicebanks use for Japanese.

use cantor_plugin_sdk::{
    API_VERSION, Info, Lyric, Note, NotePhonemes, PhoneInfo, PhoneKind, Phonemizer, parse_lyric,
};

struct JaKana;

/// Every symbol this plugin emits: (symbol, vowel?, X-SAMPA).
const INVENTORY: &[(&str, bool, &str)] = &[
    ("a", true, "a"),
    ("i", true, "i"),
    ("u", true, "M"),
    ("e", true, "e"),
    ("o", true, "o"),
    ("N", true, "N\\"),
    ("cl", false, "?"),
    ("b", false, "b"),
    ("by", false, "b_j"),
    ("ch", false, "tS"),
    ("d", false, "d"),
    ("dy", false, "d_j"),
    ("f", false, "p\\"),
    ("g", false, "g"),
    ("gw", false, "g_w"),
    ("gy", false, "g_j"),
    ("h", false, "h"),
    ("hy", false, "C"),
    ("j", false, "dZ"),
    ("k", false, "k"),
    ("kw", false, "k_w"),
    ("ky", false, "k_j"),
    ("m", false, "m"),
    ("my", false, "m_j"),
    ("n", false, "n"),
    ("ny", false, "J"),
    ("p", false, "p"),
    ("py", false, "p_j"),
    ("r", false, "4"),
    ("ry", false, "4_j"),
    ("s", false, "s"),
    ("sh", false, "S"),
    ("t", false, "t"),
    ("ts", false, "ts"),
    ("ty", false, "t_j"),
    ("v", false, "v"),
    ("w", false, "M\\"),
    ("y", false, "j"),
    ("z", false, "z"),
];

/// Consonant spellings in romaji, longest first so `sh` wins over `s`.
const ONSETS: &[&str] = &[
    "sh", "ch", "ts", "ky", "gy", "ny", "hy", "by", "py", "my", "ry", "dy", "ty", "kw", "gw", "j",
    "f", "v", "k", "g", "s", "z", "t", "d", "n", "h", "b", "p", "m", "y", "r", "w",
];

impl Phonemizer for JaKana {
    fn info() -> Info {
        Info {
            name: "Japanese kana/romaji".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            languages: vec!["ja".into()],
        }
    }

    fn inventory(lang: String) -> Vec<PhoneInfo> {
        if lang.split('-').next() != Some("ja") {
            return Vec::new();
        }
        INVENTORY
            .iter()
            .map(|&(symbol, vowel, xsampa)| PhoneInfo {
                symbol: symbol.into(),
                kind: if vowel {
                    PhoneKind::Vowel
                } else {
                    PhoneKind::Consonant
                },
                xsampa: Some(xsampa.into()),
            })
            .collect()
    }

    fn phonemize(notes: Vec<Note>) -> Result<Vec<NotePhonemes>, String> {
        Ok(notes
            .iter()
            .map(|n| match parse_lyric(&n.lyric) {
                Lyric::Syllable(s) => match phonemes(s) {
                    Ok(phonemes) => NotePhonemes {
                        phonemes,
                        warnings: Vec::new(),
                    },
                    Err(e) => NotePhonemes {
                        phonemes: Vec::new(),
                        warnings: vec![format!("`{}`: {e}", n.lyric)],
                    },
                },
                _ => NotePhonemes {
                    phonemes: Vec::new(),
                    warnings: Vec::new(),
                },
            })
            .collect())
    }
}

cantor_plugin_sdk::export_phonemizer!(JaKana);

/// Phonemes for one lyric in kana or romaji.
pub fn phonemes(lyric: &str) -> Result<Vec<String>, String> {
    let romaji = to_romaji(lyric)?;
    romaji_phonemes(&romaji)
}

/// Converts kana to romaji; romaji passes through lowercased. `っ` becomes
/// `Q` and `ー` becomes `-`, resolved by [`romaji_phonemes`].
pub fn to_romaji(text: &str) -> Result<String, String> {
    let chars: Vec<char> = text.chars().map(katakana_to_hiragana).collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
            i += 1;
            continue;
        }
        match c {
            'っ' => out.push('Q'),
            'ー' => out.push('-'),
            'ん' => out.push_str("n'"),
            _ => {
                let base = kana(c).ok_or_else(|| format!("unknown character `{c}`"))?;
                // Small ゃゅょ make yōon; small vowels make extended kana.
                if let Some(v) = next.and_then(small_y) {
                    let onset = &base[..base.len() - 1];
                    match onset {
                        "sh" | "ch" | "j" => out.push_str(onset),
                        _ => {
                            out.push_str(onset);
                            out.push('y');
                        }
                    }
                    out.push(v);
                    i += 2;
                    continue;
                }
                if let Some(v) = next.and_then(small_vowel) {
                    let onset = match base {
                        "fu" => "f",
                        "vu" => "v",
                        "te" => "t",
                        "de" => "d",
                        "u" => "w",
                        "tsu" => "ts",
                        "shi" => "sh",
                        "chi" => "ch",
                        "ji" => "j",
                        _ => return Err(format!("unsupported combination `{c}{}`", chars[i + 1])),
                    };
                    out.push_str(onset);
                    out.push(v);
                    i += 2;
                    continue;
                }
                out.push_str(base);
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Splits romaji into phonemes.
pub fn romaji_phonemes(romaji: &str) -> Result<Vec<String>, String> {
    let s = romaji.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut last_vowel: Option<char> = None;
    let mut i = 0;
    let is_vowel = |b: u8| matches!(b, b'a' | b'i' | b'u' | b'e' | b'o');
    while i < s.len() {
        let b = s[i];
        if is_vowel(b) {
            out.push((b as char).to_string());
            last_vowel = Some(b as char);
            i += 1;
        } else if b == b'-' {
            let v = last_vowel.ok_or("a long-vowel mark needs a vowel before it")?;
            out.push(v.to_string());
            i += 1;
        } else if b == b'\'' {
            i += 1;
        } else if b == b'Q' {
            out.push("cl".into());
            i += 1;
        } else if b == b'n'
            && (i + 1 == s.len() || s[i + 1] == b'\'' || (!is_vowel(s[i + 1]) && s[i + 1] != b'y'))
        {
            out.push("N".into());
            i += 1;
        } else if i + 1 < s.len() && s[i + 1] == b && b != b'n' {
            // Doubled consonant: a geminate stop.
            out.push("cl".into());
            i += 1;
        } else {
            let rest = &romaji[i..];
            let onset = ONSETS
                .iter()
                .find(|o| rest.starts_with(**o))
                .ok_or_else(|| format!("cannot read `{rest}`"))?;
            let after = i + onset.len();
            if after >= s.len() || !is_vowel(s[after]) {
                return Err(format!("`{onset}` must be followed by a vowel"));
            }
            out.push((*onset).into());
            i = after;
        }
    }
    if out.is_empty() {
        return Err("no phonemes".into());
    }
    Ok(out)
}

fn katakana_to_hiragana(c: char) -> char {
    match c {
        'ァ'..='ヶ' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn small_y(c: char) -> Option<char> {
    match c {
        'ゃ' => Some('a'),
        'ゅ' => Some('u'),
        'ょ' => Some('o'),
        _ => None,
    }
}

fn small_vowel(c: char) -> Option<char> {
    match c {
        'ぁ' => Some('a'),
        'ぃ' => Some('i'),
        'ぅ' => Some('u'),
        'ぇ' => Some('e'),
        'ぉ' => Some('o'),
        _ => None,
    }
}

fn kana(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' | 'ぁ' => "a",
        'い' | 'ぃ' => "i",
        'う' | 'ぅ' => "u",
        'え' | 'ぇ' => "e",
        'お' | 'ぉ' => "o",
        'か' => "ka",
        'き' => "ki",
        'く' => "ku",
        'け' => "ke",
        'こ' => "ko",
        'が' => "ga",
        'ぎ' => "gi",
        'ぐ' => "gu",
        'げ' => "ge",
        'ご' => "go",
        'さ' => "sa",
        'し' => "shi",
        'す' => "su",
        'せ' => "se",
        'そ' => "so",
        'ざ' => "za",
        'じ' | 'ぢ' => "ji",
        'ず' | 'づ' => "zu",
        'ぜ' => "ze",
        'ぞ' => "zo",
        'た' => "ta",
        'ち' => "chi",
        'つ' => "tsu",
        'て' => "te",
        'と' => "to",
        'だ' => "da",
        'で' => "de",
        'ど' => "do",
        'な' => "na",
        'に' => "ni",
        'ぬ' => "nu",
        'ね' => "ne",
        'の' => "no",
        'は' => "ha",
        'ひ' => "hi",
        'ふ' => "fu",
        'へ' => "he",
        'ほ' => "ho",
        'ば' => "ba",
        'び' => "bi",
        'ぶ' => "bu",
        'べ' => "be",
        'ぼ' => "bo",
        'ぱ' => "pa",
        'ぴ' => "pi",
        'ぷ' => "pu",
        'ぺ' => "pe",
        'ぽ' => "po",
        'ま' => "ma",
        'み' => "mi",
        'む' => "mu",
        'め' => "me",
        'も' => "mo",
        'や' => "ya",
        'ゆ' => "yu",
        'よ' => "yo",
        'ら' => "ra",
        'り' => "ri",
        'る' => "ru",
        'れ' => "re",
        'ろ' => "ro",
        'わ' => "wa",
        'を' => "o",
        'ゔ' => "vu",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ph(s: &str) -> Vec<String> {
        phonemes(s).unwrap()
    }

    #[test]
    fn kana_and_romaji_agree() {
        assert_eq!(ph("か"), ["k", "a"]);
        assert_eq!(ph("ka"), ["k", "a"]);
        assert_eq!(ph("シ"), ["sh", "i"]);
        assert_eq!(ph("きゃ"), ["ky", "a"]);
        assert_eq!(ph("kya"), ["ky", "a"]);
        assert_eq!(ph("しょ"), ["sh", "o"]);
        assert_eq!(ph("ちゅ"), ["ch", "u"]);
        assert_eq!(ph("ふぁ"), ["f", "a"]);
        assert_eq!(ph("ん"), ["N"]);
        assert_eq!(ph("n"), ["N"]);
        assert_eq!(ph("かっ"), ["k", "a", "cl"]);
        assert_eq!(ph("kka"), ["cl", "k", "a"]);
        assert_eq!(ph("らー"), ["r", "a", "a"]);
        assert_eq!(ph("tsu"), ["ts", "u"]);
        assert_eq!(ph("kan"), ["k", "a", "N"]);
        assert_eq!(ph("nya"), ["ny", "a"]);
    }

    #[test]
    fn invalid_lyrics_are_errors() {
        assert!(phonemes("x").is_err());
        assert!(phonemes("漢").is_err());
        assert!(phonemes("k").is_err());
    }

    #[test]
    fn phonemize_leaves_rests_and_continuations_to_the_host() {
        let note = |lyric: &str| Note {
            lyric: lyric.into(),
            lang: "ja".into(),
            start: 0.0,
            end: 0.5,
            pitch_cents: 6000.0,
        };
        let out = JaKana::phonemize(vec![note("さ"), note("+"), note("SP"), note("qq")]).unwrap();
        assert_eq!(out[0].phonemes, ["s", "a"]);
        assert!(out[1].phonemes.is_empty() && out[2].phonemes.is_empty());
        assert!(out[3].phonemes.is_empty() && !out[3].warnings.is_empty());
    }

    #[test]
    fn inventory_covers_every_emitted_symbol() {
        let inv: Vec<String> = JaKana::inventory("ja".into())
            .into_iter()
            .map(|p| p.symbol)
            .collect();
        for word in ["きゃ", "っ", "ん", "ヴ", "ふぉ", "ぢ", "でぃ", "うぇ"] {
            for p in ph(word) {
                assert!(inv.contains(&p), "{word}: {p}");
            }
        }
        assert!(JaKana::inventory("en".into()).is_empty());
    }
}
