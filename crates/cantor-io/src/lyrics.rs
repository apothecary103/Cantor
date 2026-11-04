//! Lyrics text → one syllable per note, and syllables → [`Op::SetLyrics`].
//!
//! - Japanese kana: one mora per note. Small kana (ゃゅょぁぃぅぇぉゎ and
//!   katakana forms) join the previous mora; っ/ッ joins it too unless
//!   [`LyricsOpts::sokuon_note`]; ー/〜 become `+` (hold the vowel).
//!   Other CJK characters (kanji) are one item each: convert them to kana
//!   first.
//! - Romaji: split per syllable (`sakura` → `sa ku ra`, `konnichiwa` →
//!   `ko n ni chi wa`, a doubled consonant closes the previous syllable:
//!   `kitte` → `kit te`).
//! - English: one word per note; hyphens split (`beau-ti-ful`).
//! - `+` is a melisma (the note continues the previous syllable); `-` and
//!   `br` pass through. Line breaks, blank lines and punctuation are
//!   ignored.

use cantor_core::{Diagnostic, Lyrics, Op, Tick, Track};

/// How to read lyrics text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    /// Kana by script; Latin text is romaji if every word splits into
    /// romaji syllables, else English.
    #[default]
    Auto,
    /// Japanese kana (Latin words are read as romaji).
    Japanese,
    /// Romaji.
    Romaji,
    /// English (or any language written in words).
    English,
}

/// Options for [`parse`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LyricsOpts {
    /// The language.
    pub lang: Lang,
    /// Give っ/ッ its own note instead of joining the previous mora.
    pub sokuon_note: bool,
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}')
}

fn is_cjk(c: char) -> bool {
    is_kana(c)
        || matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '々' | '〜' | '～')
}

fn is_small(c: char) -> bool {
    "ゃゅょぁぃぅぇぉゎャュョァィゥェォヮㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ".contains(c)
}

