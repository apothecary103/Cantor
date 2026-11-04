//! Cantor file formats (docs/design.md, "Crates"):
//! - `.cantor` project files: pretty JSON of a format-3 [`Project`];
//! - import of MIDI, UST, USTX and MusicXML into a [`Project`];
//! - export to UST (classic UTAU, Shift-JIS) and USTX (OpenUtau), and WAV;
//! - lyrics text → syllables → [`cantor_core::Op::SetLyrics`].
//!
//! Importers never drop anything silently: whatever they cannot carry over
//! is reported as a [`Diagnostic`] in [`Imported::diagnostics`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod common;
pub mod lyrics;
pub mod midi;
pub mod musicxml;
mod pitch;
pub mod project_file;
pub mod ust;
pub mod ustx;
pub mod v1;
pub mod wav;

pub use midi::MidiOpts;
pub use project_file::LoadError;

use cantor_core::{Diagnostic, Project};
use std::path::Path;

/// A project read from another format, with what could not be carried over.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    /// The project: canonical and valid.
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
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The OS error.
        source: std::io::Error,
    },
    /// The importer produced an invalid project (an importer bug).
    #[error("import produced an invalid project: {0:?}")]
    Invalid(Vec<Diagnostic>),
}

/// A file written by an exporter, with what it could not express.
#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    /// The file contents.
    pub bytes: Vec<u8>,
    /// What was approximated or left out.
    pub diagnostics: Vec<Diagnostic>,
}

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The requested track does not exist (or the project has none).
    #[error("no such track: {0}")]
    NoTrack(String),
    /// Encoding the output failed.
    #[error("cannot write {format}: {message}")]
    Write {
        /// Format name.
        format: &'static str,
        /// What went wrong.
        message: String,
    },
}

/// Formats [`import_path`] and [`import_bytes`] read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Standard MIDI file (`.mid`, `.midi`, `.smf`).
    Midi,
    /// Classic UTAU sequence (`.ust`), Shift-JIS or UTF-8.
    Ust,
    /// OpenUtau project (`.ustx`).
    Ustx,
    /// Uncompressed MusicXML (`.musicxml`, `.xml`).
    MusicXml,
    /// Cantor project (`.cantor`).
    Cantor,
}

impl Kind {
    /// Guesses the kind from a file extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "mid" | "midi" | "smf" => Self::Midi,
            "ust" => Self::Ust,
            "ustx" => Self::Ustx,
            "musicxml" | "xml" | "mxl" => Self::MusicXml,
            "cantor" => Self::Cantor,
            _ => return None,
        })
    }
}

/// Options for [`import_bytes`]; only some formats use them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOpts {
    /// Which MIDI track/channel holds the melody (default: chosen automatically).
    pub midi: MidiOpts,
}

/// Imports bytes of a known kind.
pub fn import_bytes(bytes: &[u8], kind: Kind, opts: &ImportOpts) -> Result<Imported, ImportError> {
    match kind {
        Kind::Midi => midi::import(bytes, &opts.midi),
        Kind::Ust => ust::import(bytes),
        Kind::Ustx => ustx::import(bytes),
        Kind::MusicXml => musicxml::import(bytes),
        Kind::Cantor => project_file::from_bytes(bytes)
            .map(|l| Imported {
                project: l.project,
                diagnostics: l.diagnostics,
            })
            .map_err(|e| ImportError::Unsupported {
                format: "Cantor project",
                message: e.to_string(),
            }),
    }
}

/// Imports a file; `kind: None` picks the format from the extension.
pub fn import_path(path: &Path, kind: Option<Kind>) -> Result<Imported, ImportError> {
    import_path_with(path, kind, &ImportOpts::default())
}

/// [`import_path`] with options.
pub fn import_path_with(
    path: &Path,
    kind: Option<Kind>,
    opts: &ImportOpts,
) -> Result<Imported, ImportError> {
    let kind = kind
        .or_else(|| Kind::from_path(path))
        .ok_or_else(|| ImportError::Unsupported {
            format: "file",
            message: format!(
                "cannot tell the format of {} from its extension; expected .mid, .ust, .ustx, \
                 .musicxml or .cantor",
                path.display()
            ),
        })?;
    let bytes = std::fs::read(path).map_err(|source| ImportError::Io {
        path: path.display().to_string(),
        source,
    })?;
    import_bytes(&bytes, kind, opts)
}
