//! `.cantor` project files: versioned JSON, optionally zstd-compressed,
//! with automatic forward migrations.
//!
//! ```json
//! { "format": "cantor-project", "version": 1, "project": { ... } }
//! ```

use cantor_core::{Diagnostic, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::Path;

/// Value of the envelope's `format` field.
pub const FORMAT: &str = "cantor-project";

/// The file-format version this build writes.
pub const CURRENT_VERSION: u32 = 2;

/// Migrations from older file versions, in order. Each one upgrades the
/// envelope from version `from` to `from + 1`. A format change adds an
/// entry here and bumps [`CURRENT_VERSION`].
pub const MIGRATIONS: &[Migration] = &[Migration {
    from: 1,
    description: "add song sections",
    apply: v1_to_v2,
}];

/// Version 2 added `project.sections` (named song regions).
fn v1_to_v2(v: &mut Value) -> Result<(), String> {
    let project = v
        .get_mut("project")
        .and_then(Value::as_object_mut)
        .ok_or("missing project")?;
    project
        .entry("sections")
        .or_insert_with(|| Value::Array(Vec::new()));
    Ok(())
}

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The on-disk envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    /// Always `"cantor-project"`.
    pub format: String,
    /// File-format version.
    pub version: u32,
    /// The project.
    pub project: Project,
}

/// One forward migration step.
#[derive(Debug)]
pub struct Migration {
    /// The version this step upgrades from.
    pub from: u32,
    /// What changed, for the load report.
    pub description: &'static str,
    /// Rewrites the envelope in place. It need not touch `version`.
    pub apply: fn(&mut Value) -> Result<(), String>,
}

/// How to write a project file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    /// Pretty-printed JSON, diff-friendly.
    #[default]
    None,
    /// zstd-compressed JSON at the given level (1–22).
    Zstd(i32),
}

/// A loaded project with what happened while loading it.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    /// The project, in canonical form.
    pub project: Project,
    /// The file version before migration, if it was migrated.
    pub migrated_from: Option<u32>,
    /// Warnings and notes about the load.
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a project could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Reading the file failed.
    #[error("cannot read project file: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are not JSON (or not zstd-compressed JSON).
    #[error("project file is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The JSON is not a Cantor project envelope.
    #[error("not a Cantor project file: {0}")]
    NotAProject(String),
    /// The file is newer than this build understands.
    #[error(
        "project file version {found} is newer than this build supports ({newest}); upgrade Cantor"
    )]
    TooNew {
        /// Version in the file.
        found: u32,
        /// Newest version this build reads.
        newest: u32,
    },
    /// No migration path from the file's version.
    #[error("no migration from project file version {0}")]
    NoMigration(u32),
    /// A migration step failed.
    #[error("migrating project file from version {from} failed: {message}")]
    Migration {
        /// Version being migrated from.
        from: u32,
        /// What went wrong.
        message: String,
    },
    /// The project violates a document invariant.
    #[error("project is invalid: {}", .0.iter().map(|d| d.message.as_str()).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Diagnostic>),
}

/// Serializes a project to file bytes.
pub fn to_bytes(project: &Project, compression: Compression) -> Vec<u8> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        format: &'a str,
        version: u32,
        project: &'a Project,
    }
    let mut json = serde_json::to_vec_pretty(&Envelope {
        format: FORMAT,
        version: CURRENT_VERSION,
        project,
    })
    .expect("projects always serialize");
    json.push(b'\n');
    match compression {
        Compression::None => json,
        Compression::Zstd(level) => {
            zstd::encode_all(json.as_slice(), level).expect("in-memory zstd cannot fail")
        }
    }
}

