//! Screenshots of the editor, rendered headlessly with the real GPU
//! renderer. They document the visual direction (docs/spec/50-ux.md) and
//! catch visual regressions: `UPDATE_SCREENS=1 cargo test -p app --test
//! screens` rewrites docs/screens/; otherwise renders are compared with the
//! committed images.

use cantor_app::state::Editor;
use cantor_app::theme::{DARK, LIGHT, Theme};
use cantor_app::view::CantorWindow;
use cantor_core::{NoteId, Op, ParamId};
use cantor_server::{Engine, EngineConfig};
use gpui_kit::{AppContext, HeadlessAppContext, px, size};
use std::path::PathBuf;
use std::sync::Arc;

fn screens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/screens")
}

fn engine() -> Arc<Engine> {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("screens");
    Arc::new(Engine::new(EngineConfig {
        singer_paths: vec![],
        vocoder_paths: vec![],
        phonemizer_paths: vec![],
        rmvpe: None,
        cache_dir: tmp.join("c"),
        render_dir: tmp.join("r"),
    }))
}

/// A demo editor with a selection, a lyric edit and a curve edit so the
/// Script Log, history and handles have content.
fn editor() -> Editor {
    let mut e = Editor::new(engine(), cantor_app::demo::project(160));
    e.viewport.cents_top = 8000.0;
    let first: Vec<NoteId> = e
        .project()
        .notes()
        .filter(|(t, _, _)| t.id.0 == 1)
        .map(|(_, _, n)| n.id)
        .take(3)
        .collect();
    e.selection.insert(first[1]);
    let _ = e.apply(Op::SetLyric {
        id: first[0],
        lyric: "ming".into(),
    });
    let _ = e.apply(Op::SetCurve {
        track: cantor_core::TrackId(1),
        param: ParamId::new("breathiness"),
        range: cantor_core::TickRange {
            start: 0,
            end: 7680,
        },
        points: vec![(0, -6.0), (3840, 4.0), (7679, -2.0)],
        interp: cantor_core::Interp::Linear,
    });
    e.run("expr.vibrato", None);
    e
}

fn capture(theme: Theme) -> Option<image::RgbaImage> {
    let platform = gpui_kit::platform::current_platform(true);
    let mut cx = HeadlessAppContext::with_platform(platform.text_system(), Arc::new(()), || {
        gpui_kit::platform::current_headless_renderer()
    });
    let window = cx
        .open_window(size(px(1440.0), px(900.0)), |window, cx| {
            gpui_kit::init(cx);
            cx.new(|cx| CantorWindow::new(editor(), theme, window, cx))
        })
        .ok()?;
    cx.run_until_parked();
    match cx.capture_screenshot(window.into()) {
        Ok(img) => Some(img),
        Err(e) => {
            eprintln!("skipping screenshots: no headless renderer ({e})");
            None
        }
    }
}

fn check(name: &str, img: image::RgbaImage) {
    let path = screens_dir().join(name);
    if std::env::var_os("UPDATE_SCREENS").is_some() {
        std::fs::create_dir_all(screens_dir()).unwrap();
        img.save(&path).unwrap();
        return;
    }
    let Ok(expected) = image::open(&path).map(|i| i.to_rgba8()) else {
        panic!("{} is missing; run with UPDATE_SCREENS=1", path.display());
    };
    assert_eq!(expected.dimensions(), img.dimensions(), "{name} size");
    let diff: f64 = expected
        .pixels()
        .zip(img.pixels())
        .map(|(a, b)| {
            a.0.iter()
                .zip(b.0.iter())
                .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs())
                .sum::<f64>()
        })
        .sum::<f64>()
        / (expected.width() * expected.height() * 4) as f64;
    // Text antialiasing differs slightly between machines; layout changes
    // move far more pixels than this.
    assert!(
        diff < 3.0,
        "{name} differs from the committed screenshot (mean abs diff {diff:.2})"
    );
}

fn main() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipping screenshots: references are rendered with the macOS text system");
        return;
    }
    for (name, theme) in [("editor-dark.png", DARK), ("editor-light.png", LIGHT)] {
        match capture(theme) {
            Some(img) => check(name, img),
            None => return,
        }
    }
    println!("screens: ok");
}
