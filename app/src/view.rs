//! The main window: piano roll with lanes, side panels and the command
//! palette. Drawing uses quads and paths only (upstream GPUI has no custom
//! shaders); every edit goes through [`Editor::apply`] as an Op.

use crate::actions::{self, REGISTRY};
use crate::state::Editor;
use crate::theme::{DARK, Theme};
use cantor_core::{
    Interp, NoteExprPatch, NoteId, Op, ParamId, PhonemeAlphabet, PhonemeOverride, Portamento,
    Project, Scale, TickRange, Vibrato,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

const KEYS_W: f32 = 60.0;
const RULER_H: f32 = 22.0;
const HEAT_H: f32 = 6.0;
const TAKES_H: f32 = 22.0;
const PHON_H: f32 = 26.0;
const PARAM_H: f32 = 84.0;
const SIDE_W: f32 = 300.0;

fn c(rgb_: u32) -> Hsla {
    rgb(rgb_).into()
}

fn ca(rgb_: u32, alpha: f32) -> Hsla {
    let mut h: Hsla = rgb(rgb_).into();
    h.a = alpha;
    h
}

/// What a mouse drag is doing.
#[derive(Clone, Debug)]
enum Drag {
    /// Moving selected notes.
    Notes {
        start: Point<Pixels>,
        dt: i64,
        dcents: f64,
    },
    /// Changing a note's vibrato depth.
    Vibrato {
        note: NoteId,
        start_y: f32,
        depth: f64,
    },
    /// Changing a note's portamento length.
    Portamento {
        note: NoteId,
        start_x: f32,
        length_ms: f64,
    },
    /// Moving a phoneme boundary (pinning a consonant's length).
    Phoneme {
        note: NoteId,
        index: usize,
        start_x: f32,
        length_ms: f64,
    },
    /// Drawing a curve in the parameter lane.
    Curve { points: Vec<(i64, f64)> },
}

/// Text prompt shown over the editor.
#[derive(Clone, Debug)]
enum Prompt {
    /// Command palette with a filter query and highlighted row.
    Palette { query: String, selected: usize },
    /// Typing text for an action (lyrics, a style prompt).
    Text {
        action: &'static str,
        title: &'static str,
        text: String,
    },
}

/// Scripted scroll/zoom benchmark state.
#[derive(Debug)]
pub struct Bench {
    frames: usize,
    last_frame: Option<Instant>,
    intervals: Vec<f64>,
    done: usize,
    started: Option<Instant>,
    render_start: Rc<Cell<Option<Instant>>>,
    samples: Rc<RefCell<Vec<f64>>>,
}

impl Bench {
    /// A benchmark of `frames` frames.
    pub fn new(frames: usize) -> Self {
        Self {
            frames,
            last_frame: None,
            intervals: Vec::new(),
            done: 0,
            started: None,
            render_start: Rc::new(Cell::new(None)),
            samples: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

/// Dictionary phonemes for a lyric and language, from the track's singer.
pub type Phonemizer = Arc<dyn Fn(&str, Option<&str>) -> Option<Vec<String>> + Send + Sync>;

/// Per-frame text shaping cache (lyrics and phonemes repeat a lot).
type TextCache = Rc<RefCell<HashMap<(String, u32, u32), ShapedLine>>>;

/// The main window view.
pub struct CantorWindow {
    editor: Editor,
    theme: Theme,
    focus: FocusHandle,
    drag: Option<Drag>,
    prompt: Option<Prompt>,
    roll_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    phon_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    param_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    text_cache: TextCache,
    rendering: bool,
    bench: Option<Bench>,
    phonemizer: Option<Phonemizer>,
}

impl std::fmt::Debug for CantorWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CantorWindow")
            .field("editor", &self.editor)
            .finish_non_exhaustive()
    }
}

impl CantorWindow {
    /// Creates the view.
    pub fn new(editor: Editor, theme: Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            editor,
            theme,
            focus,
            drag: None,
            prompt: None,
            roll_bounds: Rc::new(Cell::new(None)),
            phon_bounds: Rc::new(Cell::new(None)),
            param_bounds: Rc::new(Cell::new(None)),
            text_cache: Rc::new(RefCell::new(HashMap::new())),
            rendering: false,
            bench: None,
            phonemizer: None,
        }
    }

    /// Runs the scripted benchmark instead of waiting for input.
    pub fn with_bench(mut self, bench: Bench) -> Self {
        self.bench = Some(bench);
        self
    }

    /// Supplies dictionary phonemes for the phoneme lane.
    pub fn with_phonemizer(mut self, f: Phonemizer) -> Self {
        self.phonemizer = Some(f);
        self
    }

    /// The editor model.
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The editor model, mutably.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn local(bounds: &Rc<Cell<Option<Bounds<Pixels>>>>, p: Point<Pixels>) -> Option<(f32, f32)> {
        let b = bounds.get()?;
        let x = f32::from(p.x - b.origin.x);
        let y = f32::from(p.y - b.origin.y);
        (x >= 0.0 && y >= 0.0 && x <= f32::from(b.size.width) && y <= f32::from(b.size.height))
            .then_some((x, y))
    }

    fn run_action(&mut self, id: &str, text: Option<&str>, cx: &mut Context<Self>) {
        match id {
            "view.palette" => {
                self.prompt = Some(Prompt::Palette {
                    query: String::new(),
                    selected: 0,
                });
            }
            "lyric.enter" if text.is_none() => {
                self.prompt = Some(Prompt::Text {
                    action: "lyric.enter",
                    title: "Lyrics (space separates syllables)",
                    text: String::new(),
                });
            }
            "style.apply" if text.is_none() => {
                self.prompt = Some(Prompt::Text {
                    action: "style.apply",
                    title: "Style (e.g. breathier, more intimate)",
                    text: String::new(),
                });
            }
            "render.preview" => self.start_render(cx),
            "bridge.publish" => self.start_bridge_publish(cx),
            "macro.toggle" => {
                if let Some(script) = self.editor.toggle_recording() {
                    let path = std::env::temp_dir().join("cantor-macro.py");
                    match std::fs::write(&path, script) {
                        Ok(()) => self.editor.status = format!("macro saved to {}", path.display()),
                        Err(e) => self.editor.status = format!("saving macro failed: {e}"),
                    }
                }
            }
            _ => {
                self.editor.run(id, text);
            }
        }
        cx.notify();
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        if self.rendering {
            return;
        }
        self.rendering = true;
        self.editor.status = "rendering…".into();
        let engine = self.editor.engine().clone();
        let id = self.editor.project_id().to_owned();
        let task = cx.background_executor().spawn(async move {
            engine.render(&id, None, cantor_render::render::Quality::Preview)
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                view.rendering = false;
                match result {
                    Ok(summary) => {
                        view.editor.status = format!("rendered {:.1} s", summary.seconds);
                        view.editor.last_render = Some(summary);
                    }
                    Err(e) => view.editor.status = format!("render failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Renders and publishes to the DAW bridge's default channel.
    fn start_bridge_publish(&mut self, cx: &mut Context<Self>) {
        if self.rendering {
            return;
        }
        self.rendering = true;
        self.editor.status = "sending to the DAW…".into();
        let engine = self.editor.engine().clone();
        let id = self.editor.project_id().to_owned();
        let task = cx.background_executor().spawn(async move {
            engine.bridge_publish(&id, None, cantor_render::render::Quality::Preview)
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                view.rendering = false;
                view.editor.status = match result {
                    Ok(p) => format!(
                        "sent {:.1} s to DAW channel `{}` (generation {})",
                        p.seconds, p.channel, p.generation
                    ),
                    Err(e) => format!("sending to the DAW failed: {e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn keystroke_id(ev: &KeyDownEvent) -> String {
        let k = &ev.keystroke;
        let mut s = String::new();
        if k.modifiers.platform {
            s.push_str("cmd-");
        }
        if k.modifiers.control {
            s.push_str("ctrl-");
        }
        if k.modifiers.alt {
            s.push_str("alt-");
        }
        if k.modifiers.shift {
            s.push_str("shift-");
        }
        s.push_str(&k.key);
        s
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = Self::keystroke_id(ev);
        if let Some(prompt) = &mut self.prompt {
            let typed = ev
                .keystroke
                .key_char
                .clone()
                .filter(|_| !ev.keystroke.modifiers.platform);
            match prompt {
                Prompt::Palette { query, selected } => {
                    let matches = palette_matches(query);
                    match key.as_str() {
                        "escape" => self.prompt = None,
                        "enter" => {
                            let id = matches.get(*selected).map(|a| a.id);
                            self.prompt = None;
                            if let Some(id) = id {
                                self.run_action(id, None, cx);
                            }
                        }
                        "up" => *selected = selected.saturating_sub(1),
                        "down" => *selected = (*selected + 1).min(matches.len().saturating_sub(1)),
                        "backspace" => {
                            query.pop();
                            *selected = 0;
                        }
                        _ => {
                            if let Some(t) = typed {
                                query.push_str(&t);
                                *selected = 0;
                            }
                        }
                    }
                }
                Prompt::Text { action, text, .. } => match key.as_str() {
                    "escape" => self.prompt = None,
                    "enter" => {
                        let (a, t) = (*action, text.clone());
                        self.prompt = None;
                        self.run_action(a, Some(&t), cx);
                    }
                    "backspace" => {
                        text.pop();
                    }
                    _ => {
                        if let Some(t) = typed {
                            text.push_str(&t);
                        }
                    }
                },
            }
            cx.notify();
            return;
        }
        if let Some(a) = REGISTRY.iter().find(|a| a.keys.contains(&key.as_str())) {
            self.run_action(a.id, None, cx);
        }
    }

    fn on_roll_down(&mut self, ev: &MouseDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some((x, y)) = Self::local(&self.roll_bounds, ev.position) else {
            return;
        };
        if x < KEYS_W || y < RULER_H {
            return;
        }
        let (x, y) = ((x - KEYS_W) as f64, (y - RULER_H) as f64);
        let vp = self.editor.viewport;
        // Smart pitch handles of a single selected note take priority.
        if self.editor.selection.len() == 1 {
            let id = *self.editor.selection.iter().next().expect("one");
            if let Some((_, part, n)) = self.editor.project().note(id) {
                let sx = vp.x((part.start + n.start) as f64);
                let ex = vp.x((part.start + n.start + n.length) as f64);
                let ny = vp.y(n.pitch_cents);
                let vib = n.expr.vibrato.as_ref().map_or(30.0, |v| v.depth_cents);
                let vx = sx + (ex - sx) * 0.65;
                if (x - vx).abs() < 6.0 && (y - (ny - vib * vp.px_per_cent - 8.0)).abs() < 6.0 {
                    self.drag = Some(Drag::Vibrato {
                        note: id,
                        start_y: y as f32,
                        depth: vib,
                    });
                    return;
                }
                let plen = n.expr.portamento.length_ms.unwrap_or(120.0);
                if (x - (sx - 4.0)).abs() < 6.0 && (y - ny).abs() < 8.0 {
                    self.drag = Some(Drag::Portamento {
                        note: id,
                        start_x: x as f32,
                        length_ms: plen,
                    });
                    return;
                }
            }
        }
        let tick = vp.tick(x).round() as i64;
        self.editor.cursor = (tick, vp.cents(y));
        match self.editor.note_at(x, y) {
            Some(id) => {
                if ev.modifiers.shift {
                    if !self.editor.selection.remove(&id) {
                        self.editor.selection.insert(id);
                    }
                } else if !self.editor.selection.contains(&id) {
                    self.editor.selection = [id].into();
                }
                self.drag = Some(Drag::Notes {
                    start: ev.position,
                    dt: 0,
                    dcents: 0.0,
                });
            }
            None => {
                if ev.click_count >= 2 {
                    self.run_action("note.add", None, cx);
                } else if !ev.modifiers.shift {
                    self.editor.selection.clear();
                }
            }
        }
        cx.notify();
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let vp = self.editor.viewport;
        let grid = self.editor.grid.max(1);
        match &mut self.drag {
            Some(Drag::Notes { start, dt, dcents }) => {
                let dx = f64::from(f32::from(ev.position.x - start.x));
                let dy = f64::from(f32::from(ev.position.y - start.y));
                *dt = ((dx / vp.px_per_tick) / grid as f64).round() as i64 * grid;
                *dcents = -((dy / vp.px_per_cent) / 100.0).round() * 100.0;
            }
            Some(Drag::Vibrato { start_y, depth, .. }) => {
                if let Some((_, y)) = Self::local(&self.roll_bounds, ev.position) {
                    *depth = (*depth + f64::from(*start_y - (y - RULER_H)) / vp.px_per_cent)
                        .clamp(0.0, 300.0);
                    *start_y = y - RULER_H;
                }
            }
            Some(Drag::Portamento {
                start_x, length_ms, ..
            }) => {
                if let Some((x, _)) = Self::local(&self.roll_bounds, ev.position) {
                    let tempo = self.editor.project().tempo();
                    let ms_per_px = 1000.0 / (vp.px_per_tick * tempo.bpm_at(0) * 32.0);
                    *length_ms = (*length_ms - f64::from(x - KEYS_W - *start_x) * ms_per_px)
                        .clamp(10.0, 1000.0);
                    *start_x = x - KEYS_W;
                }
            }
            Some(Drag::Phoneme {
                start_x, length_ms, ..
            }) => {
                if let Some((x, _)) = Self::local(&self.phon_bounds, ev.position) {
                    let tempo = self.editor.project().tempo();
                    let ms_per_px = 1000.0 / (vp.px_per_tick * tempo.bpm_at(0) * 32.0);
                    *length_ms = (*length_ms - f64::from(x - KEYS_W - *start_x) * ms_per_px)
                        .clamp(10.0, 500.0);
                    *start_x = x - KEYS_W;
                }
            }
            Some(Drag::Curve { points }) => {
                if let Some((x, y)) = Self::local(&self.param_bounds, ev.position) {
                    let tick = vp.tick(f64::from(x - KEYS_W)).round() as i64;
                    let value = curve_value_at_y(&self.editor.param, y as f64);
                    if points.last().is_none_or(|(t, _)| tick > *t) {
                        points.push((tick, value));
                    }
                }
            }
            None => return,
        }
        cx.notify();
    }

    fn on_up(&mut self, _ev: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let op = match drag {
            Drag::Notes { dt, dcents, .. } if dt != 0 || dcents != 0.0 => Some(Op::MoveNotes {
                ids: self.editor.context().selection,
                dt,
                dcents,
                carry_curves: true,
            }),
            Drag::Vibrato { note, depth, .. } => {
                let base = self
                    .editor
                    .project()
                    .note(note)
                    .and_then(|(_, _, n)| n.expr.vibrato.clone())
                    .unwrap_or(Vibrato {
                        depth_cents: 0.0,
                        rate_hz: 5.5,
                        start: 0.3,
                        fade_in: 0.25,
                        fade_out: 0.15,
                        phase: 0.0,
                        drift: 0.5,
                    });
                Some(Op::SetNoteExpr {
                    id: note,
                    expr: NoteExprPatch {
                        vibrato: Some(Some(Vibrato {
                            depth_cents: depth.round(),
                            ..base
                        })),
                        ..Default::default()
                    },
                })
            }
            Drag::Portamento {
                note, length_ms, ..
            } => {
                let base = self
                    .editor
                    .project()
                    .note(note)
                    .map(|(_, _, n)| n.expr.portamento.clone())
                    .unwrap_or_default();
                Some(Op::SetNoteExpr {
                    id: note,
                    expr: NoteExprPatch {
                        portamento: Some(Portamento {
                            length_ms: Some(length_ms.round()),
                            ..base
                        }),
                        ..Default::default()
                    },
                })
            }
            Drag::Phoneme {
                note,
                index,
                length_ms,
                ..
            } => {
                let p = self.editor.project().clone();
                p.note(note).map(|(_, _, n)| {
                    let mut phones = self.phonemes_of(
                        n.phonemes.as_deref(),
                        &n.lyric,
                        n.lang.as_ref().map(|l| l.0.as_str()),
                    );
                    if let Some(ph) = phones.get_mut(index) {
                        ph.length_ms = Some(length_ms.round());
                    }
                    Op::SetPhonemes {
                        id: note,
                        phonemes: Some(phones),
                    }
                })
            }
            Drag::Curve { points } if points.len() >= 2 => Some(Op::SetCurve {
                track: self.editor.track,
                param: ParamId::new(self.editor.param.clone()),
                range: TickRange {
                    start: points[0].0,
                    end: points[points.len() - 1].0 + 1,
                },
                points,
                interp: Interp::Linear,
            }),
            _ => None,
        };
        if let Some(op) = op {
            let _ = self.editor.apply(op);
        }
        cx.notify();
    }

    fn phonemes_of(
        &self,
        overrides: Option<&[PhonemeOverride]>,
        lyric: &str,
        lang: Option<&str>,
    ) -> Vec<PhonemeOverride> {
        if let Some(o) = overrides {
            return o.to_vec();
        }
        let symbols = self
            .phonemizer
            .as_ref()
            .and_then(|f| f(lyric, lang))
            .unwrap_or_else(|| vec![lyric.to_owned()]);
        symbols
            .into_iter()
            .map(|s| PhonemeOverride {
                symbol: s,
                alphabet: PhonemeAlphabet::Native,
                offset_ms: None,
                length_ms: None,
            })
            .collect()
    }

    fn on_phon_down(&mut self, ev: &MouseDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some((x, _)) = Self::local(&self.phon_bounds, ev.position) else {
            return;
        };
        let x = (x - KEYS_W) as f64;
        let vp = self.editor.viewport;
        let p = self.editor.project().clone();
        let Some(track) = p.track(self.editor.track) else {
            return;
        };
        for part in &track.parts {
            for n in &part.notes {
                let sx = vp.x((part.start + n.start) as f64);
                let phones = self.phonemes_of(
                    n.phonemes.as_deref(),
                    &n.lyric,
                    n.lang.as_ref().map(|l| l.0.as_str()),
                );
                // Leading consonants sit before the onset; their start
                // boundaries are draggable.
                let layout = phoneme_layout(&phones, sx, vp, &p);
                for (i, (bx, _)) in layout.iter().enumerate() {
                    if i < phones.len().saturating_sub(1) && (x - bx).abs() < 5.0 {
                        let ms = phones[i].length_ms.unwrap_or(80.0);
                        self.drag = Some(Drag::Phoneme {
                            note: n.id,
                            index: i,
                            start_x: x as f32,
                            length_ms: ms,
                        });
                        cx.notify();
                        return;
                    }
                }
            }
        }
    }

    fn on_param_down(&mut self, ev: &MouseDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some((x, y)) = Self::local(&self.param_bounds, ev.position) else {
            return;
        };
        let tick = self.editor.viewport.tick(f64::from(x - KEYS_W)).round() as i64;
        self.drag = Some(Drag::Curve {
            points: vec![(tick, curve_value_at_y(&self.editor.param, y as f64))],
        });
        cx.notify();
    }

    fn on_takes_down(&mut self, ev: &MouseDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        // Clicking a take box selects that take for its phrase.
        let Some(b) = self.roll_bounds.get() else {
            return;
        };
        let x = f64::from(f32::from(ev.position.x - b.origin.x) - KEYS_W);
        let vp = self.editor.viewport;
        let p = self.editor.project().clone();
        let Some(track) = p.track(self.editor.track) else {
            return;
        };
        for span in cantor_core::phrase_spans(&p, track) {
            let (sx, ex) = (vp.x(span.start as f64), vp.x(span.end as f64));
            if x < sx || x > ex {
                continue;
            }
            let lane = p.takes.get(&span.id);
            let n = lane.map_or(0, |l| l.takes.len()) + 1;
            let slot = (((x - sx) / (ex - sx)) * n as f64).floor() as usize;
            let take = if slot == 0 {
                None
            } else {
                lane.and_then(|l| l.takes.get(slot - 1)).map(|t| t.id)
            };
            let _ = self.editor.apply(Op::SelectTake {
                phrase: span.id,
                take,
            });
            cx.notify();
            return;
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let d = ev.delta.pixel_delta(px(20.0));
        let (dx, dy) = (f64::from(f32::from(d.x)), f64::from(f32::from(d.y)));
        if ev.modifiers.platform || ev.modifiers.control {
            let x = Self::local(&self.roll_bounds, ev.position)
                .map_or(0.0, |(x, _)| f64::from(x - KEYS_W));
            self.editor.viewport.zoom_x((dy / 200.0).exp(), x);
        } else if ev.modifiers.shift {
            self.editor.viewport.scroll(-dy, 0.0);
        } else {
            self.editor.viewport.scroll(-dx, -dy);
        }
        cx.notify();
    }

    fn bench_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = &mut self.bench else { return };
        if b.started.is_none() {
            b.started = Some(Instant::now());
        }
        let now = Instant::now();
        if let Some(prev) = b.last_frame.replace(now) {
            b.intervals
                .push(now.duration_since(prev).as_secs_f64() * 1000.0);
        }
        b.render_start.set(Some(now));
        if b.done >= b.frames {
            let mut s = b.samples.borrow().clone();
            s.sort_by(f64::total_cmp);
            let median = s.get(s.len() / 2).copied().unwrap_or(f64::NAN);
            let p95 = s.get(s.len() * 95 / 100).copied().unwrap_or(f64::NAN);
            let wall = b.started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            let mut iv = b.intervals.clone();
            iv.sort_by(f64::total_cmp);
            let interval_median = iv.get(iv.len() / 2).copied().unwrap_or(f64::NAN);
            println!(
                "{}",
                serde_json::json!({
                    "frames": s.len(),
                    "notes": self.editor.project().notes().count(),
                    "frame_ms_median": median,
                    "frame_ms_p95": p95,
                    "fps_equivalent_median": 1000.0 / median,
                    "frame_interval_ms_median": interval_median,
                    "presented_fps_median": 1000.0 / interval_median,
                    "presented_fps": s.len() as f64 / wall,
                })
            );
            cx.quit();
            return;
        }
        // Scripted motion: scroll right, oscillate zoom, drift vertically.
        let i = b.done as f64;
        self.editor.viewport.scroll(14.0, (i / 30.0).sin() * 6.0);
        let factor = if (b.done / 60) % 2 == 0 {
            1.01
        } else {
            1.0 / 1.01
        };
        self.editor.viewport.zoom_x(factor, 400.0);
        b.done += 1;
        window.request_animation_frame();
    }
}

fn curve_value_at_y(param: &str, y: f64) -> f64 {
    let (lo, hi) = match cantor_core::params::lookup(param) {
        Some(d) => (-d.max_delta() / 4.0, d.max_delta() / 4.0),
        None => (-1.0, 1.0),
    };
    let t = (1.0 - y / f64::from(PARAM_H)).clamp(0.0, 1.0);
    lo + (hi - lo) * t
}

fn palette_matches(query: &str) -> Vec<&'static actions::ActionDef> {
    let q = query.to_lowercase();
    REGISTRY
        .iter()
        .filter(|a| {
            q.split_whitespace()
                .all(|w| a.name.to_lowercase().contains(w) || a.id.contains(w))
        })
        .collect()
}

/// Phoneme boundary positions: leading consonants before the note start,
/// the rest from the note start on. Returns (start x, end x) per phoneme.
fn phoneme_layout(
    phones: &[PhonemeOverride],
    note_x: f64,
    vp: crate::state::Viewport,
    p: &Project,
) -> Vec<(f64, f64)> {
    let px_per_ms = vp.px_per_tick * p.tempo().bpm_at(0) * 32.0 / 1000.0;
    let vowels = ["a", "e", "i", "o", "u", "v", "y"];
    let nucleus = phones
        .iter()
        .position(|ph| {
            let s = ph.symbol.rsplit('/').next().unwrap_or("");
            vowels.iter().any(|v| s.starts_with(v)) || s == "SP" || s == "AP"
        })
        .unwrap_or(0);
    let mut out = vec![(0.0, 0.0); phones.len()];
    let mut x = note_x;
    for i in (0..nucleus).rev() {
        let w = phones[i].length_ms.unwrap_or(80.0) * px_per_ms;
        out[i] = (x - w, x);
        x -= w;
    }
    let mut x = note_x;
    for (i, ph) in phones.iter().enumerate().skip(nucleus) {
        let w = ph.length_ms.unwrap_or(f64::NAN);
        let end = if w.is_nan() {
            x + 1e6
        } else {
            x + w * px_per_ms
        };
        out[i] = (x, end);
        x = end;
    }
    out
}

/// Pitch classes of a scale.
fn scale_classes(scale: Scale) -> &'static [u8] {
    match scale {
        Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
        Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
        Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
        Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
        Scale::Lydian => &[0, 2, 4, 6, 7, 9, 11],
        Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
        Scale::Locrian => &[0, 1, 3, 5, 6, 8, 10],
        Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    }
}

/// Paints shaped text, cached across frames.
fn text(
    window: &mut Window,
    cx: &mut App,
    cache: &TextCache,
    s: &str,
    size: f32,
    color: u32,
    at: Point<Pixels>,
) {
    let key = (s.to_owned(), size.to_bits(), color);
    let line = cache.borrow().get(&key).cloned();
    let line = match line {
        Some(l) => l,
        None => {
            let font = window.text_style().font();
            let run = TextRun {
                len: s.len(),
                font,
                color: c(color),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let l = window
                .text_system()
                .shape_line(s.to_owned().into(), px(size), &[run], None);
            cache.borrow_mut().insert(key, l.clone());
            l
        }
    };
    let _ = line.paint(at, px(size * 1.3), TextAlign::Left, None, window, cx);
}

fn rect(window: &mut Window, b: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(fill(b, color));
}

fn bounds(x: f64, y: f64, w: f64, h: f64, origin: Point<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(origin.x + px(x as f32), origin.y + px(y as f32)),
        size: size(px(w.max(0.0) as f32), px(h.max(0.0) as f32)),
    }
}

/// Everything a frame's canvases need, captured by value.
#[derive(Clone)]
struct Frame {
    project: Arc<Project>,
    vp: crate::state::Viewport,
    theme: Theme,
    selection: std::collections::BTreeSet<NoteId>,
    track: cantor_core::TrackId,
    drag_offset: (i64, f64),
    heat: HashMap<NoteId, f64>,
    param: String,
    curve_preview: Vec<(i64, f64)>,
    phones: HashMap<NoteId, Vec<PhonemeOverride>>,
}

impl Render for CantorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.bench_step(window, cx);
        let t = self.theme;
        let project = self.editor.project().clone();
        let heat: HashMap<NoteId, f64> = self
            .editor
            .last_render
            .as_ref()
            .and_then(|r| r.analysis.as_ref())
            .map(|a| {
                a.notes()
                    .filter_map(|n| n.pitch_error_p95.map(|e| (n.id, e)))
                    .collect()
            })
            .unwrap_or_default();
        let mut phones = HashMap::new();
        if let Some(track) = project.track(self.editor.track) {
            let lo = self.editor.viewport.tick(-200.0);
            let hi = self.editor.viewport.tick(4000.0);
            for part in &track.parts {
                for n in &part.notes {
                    let s = (part.start + n.start) as f64;
                    if s + n.length as f64 >= lo && s <= hi {
                        phones.insert(
                            n.id,
                            self.phonemes_of(
                                n.phonemes.as_deref(),
                                &n.lyric,
                                n.lang.as_ref().map(|l| l.0.as_str()),
                            ),
                        );
                    }
                }
            }
        }
        let frame = Frame {
            project: project.clone(),
            vp: self.editor.viewport,
            theme: t,
            selection: self.editor.selection.clone(),
            track: self.editor.track,
            drag_offset: match &self.drag {
                Some(Drag::Notes { dt, dcents, .. }) => (*dt, *dcents),
                _ => (0, 0.0),
            },
            heat,
            param: self.editor.param.clone(),
            curve_preview: match &self.drag {
                Some(Drag::Curve { points }) => points.clone(),
                _ => Vec::new(),
            },
            phones,
        };
        let summary = self
            .editor
            .last_render
            .as_ref()
            .and_then(|r| r.analysis.as_ref())
            .and_then(|a| a.tracks.first())
            .and_then(|tr| tr.pitch.p95_cents)
            .map_or(String::new(), |p| format!("pitch p95 {p:.0} cents"));

        let toolbar = div()
            .h(px(32.0))
            .px_3()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .bg(c(t.surface))
            .border_b_1()
            .border_color(c(t.border))
            .child(
                div()
                    .text_color(c(t.text))
                    .child(project.meta.title.clone()),
            )
            .child(
                div()
                    .text_color(c(t.text_quiet))
                    .child(format!("rev {}", self.editor.revision().0)),
            )
            .child(
                div()
                    .text_color(c(t.text_quiet))
                    .child(format!("grid 1/{}", 1920 * 4 / self.editor.grid.max(1))),
            )
            .child(div().text_color(c(t.text_quiet)).child(summary))
            .child(
                div()
                    .text_color(c(if self.editor.recording.is_some() {
                        t.heat
                    } else {
                        t.text_quiet
                    }))
                    .child(if self.editor.recording.is_some() {
                        "● recording macro"
                    } else {
                        ""
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_color(c(t.text_quiet))
                    .child(self.editor.status.clone()),
            );

        let roll = {
            let f = frame.clone();
            let cache = self.text_cache.clone();
            let bounds_cell = self.roll_bounds.clone();
            canvas(
                move |b, _, _| bounds_cell.set(Some(b)),
                move |b, _, window, cx| paint_roll(&f, b, window, cx, &cache),
            )
            .size_full()
        };
        let lane = |h: f32, kind: u8, bounds_cell: Option<Rc<Cell<Option<Bounds<Pixels>>>>>| {
            let f = frame.clone();
            let cache = self.text_cache.clone();
            canvas(
                move |b, _, _| {
                    if let Some(cell) = &bounds_cell {
                        cell.set(Some(b));
                    }
                },
                move |b, _, window, cx| match kind {
                    0 => paint_heat(&f, b, window),
                    1 => paint_takes(&f, b, window, cx, &cache),
                    2 => paint_phonemes(&f, b, window, cx, &cache),
                    _ => paint_param(&f, b, window, cx, &cache),
                },
            )
            .w_full()
            .h(px(h))
        };

        let history = self.editor.history();
        let side = div()
            .w(px(SIDE_W))
            .h_full()
            .flex()
            .flex_col()
            .bg(c(t.surface))
            .border_l_1()
            .border_color(c(t.border))
            .child(panel_title("History", t))
            .child(
                div()
                    .id("history")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .children(history.iter().rev().take(200).map(|e| {
                        let node = e.node.0;
                        div()
                            .id(("h", node as usize))
                            .py(px(2.0))
                            .px_1()
                            .rounded(px(t.radius))
                            .when(e.current, |d| d.bg(ca(t.accent, 0.22)))
                            .text_color(c(if e.current { t.text } else { t.text_quiet }))
                            .child(format!(
                                "{} {}{}",
                                e.revision.0,
                                e.label,
                                if e.checkpoints.is_empty() {
                                    String::new()
                                } else {
                                    format!("  ◆ {}", e.checkpoints.join(", "))
                                }
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.editor.restore(&format!("#{node}"));
                                cx.notify();
                            }))
                    })),
            )
            .child(panel_title("Script Log", t))
            .child(
                div()
                    .id("script-log")
                    .h(px(240.0))
                    .overflow_y_scroll()
                    .px_2()
                    .text_size(px(11.0))
                    .children(self.editor.script_log.iter().rev().take(200).map(|e| {
                        div()
                            .py(px(2.0))
                            .text_color(c(t.text_quiet))
                            .child(e.python.clone())
                    })),
            );

        let editor_col = div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(roll)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_roll_down)),
            )
            .child(lane(HEAT_H, 0, None))
            .child(
                div()
                    .child(lane(TAKES_H, 1, None))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_takes_down)),
            )
            .child(
                div()
                    .child(lane(PHON_H, 2, Some(self.phon_bounds.clone())))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_phon_down)),
            )
            .child(
                div()
                    .child(lane(PARAM_H, 3, Some(self.param_bounds.clone())))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_param_down)),
            );

        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(t.background))
            .text_color(c(t.text))
            .text_size(px(12.0))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(editor_col)
                    .child(side),
            );

        if let Some(prompt) = &self.prompt {
            root = root.child(prompt_overlay(prompt, t));
        }
        // Painted last: the end of this frame's CPU work (layout + paint of
        // every element), for the benchmark.
        if let Some(b) = &self.bench {
            let (start, samples) = (b.render_start.clone(), b.samples.clone());
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, _, _| {
                        if let Some(s) = start.take() {
                            samples
                                .borrow_mut()
                                .push(s.elapsed().as_secs_f64() * 1000.0);
                        }
                    },
                )
                .absolute()
                .size_0(),
            );
        }
        root
    }
}