/// Splits lyrics text into syllables, one per note.
pub fn parse(text: &str, opts: LyricsOpts) -> Vec<String> {
    let words: Vec<String> = text
        .split(|c: char| {
            c.is_whitespace() || "、。，．,.!?！？「」『』()（）[]［］\"“”…・;:".contains(c)
        })
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    let latin: Vec<&String> = words.iter().filter(|w| !w.chars().any(is_cjk)).collect();
    let romaji = match opts.lang {
        Lang::Romaji | Lang::Japanese => true,
        Lang::English => false,
        Lang::Auto => {
            let any_kana = words.iter().any(|w| w.chars().any(is_cjk));
            any_kana
                || (!latin.is_empty()
                    && latin.iter().all(|w| {
                        matches!(w.as_str(), "+" | "-" | "br")
                            || w.split('-')
                                .filter(|p| !p.is_empty())
                                .all(|p| romaji_split(&p.to_lowercase()).is_some())
                    }))
        }
    };
    let mut out = Vec::new();
    for w in &words {
        if matches!(w.as_str(), "+" | "-" | "br") {
            out.push(w.clone());
        } else if w.chars().any(is_cjk) {
            kana(w, opts.sokuon_note, &mut out);
        } else if romaji {
            for part in w.split('-') {
                if part.is_empty() {
                    out.push("+".into()); // "a-" or "--": hold
                    continue;
                }
                let lower = part.to_lowercase();
                match romaji_split(&lower) {
                    Some(s) => out.extend(s),
                    None => out.push(lower),
                }
            }
            if w.ends_with('-') {
                out.pop();
                out.push("+".into());
            }
        } else {
            out.extend(
                w.split('-')
                    .map(|p| {
                        p.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '+')
                    })
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    out
}

fn kana(w: &str, sokuon_note: bool, out: &mut Vec<String>) {
    let start = out.len();
    for c in w.chars() {
        let joins = is_small(c) || (!sokuon_note && matches!(c, 'っ' | 'ッ'));
        match c {
            'ー' | '〜' | '～' | '-' | '+' => out.push("+".into()),
            _ if joins && out.len() > start && out.last().is_some_and(|l| l != "+") => {
                out.last_mut().expect("checked").push(c);
            }
            _ if is_cjk(c) || c.is_alphanumeric() => out.push(c.into()),
            _ => {}
        }
    }
}

/// Splits one romaji word into syllables, or `None` if it is not romaji.
fn romaji_split(w: &str) -> Option<Vec<String>> {
    const ONSETS: &[&str] = &[
        "kw", "gw", "ky", "gy", "sy", "zy", "ty", "dy", "ny", "hy", "by", "py", "my", "ry", "jy",
        "sh", "ch", "ts", "th", "dh", "wh", "fy", "vy", "k", "g", "s", "z", "t", "d", "n", "h",
        "b", "p", "m", "y", "r", "w", "f", "j", "v", "",
    ];
    let w = w.trim_matches('\'');
    if w.is_empty() || !w.chars().all(|c| c.is_ascii_lowercase() || c == '\'') {
        return None;
    }
    let b = w.as_bytes();
    let vowel = |c: u8| b"aeiou".contains(&c);
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\'' {
            i += 1;
            continue;
        }
        // Moraic n: before a consonant (not y), an apostrophe or the end.
        if b[i] == b'n'
            && (i + 1 == b.len() || b[i + 1] == b'\'' || (!vowel(b[i + 1]) && b[i + 1] != b'y'))
        {
            out.push("n".into());
            i += 1;
            continue;
        }
        // Gemination: a doubled consonant closes the previous syllable.
        if i + 1 < b.len() && b[i] == b[i + 1] && !vowel(b[i]) && b[i] != b'n' {
            out.last_mut()?.push(b[i] as char);
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"tch") {
            out.last_mut()?.push('t');
            i += 1;
            continue;
        }
        let onset = ONSETS.iter().find(|o| {
            b[i..].starts_with(o.as_bytes()) && b.get(i + o.len()).is_some_and(|&c| vowel(c))
        })?;
        let end = i + onset.len() + 1;
        out.push(w[i..end].to_owned());
        i = end;
    }
    Some(out)
}

/// Builds the [`Op::SetLyrics`] that puts `syllables` on consecutive notes
/// of `track` (starting in `range`, if given), skipping notes that extend
/// the previous one (`+...`) or are rests/breaths. A diagnostic reports a
/// count mismatch; the shorter of the two sets the number assigned.
pub fn assign(
    track: &Track,
    syllables: &[String],
    range: Option<(Tick, Tick)>,
) -> (Op, Vec<Diagnostic>) {
    let notes: Vec<_> = track
        .notes
        .iter()
        .filter(|n| range.is_none_or(|(a, b)| n.start >= a && n.start < b))
        .filter(|n| !n.is_extension() && !n.is_rest_or_breath())
        .collect();
    let k = notes.len().min(syllables.len());
    let mut diags = Vec::new();
    if notes.len() != syllables.len() {
        let detail = if syllables.len() > notes.len() {
            format!(
                "the last {} syllables were not used (first unused: {:?})",
                syllables.len() - k,
                syllables[k]
            )
        } else {
            format!(
                "the last {} notes kept their lyrics (first: {} at tick {})",
                notes.len() - k,
                notes[k].id,
                notes[k].start
            )
        };
        diags.push(
            Diagnostic::warning(
                "lyrics.count_mismatch",
                format!(
                    "{} syllables for {} singable notes; {detail}",
                    syllables.len(),
                    notes.len()
                ),
            )
            .on_track(track.id),
        );
    }
    let op = Op::SetLyrics {
        track: track.id,
        ids: notes[..k].iter().map(|n| n.id).collect(),
        lyrics: Lyrics::List(syllables[..k].to_vec()),
    };
    (op, diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Vec<String> {
        parse(text, LyricsOpts::default())
    }

    #[test]
    fn romaji_syllables() {
        assert_eq!(romaji_split("sakura").unwrap(), ["sa", "ku", "ra"]);
        assert_eq!(
            romaji_split("konnichiwa").unwrap(),
            ["ko", "n", "ni", "chi", "wa"]
        );
        assert_eq!(romaji_split("kitte").unwrap(), ["kit", "te"]);
        assert_eq!(romaji_split("kyou").unwrap(), ["kyo", "u"]);
        assert!(romaji_split("hello").is_none());
    }

    #[test]
    fn small_kana() {
        assert_eq!(p("きゃ"), ["きゃ"]);
        assert_eq!(
            parse(
                "ずっと",
                LyricsOpts {
                    sokuon_note: true,
                    ..Default::default()
                }
            ),
            ["ず", "っ", "と"]
        );
    }
}
