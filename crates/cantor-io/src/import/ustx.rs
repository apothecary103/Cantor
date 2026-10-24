//! OpenUtau USTX importer.

use super::{ImportError, Imported};

/// Imports a OpenUtau USTX file.
pub fn import(_bytes: &[u8]) -> Result<Imported, ImportError> {
    Err(ImportError::Unsupported {
        format: "OpenUtau USTX",
        message: "importer not implemented yet".into(),
    })
}
