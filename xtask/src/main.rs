//! `cargo xtask <command>`: Cantor's build, CI and maintenance tasks.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// Crates with `unsafe` code or concurrency worth checking under Miri.
const MIRI_CRATES: &[&str] = &["cantor-core"];

/// Crates whose tests run under ThreadSanitizer. cantor-audio (realtime
/// mixer) and the engine thread join this list when they land.
const TSAN_CRATES: &[&str] = &["cantor-core"];

/// A dependency-free workspace crate used to observe the codegen backend.
const CODEGEN_PROBE: &str = "cantor-plugin-sdk";

/// The official MCP conformance suite. 0.1.x does not cover 2026-07-28.
/// See docs/decisions/0013-mcp-conformance.md.
const CONFORMANCE_PACKAGE: &str = "@modelcontextprotocol/conformance@0.2.0-alpha.12";
/// The MCP specification revision whose required scenarios must pass.
const MCP_REVISION: &str = "2026-07-28";
const CONFORMANCE_BIN: &str = "cantor-mcp-conformance";
const CONFORMANCE_PORT: u16 = 8001;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci              fmt check, clippy -D warnings, tests, schema check, codegen check
  schema [--check] regenerate schema/ from Rust types (--check: fail on drift)
  codegen-check   assert debug builds use Cranelift and release builds use LLVM
  miri            run Miri on crates with unsafe code
  tsan            run ThreadSanitizer on the engine and audio crates
  conformance     run the official MCP conformance suite against the fixture server
  python          build the Python wheel, install it in a venv, run the docs example and API tests
                  (needs network for npx; not part of ci)
  app           run the desktop app (arrives in M4)
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
        Some("conformance") => conformance(),
        Some("python") => python(),
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

/// Builds `bin` of `package` and returns the executable's path, as cargo
/// reports it (so CARGO_TARGET_DIR and build-dir layouts just work).
fn build_bin(package: &str, bin: &str) -> Result<PathBuf> {
    eprintln!("\n==> build {bin}");
    let out = cargo()
        .args(["build", "--locked", "-p", package, "--bin", bin])
        .arg("--message-format=json-render-diagnostics")
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !out.status.success() {
        return Err(format!("building {bin} failed ({})", out.status));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == bin)
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
        .ok_or_else(|| format!("cargo reported no executable for {bin}"))
}

/// Kills the child process when dropped, so an early return or panic never
/// leaves the fixture server holding the port.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Builds and starts the MCP conformance fixture server, runs the official
/// suite's required scenarios for [`MCP_REVISION`] against it, and stops it.
fn conformance() -> Result {
    let exe = build_bin("cantor-mcp", CONFORMANCE_BIN)?;
    let addr = SocketAddr::from(([127, 0, 0, 1], CONFORMANCE_PORT));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
        return Err(format!(
            "{addr} is already in use; stop whatever listens there and retry"
        ));
    }

    let log_path = exe.with_extension("log");
    let log = std::fs::File::create(&log_path).map_err(|e| format!("{log_path:?}: {e}"))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    eprintln!(
        "\n==> start {CONFORMANCE_BIN} on {addr} (log: {})",
        log_path.display()
    );
    let child = Command::new(&exe)
        .env("PORT", CONFORMANCE_PORT.to_string())
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;
    let mut server = KillOnDrop(child);

    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
        if let Some(status) = server.0.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "{CONFORMANCE_BIN} exited ({status}) before listening; see {}",
                log_path.display()
            ));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "{CONFORMANCE_BIN} did not listen on {addr} within 30 s"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let url = format!("http://{addr}/mcp");
    let mut npx = Command::new("npx");
    npx.current_dir(root()).args([
        "-y",
        CONFORMANCE_PACKAGE,
        "server",
        "--url",
        &url,
        "--requirements",
        MCP_REVISION,
    ]);
    // Keep npx's package cache next to the build, never in ~/.npm.
    if std::env::var_os("npm_config_cache").is_none() {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root().join("target"));
        npx.env("npm_config_cache", target.join("npm-cache"));
    }
    let baseline = root().join("crates/cantor-mcp/conformance/expected-failures.yaml");
    if baseline.exists() {
        npx.arg("--expected-failures").arg(&baseline);
    }
    let result = run(
        &format!("MCP conformance suite ({CONFORMANCE_PACKAGE}, {MCP_REVISION})"),
        &mut npx,
    );
    drop(server);
    result
}

/// The first ```python block of docs/spec/60-api.md, verbatim.
fn spec_python_example() -> Result<String> {
    let spec =
        std::fs::read_to_string(root().join("docs/spec/60-api.md")).map_err(|e| e.to_string())?;
    let start = spec
        .find("```python\n")
        .ok_or("no python block in 60-api.md")?
        + "```python\n".len();
    let len = spec[start..]
        .find("```")
        .ok_or("unterminated python block")?;
    Ok(spec[start..start + len].to_owned())
}

/// Builds the wheel with maturin, installs it into a fresh venv (no
/// network), runs the spec's example unchanged and the API tests.
fn python() -> Result {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("target"));
    let wheels = target.join("wheels");
    let _ = std::fs::remove_dir_all(&wheels);
    run(
        "maturin build",
        Command::new("maturin")
            .current_dir(root())
            .args([
                "build",
                "--release",
                "--locked",
                "-i",
                "python3",
                "--manifest-path",
            ])
            .arg(root().join("crates/cantor-py/Cargo.toml"))
            .arg("--out")
            .arg(&wheels),
    )?;
    let wheel = std::fs::read_dir(&wheels)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "whl"))
        .ok_or("maturin produced no wheel")?;
    eprintln!("wheel: {}", wheel.display());

    let venv = target.join("py-venv");
    let _ = std::fs::remove_dir_all(&venv);
    run(
        "venv",
        Command::new("python3").args(["-m", "venv"]).arg(&venv),
    )?;
    let py = venv.join("bin/python");
    run(
        "pip install",
        Command::new(&py)
            .args([
                "-m",
                "pip",
                "install",
                "--quiet",
                "--no-index",
                "--force-reinstall",
            ])
            .arg(&wheel),
    )?;

    let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN)?;
    cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN)?;
    let rmvpe = cantor_fixtures::ensure(&cantor_fixtures::RMVPE)?;
    let work = target.join("py-example");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let fixture = root().join("crates/cantor-cli/tests/fixtures/twinkle.cantor");
    std::fs::copy(&fixture, work.join("song.cantor")).map_err(|e| e.to_string())?;
    std::fs::write(work.join("example.py"), spec_python_example()?).map_err(|e| e.to_string())?;
    let env = |c: &mut Command| {
        c.current_dir(&work)
            .env("CANTOR_SINGERS", vb.parent().expect("voicebank parent"))
            .env("CANTOR_VOCODERS", cantor_fixtures::dir())
            .env("CANTOR_RMVPE", &rmvpe)
            .env("CANTOR_CACHE", work.join("cache"));
    };
    let tests = root().join("crates/cantor-py/tests");
    let mut setup = Command::new(&py);
    setup.arg(tests.join("setup_example.py")).arg("song.cantor");
    env(&mut setup);
    run("prepare song.cantor", &mut setup)?;
    let mut example = Command::new(&py);
    example.arg("example.py");
    env(&mut example);
    run("docs/spec/60-api.md example (unchanged)", &mut example)?;
    let mut api = Command::new(&py);
    api.arg(tests.join("test_api.py")).arg("song.cantor");
    env(&mut api);
    run("Python API tests", &mut api)?;
    eprintln!("\nxtask python: wheel built, installed and the spec example ran");
    Ok(())
}
