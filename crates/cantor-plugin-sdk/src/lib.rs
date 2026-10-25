//! Guest-side SDK for Cantor plugins: WASM components the host loads in a
//! sandbox. Today's plugin kind is the phonemizer (lyrics → native
//! phonemes), defined by `wit/phonemizer.wit`.
//!
//! This crate must build on stable Rust: no `#![feature]`, ever.
//!
//! A phonemizer implements [`Phonemizer`] and exports it:
//!
//! ```ignore
//! use cantor_plugin_sdk::*;
//!
//! struct Mine;
//!
//! impl Phonemizer for Mine {
//!     fn info() -> Info { /* name, version, API_VERSION, languages */ }
//!     fn inventory(lang: String) -> Vec<PhoneInfo> { /* symbols + X-SAMPA */ }
//!     fn phonemize(notes: Vec<Note>) -> Result<Vec<NotePhonemes>, String> { /* … */ }
//! }
//!
//! cantor_plugin_sdk::export_phonemizer!(Mine);
//! ```
//!
//! Build it with `cargo build --release --target wasm32-wasip2`; the
//! `.wasm` in `target/wasm32-wasip2/release/` is the plugin. See
//! `templates/phonemizer` for a complete example.

#![deny(unsafe_code)]

/// Version of the plugin interface this SDK targets. The host refuses
/// plugins whose [`Info::api_version`] differs.
pub const API_VERSION: u32 = 1;

/// Pulses per quarter note, identical to the host's `cantor_core::PPQ`.
pub const PPQ: u32 = 1920;

/// Generated bindings for the `phonemizer` world.
#[allow(missing_docs, unsafe_code, clippy::all)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "phonemizer",
        pub_export_macro: true,
        export_macro_name: "__export_phonemizer",
        default_bindings_module: "cantor_plugin_sdk::bindings",
    });
}

pub use bindings::Guest as Phonemizer;
pub use bindings::cantor::plugin::phonemizer_types::{
    Info, Note, NotePhonemes, PhoneInfo, PhoneKind,
};

/// Exports a [`Phonemizer`] implementation as the plugin's entry points.
#[macro_export]
macro_rules! export_phonemizer {
    ($t:ident) => {
        $crate::bindings::__export_phonemizer!($t with_types_in $crate::bindings);
    };
}

/// How a lyric relates to its note, following the host's conventions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lyric<'a> {
    /// `+`: continues the previous note's vowel.
    Continue,
    /// `SP`, `R`, `-` or empty: silence.
    Rest,
    /// `AP` or `br`: a breath.
    Breath,
    /// A syllable, with surrounding `-` word markers removed.
    Syllable(&'a str),
}

/// Classifies a lyric the way the host does. Plugins can return an empty
/// phoneme list for anything but [`Lyric::Syllable`] and let the host
/// handle it.
pub fn parse_lyric(lyric: &str) -> Lyric<'_> {
    let t = lyric.trim();
    match t {
        "" | "-" | "SP" | "R" | "r" => Lyric::Rest,
        "AP" | "br" | "breath" => Lyric::Breath,
        _ if t.starts_with('+') => Lyric::Continue,
        _ => Lyric::Syllable(t.trim_matches('-')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppq_matches_core_contract() {
        assert_eq!(PPQ % 3, 0);
        assert_eq!(PPQ % 5, 0);
    }

    #[test]
    fn lyrics_follow_host_conventions() {
        assert_eq!(parse_lyric("+"), Lyric::Continue);
        assert_eq!(parse_lyric("+~"), Lyric::Continue);
        assert_eq!(parse_lyric(" SP "), Lyric::Rest);
        assert_eq!(parse_lyric("br"), Lyric::Breath);
        assert_eq!(parse_lyric("hel-"), Lyric::Syllable("hel"));
    }
}
