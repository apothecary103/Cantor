//! `cantor`: the engine, HTTP API + web editor, MCP server and CLI in one
//! binary (docs/design.md "CLI").

use cantor::engine::{self, Caller, Engine};
use cantor::{http, mcp};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "cantor",
    version,
    about = "UTAU-style singing synthesis: web editor, JSON API, MCP server and CLI"
)]
struct Cli {
    /// Extra voicebank folders (also CANTOR_VOICEBANKS, ./voicebanks, ~/.local/share/cantor/voicebanks).
    #[arg(long = "voicebanks", global = true)]
    voicebanks: Vec<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve the web editor, the JSON API and MCP (streamable HTTP at /mcp).
    Serve {
        /// Port on 127.0.0.1.
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Project or file to open (.cantor, or anything `import` reads).
        project: Option<PathBuf>,
    },
    /// MCP server on stdio.
    Mcp {
        /// Project to open first.
        project: Option<PathBuf>,
    },
    /// List voicebanks.
    Voicebanks,
    /// Summarize a project.
    Info { project: PathBuf },
    /// Import a MIDI/UST/USTX/MusicXML file into a .cantor project.
    Import {
        file: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Run a tuner pass on a project file (in place unless -o).
    Tune {
        project: PathBuf,
        /// auto_pitch, auto_dynamics, breaths, section_color, match_reference, humanize.
        pass: String,
        /// Track id (default: the first track).
        #[arg(long)]
        track: Option<u64>,
        /// Pass parameters as JSON.
        #[arg(long, default_value = "{}")]
        params: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Apply ops (a JSON array) to a project file (in place unless -o).
    Ops {
        project: PathBuf,
        /// JSON array of ops, or @file.json.
        ops: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Render a project to WAV and print the report.
    Render {
        project: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Export a project as wav, ust, ustx or cantor.
    Export {
        project: PathBuf,
        format: String,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// MIDI (+ lyrics, + reference vocal) → tuned, rendered cover in one go.
    Cover {
        #[arg(long)]
        midi: PathBuf,
        /// Lyrics text (kana, romaji or space-separated syllables).
        #[arg(long)]
        lyrics: Option<PathBuf>,
        /// The original vocal (WAV) to follow.
        #[arg(long)]
        audio: Option<PathBuf>,
        /// Voicebank id (default: the first one found).
        #[arg(long)]
        singer: Option<String>,
        /// natural, pop, ballad, rock, robotic.
        #[arg(long, default_value = "natural")]
        style: String,
        /// Output folder.
        #[arg(short, long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CANTOR_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cantor: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res = Result<(), Box<dyn std::error::Error>>;

fn print(v: &impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn load(e: &Engine, path: &Path) -> Res {
    let is_project = path.extension().is_some_and(|x| x == "cantor");
    let r = if is_project {
        e.open(path, &Caller::cli())?
    } else {
        e.import(path, None, None, &Caller::cli())?
    };
    for d in &r.diagnostics {
        eprintln!("{:?} {}: {}", d.level, d.code, d.message);
    }
    Ok(())
}

fn first_track(e: &Engine) -> Result<cantor_core::TrackId, Box<dyn std::error::Error>> {
    Ok(e.project()
        .tracks
        .first()
        .ok_or("the project has no tracks")?
        .id)
}

fn tune(e: &Engine, pass: &str, track: cantor_core::TrackId, params: serde_json::Value) -> Res {
    let req = serde_json::from_value(json!({ "pass": pass, "track": track, "params": params }))?;
    let r = e.tune(req, &Caller::cli())?;
    eprintln!("{}", r.summary);
    Ok(())
}

fn run(cli: Cli) -> Res {
    let engine = Arc::new(Engine::new(engine::voicebank_paths(&cli.voicebanks)));
    let e = &*engine;
    if e.voicebanks().is_empty() {
        tracing::warn!(
            "no voicebanks found; put UTAU voicebank folders in ./voicebanks, \
             ~/.local/share/cantor/voicebanks, or pass --voicebanks DIR"
        );
    }
    match cli.cmd {
        Cmd::Serve { port, project } => {
            if let Some(p) = project {
                load(e, &p)?;
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                let listener = tokio::net::TcpListener::bind(addr).await?;
                let app = http::router(engine.clone()).merge(mcp::router(engine));
                tracing::info!("Cantor at http://{addr}  (MCP: http://{addr}/mcp)");
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await
            })?;
        }
        Cmd::Mcp { project } => {
            if let Some(p) = project {
                load(e, &p)?;
            }
            tokio::runtime::Runtime::new()?.block_on(mcp::serve_stdio(engine))?;
        }
        Cmd::Voicebanks => print(&e.voicebanks()),
        Cmd::Info { project } => {
            load(e, &project)?;
            let p = e.project();
            print(
                &json!({ "meta": p.meta, "tempo": p.tempo, "tracks": p.tracks.iter().map(|t| json!({
                "id": t.id, "name": t.name, "singer": t.singer, "notes": t.notes.len() })).collect::<Vec<_>>() }),
            );
        }
        Cmd::Import { file, out } => {
            load(e, &file)?;
            e.save(Some(&out))?;
        }
        Cmd::Tune {
            project,
            pass,
            track,
            params,
            out,
        } => {
            load(e, &project)?;
            let t = track.map_or_else(|| first_track(e), |t| Ok(cantor_core::TrackId(t)))?;
            tune(e, &pass, t, serde_json::from_str(&params)?)?;
            e.save(Some(out.as_deref().unwrap_or(&project)))?;
        }
        Cmd::Ops { project, ops, out } => {
            load(e, &project)?;
            let text = match ops.strip_prefix('@') {
                Some(f) => std::fs::read_to_string(f)?,
                None => ops,
            };
            let r = e.apply_ops(serde_json::from_str(&text)?, None, &Caller::cli())?;
            print(&r);
            e.save(Some(out.as_deref().unwrap_or(&project)))?;
        }
        Cmd::Render { project, out } => {
            load(e, &project)?;
            let r = e.render(None, &Caller::cli())?;
            std::fs::copy(&r.wav_path, &out)?;
            print(&json!({ "wav": out, "report": r.report, "diagnostics": r.diagnostics }));
        }
        Cmd::Export {
            project,
            format,
            out,
        } => {
            load(e, &project)?;
            let (bytes, _, _, diags) = e.export(&format)?;
            std::fs::write(&out, bytes)?;
            for d in diags {
                eprintln!("{:?} {}: {}", d.level, d.code, d.message);
            }
        }
        Cmd::Cover {
            midi,
            lyrics,
            audio,
            singer,
            style,
            out,
        } => cover(
            e,
            &midi,
            lyrics.as_deref(),
            audio.as_deref(),
            singer,
            &style,
            &out,
        )?,
    }
    Ok(())
}

/// The whole pipeline (docs/agent-guide.md, steps 1–5) without an agent.
fn cover(
    e: &Engine,
    midi: &Path,
    lyrics: Option<&Path>,
    audio: Option<&Path>,
    singer: Option<String>,
    style: &str,
    out: &Path,
) -> Res {
    let c = Caller::cli();
    load(e, midi)?;
    if let Some(s) = singer {
        if !e.voicebanks().iter().any(|v| v.id == s) {
            return Err(format!("no voicebank `{s}`; `cantor voicebanks` lists them").into());
        }
        let ops = e
            .project()
            .tracks
            .iter()
            .map(|t| {
                serde_json::from_value(
                    json!({ "op": "update_track", "track": t.id, "patch": { "singer": s } }),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        e.apply_ops(ops, None, &c)?;
    }
    if let Some(l) = lyrics {
        let r = e.import(l, Some("lyrics"), None, &c)?;
        for d in r.diagnostics {
            eprintln!("{:?} {}: {}", d.level, d.code, d.message);
        }
    }
    if let Some(a) = audio {
        e.import(a, Some("audio"), None, &c)?;
    }
    let track = first_track(e)?;
    e.checkpoint("draft", &c)?;
    tune(e, "auto_pitch", track, json!({ "style": style }))?;
    tune(e, "auto_dynamics", track, json!({}))?;
    tune(e, "breaths", track, json!({}))?;
    if audio.is_some() {
        tune(e, "match_reference", track, json!({}))?;
    }
    std::fs::create_dir_all(out)?;
    let r = e.render(None, &c)?;
    std::fs::copy(&r.wav_path, out.join("cover.wav"))?;
    e.save(Some(&out.join("cover.cantor")))?;
    std::fs::write(
        out.join("report.json"),
        serde_json::to_vec_pretty(&r.report)?,
    )?;
    let pe = |s: &Option<cantor_analysis::report::ErrorStats>| {
        s.as_ref()
            .map_or("—".to_owned(), |s| format!("{:.0}¢", s.p95))
    };
    eprintln!(
        "cover: {} · pitch p95 vs score {} · vs reference {} · {} missing aliases · {}",
        out.join("cover.wav").display(),
        pe(&r.report.pitch_error),
        pe(&r.report.ref_pitch_error),
        r.report.missing_aliases.len(),
        r.report
            .lufs
            .map_or("silent".into(), |l| format!("{l:.1} LUFS")),
    );
    if !r.report.missing_aliases.is_empty() {
        let l: Vec<&str> = r
            .report
            .missing_aliases
            .iter()
            .map(|m| m.lyric.as_str())
            .collect();
        eprintln!("missing aliases: {}", l.join(" "));
    }
    Ok(())
}
