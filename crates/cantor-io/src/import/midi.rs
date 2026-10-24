//! MIDI importer.

use super::{ImportError, Imported};

/// Imports a MIDI file.
pub fn import(_bytes: &[u8]) -> Result<Imported, ImportError> {
    Err(ImportError::Unsupported {
        format: "MIDI",
        message: "importer not implemented yet".into(),
    })
}
