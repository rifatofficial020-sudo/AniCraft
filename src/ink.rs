//! Brush strokes whose outline is still being traced (ADR 0129, issue #519).
//!
//! Tracing a long stroke's outline takes longer than a frame (a tenth of a second and more for
//! a stroke drawn over many seconds), and it depends on nothing but the stroke. So when the
//! pointer lifts on a long stroke, the trace runs on another thread; the stroke stays on the
//! stage as it was drawn, and the next stroke can start at once. When the trace is done the
//! stroke is painted by the same `draw.brush` as ever, which takes the traced outline
//! (`Session::staged_stroke`) instead of tracing it again.
//!
//! Strokes are painted in the order they were drawn, and nothing else may come between a stroke
//! and its paint: before any other edit (a key, a click anywhere but a brush on the stage, a
//! command from an assistant, playback, another document), the waiting strokes are finished
//! first, on this thread ([`flush`]). The wait is bounded, since a trace is
//! (`geom::brush::MAX_STAMP_WORK`).

use crate::app::{App, Tool};
use crate::brushtool::{self, BrushOptions};
use crate::pathtools::Xf;
use anicraft_engine::cmds_brush::{PaintMode, PreparedStroke, stroke_request};
use anicraft_engine::doc::Rgba;
use anicraft_engine::geom::Vec2;
use serde_json::Value;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Strokes with fewer samples are traced where they are: they take less than a frame.
pub const DEFER_SAMPLES: usize = 400;
/// Most strokes waiting at once; one more waits for the first.
const MAX_WAITING: usize = 8;

/// A stroke on its way into the document.
pub struct Pending {
    /// The stroke as it was drawn, for the stage.
    samples: Vec<(Vec2, Option<f32>)>,
    tip: BrushOptions,
    paint: Rgba,
    alpha: f32,
    /// What `draw.brush` is run with.
    params: Value,
    mode: PaintMode,
    done: Receiver<PreparedStroke>,
}

/// Takes a Brush stroke whose trace should not hold the window up: `false` when it is short
/// enough to paint right away (nothing waiting before it), or can't be handed over (the web
/// build has one thread; a stroke the command would refuse is left for it to refuse).
pub fn defer(app: &mut App, ctx: &egui::Context, samples: &[(Vec2, Option<f32>)], params: &Value, mode: PaintMode) -> bool {
    if crate::platform::WEB || (app.ink.is_empty() && samples.len() < DEFER_SAMPLES) {
        return false;
    }
    let Ok(request) = stroke_request(&app.s, params) else { return false };
    if app.ink.len() >= MAX_WAITING {
        flush(app);
    }
    let (tx, done) = channel();
    let wake = ctx.clone();
    let started = std::thread::Builder::new().name("anicraft-ink".into()).spawn(move || {
        let _ = tx.send(request.prepare());
        wake.request_repaint();
    });
    if started.is_err() {
        return false;
    }
    let tip = *brushtool::options(app);
    let paint = app.s.style.fill;
    let alpha = brushtool::intended_alpha(&tip, paint.a, samples);
    app.ink.push(Pending { samples: samples.to_vec(), tip, paint, alpha, params: params.clone(), mode, done });
    true
}

/// Paints the waiting strokes whose traces are done, oldest first, stopping at the first that
/// is not.
pub fn poll(app: &mut App) {
    if app.ink.is_empty() {
        return;
    }
    // Out of the app while they are painted, so painting one never waits on the others.
    let mut waiting = std::mem::take(&mut app.ink).into_iter();
    let mut rest = Vec::new();
    for p in waiting.by_ref() {
        match p.done.try_recv() {
            Ok(traced) => paint(app, &p, Some(traced)),
            // The thread is gone without an outline: the command traces it.
            Err(TryRecvError::Disconnected) => paint(app, &p, None),
            Err(TryRecvError::Empty) => {
                rest.push(p);
                break;
            }
        }
    }
    rest.extend(waiting);
    app.ink = rest;
}

/// Paints every waiting stroke now, waiting for its trace: before anything else edits.
pub fn flush(app: &mut App) {
    for p in std::mem::take(&mut app.ink) {
        let traced = p.done.recv().ok();
        paint(app, &p, traced);
    }
}

fn paint(app: &mut App, p: &Pending, traced: Option<PreparedStroke>) {
    app.s.staged_stroke = traced;
    brushtool::paint(app, p.params.clone(), p.mode);
    app.s.staged_stroke = None;
}

/// Whether what this frame brings may go ahead of the waiting strokes: looking around and
/// painting more. Anything that could edit (a key, text, a click that is not a brush on the
/// stage, a dropped file) may not.
fn only_painting(app: &App, ctx: &egui::Context) -> bool {
    if app.playing || app.modal_open() || app.palette.is_some() {
        return false;
    }
    // The presses this frame, unless something else came with them (the context is not asked
    // about its layers while its input is being read).
    let presses: Option<Vec<egui::Pos2>> = ctx.input(|i| {
        if !i.raw.dropped_files.is_empty() {
            return None;
        }
        let mut presses = Vec::new();
        for e in &i.raw.events {
            match e {
                egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_) | egui::Event::PointerGone | egui::Event::Touch { .. } | egui::Event::MouseWheel { .. } | egui::Event::Zoom(_) | egui::Event::WindowFocused(_) => {}
                egui::Event::PointerButton { pressed: false, .. } => {}
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => presses.push(*pos),
                _ => return None,
            }
        }
        Some(presses)
    });
    let Some(presses) = presses else { return false };
    // A press paints when the Brush is in hand and it lands on the stage itself.
    let canvas = app.stage_cache.canvas;
    presses.iter().all(|pos| app.tool == Tool::Brush && canvas.contains(*pos) && ctx.layer_id_at(*pos).is_none_or(|l| l.order == egui::Order::Background))
}

/// At the start of a frame: strokes whose traces are done are painted; if this frame's input
/// could edit, all of them are, first.
pub fn frame(app: &mut App, ctx: &egui::Context) {
    if app.ink.is_empty() {
        return;
    }
    if only_painting(app, ctx) {
        poll(app);
    } else {
        flush(app);
    }
}

/// The waiting strokes on the stage, as they were drawn.
pub fn draw(app: &App, painter: &egui::Painter, xf: Xf) {
    for (slot, p) in app.ink.iter().enumerate() {
        brushtool::draw_ink(painter, xf, &p.tip, &p.samples, p.paint, p.alpha, slot + 1);
    }
}
