//! `.cantor` project files: the [`Project`] as pretty-printed JSON
//! (`{"format": 3, ...}`). Cantor v1 projects are converted (see [`crate::v1`]);
//! other formats are rejected with a message that says what the file is.

use cantor_core::{Diagnostic, FORMAT, Project};
use serde_json::Value;
use std::path::Path;

/// A loaded project with warnings about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// The project, in canonical form.
    pub project: Project,
    /// Non-fatal findings (warnings from validation).
    pub diagnostics: Vec<Diagnostic>,
    /// Converted from a Cantor v1 project (save it elsewhere, not over the original).
    pub from_v1: bool,
}

/// Why a project file could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Reading the file failed.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The OS error.
        source: std::io::Error,
    },
    /// Not JSON, or not a project.
    #[error("not a Cantor project: {0}")]
    Parse(String),
    /// A project of another format version.
    #[error("{0}")]
    Format(String),
    /// The project breaks the model's rules.
    #[error("invalid project: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Diagnostic>),
}

/// Parses a project file.
pub fn from_bytes(bytes: &[u8]) -> Result<Loaded, LoadError> {
    if bytes.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return Err(LoadError::Format(
            "this is a zstd-compressed project from Cantor v1; this version reads only \
             uncompressed format-3 projects"
                .into(),
        ));
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| LoadError::Parse(e.to_string()))?;
    match value.get("format") {
        Some(Value::Number(n)) if n.as_u64() == Some(u64::from(FORMAT)) => {}
        Some(Value::String(s)) if s == "cantor-project" => {
            let (mut project, mut diagnostics) =
                crate::v1::convert(&value).map_err(LoadError::Format)?;
            project.canonicalize();
            let (errors, warnings): (Vec<_>, Vec<_>) = project
                .validate()
                .into_iter()
                .partition(Diagnostic::is_error);
            if !errors.is_empty() {
                return Err(LoadError::Invalid(errors));
            }
            diagnostics.extend(warnings);
            return Ok(Loaded {
                project,
                diagnostics,
                from_v1: true,
            });
        }
        Some(other) => {
            return Err(LoadError::Format(format!(
                "project format {other} is not supported; this version reads format {FORMAT}"
            )));
        }
        None => {
            return Err(LoadError::Parse(format!(
                "no `format` field; expected a format-{FORMAT} project object"
            )));
        }
    }
    let mut project: Project =
        serde_json::from_value(value).map_err(|e| LoadError::Parse(e.to_string()))?;
    project.canonicalize();
    let (errors, diagnostics): (Vec<_>, Vec<_>) = project
        .validate()
        .into_iter()
        .partition(Diagnostic::is_error);
    if !errors.is_empty() {
        return Err(LoadError::Invalid(errors));
    }
    Ok(Loaded {
        project,
        diagnostics,
        from_v1: false,
    })
}

/// Serializes a project as pretty JSON with a trailing newline.
pub fn to_bytes(project: &Project) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(project).expect("a project always serializes");
    out.push(b'\n');
    out
}

/// Loads a project file.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let bytes = std::fs::read(path).map_err(|source| LoadError::Io {
        path: path.display().to_string(),
        source,
    })?;
    from_bytes(&bytes)
}

/// Saves a project file atomically (write to a temporary file, then rename).
pub fn save(path: &Path, project: &Project) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, to_bytes(project))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_core::{Note, NoteId, Track, TrackId};

    #[test]
    fn round_trips_and_rejects_other_formats() {
        let mut p = Project::default();
        let mut t = Track {
            id: TrackId(1),
            name: "Lead".into(),
            ..Track::default()
        };
        let mut n = Note::new(0, 480, 60, "あ");
        n.id = NoteId(1);
        t.notes.push(n);
        p.tracks.push(t);
        let loaded = from_bytes(&to_bytes(&p)).unwrap();
        assert_eq!(loaded.project, p);
        assert!(loaded.diagnostics.is_empty());

        let v1 = br#"{"format":"cantor-project","version":2,"project":{}}"#;
        assert!(matches!(from_bytes(v1), Err(LoadError::Format(m)) if m.contains("v1")));
        assert!(matches!(
            from_bytes(br#"{"format":2}"#),
            Err(LoadError::Format(_))
        ));
        assert!(matches!(
            from_bytes(b"\x28\xB5\x2F\xFD.."),
            Err(LoadError::Format(_))
        ));
        assert!(matches!(from_bytes(b"{}"), Err(LoadError::Parse(_))));
        assert!(matches!(from_bytes(b"nope"), Err(LoadError::Parse(_))));
    }
}
