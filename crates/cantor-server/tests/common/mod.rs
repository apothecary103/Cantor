//! Shared setup: an engine over the downloaded fixtures.

use cantor_server::{Engine, EngineConfig};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub(crate) const TWINKLE: &str = include_str!("../../../cantor-cli/tests/fixtures/twinkle.cantor");

pub(crate) fn engine(name: &str) -> Engine {
    let vb = cantor_fixtures::ensure(&cantor_fixtures::QIXUAN).unwrap();
    cantor_fixtures::ensure(&cantor_fixtures::PC_NSF_HIFIGAN).unwrap();
    let rmvpe = cantor_fixtures::ensure(&cantor_fixtures::RMVPE).unwrap();
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&work);
    Engine::new(EngineConfig {
        singer_paths: vec![vb.parent().unwrap().to_path_buf()],
        vocoder_paths: vec![cantor_fixtures::dir()],
        rmvpe: Some(rmvpe),
        cache_dir: work.join("cache"),
        render_dir: work.join("renders"),
    })
}

#[allow(dead_code)]
pub(crate) fn twinkle_file(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-project"));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("twinkle.cantor");
    std::fs::write(&p, TWINKLE).unwrap();
    p
}
