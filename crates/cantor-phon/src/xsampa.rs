//! Universal X-SAMPA input (docs/spec/40-algorithms.md §1).
//!
//! Each phonemizer declares a one-to-one mapping between X-SAMPA and its
//! native symbols for every language it sings, so users and agents can type
//! phonemes for any language in one notation. Built-in maps cover the
//! symbol sets DiffSinger voicebanks commonly use (Opencpop-style pinyin
//! finals for Chinese, romaji for Japanese, ARPAbet for English); a
//! voicebank may ship `xsampa-<lang>.txt` to declare its own.
//!
//! File format: one `native<TAB or spaces>xsampa` pair per line; `#` starts
//! a comment. Both columns must be unique.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// A one-to-one mapping between native symbols and X-SAMPA for one language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XSampaMap {
    to_native: HashMap<String, String>,
    to_xsampa: BTreeMap<String, String>,
}

impl XSampaMap {
    /// Parses a mapping file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut m = Self::default();
        for (i, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut cols = line.split_whitespace();
            let (Some(native), Some(xs), None) = (cols.next(), cols.next(), cols.next()) else {
                return Err(format!(
                    "line {}: expected `native xsampa`, got `{line}`",
                    i + 1
                ));
            };
            if m.to_xsampa.insert(native.into(), xs.into()).is_some() {
                return Err(format!("line {}: native symbol `{native}` repeats", i + 1));
            }
            if m.to_native.insert(xs.into(), native.into()).is_some() {
                return Err(format!("line {}: X-SAMPA `{xs}` repeats", i + 1));
            }
        }
        Ok(m)
    }

    /// The built-in map for a language (`zh`, `ja`, `en`; region subtags are
    /// ignored).
    pub fn builtin(lang: &str) -> Option<&'static XSampaMap> {
        static MAPS: OnceLock<BTreeMap<&'static str, XSampaMap>> = OnceLock::new();
        let maps = MAPS.get_or_init(|| {
            [("zh", ZH), ("ja", JA), ("en", EN)]
                .into_iter()
                .map(|(l, t)| (l, Self::parse(t).expect("built-in X-SAMPA maps are valid")))
                .collect()
        });
        maps.get(lang.split(['-', '_']).next().unwrap_or(lang))
    }

    /// The native symbol for an X-SAMPA symbol.
    pub fn to_native(&self, xsampa: &str) -> Option<&str> {
        self.to_native.get(xsampa).map(String::as_str)
    }

    /// The X-SAMPA symbol for a native symbol.
    pub fn to_xsampa(&self, native: &str) -> Option<&str> {
        self.to_xsampa.get(native).map(String::as_str)
    }

    /// Every `(native, xsampa)` pair, sorted by native symbol.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.to_xsampa.iter().map(|(n, x)| (n.as_str(), x.as_str()))
    }

    /// Number of pairs.
    pub fn len(&self) -> usize {
        self.to_xsampa.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.to_xsampa.is_empty()
    }
}

/// Mandarin, Opencpop-style initials and finals. Finals with a medial are
/// written as glide + vowel, so each native symbol is one X-SAMPA string.
const ZH: &str = "\
b p
p p_h
m m
f f
d t
t t_h
n n
l l
g k
k k_h
h x
j ts\\
q ts\\_h
x s\\
zh ts`
ch ts`_h
sh s`
r z`
z ts
c ts_h
s s
y j
w w
a a
o o
e 7
i i
u u
v y
i0 z=          # apical vowel after z c s
ir z`=         # apical vowel after zh ch sh r
er @`
ai aI
ei eI
ao AU
ou oU
an an
en @n
ang AN
eng @N
ong UN
ia ja
ie jE
iao jAU
iu joU
ian jEn
in in
iang jAN
ing iN
iong jUN
io jo
ua wa
uo wO
uai waI
ui weI
uan wan
un w@n
uang wAN
ueng w@N
um um
ve HE
van HEn
vn yn
";

/// Japanese, romaji-style phonemes.
const JA: &str = "\
a a
i i
u M
e e
o o
N N\\
cl ?
b b
by b_j
ch tS
d d
dy d_j
f p\\
g g
gw g_w
gy g_j
h h
hy C
j dZ
k k
kw k_w
ky k_j
m m
my m_j
n n
ny J
p p
py p_j
r 4
ry 4_j
s s
sh S
t t
ts ts
ty t_j
v v
w M\\
y j
z z
";

/// English, ARPAbet.
const EN: &str = "\
aa A
ae {
ah V
ao O
aw aU
ax @
ay aI
eh E
er 3`
ey eI
ih I
iy i
ow oU
oy OI
uh U
uw u
b b
ch tS
d d
dh D
dx 4
f f
g g
hh h
jh dZ
k k
l l
m m
n n
ng N
p p
r r\\
s s
sh S
t t
th T
v v
w w
y j
z z
zh Z
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_maps_are_one_to_one_and_round_trip() {
        for lang in ["zh", "ja", "en"] {
            let m = XSampaMap::builtin(lang).unwrap();
            assert!(m.len() > 35, "{lang}: {}", m.len());
            for (n, x) in m.pairs() {
                assert_eq!(m.to_native(x), Some(n), "{lang} {n} {x}");
            }
        }
        let zh = XSampaMap::builtin("zh-CN").unwrap();
        assert_eq!(zh.to_native("ts`_h"), Some("ch"));
        assert_eq!(zh.to_xsampa("v"), Some("y"));
        assert!(XSampaMap::builtin("ko").is_none());
    }

    #[test]
    fn parse_rejects_duplicates_and_bad_lines() {
        assert!(
            XSampaMap::parse("a a\nb a\n")
                .unwrap_err()
                .contains("repeats")
        );
        assert!(
            XSampaMap::parse("a a\na b\n")
                .unwrap_err()
                .contains("repeats")
        );
        assert!(XSampaMap::parse("a\n").unwrap_err().contains("expected"));
        let m = XSampaMap::parse("# comment\nsh S  # trailing\n\n").unwrap();
        assert_eq!(m.to_native("S"), Some("sh"));
    }
}