fn panel_title(title: &str, t: Theme) -> Div {
    div()
        .px_2()
        .py_1()
        .text_size(px(11.0))
        .text_color(c(t.text_quiet))
        .border_b_1()
        .border_color(c(t.border))
        .child(title.to_owned())
}

fn prompt_overlay(prompt: &Prompt, t: Theme) -> Div {
    let (title, query, rows): (&str, String, Vec<(String, String, bool)>) = match prompt {
        Prompt::Palette { query, selected } => (
            "Command",
            query.clone(),
            palette_matches(query)
                .into_iter()
                .take(12)
                .enumerate()
                .map(|(i, a)| {
                    (
                        a.name.to_owned(),
                        a.keys.first().copied().unwrap_or("").to_owned(),
                        i == *selected,
                    )
                })
                .collect(),
        ),
        Prompt::Text { title, text, .. } => (*title, text.clone(), Vec::new()),
    };
    div()
        .absolute()
        .top(px(64.0))
        .left(px(160.0))
        .w(px(520.0))
        .bg(c(t.surface))
        .border_1()
        .border_color(c(t.accent))
        .rounded(px(t.radius))
        .flex()
        .flex_col()
        .child(
            div()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(c(t.border))
                .child(format!("{title}: {query}▏")),
        )
        .children(rows.into_iter().map(|(name, key, selected)| {
            div()
                .px_3()
                .py_1()
                .flex()
                .flex_row()
                .when(selected, |d| d.bg(ca(t.accent, 0.22)))
                .child(div().flex_1().child(name))
                .child(div().text_color(c(t.text_quiet)).child(key))
        }))
}

