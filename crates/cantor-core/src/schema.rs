//! The schema registry: JSON Schemas for the public types, generated from
//! the Rust definitions. `cargo xtask schema` writes them to `schema/`.

use crate::diag::Diagnostic;
use crate::document::ApplyResult;
use crate::effects::EffectDescriptor;
use crate::model::Project;
use crate::op::Op;
use crate::params::ParamDescriptor;
use schemars::{JsonSchema, Schema, generate::SchemaSettings};

/// A named schema, written to `schema/<name>.schema.json`.
#[derive(Debug)]
pub struct SchemaEntry {
    /// File stem.
    pub name: &'static str,
    /// The schema.
    pub schema: Schema,
}

/// Generates the schema for `T` with Cantor's settings (draft 2020-12).
pub fn schema_for<T: JsonSchema>() -> Schema {
    SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>()
}

/// Schemas for cantor-core's public types.
pub fn registry() -> Vec<SchemaEntry> {
    vec![
        SchemaEntry {
            name: "project",
            schema: schema_for::<Project>(),
        },
        SchemaEntry {
            name: "op",
            schema: schema_for::<Op>(),
        },
        SchemaEntry {
            name: "apply-result",
            schema: schema_for::<ApplyResult>(),
        },
        SchemaEntry {
            name: "diagnostic",
            schema: schema_for::<Diagnostic>(),
        },
        SchemaEntry {
            name: "param-descriptor",
            schema: schema_for::<ParamDescriptor>(),
        },
        SchemaEntry {
            name: "effect-descriptor",
            schema: schema_for::<EffectDescriptor>(),
        },
    ]
}