/// Writes a project file atomically (temporary file, then rename).
pub fn save(path: &Path, project: &Project, compression: Compression) -> std::io::Result<()> {
    let bytes = to_bytes(project, compression);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = Path::new(&tmp);
    {
        let mut f = std::fs::File::create(tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

/// Reads and migrates a project file.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    from_bytes(&std::fs::read(path)?)
}

/// Parses and migrates project file bytes.
pub fn from_bytes(bytes: &[u8]) -> Result<Loaded, LoadError> {
    from_bytes_with(bytes, MIGRATIONS, CURRENT_VERSION)
}

/// Like [`from_bytes`], with an explicit migration chain and target version.
/// Exposed so the migration machinery can be tested with synthetic steps.
pub fn from_bytes_with(
    bytes: &[u8],
    migrations: &[Migration],
    current: u32,
) -> Result<Loaded, LoadError> {
    let json = if bytes.starts_with(&ZSTD_MAGIC) {
        zstd::decode_all(bytes)?
    } else {
        bytes.to_vec()
    };
    let mut value: Value = serde_json::from_slice(&json)?;
    let mut diagnostics = Vec::new();
    let migrated_from = migrate(&mut value, migrations, current, &mut diagnostics)?;
    let file: ProjectFile = serde_json::from_value(value)?;
    let mut project = file.project;

    let before = project.clone();
    project.canonicalize();
    if project != before {
        diagnostics.push(Diagnostic::info(
            "file.canonicalized",
            "notes, parts or curve points were stored out of order and have been sorted",
            None,
        ));
    }
    let (errors, rest): (Vec<_>, Vec<_>) = project
        .validate()
        .into_iter()
        .partition(Diagnostic::is_error);
    if !errors.is_empty() {
        return Err(LoadError::Invalid(errors));
    }
    diagnostics.extend(rest);
    Ok(Loaded {
        project,
        migrated_from,
        diagnostics,
    })
}

/// Upgrades an envelope to `current`. Returns the original version if any
/// step ran.
fn migrate(
    value: &mut Value,
    migrations: &[Migration],
    current: u32,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<u32>, LoadError> {
    let obj = value
        .as_object()
        .ok_or_else(|| LoadError::NotAProject("top level is not an object".into()))?;
    if obj.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(LoadError::NotAProject(format!(
            "missing \"format\": \"{FORMAT}\""
        )));
    }
    let original = obj
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| LoadError::NotAProject("missing or invalid \"version\"".into()))?;
    if original > current {
        return Err(LoadError::TooNew {
            found: original,
            newest: current,
        });
    }
    let mut version = original;
    while version < current {
        let step = migrations
            .iter()
            .find(|m| m.from == version)
            .ok_or(LoadError::NoMigration(version))?;
        (step.apply)(value).map_err(|message| LoadError::Migration {
            from: version,
            message,
        })?;
        version += 1;
        value["version"] = Value::from(version);
        diagnostics.push(Diagnostic::info(
            "file.migrated",
            format!(
                "migrated from version {} to {version}: {}",
                version - 1,
                step.description
            ),
            None,
        ));
    }
    Ok((original != current).then_some(original))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cantor_core::{Note, NoteId, Part, PartId, Track, TrackId};

    fn sample() -> Project {
        let mut p = Project::new();
        p.meta.title = "Sample".into();
        p.tracks.push(Track {
            id: TrackId(1),
            name: "Lead".into(),
            singer: Default::default(),
            backend: Default::default(),
            mix: Default::default(),
            effects: vec![],
            doubling: Default::default(),
            voice_mix: vec![],
            curves: Default::default(),
            parts: vec![Part {
                id: PartId(2),
                name: "Verse".into(),
                start: 0,
                length: 7680,
                notes: vec![Note {
                    id: NoteId(3),
                    start: 0,
                    length: 1920,
                    pitch_cents: 6000.0,
                    lyric: "la".into(),
                    lang: None,
                    phonemes: None,
                    expr: Default::default(),
                    modes: Default::default(),
                }],
            }],
        });
        p
    }

    #[test]
    fn json_round_trip_is_byte_identical() {
        let p = sample();
        let bytes = to_bytes(&p, Compression::None);
        let loaded = from_bytes(&bytes).unwrap();
        assert_eq!(loaded.project, p);
        assert_eq!(loaded.migrated_from, None);
        assert!(loaded.diagnostics.is_empty());
        assert_eq!(to_bytes(&loaded.project, Compression::None), bytes);
    }

    #[test]
    fn zstd_round_trip() {
        let p = sample();
        let bytes = to_bytes(&p, Compression::Zstd(3));
        assert!(bytes.starts_with(&ZSTD_MAGIC));
        assert_eq!(from_bytes(&bytes).unwrap().project, p);
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("cantor-io-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.cantor");
        save(&path, &sample(), Compression::None).unwrap();
        assert_eq!(load(&path).unwrap().project, sample());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A real version-1 file (before sections) loads and reports the
    /// migration.
    #[test]
    fn migrates_version_1_files() {
        let mut v: Value = serde_json::from_slice(&to_bytes(&sample(), Compression::None)).unwrap();
        v["version"] = Value::from(1);
        v["project"].as_object_mut().unwrap().remove("sections");
        let loaded = from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(loaded.migrated_from, Some(1));
        assert_eq!(loaded.project, sample());
        assert_eq!(loaded.diagnostics[0].code, "file.migrated");
    }

    #[test]
    fn rejects_newer_versions() {
        let mut v: Value = serde_json::from_slice(&to_bytes(&sample(), Compression::None)).unwrap();
        v["version"] = Value::from(CURRENT_VERSION + 1);
        let err = from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert!(matches!(err, LoadError::TooNew { .. }), "{err}");
    }

    #[test]
    fn rejects_other_json() {
        let err = from_bytes(br#"{"hello": 1}"#).unwrap_err();
        assert!(matches!(err, LoadError::NotAProject(_)), "{err}");
    }

    #[test]
    fn rejects_invalid_projects() {
        let mut p = sample();
        p.tempo_map[0].bpm = -5.0;
        let err = from_bytes(&to_bytes(&p, Compression::None)).unwrap_err();
        assert!(matches!(err, LoadError::Invalid(_)), "{err}");
    }

    #[test]
    fn sorts_out_of_order_notes_and_says_so() {
        let mut p = sample();
        let notes = &mut p.tracks[0].parts[0].notes;
        let mut second = notes[0].clone();
        second.id = NoteId(4);
        second.start = 1920;
        notes.insert(0, second);
        let loaded = from_bytes(&to_bytes(&p, Compression::None)).unwrap();
        let ids: Vec<_> = loaded.project.tracks[0].parts[0]
            .notes
            .iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ids, [NoteId(3), NoteId(4)]);
        assert_eq!(loaded.diagnostics[0].code, "file.canonicalized");
    }

    /// A synthetic history: version 1 called the title `name`, version 2
    /// stored BPM as `tempo`. Migrating a v1 file must reach v3.
    #[test]
    fn migrations_chain_forward() {
        fn v1_to_v2(v: &mut Value) -> Result<(), String> {
            let meta = v["project"]["meta"].as_object_mut().ok_or("no meta")?;
            let name = meta.remove("name").ok_or("no name")?;
            meta.insert("title".into(), name);
            Ok(())
        }
        fn v2_to_v3(v: &mut Value) -> Result<(), String> {
            for t in v["project"]["tempo_map"].as_array_mut().ok_or("no tempo")? {
                let obj = t.as_object_mut().ok_or("bad tempo")?;
                let bpm = obj.remove("tempo").ok_or("no tempo field")?;
                obj.insert("bpm".into(), bpm);
            }
            Ok(())
        }
        let chain = [
            Migration {
                from: 1,
                description: "rename meta.name to meta.title",
                apply: v1_to_v2,
            },
            Migration {
                from: 2,
                description: "rename tempo to bpm",
                apply: v2_to_v3,
            },
        ];
        let v1 = br#"{
            "format": "cantor-project",
            "version": 1,
            "project": {
                "meta": { "name": "Old song" },
                "tempo_map": [{ "tick": 0, "tempo": 90.0 }],
                "time_sig_map": [{ "bar": 0, "numerator": 3, "denominator": 4 }]
            }
        }"#;
        let loaded = from_bytes_with(v1, &chain, 3).unwrap();
        assert_eq!(loaded.migrated_from, Some(1));
        assert_eq!(loaded.project.meta.title, "Old song");
        assert_eq!(loaded.project.tempo_map[0].bpm, 90.0);
        assert_eq!(loaded.diagnostics.len(), 2);

        let err = from_bytes_with(v1, &chain[1..], 3).unwrap_err();
        assert!(matches!(err, LoadError::NoMigration(1)), "{err}");
    }
}