fn track_color(f: &Frame) -> u32 {
    let i = f
        .project
        .tracks
        .iter()
        .position(|t| t.id == f.track)
        .unwrap_or(0);
    f.theme.tracks[i % f.theme.tracks.len()]
}

fn paint_roll(f: &Frame, b: Bounds<Pixels>, window: &mut Window, cx: &mut App, cache: &TextCache) {
    let t = f.theme;
    let (w, h) = (
        f64::from(f32::from(b.size.width)),
        f64::from(f32::from(b.size.height)),
    );
    let o = b.origin;
    let kx = f64::from(KEYS_W);
    let ry = f64::from(RULER_H);
    let vp = f.vp;
    rect(window, b, c(t.lane));

    // Rows: in-scale rows lighter, out-of-scale darker (key map at view start).
    let key = f
        .project
        .key_map
        .iter()
        .rev()
        .find(|k| k.tick as f64 <= vp.tick0.max(0.0))
        .or(f.project.key_map.first())
        .copied();
    let lo = vp.cents(h - ry).floor() as i64 / 100;
    let hi = vp.cents(0.0).ceil() as i64 / 100 + 1;
    for semi in lo..=hi {
        let y0 = vp.y((semi * 100 + 50) as f64) + ry;
        let rh = 100.0 * vp.px_per_cent;
        let pc = semi.rem_euclid(12) as u8;
        let in_scale =
            key.is_none_or(|k| scale_classes(k.scale).contains(&((pc + 12 - k.tonic) % 12)));
        if !in_scale {
            rect(window, bounds(kx, y0, w - kx, rh, o), c(t.lane_alt));
        }
        let black = [1, 3, 6, 8, 10].contains(&pc);
        rect(
            window,
            bounds(0.0, y0, kx, rh, o),
            c(if black { t.background } else { t.surface }),
        );
        if pc == 0 && rh > 7.0 {
            text(
                window,
                cx,
                cache,
                &format!("C{}", semi / 12 - 1),
                9.0,
                t.text_quiet,
                point(o.x + px(4.0), o.y + px((y0 + rh / 2.0 - 6.0) as f32)),
            );
        }
    }

    // Beat and bar lines; sections in the ruler.
    rect(window, bounds(kx, 0.0, w - kx, ry, o), c(t.surface));
    let meter = f.project.meter();
    let tick_lo = vp.tick(0.0).max(0.0) as i64;
    let tick_hi = vp.tick(w - kx) as i64;
    let (first_bar, _) = meter.tick_to_bar(tick_lo);
    let mut bar = first_bar;
    loop {
        let start = meter.bar_to_tick(bar);
        if start > tick_hi {
            break;
        }
        let next = meter.bar_to_tick(bar + 1);
        let beats = f
            .project
            .time_sig_map
            .iter()
            .rev()
            .find(|s| s.bar <= bar)
            .map_or(4, |s| s.numerator) as i64;
        for k in 0..beats {
            let bt = start + (next - start) * k / beats;
            let x = vp.x(bt as f64) + kx;
            if x > kx {
                rect(
                    window,
                    bounds(x, ry, 1.0, h - ry, o),
                    c(if k == 0 { t.grid_strong } else { t.grid }),
                );
            }
        }
        let x = vp.x(start as f64) + kx;
        if x > kx {
            text(
                window,
                cx,
                cache,
                &format!("{}", bar + 1),
                10.0,
                t.text_quiet,
                point(o.x + px(x as f32 + 3.0), o.y + px(4.0)),
            );
        }
        bar += 1;
    }
    for s in &f.project.sections {
        let (x0, x1) = (vp.x(s.start as f64) + kx, vp.x(s.end as f64) + kx);
        if x1 < kx || x0 > w {
            continue;
        }
        rect(
            window,
            bounds(x0.max(kx), ry - 3.0, (x1 - x0.max(kx)).max(0.0), 2.0, o),
            c(t.grid_strong),
        );
        text(
            window,
            cx,
            cache,
            &s.name,
            10.0,
            t.text_quiet,
            point(o.x + px(x0.max(kx) as f32 + 4.0), o.y + px(4.0)),
        );
    }

    // Notes of all tracks; the focused track on top. Lyrics are painted
    // last, above the pitch curves.
    let mut labels: Vec<(String, Point<Pixels>)> = Vec::new();
    let mut tracks: Vec<_> = f.project.tracks.iter().enumerate().collect();
    tracks.sort_by_key(|(_, tr)| tr.id == f.track);
    for (ti, track) in tracks {
        let focused = track.id == f.track;
        let color = t.tracks[ti % t.tracks.len()];
        for part in &track.parts {
            for n in &part.notes {
                let sel = focused && f.selection.contains(&n.id);
                let (dt, dc) = if sel { f.drag_offset } else { (0, 0.0) };
                let s = (part.start + n.start + dt) as f64;
                let x0 = vp.x(s) + kx;
                let x1 = vp.x(s + n.length as f64) + kx;
                if x1 < kx || x0 > w {
                    continue;
                }
                let y = vp.y(n.pitch_cents + dc + 50.0) + ry;
                let nh = (100.0 * vp.px_per_cent).max(3.0);
                if y + nh < ry || y > h {
                    continue;
                }
                let nb = bounds(x0.max(kx), y, (x1 - x0.max(kx) - 1.0).max(1.0), nh - 1.0, o);
                let fill_c = if focused {
                    ca(color, 0.85)
                } else {
                    ca(color, 0.28)
                };
                let edge = if sel {
                    c(t.accent)
                } else {
                    ca(0xffffff, if focused { 0.22 } else { 0.08 })
                };
                window.paint_quad(quad(
                    nb,
                    px(t.radius.min(2.0)),
                    fill_c,
                    px(1.0),
                    edge,
                    BorderStyle::default(),
                ));
                if focused && x1 - x0 > 18.0 && nh >= 12.0 {
                    labels.push((
                        n.lyric.clone(),
                        point(nb.origin.x + px(4.0), nb.origin.y + px(0.0)),
                    ));
                }
            }
        }
        if !focused {
            continue;
        }
        // Pitch curves inside each note: model prediction faint, user
        // delta solid, drawn as offsets from the note's pitch where an edit
        // exists (no lines between notes).
        if let Some(curve) = track.curves.get("pitch") {
            let step = (3.0 / vp.px_per_tick).max(1.0) as i64;
            for part in &track.parts {
                for n in &part.notes {
                    let s0 = part.start + n.start;
                    let s1 = s0 + n.length;
                    let ny = vp.y(n.pitch_cents) + ry;
                    if (s1 as f64) < vp.tick(0.0)
                        || s0 as f64 > vp.tick(w - kx)
                        || ny < ry + 4.0
                        || ny > h
                    {
                        continue;
                    }
                    for (layer, alpha) in [
                        (&curve.model_prediction, t.model_curve_alpha),
                        (&curve.user_delta, 1.0),
                    ] {
                        let mut path = PathBuilder::stroke(px(t.curve_width));
                        let mut started = false;
                        let mut tick = s0;
                        while tick < s1 {
                            match layer.value_in_span(tick).filter(|v| v.abs() > 0.5) {
                                Some(v) => {
                                    let p = point(
                                        o.x + px((vp.x(tick as f64) + kx) as f32),
                                        o.y + px((vp.y(n.pitch_cents + v) + ry) as f32),
                                    );
                                    if started {
                                        path.line_to(p);
                                    } else {
                                        path.move_to(p);
                                        started = true;
                                    }
                                }
                                None if started => break,
                                None => {}
                            }
                            tick += step;
                        }
                        if started && let Ok(p) = path.build() {
                            window.paint_path(p, ca(t.text, alpha));
                        }
                    }
                }
            }
        }
        for (label, at) in labels.drain(..) {
            text(window, cx, cache, &label, 11.0, 0x111216, at);
        }
        // Smart pitch handles for a single selected note.
        if f.selection.len() == 1
            && let Some(id) = f.selection.iter().next()
            && let Some((_, part, n)) = f.project.note(*id)
        {
            let sx = vp.x((part.start + n.start) as f64) + kx;
            let ex = vp.x((part.start + n.start + n.length) as f64) + kx;
            let ny = vp.y(n.pitch_cents) + ry;
            let vib = n.expr.vibrato.as_ref().map_or(30.0, |v| v.depth_cents);
            let vx = sx + (ex - sx) * 0.65;
            let hy = ny - vib * vp.px_per_cent - 8.0;
            window.paint_quad(quad(
                bounds(vx - 4.0, hy - 4.0, 8.0, 8.0, o),
                px(1.0),
                c(t.accent),
                px(0.0),
                c(t.accent),
                BorderStyle::default(),
            ));
            window.paint_quad(quad(
                bounds(sx - 8.0, ny - 4.0, 8.0, 8.0, o),
                px(1.0),
                c(t.background),
                px(1.0),
                c(t.accent),
                BorderStyle::default(),
            ));
        }
    }
}

