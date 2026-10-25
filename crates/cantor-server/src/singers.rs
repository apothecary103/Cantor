//! The singer registry: voicebanks found on the search path.

use cantor_core::params::{self, ParamDescriptor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A singer available to render with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SingerInfo {
    /// Identifier: the voicebank directory name. Tracks name it in `singer.id`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Synthesis backend.
    pub backend: String,
    /// Languages (BCP 47 tags) the voice sings.
    pub languages: Vec<String>,
    /// Language used when a note has none.
    pub default_language: String,
    /// The vocoder the voicebank expects.
    pub vocoder: String,
    /// Licence terms.
    pub license: License,
}

/// Licence information declared by a voicebank.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct License {
    /// Author, from the voicebank's character file.
    pub author: Option<String>,
    /// Web page, from the voicebank's character file.
    pub web: Option<String>,
    /// Terms-of-use files shipped with the voicebank (paths).
    pub terms_files: Vec<String>,
    /// Voice cloning or conversion attestation present. Cloned voices
    /// without one are refused (docs/spec/00-vision.md).
    pub consent_attestation: bool,
}

/// What a singer supports, for clients that edit parameters.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SingerSchema {
    /// The singer.
    pub singer: SingerInfo,
    /// Every registered parameter, with whether this singer honors it.
    pub parameters: Vec<ParamSupport>,
    /// Vocal modes per dimension (pitch, timbre, pronunciation).
    pub modes: Vec<String>,
    /// Retake dimensions the backend can vary.
    pub retake_dims: Vec<String>,
    /// X-SAMPA spelling of each native phoneme, by language. Phoneme
    /// overrides with `alphabet: "x_sampa"` use the right-hand symbols.
    pub xsampa: BTreeMap<String, BTreeMap<String, String>>,
}

/// A parameter and whether a singer supports it.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ParamSupport {
    /// The registry entry.
    #[serde(flatten)]
    pub descriptor: ParamDescriptor,
    /// Whether edits to this parameter are rendered.
    pub supported: bool,
}

/// Finds voicebanks (directories with `dsconfig.yaml`) in `paths`.
pub fn scan(paths: &[PathBuf]) -> Vec<(SingerInfo, PathBuf)> {
    let mut out = Vec::new();
    for dir in paths {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if let Some(info) = describe(&p) {
                out.push((info, p));
            }
        }
    }
    out
}

/// Reads a voicebank's metadata without loading its models: DiffSinger
/// (`dsconfig.yaml`) or UTAU (`oto.ini`, at the top or one level down).
pub fn describe(dir: &Path) -> Option<SingerInfo> {
    let id = dir.file_name()?.to_string_lossy().into_owned();
    let character = read_text_any(&dir.join("character.txt"));
    let field = |key: &str| {
        character.lines().find_map(|l| {
            l.trim_start_matches('\u{feff}')
                .strip_prefix(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
    };
    let yaml = std::fs::read_to_string(dir.join("character.yaml")).unwrap_or_default();
    let license = License {
        author: field("author="),
        web: field("web="),
        terms_files: terms_files(dir),
        consent_attestation: yaml.contains("consent_attestation"),
    };
    let name = field("name=").unwrap_or_else(|| id.clone());
    if let Ok(dsconfig) = std::fs::read_to_string(dir.join("dsconfig.yaml")) {
        let value = |key: &str| {
            dsconfig.lines().find_map(|l| {
                l.strip_prefix(key)
                    .map(|v| v.trim().trim_matches('"').to_owned())
            })
        };
        let mut languages = Vec::new();
        if let Some(file) = value("languages:")
            && let Ok(text) = std::fs::read_to_string(dir.join(file))
            && let Ok(map) = serde_json::from_str::<BTreeMap<String, i64>>(&text)
        {
            languages = map.into_keys().collect();
        }
        let default_language = if yaml.contains("Japanese") {
            "ja"
        } else if yaml.contains("English") {
            "en"
        } else {
            "zh"
        }
        .to_owned();
        return Some(SingerInfo {
            name,
            id,
            backend: "diffsinger".into(),
            languages,
            default_language,
            vocoder: value("vocoder:").unwrap_or_default(),
            license,
        });
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("svsio.yaml")) {
        let languages: Vec<String> = serde_saphyr::from_str::<SvsIoSinger>(&text)
            .map(|s| s.languages)
            .unwrap_or_default();
        return Some(SingerInfo {
            name,
            id,
            backend: "svsio".into(),
            default_language: languages.first().cloned().unwrap_or_default(),
            languages,
            vocoder: "remote".into(),
            license,
        });
    }
    if cantor_backends::utau::is_voicebank(dir) {
        // UTAU banks do not declare a language; nearly all are Japanese.
        let lang = if yaml.contains("English") { "en" } else { "ja" };
        return Some(SingerInfo {
            name,
            id,
            backend: "utau".into(),
            languages: vec![lang.into()],
            default_language: lang.into(),
            vocoder: "world".into(),
            license,
        });
    }
    None
}

/// The part of `svsio.yaml` the registry reads.
#[derive(Deserialize)]
struct SvsIoSinger {
    #[serde(default)]
    languages: Vec<String>,
}

/// Text in UTF-8 or, failing that, Shift-JIS (UTAU's historical encoding).
/// Empty when the file is missing.
fn read_text_any(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => encoding_rs::SHIFT_JIS.decode(e.as_bytes()).0.into_owned(),
    }
}

/// Terms-of-use, licence and readme files in a voicebank.
fn terms_files(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            n.contains("terms")
                || n.contains("license")
                || n.contains("readme")
                || n.contains("规约")
                || n.contains("規約")
        })
        .map(|p| p.display().to_string())
        .collect();
    out.sort();
    out
}

/// The schema of a singer given the parameters its backend honors.
pub fn schema(
    info: SingerInfo,
    supported: &[&str],
    xsampa: BTreeMap<String, BTreeMap<String, String>>,
) -> SingerSchema {
    SingerSchema {
        xsampa,
        parameters: params::registry()
            .iter()
            .map(|d| ParamSupport {
                descriptor: d.clone(),
                supported: supported.contains(&d.id),
            })
            .collect(),
        modes: Vec::new(),
        retake_dims: vec!["pitch".into(), "timbre".into(), "expression".into()],
        singer: info,
    }
}
