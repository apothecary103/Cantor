//! Importers for other formats. Each returns a canonical, validated
//! [`Project`] plus diagnostics for everything it could not carry over:
//! an importer never drops data silently.

pub mod midi;
pub mod musicxml;
pub mod ustx;

use cantor_core::{Diagnostic, NoteId, PPQ, PartId, Project, Tick, TrackId};
use std::path::Path;

/// The result of an import.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    /// The imported project, canonical and valid.
    pub project: Project,
    /// What was approximated, ignored or unsupported.
    pub diagnostics: Vec<Diagnostic>,
}

/// Why an import failed.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The input is malformed.
    #[error("cannot parse {format}: {message}")]
    Parse {
        /// Format name.
        format: &'static str,
        /// What went wrong.
        message: String,
    },
    /// The input is well-formed but uses something Cantor cannot import.
    #[error("unsupported {format} input: {message}")]
    Unsupported {
        /// Format name.
        format: &'static str,
        /// What is unsupported.
        message: String,
    },
    /// The importer produced an invalid project (an importer bug).
    #[error("import produced an invalid project: {0:?}")]
    Invalid(Vec<Diagnostic>),
}

/// Formats Cantor can import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// OpenUtau project (`.ustx`).
    Ustx,
    /// Standard MIDI file (`.mid`, `.midi`).
    Midi,
    /// Uncompressed MusicXML (`.musicxml`, `.xml`).
    MusicXml,
}

impl Format {
    /// Guesses the format from a file extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "ustx" => Some(Self::Ustx),
            "mid" | "midi" | "smf" => Some(Self::Midi),
            "musicxml" | "xml" => Some(Self::MusicXml),
            _ => None,
        }
    }
}

/// Imports bytes in the given format.
pub fn import(bytes: &[u8], format: Format) -> Result<Imported, ImportError> {
    match format {
        Format::Ustx => ustx::import(bytes),
        Format::Midi => midi::import(bytes),
        Format::MusicXml => musicxml::import(bytes),
    }
}

/// Imports a file, choosing the format from its extension.
pub fn import_path(path: &Path) -> Result<Imported, ImportError> {
    let format = Format::from_path(path).ok_or_else(|| ImportError::Unsupported {
        format: "file",
        message: format!("unknown extension for {}", path.display()),
    })?;
    let bytes = std::fs::read(path).map_err(|e| ImportError::Parse {
        format: "file",
        message: format!("{}: {e}", path.display()),
    })?;
    import(&bytes, format)
}

/// Hands out ids from one counter, so every id in an imported project is
/// unique and ids read in file order.
#[derive(Debug)]
pub(crate) struct Ids {
    next: u64,
}

impl Ids {
    pub(crate) fn new() -> Self {
        Self { next: 1 }
    }

    fn bump(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    pub(crate) fn track(&mut self) -> TrackId {
        TrackId(self.bump())
    }

    pub(crate) fn part(&mut self) -> PartId {
        PartId(self.bump())
    }

    pub(crate) fn note(&mut self) -> NoteId {
        NoteId(self.bump())
    }
}

/// Converts a tick count at resolution `from_ppq` to Cantor ticks.
/// Returns the nearest tick and whether the conversion was exact.
pub(crate) fn rescale(ticks: i64, from_ppq: u32) -> (Tick, bool) {
    let num = i128::from(ticks) * i128::from(PPQ);
    let den = i128::from(from_ppq);
    let q = num.div_euclid(den);
    let r = num.rem_euclid(den);
    let rounded = if 2 * r >= den { q + 1 } else { q };
    (rounded as Tick, r == 0)
}

/// Canonicalizes and validates an imported project.
pub(crate) fn finish(
    mut project: Project,
    diagnostics: Vec<Diagnostic>,
) -> Result<Imported, ImportError> {
    project.canonicalize();
    let errors: Vec<_> = project
        .validate()
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect();
    if !errors.is_empty() {
        return Err(ImportError::Invalid(errors));
    }
    Ok(Imported {
        project,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescale_exact_and_rounded() {
        assert_eq!(rescale(480, 480), (1920, true));
        assert_eq!(rescale(96, 96), (1920, true));
        assert_eq!(rescale(1, 1000), (2, false)); // 1.92 -> 2
        assert_eq!(rescale(-480, 480), (-1920, true));
    }
}