fn paint_heat(f: &Frame, b: Bounds<Pixels>, window: &mut Window) {
    let t = f.theme;
    rect(window, b, c(t.surface));
    let Some(track) = f.project.track(f.track) else {
        return;
    };
    let kx = f64::from(KEYS_W);
    for part in &track.parts {
        for n in &part.notes {
            let Some(err) = f.heat.get(&n.id) else {
                continue;
            };
            let x0 = f.vp.x((part.start + n.start) as f64) + kx;
            let x1 = f.vp.x((part.start + n.start + n.length) as f64) + kx;
            let heat = ((err - 15.0) / 45.0).clamp(0.0, 1.0) as f32;
            let color = if *err > 30.0 {
                ca(t.heat, 0.35 + 0.65 * heat)
            } else {
                ca(t.tracks[1], 0.5)
            };
            rect(
                window,
                bounds(
                    x0.max(kx),
                    1.0,
                    (x1 - x0.max(kx) - 1.0).max(0.0),
                    f64::from(HEAT_H) - 2.0,
                    b.origin,
                ),
                color,
            );
        }
    }
}

fn lane_label(
    window: &mut Window,
    cx: &mut App,
    cache: &TextCache,
    b: Bounds<Pixels>,
    t: Theme,
    label: &str,
) {
    rect(
        window,
        Bounds {
            origin: b.origin,
            size: size(px(KEYS_W), b.size.height),
        },
        c(t.surface),
    );
    text(
        window,
        cx,
        cache,
        label,
        9.0,
        t.text_quiet,
        point(b.origin.x + px(4.0), b.origin.y + px(4.0)),
    );
}

