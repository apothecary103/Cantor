//! MusicXML importer.

use super::{ImportError, Imported};

/// Imports a MusicXML file.
pub fn import(_bytes: &[u8]) -> Result<Imported, ImportError> {
    Err(ImportError::Unsupported {
        format: "MusicXML",
        message: "importer not implemented yet".into(),
    })
}
