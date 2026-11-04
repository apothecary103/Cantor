//! JSON Schemas for the public types, generated from the Rust definitions.
//! `cargo xtask schema` writes them to `schema/`.

use crate::apply::ApplyResult;
use crate::diag::Diagnostic;
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
    let e = |name, schema| SchemaEntry { name, schema };
    vec![
        e("project", schema_for::<Project>()),
        e("op", schema_for::<Op>()),
        e("apply-result", schema_for::<ApplyResult>()),
        e("diagnostic", schema_for::<Diagnostic>()),
        e("param-descriptor", schema_for::<ParamDescriptor>()),
        e("effect-descriptor", schema_for::<EffectDescriptor>()),
    ]
}