fn paint_takes(f: &Frame, b: Bounds<Pixels>, window: &mut Window, cx: &mut App, cache: &TextCache) {
    let t = f.theme;
    rect(window, b, c(t.background));
    rect(
        window,
        Bounds {
            origin: b.origin,
            size: size(b.size.width, px(1.0)),
        },
        c(t.border),
    );
    let kx = f64::from(KEYS_W);
    if let Some(track) = f.project.track(f.track) {
        for span in cantor_core::phrase_spans(&f.project, track) {
            let (x0, x1) = (f.vp.x(span.start as f64) + kx, f.vp.x(span.end as f64) + kx);
            if x1 < kx {
                continue;
            }
            let lane = f.project.takes.get(&span.id);
            let n = lane.map_or(0, |l| l.takes.len()) + 1;
            let slot_w = (x1 - x0) / n as f64;
            for slot in 0..n {
                let selected = match lane.and_then(|l| l.selected) {
                    None => slot == 0,
                    Some(s) => lane
                        .and_then(|l| l.takes.get(slot.wrapping_sub(1)))
                        .is_some_and(|tk| tk.id == s),
                };
                let sx = x0 + slot_w * slot as f64;
                let tb = bounds(
                    sx.max(kx),
                    4.0,
                    (slot_w - 2.0).max(1.0),
                    f64::from(TAKES_H) - 8.0,
                    b.origin,
                );
                window.paint_quad(quad(
                    tb,
                    px(t.radius.min(2.0)),
                    if selected {
                        ca(t.accent, 0.35)
                    } else {
                        c(t.surface)
                    },
                    px(1.0),
                    c(if selected { t.accent } else { t.border }),
                    BorderStyle::default(),
                ));
                if slot_w > 28.0 {
                    let label = if slot == 0 {
                        "orig".to_owned()
                    } else {
                        format!("take {slot}")
                    };
                    text(
                        window,
                        cx,
                        cache,
                        &label,
                        9.0,
                        t.text_quiet,
                        point(tb.origin.x + px(4.0), tb.origin.y + px(2.0)),
                    );
                }
            }
        }
    }
    lane_label(window, cx, cache, b, t, "Takes");
}

