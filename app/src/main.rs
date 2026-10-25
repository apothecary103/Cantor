//! `cantor-app [song.cantor | --demo N | --bench [FRAMES]]`

use cantor_app::state::Editor;
use cantor_app::view::{Bench, CantorWindow, default_theme};
use cantor_server::{Engine, EngineConfig};
use gpui_kit::*;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let engine = Arc::new(Engine::new(EngineConfig::from_env()));
    let (project, bench) = match args.first().map(String::as_str) {
        Some("--bench") => {
            let frames = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(600);
            (cantor_app::demo::project(2000), Some(frames))
        }
        Some("--demo") => (
            cantor_app::demo::project(args.get(1).and_then(|a| a.parse().ok()).unwrap_or(200)),
            None,
        ),
        Some(path) => match cantor_io::project_file::load(std::path::Path::new(path)) {
            Ok(l) => (l.project, None),
            Err(e) => {
                eprintln!("cantor-app: {e}");
                std::process::exit(1);
            }
        },
        None => (cantor_app::demo::project(120), None),
    };
    let singer = project
        .tracks
        .first()
        .map(|t| t.singer.id.clone())
        .unwrap_or_default();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1440.0), px(900.0)),
                cx,
            ))),
            ..WindowOptions::default()
        };
        gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| {
                let editor = Editor::new(engine.clone(), project.clone());
                let e = engine.clone();
                let mut view = CantorWindow::new(editor, default_theme(), window, cx)
                    .with_phonemizer(Arc::new(move |lyric: &str, lang: Option<&str>| {
                        e.phonemes(&singer, lyric, lang)
                    }));
                if let Some(frames) = bench {
                    view = view.with_bench(Bench::new(frames));
                }
                view
            })
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
