//! `cantor`: render, import and edit projects from the command line.

use cantor_backends::DiffSinger;
use cantor_core::{Diagnostic, Document, Op, Severity};
use cantor_io::project_file::{self, Compression};
use cantor_render::cache::DiskCache;
use cantor_render::render::{Quality, Renderer};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "cantor", version, about = "Cantor singing voice synthesis")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a project to a WAV file.
    Render {
        /// The .cantor project.
        project: PathBuf,
        /// Output WAV file.
        #[arg(short, long)]
        output: PathBuf,
        /// Render quality.
        #[arg(long, value_enum, default_value_t = QualityArg::Final)]
        quality: QualityArg,
        /// Voicebank directory (default: the track's singer under $CANTOR_SINGERS).
        #[arg(long)]
        voicebank: Option<PathBuf>,
        /// Vocoder directory (default: the voicebank's vocoder under $CANTOR_VOCODERS).
        #[arg(long)]
        vocoder: Option<PathBuf>,
        /// Render cache directory (default: $CANTOR_CACHE or the user cache dir).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Print the render report (hash, trace, diagnostics) as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Import USTX, MIDI or MusicXML into a .cantor project.
    Import {
        /// The file to import.
        input: PathBuf,
        /// Output .cantor file.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Edit projects with ops.
    Ops {
        #[command(subcommand)]
        command: OpsCommand,
    },
}

#[derive(Subcommand)]
enum OpsCommand {
    /// Apply a JSON op (or array of ops, applied as one batch) to a project.
    Apply {
        /// The .cantor project, rewritten in place.
        project: PathBuf,
        /// JSON file with an op or an array of ops.
        ops: PathBuf,
        /// Report what would change without saving.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum QualityArg {
    Preview,
    Final,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("CANTOR_LOG"))
        .with_writer(std::io::stderr)
        .init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cantor: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Render {
            project,
            output,
            quality,
            voicebank,
            vocoder,
            cache,
            json,
        } => render(&project, &output, quality, voicebank, vocoder, cache, json),
        Command::Import { input, output } => {
            let imported = cantor_io::import::import_path(&input).map_err(|e| e.to_string())?;
            print_diagnostics(&imported.diagnostics);
            project_file::save(&output, &imported.project, Compression::None)
                .map_err(|e| format!("{}: {e}", output.display()))
        }
        Command::Ops {
            command:
                OpsCommand::Apply {
                    project,
                    ops,
                    dry_run,
                },
        } => {
            let loaded = project_file::load(&project).map_err(|e| e.to_string())?;
            let text =
                std::fs::read_to_string(&ops).map_err(|e| format!("{}: {e}", ops.display()))?;
            let op = match serde_json::from_str::<Vec<Op>>(&text) {
                Ok(ops) => Op::Batch {
                    label: ops_label(&ops),
                    ops,
                },
                Err(_) => serde_json::from_str::<Op>(&text)
                    .map_err(|e| format!("{}: not an op or array of ops: {e}", ops.display()))?,
            };
            let mut doc = Document::new(loaded.project);
            let result = if dry_run {
                doc.dry_run(&op)
            } else {
                doc.apply(op)
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&result).expect("results serialize")
            );
            if !result.applied {
                return Err("the ops were rejected; nothing changed".into());
            }
            if !dry_run {
                project_file::save(&project, doc.project(), Compression::None)
                    .map_err(|e| format!("{}: {e}", project.display()))?;
            }
            Ok(())
        }
    }
}

fn ops_label(ops: &[Op]) -> String {
    match ops {
        [one] => one.label(),
        _ => format!("{} edits", ops.len()),
    }
}

fn render(
    project: &Path,
    output: &Path,
    quality: QualityArg,
    voicebank: Option<PathBuf>,
    vocoder: Option<PathBuf>,
    cache: Option<PathBuf>,
    json: bool,
) -> Result<(), String> {
    let loaded = project_file::load(project).map_err(|e| e.to_string())?;
    let p = loaded.project;
    let singers: Vec<&str> = {
        let mut s: Vec<&str> = p.tracks.iter().map(|t| t.singer.id.as_str()).collect();
        s.sort_unstable();
        s.dedup();
        s
    };
    let voicebank = match voicebank {
        Some(v) => v,
        None => match singers.as_slice() {
            [one] => find_in_path("CANTOR_SINGERS", one)?,
            [] => return Err("the project has no tracks".into()),
            _ => {
                return Err(format!(
                    "tracks use several singers ({}); one singer per render until M3",
                    singers.join(", ")
                ));
            }
        },
    };
    let vocoder = match vocoder {
        Some(v) => v,
        None => {
            let name = vocoder_name(&voicebank)?;
            find_in_path("CANTOR_VOCODERS", &name)?
        }
    };
    let backend = DiffSinger::load(&voicebank, &vocoder).map_err(|e| e.to_string())?;
    let cache_dir = cache
        .or_else(|| std::env::var_os("CANTOR_CACHE").map(PathBuf::from))
        .unwrap_or_else(default_cache_dir);
    let cache = DiskCache::new(&cache_dir).map_err(|e| format!("{}: {e}", cache_dir.display()))?;
    let renderer = Renderer::new(Arc::new(backend), cache);
    let q = match quality {
        QualityArg::Preview => Quality::Preview,
        QualityArg::Final => Quality::Final,
    };
    let out = renderer.render(&p, q).map_err(|e| e.to_string())?;
    cantor_render::wav::write_stereo_f32(output, out.sample_rate, &out.samples)
        .map_err(|e| format!("{}: {e}", output.display()))?;
    if json {
        let report = serde_json::json!({
            "output": output,
            "sample_rate": out.sample_rate,
            "seconds": out.samples.len() as f64 / 2.0 / f64::from(out.sample_rate),
            "audio_hash": out.audio_hash.hex(),
            "trace": out.trace,
            "diagnostics": out.diagnostics,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("reports serialize")
        );
    } else {
        print_diagnostics(&out.diagnostics);
        eprintln!(
            "rendered {} phrases to {} (audio hash {})",
            out.trace.phrases.len(),
            output.display(),
            &out.audio_hash.hex()[..16]
        );
    }
    Ok(())
}

/// Finds `name` as a directory in a colon-separated search path variable.
fn find_in_path(var: &str, name: &str) -> Result<PathBuf, String> {
    let paths = std::env::var_os(var).ok_or_else(|| {
        format!(
            "cannot find `{name}`: pass it explicitly or set {var} to directories that contain it"
        )
    })?;
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .find(|d| d.is_dir())
        .ok_or_else(|| format!("`{name}` is not in any {var} directory"))
}

fn vocoder_name(voicebank: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(voicebank.join("dsconfig.yaml"))
        .map_err(|e| format!("{}: {e}", voicebank.display()))?;
    text.lines()
        .find_map(|l| l.strip_prefix("vocoder:"))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .ok_or_else(|| "the voicebank names no vocoder; pass --vocoder".into())
}

fn default_cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/Cantor/render")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("cantor/render")
    }
}

fn print_diagnostics(diags: &[Diagnostic]) {
    for d in diags {
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        eprintln!("{level}[{}]: {}", d.code, d.message);
    }
}