fn paint_phonemes(
    f: &Frame,
    b: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
    cache: &TextCache,
) {
    let t = f.theme;
    rect(window, b, c(t.lane_alt));
    let kx = f64::from(KEYS_W);
    let w = f64::from(f32::from(b.size.width));
    if let Some(track) = f.project.track(f.track) {
        for part in &track.parts {
            for (i, n) in part.notes.iter().enumerate() {
                let Some(phones) = f.phones.get(&n.id) else {
                    continue;
                };
                let sx = f.vp.x((part.start + n.start) as f64) + kx;
                let ex = f.vp.x((part.start + n.start + n.length) as f64) + kx;
                if ex < kx || sx > w {
                    continue;
                }
                let next_start = part
                    .notes
                    .get(i + 1)
                    .map_or(ex, |m| f.vp.x((part.start + m.start) as f64) + kx);
                let layout = phoneme_layout(phones, sx - kx, f.vp, &f.project);
                for (j, (a, z)) in layout.iter().enumerate() {
                    let (a, z) = (a + kx, (z + kx).min(next_start.max(ex)));
                    let pinned = phones[j].length_ms.is_some();
                    rect(
                        window,
                        bounds(a.max(kx), 3.0, 1.0, f64::from(PHON_H) - 6.0, b.origin),
                        c(if pinned { t.accent } else { t.grid_strong }),
                    );
                    if z - a > 14.0 && a >= kx {
                        let sym = phones[j].symbol.rsplit('/').next().unwrap_or("").to_owned();
                        text(
                            window,
                            cx,
                            cache,
                            &sym,
                            10.0,
                            t.text,
                            point(b.origin.x + px(a as f32 + 3.0), b.origin.y + px(6.0)),
                        );
                    }
                }
            }
        }
    }
    lane_label(window, cx, cache, b, t, "Phonemes");
}

