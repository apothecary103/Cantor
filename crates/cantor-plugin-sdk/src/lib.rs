//! Guest-side SDK for Cantor plugins (phonemizers and script plugins
//! compiled to WASM components).
//!
//! This crate must build on stable Rust: no `#![feature]`, ever. The WIT
//! bindings arrive with the phonemizer host in M5; until then it only fixes
//! the shared vocabulary plugins and host agree on.

#![forbid(unsafe_code)]

/// Version of the plugin interface this SDK targets.
pub const PLUGIN_API_VERSION: u32 = 0;

/// Pulses per quarter note, identical to the host's `cantor_core::PPQ`.
pub const PPQ: u32 = 1920;

/// Which symbol set a phoneme uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PhonemeAlphabet {
    /// The phonemizer's own symbols.
    #[default]
    Native,
    /// Universal X-SAMPA.
    XSampa,
}

/// A phoneme produced by a phonemizer.
#[derive(Clone, Debug, PartialEq)]
pub struct Phoneme {
    /// Symbol in `alphabet`.
    pub symbol: String,
    /// Symbol set.
    pub alphabet: PhonemeAlphabet,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppq_matches_core_contract() {
        assert_eq!(PPQ % 3, 0);
        assert_eq!(PPQ % 5, 0);
        assert_eq!(PhonemeAlphabet::default(), PhonemeAlphabet::Native);
    }
}
