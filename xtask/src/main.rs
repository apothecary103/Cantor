//! `cargo xtask <command>`: Cantor's build, CI and maintenance tasks.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Crates with `unsafe` code or concurrency worth checking under Miri.
const MIRI_CRATES: &[&str] = &["cantor-core"];

/// Crates whose tests run under ThreadSanitizer. cantor-audio (realtime
/// mixer) and the engine thread join this list when they land.
const TSAN_CRATES: &[&str] = &["cantor-core"];

/// A dependency-free workspace crate used to observe the codegen backend.
const CODEGEN_PROBE: &str = "cantor-plugin-sdk";

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci              fmt check, clippy -D warnings, tests, schema check, codegen check
  schema [--check] regenerate schema/ from Rust types (--check: fail on drift)
  codegen-check   assert debug builds use Cranelift and release builds use LLVM
  miri            run Miri on crates with unsafe code
  tsan            run ThreadSanitizer on the engine and audio crates
  app             run the desktop app (arrives in M4)
  bench           render and UI benchmarks (arrive in M2/M4)";

type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("schema") => schema(rest.contains(&"--check")),
        Some("codegen-check") => codegen_check(),
        Some("miri") => miri(),
        Some("tsan") => tsan(),
        Some("app") => Err("the desktop app arrives in milestone M4".into()),
        Some("bench") => Err("benchmarks arrive with rendering (M2) and the app (M4)".into()),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_path_buf()
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(step: &str, cmd: &mut Command) -> Result {
    eprintln!("\n==> {step}");
    let status = cmd.status().map_err(|e| format!("{step}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{step} failed ({status})"))
    }
}

fn ci() -> Result {
    run("rustfmt", cargo().args(["fmt", "--all", "--check"]))?;
    run(
        "clippy",
        cargo().args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ]),
    )?;
    run(
        "tests",
        cargo().args(["test", "--workspace", "--all-targets", "--locked"]),
    )?;
    run(
        "doctests",
        cargo().args(["test", "--workspace", "--doc", "--locked"]),
    )?;
    eprintln!("\n==> schema check");
    schema(true)?;
    eprintln!("\n==> codegen check");
    codegen_check()?;
    eprintln!("\nxtask ci: all checks passed");
    Ok(())
}

/// Every generated file under `schema/`, as (file name, contents).
fn generated_schemas() -> Vec<(String, String)> {
    let mut entries = cantor_core::schema::registry();
    entries.extend(cantor_io::schema::registry());
    let mut files: Vec<(String, String)> = entries
        .into_iter()
        .map(|e| (format!("{}.schema.json", e.name), pretty(&e.schema)))
        .collect();
    files.push((
        "params.json".into(),
        pretty(&cantor_core::params::registry()),
    ));
    files.sort();
    files
}

fn pretty<T: serde::Serialize + ?Sized>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("schemas serialize");
    s.push('\n');
    s
}

fn schema(check: bool) -> Result {
    let dir = root().join("schema");
    let files = generated_schemas();
    let wanted: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    let existing: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    let stale: Vec<&String> = existing
        .iter()
        .filter(|n| !wanted.contains(&n.as_str()))
        .collect();

    if check {
        let mut drift: Vec<String> = files
            .iter()
            .filter(|(name, body)| {
                std::fs::read_to_string(dir.join(name)).ok().as_deref() != Some(body.as_str())
            })
            .map(|(name, _)| name.clone())
            .collect();
        drift.extend(stale.iter().map(|n| format!("{n} (stale)")));
        if drift.is_empty() {
            eprintln!("schema/ is up to date ({} files)", files.len());
            return Ok(());
        }
        return Err(format!(
            "schema/ is out of date: {}. Run `cargo xtask schema` and commit the result.",
            drift.join(", ")
        ));
    }

    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for name in stale {
        std::fs::remove_file(dir.join(name)).map_err(|e| e.to_string())?;
    }
    for (name, body) in &files {
        std::fs::write(dir.join(name), body).map_err(|e| format!("{name}: {e}"))?;
    }
    eprintln!("wrote {} files to schema/", files.len());
    Ok(())
}

/// Builds the probe crate in dev and release profiles and checks which
/// codegen backend cargo passes to rustc.
fn codegen_check() -> Result {
    let target_dir = root().join("target").join("codegen-check");
    for (profile, release, want, forbid) in [
        (
            "dev",
            false,
            "codegen-backend=cranelift",
            "codegen-backend=llvm",
        ),
        (
            "release",
            true,
            "codegen-backend=llvm",
            "codegen-backend=cranelift",
        ),
    ] {
        let mut clean = cargo();
        clean
            .args(["clean", "--quiet", "-p", CODEGEN_PROBE])
            .arg("--target-dir")
            .arg(&target_dir);
        if release {
            clean.arg("--release");
        }
        run(&format!("clean probe ({profile})"), &mut clean)?;

        let mut build = cargo();
        build.args([
            "build",
            "--verbose",
            "--locked",
            "--lib",
            "-p",
            CODEGEN_PROBE,
        ]);
        build.arg("--target-dir").arg(&target_dir);
        if release {
            build.arg("--release");
        }
        let out = build.output().map_err(|e| e.to_string())?;
        let log = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            return Err(format!(
                "building the codegen probe ({profile}) failed:\n{log}"
            ));
        }
        let crate_flag = format!("--crate-name {}", CODEGEN_PROBE.replace('-', "_"));
        let line = log
            .lines()
            .find(|l| l.contains("Running") && l.contains(&crate_flag))
            .ok_or_else(|| {
                format!("no rustc invocation for {CODEGEN_PROBE} in the {profile} build log")
            })?;
        let flags = line.replace("-Z codegen-backend", "-Zcodegen-backend");
        if !flags.contains(want) || flags.contains(forbid) {
            return Err(format!(
                "{profile} build must use `{want}`, but rustc ran as:\n{line}"
            ));
        }
        eprintln!("{profile}: rustc runs with {want}");
    }
    Ok(())
}

fn host_triple() -> Result<String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|e| format!("rustc -vV: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
        .ok_or_else(|| "rustc -vV printed no host triple".into())
}

fn miri() -> Result {
    run("miri setup", cargo().args(["miri", "setup"]))?;
    let mut cmd = cargo();
    cmd.args(["miri", "test", "--locked"]);
    for c in MIRI_CRATES {
        cmd.args(["-p", c]);
    }
    // Miri interprets MIR; the Cranelift backend setting does not apply.
    cmd.env("CARGO_PROFILE_DEV_CODEGEN_BACKEND", "llvm");
    cmd.env("PROPTEST_CASES", "8");
    run("miri test", &mut cmd)
}

fn tsan() -> Result {
    let host = host_triple()?;
    let mut cmd = cargo();
    cmd.args(["test", "--locked", "-Zbuild-std", "--target", &host]);
    for c in TSAN_CRATES {
        cmd.args(["-p", c]);
    }
    // Sanitizers need LLVM; separate target dir keeps instrumented
    // artifacts away from normal builds.
    cmd.env("CARGO_PROFILE_DEV_CODEGEN_BACKEND", "llvm");
    cmd.env("RUSTFLAGS", "-Zsanitizer=thread");
    cmd.env("RUSTDOCFLAGS", "-Zsanitizer=thread");
    cmd.env("TSAN_OPTIONS", "halt_on_error=1");
    cmd.env("CARGO_TARGET_DIR", root().join("target").join("tsan"));
    run(&format!("tsan test ({})", TSAN_CRATES.join(", ")), &mut cmd)
}
