//! Schemas for cantor-io's public types; see `cantor_core::schema`.

use crate::project_file::ProjectFile;
use cantor_core::schema::{SchemaEntry, schema_for};

/// Schemas for the file formats cantor-io writes.
pub fn registry() -> Vec<SchemaEntry> {
    vec![SchemaEntry {
        name: "project-file",
        schema: schema_for::<ProjectFile>(),
    }]
}
