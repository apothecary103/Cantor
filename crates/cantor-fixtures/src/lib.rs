//! Test fixtures that are too large or too restrictively licensed to commit:
//! downloaded from pinned URLs, verified by SHA-256 and unpacked on first
//! use. Model weights are never committed (CLAUDE.md).
//!
//! Location: `$CANTOR_FIXTURES`, else `<workspace>/tests/fixtures/downloaded`.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// How a fixture is packaged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packaging {
    /// A zip archive, unpacked into a directory.
    Zip,
    /// A single file, used as downloaded.
    File,
}

/// A downloadable fixture.
#[derive(Clone, Copy, Debug)]
pub struct Fixture {
    /// Zip archive or plain file.
    pub packaging: Packaging,
    /// Directory name after unpacking.
    pub name: &'static str,
    /// Pinned URL (a release asset, never "latest").
    pub url: &'static str,
    /// Archive file name.
    pub file: &'static str,
    /// Archive size in bytes.
    pub size: u64,
    /// Archive SHA-256, lowercase hex.
    pub sha256: &'static str,
    /// Directory inside the archive that holds the content ("" = top).
    pub subdir: &'static str,
    /// Licence, shown when the fixture is fetched.
    pub license: &'static str,
}

/// Qixuan v2.7.0, an OpenUtau-format DiffSinger voicebank by yqzhishen
/// (OpenVPI). Terms of use permit inference with new programs; no
/// modification, conversion or training use. See docs/decisions/0010.
pub const QIXUAN: Fixture = Fixture {
    packaging: Packaging::Zip,
    name: "qixuan-2.7.0",
    url: "https://github.com/yqzhishen/qixuan-diffsinger/releases/download/v2.7.0/Qixuan_v2.7.0_DiffSinger_OpenUtau.zip",
    file: "Qixuan_v2.7.0_DiffSinger_OpenUtau.zip",
    size: 420_707_627,
    sha256: "fe8ee7c95883d327a2b7facbbe832a7a53e1344918dc791ca83fc23b08034110",
    subdir: "Qixuan_v2.7.0_DiffSinger_OpenUtau",
    license: "Qixuan Terms of Use (https://github.com/yqzhishen/qixuan-diffsinger/tree/main/terms_of_use)",
};

/// PC-NSF-HiFiGAN 44.1 kHz, hop 512, 128 mel bins (OpenVPI, 2025.02),
/// packaged for OpenUtau.
pub const PC_NSF_HIFIGAN: Fixture = Fixture {
    packaging: Packaging::Zip,
    name: "pc-nsf-hifigan-2025.02",
    url: "https://github.com/openvpi/vocoders/releases/download/pc-nsf-hifigan-44.1k-hop512-128bin-2025.02/pc_nsf_hifigan_44.1k_hop512_128bin_2025.02.oudep",
    file: "pc_nsf_hifigan_44.1k_hop512_128bin_2025.02.oudep",
    size: 52_760_610,
    sha256: "ba7d43142d41f6900c8264b5662ca7125a50feb8760bb8b9615c61a8f5e6902e",
    subdir: "",
    license: "CC BY-NC-SA 4.0 (https://github.com/openvpi/vocoders)",
};

/// RMVPE pitch estimator (Wei et al., Interspeech 2023), ONNX export from
/// the RVC project: input log-mel `[1, 128, T]`, output 360-bin salience.
pub const RMVPE: Fixture = Fixture {
    packaging: Packaging::File,
    name: "rmvpe",
    url: "https://huggingface.co/lj1995/VoiceConversionWebUI/resolve/e6d0c1a17da07c33557852f9dfa2bd44cc75737d/rmvpe.onnx",
    file: "rmvpe.onnx",
    size: 361_688_443,
    sha256: "5370e71ac80af8b4b7c793d27efd51fd8bf962de3a7ede0766dac0befa3660fd",
    subdir: "",
    license: "MIT (https://huggingface.co/lj1995/VoiceConversionWebUI)",
};

/// Every fixture, for `cargo xtask fixtures`.
pub const ALL: &[Fixture] = &[QIXUAN, PC_NSF_HIFIGAN, RMVPE];

/// Where fixtures live.
pub fn dir() -> PathBuf {
    std::env::var_os("CANTOR_FIXTURES").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/downloaded"),
        PathBuf::from,
    )
}

/// Returns the unpacked fixture directory, downloading and verifying it
/// first if needed. Safe to call from parallel tests.
pub fn ensure(f: &Fixture) -> Result<PathBuf, String> {
    let root = dir();
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let unpacked = root.join(f.name);
    let content = match f.packaging {
        Packaging::Zip => unpacked.join(f.subdir),
        Packaging::File => root.join(f.file),
    };
    let done = unpacked.join(".cantor-verified");
    if done.exists() {
        return Ok(content);
    }
    let _lock = Lock::acquire(&root.join(format!("{}.lock", f.name)))?;
    if done.exists() {
        return Ok(content);
    }
    let archive = root.join(f.file);
    if !archive.exists() || sha256(&archive)? != f.sha256 {
        eprintln!(
            "fetching fixture {} ({} MB, {})",
            f.name,
            f.size / 1_000_000,
            f.license
        );
        let tmp = root.join(format!("{}.part", f.file));
        run(Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--retry",
                "3",
                "-o",
            ])
            .arg(&tmp)
            .arg(f.url))?;
        let got = sha256(&tmp)?;
        if got != f.sha256 {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "{}: SHA-256 {got} does not match {}",
                f.url, f.sha256
            ));
        }
        std::fs::rename(&tmp, &archive).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    if f.packaging == Packaging::Zip {
        run(Command::new("unzip")
            .args(["-q", "-o"])
            .arg(&archive)
            .arg("-d")
            .arg(&unpacked))?;
    }
    std::fs::write(&done, f.sha256).map_err(|e| e.to_string())?;
    Ok(content)
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{cmd:?} failed ({status})"))
}

/// SHA-256 of a file, lowercase hex.
pub fn sha256(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// A lock file held while a fixture is fetched.
struct Lock(PathBuf);

impl Lock {
    fn acquire(path: &Path) -> Result<Self, String> {
        let deadline = Instant::now() + Duration::from_secs(30 * 60);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => return Ok(Self(path.to_path_buf())),
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(500))
                }
                Err(e) => return Err(format!("waiting for {}: {e}", path.display())),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
