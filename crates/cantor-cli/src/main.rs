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
        /// DiffSinger voicebank directory. Without it the engine renders
        /// every track with its own singer from $CANTOR_SINGERS (DiffSinger,
        /// UTAU or svs.io), with analysis when $CANTOR_RMVPE is set.
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
    /// Render a project and print the analysis report (pitch, timing,
    /// loudness, artifacts). Singers come from $CANTOR_SINGERS; pitch
    /// analysis needs $CANTOR_RMVPE.
    Analyze {
        /// The .cantor project.
        project: PathBuf,
        /// Render quality.
        #[arg(long, value_enum, default_value_t = QualityArg::Preview)]
        quality: QualityArg,
        /// Print the full report as JSON instead of a summary.
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
    /// Apply a plain-language style ("breathier, more intimate, less
    /// vibrato") to a track as curve and expression edits; rewrites the
    /// project in place.
    Style {
        /// The .cantor project.
        project: PathBuf,
        /// The style prompt.
        prompt: String,
        /// Track id (default: the first track).
        #[arg(long)]
        track: Option<u64>,
        /// First tick (default: the track's start).
        #[arg(long)]
        start: Option<i64>,
        /// Tick after the range (default: the track's end).
        #[arg(long)]
        end: Option<i64>,
        /// Report the interpretation and the edits without saving.
        #[arg(long)]
        dry_run: bool,
    },
    /// Convert a sung recording into a project a Cantor singer sings.
    /// Requires the performer's consent, which is recorded in the project.
    Convert {
        /// WAV file of the performance.
        input: PathBuf,
        /// Output .cantor project.
        #[arg(short, long)]
        output: PathBuf,
        /// Singer id (from $CANTOR_SINGERS) that sings the result.
        #[arg(long)]
        singer: String,
        /// Lyrics, space-separated, one per detected note.
        #[arg(long, default_value = "")]
        lyrics: String,
        /// Who performed the recording.
        #[arg(long)]
        performer: String,
        /// The performer's consent statement.
        #[arg(long)]
        consent: String,
        /// When consent was given.
        #[arg(long)]
        date: String,
        /// Confirms the performer gave this consent.
        #[arg(long)]
        confirm: bool,
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
        } => match (voicebank, vocoder) {
            (None, None) => engine_render(&project, Some(&output), quality, cache, json),
            (voicebank, vocoder) => {
                render(&project, &output, quality, voicebank, vocoder, cache, json)
            }
        },
        Command::Analyze {
            project,
            quality,
            json,
        } => engine_render(&project, None, quality, None, json),
        Command::Import { input, output } => {
            let imported = cantor_io::import::import_path(&input).map_err(|e| e.to_string())?;
            print_diagnostics(&imported.diagnostics);
            project_file::save(&output, &imported.project, Compression::None)
                .map_err(|e| format!("{}: {e}", output.display()))
        }
        Command::Style {
            project,
            prompt,
            track,
            start,
            end,
            dry_run,
        } => style(&project, &prompt, track, start, end, dry_run),
        Command::Convert {
            input,
            output,
            singer,
            lyrics,
            performer,
            consent,
            date,
            confirm,
        } => {
            let engine = cantor_server::Engine::new(cantor_server::EngineConfig::from_env());
            let lyrics: Vec<String> = lyrics.split_whitespace().map(str::to_owned).collect();
            let consent = cantor_server::convert::ConsentAttestation {
                performer,
                statement: consent,
                date,
                confirmed: confirm,
            };
            let info = engine
                .convert_audio(&input, &singer, &lyrics, &consent)
                .map_err(|e| e.to_string())?;
            let path = engine
                .save_project(&info.project_id, Some(&output))
                .map_err(|e| e.to_string())?;
            let (p, _) = engine
                .snapshot(&info.project_id)
                .map_err(|e| e.to_string())?;
            eprintln!(
                "converted {} notes to {}",
                p.tracks[0].parts[0].notes.len(),
                path.display()
            );
            Ok(())
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

fn style(
    project: &Path,
    prompt: &str,
    track: Option<u64>,
    start: Option<i64>,
    end: Option<i64>,
    dry_run: bool,
) -> Result<(), String> {
    let loaded = project_file::load(project).map_err(|e| e.to_string())?;
    let p = loaded.project;
    let t = match track {
        Some(id) => p.track(cantor_core::TrackId(id)),
        None => p.tracks.first(),
    }
    .ok_or("no such track")?;
    let range = cantor_core::TickRange {
        start: start.unwrap_or_else(|| t.parts.iter().map(|x| x.start).min().unwrap_or(0)),
        end: end.unwrap_or_else(|| {
            t.parts
                .iter()
                .map(|x| x.start + x.length)
                .max()
                .unwrap_or(1)
        }),
    };
    let plan = cantor_core::style::interpret(prompt);
    let (op, diags) = cantor_core::style::ops(&p, t.id, range, prompt, &plan);
    let mut doc = Document::new(p.clone());
    let result = op.map(|op| {
        if dry_run {
            doc.dry_run(&op)
        } else {
            doc.apply(op)
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "plan": plan,
            "diagnostics": diags,
            "result": result,
        }))
        .expect("results serialize")
    );
    match result {
        Some(r) if r.applied => {
            if !dry_run {
                project_file::save(project, doc.project(), Compression::None)
                    .map_err(|e| format!("{}: {e}", project.display()))?;
            }
            Ok(())
        }
        Some(_) => Err("the style edits were rejected; nothing changed".into()),
        None => Err("the prompt changes nothing".into()),
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

/// Renders through the engine (every backend, every singer of the
/// project) and writes the WAV to `output`, or only reports when `output`
/// is `None` (`cantor analyze`).
fn engine_render(
    project: &Path,
    output: Option<&Path>,
    quality: QualityArg,
    cache: Option<PathBuf>,
    json: bool,
) -> Result<(), String> {
    let mut config = cantor_server::EngineConfig::from_env();
    if let Some(c) = cache {
        config.cache_dir = c;
    }
    let engine = cantor_server::Engine::new(config);
    let info = engine.open_project(project).map_err(|e| e.to_string())?;
    let q = match quality {
        QualityArg::Preview => Quality::Preview,
        QualityArg::Final => Quality::Final,
    };
    let r = engine
        .render(&info.project_id, None, q)
        .map_err(|e| e.to_string())?;
    if let Some(out) = output {
        let wav = r.audio_uri.trim_start_matches("file://");
        std::fs::copy(wav, out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    if json {
        let mut v = serde_json::to_value(&r).expect("reports serialize");
        if let Some(out) = output {
            v["output"] = serde_json::json!(out);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&v).expect("reports serialize")
        );
        return Ok(());
    }
    print_diagnostics(&r.diagnostics);
    if let Some(out) = output {
        eprintln!(
            "rendered {} phrases to {} (audio hash {})",
            r.trace.phrases.len(),
            out.display(),
            &r.audio_hash[..16]
        );
    }
    match &r.analysis {
        Some(a) => {
            for t in &a.tracks {
                let fmt = |v: Option<f64>, unit: &str| {
                    v.map_or("n/a".to_owned(), |v| format!("{v:.1} {unit}"))
                };
                println!(
                    "{}: pitch error mean {}, p95 {}; onset p95 {}",
                    t.track,
                    fmt(t.pitch.mean_cents, "cents"),
                    fmt(t.pitch.p95_cents, "cents"),
                    fmt(t.timing.p95_abs_ms, "ms"),
                );
            }
            println!(
                "loudness {:.1} LUFS integrated, true peak {:.1} dBTP; {} artifact(s)",
                a.loudness.integrated_lufs.unwrap_or(f64::NEG_INFINITY),
                a.loudness.true_peak_dbtp.unwrap_or(f64::NEG_INFINITY),
                a.artifacts.len()
            );
        }
        None => println!("no analysis (set CANTOR_RMVPE for pitch and timing)"),
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