fn paint_param(f: &Frame, b: Bounds<Pixels>, window: &mut Window, cx: &mut App, cache: &TextCache) {
    let t = f.theme;
    rect(window, b, c(t.background));
    rect(
        window,
        Bounds {
            origin: b.origin,
            size: size(b.size.width, px(1.0)),
        },
        c(t.border),
    );
    let kx = f64::from(KEYS_W);
    let w = f64::from(f32::from(b.size.width));
    let h = f64::from(PARAM_H);
    let mid = h / 2.0;
    rect(window, bounds(kx, mid, w - kx, 1.0, b.origin), c(t.grid));
    let (lo, hi) = match cantor_core::params::lookup(&f.param) {
        Some(d) => (-d.max_delta() / 4.0, d.max_delta() / 4.0),
        None => (-1.0, 1.0),
    };
    let ypos = |v: f64| (1.0 - (v - lo) / (hi - lo)) * h;
    let draw = |window: &mut Window, pts: &[(i64, f64)], interp: Interp, alpha: f32| {
        if pts.len() < 2 {
            return;
        }
        let curve = cantor_core::Curve {
            points: pts.to_vec(),
            interp,
        };
        let mut path = PathBuilder::stroke(px(t.curve_width));
        let step = (3.0 / f.vp.px_per_tick).max(1.0) as i64;
        let (a, z) = (pts[0].0, pts[pts.len() - 1].0);
        let lo_t = f.vp.tick(0.0).max(a as f64) as i64;
        let hi_t = (f.vp.tick(w - kx) as i64).min(z);
        let mut started = false;
        let mut tick = lo_t;
        while tick <= hi_t {
            if let Some(v) = curve.value_in_span(tick) {
                let p = point(
                    b.origin.x + px((f.vp.x(tick as f64) + kx) as f32),
                    b.origin.y + px(ypos(v) as f32),
                );
                if started {
                    path.line_to(p);
                } else {
                    path.move_to(p);
                    started = true;
                }
            }
            tick += step;
        }
        if started && let Ok(p) = path.build() {
            window.paint_path(p, ca(t.tracks[0], alpha));
        }
    };
    if let Some(track) = f.project.track(f.track)
        && let Some(curve) = track.curves.get(f.param.as_str())
    {
        draw(
            window,
            &curve.model_prediction.points,
            curve.model_prediction.interp,
            t.model_curve_alpha,
        );
        draw(
            window,
            &curve.user_delta.points,
            curve.user_delta.interp,
            1.0,
        );
    }
    draw(window, &f.curve_preview, Interp::Linear, 1.0);
    lane_label(window, cx, cache, b, t, &f.param);
}

/// The default theme.
pub fn default_theme() -> Theme {
    match std::env::var("CANTOR_THEME").as_deref() {
        Ok("light") => crate::theme::LIGHT,
        _ => DARK,
    }
}

impl Focusable for CantorWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Track color for tests and docs.
pub fn focused_track_color(f_project: &Project, theme: Theme) -> u32 {
    let _ = f_project;
    theme.tracks[0]
}

#[allow(dead_code)]
fn _frame_color(f: &Frame) -> u32 {
    track_color(f)
}
