//! File › Import › Import Video (ADR 0116): what the video is, how to take it (frames in the
//! document, or a reference layer to trace over), what that costs before anything starts, then
//! a progress window with Cancel while ffmpeg decodes on another thread. The document changes
//! once, at the end, through `file.import_video` (one undo step); a cancel or a failure leaves
//! it as it was. Reference videos' pictures for the stage come from [`stage_frames`].

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{FORM_LABEL_DIALOG, T, TARGET, Type, form_note, form_row, text};
use anicraft_engine::EngineError;
use anicraft_engine::publish::Control;
use anicraft_engine::video::{self, Options, Prepared, Probe, RefFrame};
use egui::Key;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// How the video comes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Frames,
    Reference,
}

/// What is running on the other thread.
enum Work {
    /// ffmpeg is describing the file.
    Probing(Receiver<Result<Probe, EngineError>>),
    /// ffmpeg is decoding: the import's params, its Cancel and frame count, and the result.
    Importing { params: Value, control: Arc<Control>, done: Receiver<Result<Prepared, EngineError>> },
}

/// What the window shows this frame.
enum View {
    Importing(Arc<Control>),
    Reading,
    Failed(String),
    Ready(Probe),
}

/// The Import Video window while it is open.
pub struct VideoImport {
    pub path: PathBuf,
    /// What ffmpeg says the file holds, or why it couldn't say.
    pub info: Option<Result<Probe, String>>,
    pub mode: Mode,
    /// One picture every `step` frames of the document.
    pub step: u32,
    /// Longest edge of the pictures, in pixels.
    pub max_edge: u32,
    pub audio: bool,
    /// Seconds into the video to begin at, and seconds to take (0: to the end).
    pub start: f64,
    pub length: f64,
    /// The ffmpeg to use; `None`: found as Export Video finds it.
    ffmpeg: Option<String>,
    work: Option<Work>,
    /// The tab the import is for: a result arriving on another document is dropped.
    tab: u64,
}

/// Memory an embedded video may take before the window suggests a reference layer instead.
const COMFORTABLE: u64 = 256 << 20;

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{:.0} MB", mb.max(1.0)) }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Opens the window for the video at `path`; ffmpeg describes the file on another thread.
pub fn open(app: &mut App, path: PathBuf) {
    open_using(app, path, None);
}

/// [`open`] with the ffmpeg at `ffmpeg` (`None`: the one on this computer).
pub fn open_using(app: &mut App, path: PathBuf, ffmpeg: Option<String>) {
    app.playing = false;
    let (tx, rx) = channel();
    let file = path.clone();
    let program = video::program(ffmpeg.as_deref());
    let started = std::thread::Builder::new().name("anicraft-video-probe".into()).spawn(move || {
        let _ = tx.send(video::probe(&program, &file));
    });
    let mut d = VideoImport { path, ffmpeg, info: None, mode: Mode::Frames, step: 1, max_edge: video::DEFAULT_EDGE, audio: true, start: 0.0, length: 0.0, work: Some(Work::Probing(rx)), tab: app.tabs.active_id() };
    if started.is_err() {
        d.work = None;
        d.info = Some(Err(tr!("The video could not be read: this computer would not start another thread.").to_string()));
    }
    app.video_import = Some(d);
}

/// The window with a description already in hand (screenshots and tests: no ffmpeg needed).
pub fn open_with(app: &mut App, path: PathBuf, probe: Probe) {
    let mut d = VideoImport { path, ffmpeg: None, info: None, mode: Mode::Frames, step: 1, max_edge: video::DEFAULT_EDGE, audio: probe.audio, start: 0.0, length: 0.0, work: None, tab: app.tabs.active_id() };
    described(app, &mut d, probe);
    app.video_import = Some(d);
}

/// The options as the engine takes them.
fn options(d: &VideoImport) -> Options {
    Options { step: d.step, max_edge: d.max_edge, start: d.start, length: (d.length > 0.0).then_some(d.length), max_frames: video::MAX_EMBED_FRAMES }
}

/// What the import would take, with the document as it is.
fn plan(app: &App, d: &VideoImport, probe: &Probe) -> video::Plan {
    let stage = app.s.doc.stage;
    video::plan(probe, stage.fps, (stage.width, stage.height), &options(d))
}

/// ffmpeg described the file: a long or large video starts as a reference layer, which costs
/// nothing in the document; a short one as frames, as Animate embeds a video.
fn described(app: &App, d: &mut VideoImport, probe: Probe) {
    d.audio = probe.audio;
    let p = plan(app, d, &probe);
    d.mode = if probe.duration.is_some() && (p.truncated || p.memory > COMFORTABLE) { Mode::Reference } else { Mode::Frames };
    d.info = Some(Ok(probe));
}

/// The params `file.import_video` gets for what the window shows.
pub fn params(d: &VideoImport) -> Value {
    let mut p = json!({"path": d.path.display().to_string(), "mode": if d.mode == Mode::Frames { "frames" } else { "reference" }});
    if d.start > 0.0 {
        p["start"] = json!(d.start);
    }
    if let Some(f) = &d.ffmpeg {
        p["ffmpeg"] = json!(f);
    }
    if d.mode == Mode::Frames {
        p["step"] = json!(d.step);
        p["max_edge"] = json!(d.max_edge);
        p["audio"] = json!(d.audio);
        if d.length > 0.0 {
            p["length"] = json!(d.length);
        }
    }
    p
}

/// Import was pressed: ffmpeg decodes on another thread, counted and cancellable.
pub(crate) fn begin(app: &mut App) {
    let Some(p) = app.video_import.as_ref().map(params) else { return };
    let req = match video::request(&app.s, &p) {
        Ok(r) => r,
        Err(e) => {
            let why = crate::i18n::t_dyn(&e.to_string()).into_owned();
            app.status_err(why);
            return;
        }
    };
    let control = Arc::new(Control::default());
    let (tx, rx) = channel();
    let job = control.clone();
    let started = std::thread::Builder::new().name("anicraft-video-import".into()).spawn(move || {
        let _ = tx.send(video::prepare(&req, Some(&job)));
    });
    if started.is_err() {
        app.status_err(tr!("The video could not be imported: this computer would not start another thread."));
        return;
    }
    if let Some(d) = &mut app.video_import {
        d.work = Some(Work::Importing { params: p, control, done: rx });
    }
}

/// Takes what the other thread finished. The document changes here, in one command.
fn poll(app: &mut App, ctx: &egui::Context) {
    let Some(mut d) = app.video_import.take() else { return };
    let Some(work) = d.work.take() else {
        app.video_import = Some(d);
        return;
    };
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
    match work {
        Work::Probing(rx) => {
            match rx.try_recv() {
                Ok(Ok(probe)) => described(app, &mut d, probe),
                Ok(Err(e)) => d.info = Some(Err(crate::i18n::t_dyn(&e.to_string()).into_owned())),
                Err(TryRecvError::Empty) => d.work = Some(Work::Probing(rx)),
                Err(TryRecvError::Disconnected) => d.info = Some(Err(tr!("The video could not be read.").to_string())),
            }
            app.video_import = Some(d);
        }
        Work::Importing { params, control, done } => {
            let result = match done.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => {
                    d.work = Some(Work::Importing { params, control, done });
                    app.video_import = Some(d);
                    return;
                }
                // i18n-exempt: an engine-style message, translated where it is shown
                Err(TryRecvError::Disconnected) => Err(EngineError::State("the video could not be read".into())),
            };
            // Done, one way or another: the window closes.
            match result {
                // Another document is in front now: this one stays as it is.
                Ok(_) if app.tabs.active_id() != d.tab => app.status_err(tr!("The video was not imported: another document was opened while it was being read. Import it again.")),
                Ok(prepared) => {
                    app.s.staged_video = Some(prepared);
                    app.perform_here("file.import_video", params);
                    app.s.staged_video = None;
                }
                Err(EngineError::Cancelled) => app.status_ok(tr!("Import cancelled. Nothing was changed.")),
                Err(e) => {
                    let why = crate::i18n::t_dyn(&e.to_string()).into_owned();
                    app.status_err(why);
                }
            }
        }
    }
}

/// Cancels the import that is running: ffmpeg stops at its next frame, nothing changes.
pub fn cancel(app: &mut App) {
    match app.video_import.as_ref().and_then(|d| d.work.as_ref()) {
        Some(Work::Importing { control, .. }) => control.cancel(),
        _ => app.video_import = None,
    }
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    poll(app, ctx);
    let Some(mut d) = app.video_import.take() else { return };
    let importing = matches!(d.work, Some(Work::Importing { .. }));
    let title = if importing { tr!("Importing Video") } else { tr!("Import Video") };
    let view = match (&d.work, &d.info) {
        (Some(Work::Importing { control, .. }), _) => View::Importing(control.clone()),
        (Some(Work::Probing(_)), _) | (None, None) => View::Reading,
        (None, Some(Err(why))) => View::Failed(why.clone()),
        (None, Some(Ok(probe))) => View::Ready(probe.clone()),
    };
    let (mut submit, mut close) = (false, false);
    let mut open = true;
    let shown = crate::windows::new(ctx, title).id(egui::Id::new("video_import")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0]).show(ctx, |ui| {
        ui.set_min_width(460.0);
        ui.set_max_width(520.0);
        ui.label(text(Type::Body, file_name(&d.path)).strong());
        match view {
            View::Importing(control) => progress(ui, &control, &mut close),
            View::Reading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(text(Type::Body, tr!("Reading the video with ffmpeg…")));
                });
                buttons(ui, None, &mut submit, &mut close);
            }
            View::Failed(why) => {
                // No ffmpeg, or a file it can't read: what happened and what to do, in words.
                ui.label(text(Type::Body, why).color(T.error));
                ui.label(text(Type::Caption, tr!("AniCraft reads video with the ffmpeg on this computer and holds no video decoder of its own. ffmpeg is free: install it from ffmpeg.org (or with your system's package manager), then import again.")).color(T.text_dim));
                buttons(ui, None, &mut submit, &mut close);
            }
            View::Ready(probe) => {
                form(app, ui, &mut d, &probe);
                buttons(ui, Some(if d.mode == Mode::Frames { tr!("Import") } else { tr!("Add Reference Layer") }), &mut submit, &mut close);
            }
        }
    });
    crate::a11y::name_title_bar(&shown, title);
    if !open {
        close = true;
    }
    app.video_import = Some(d);
    if submit {
        begin(app);
    } else if close {
        cancel(app);
    }
}

/// OK and Cancel (`ok`: the first button's label; `None`: only Close).
fn buttons(ui: &mut egui::Ui, ok: Option<&str>, submit: &mut bool, close: &mut bool) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if let Some(ok) = ok {
            let r = ui.add(egui::Button::new(ok).min_size(egui::vec2(96.0, TARGET)));
            if r.clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                *submit = true;
            }
        }
        let label = if ok.is_some() { tr!("Cancel") } else { tr!("Close") };
        if ui.add(egui::Button::new(label).min_size(egui::vec2(96.0, TARGET))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            *close = true;
        }
    });
}

/// The decode's progress, with Cancel.
fn progress(ui: &mut egui::Ui, control: &Control, close: &mut bool) {
    let (done, total) = control.progress();
    let frac = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    ui.label(text(Type::Body, tr!("Reading frames with ffmpeg. The document changes when it is done.")));
    ui.add(egui::ProgressBar::new(frac).desired_width(ui.available_width()).text(text(Type::DataSmall, format!("{done} / {total}")))).named(tr!("Frames read"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let cancelling = control.cancelled();
        if ui.add_enabled(!cancelling, egui::Button::new(tr!("Cancel")).min_size(egui::vec2(96.0, TARGET))).on_hover_text(tr!("Stop reading the video. The document stays as it is")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            *close = true;
        }
        if cancelling {
            ui.label(text(Type::Caption, tr!("Stopping…")));
        }
    });
}

/// The video, the two ways to take it, their options, and what it will cost.
fn form(app: &App, ui: &mut egui::Ui, d: &mut VideoImport, probe: &Probe) {
    // What the file is: numbers in the data face.
    let mut facts = format!("{} × {}", probe.width, probe.height);
    if let Some(s) = probe.duration {
        facts.push_str(&format!(" · {s:.2} s")); // i18n-exempt: a number with its unit
    }
    if let Some(f) = probe.fps {
        facts.push_str(&format!(" · {f:.2} fps")); // i18n-exempt: a number with its unit
    }
    ui.horizontal(|ui| {
        ui.label(text(Type::Data, facts).color(T.text_dim));
        ui.label(text(Type::Caption, if probe.audio { tr!("with sound") } else { tr!("no sound") }).color(T.text_dim));
    });
    ui.add_space(4.0);
    form_row(ui, FORM_LABEL_DIALOG, tr!("Import as"), |ui| {
        ui.vertical(|ui| {
            if ui.radio(d.mode == Mode::Frames, tr!("Frames in the document")).on_hover_text(tr!("The video's frames become bitmaps on a new layer, one keyframe each at the document's frame rate, with its sound. They are saved in the document and export with the movie")).clicked() {
                d.mode = Mode::Frames;
            }
            let can = probe.duration.is_some();
            let r = ui.add_enabled(can, egui::RadioButton::new(d.mode == Mode::Reference, tr!("Reference layer for tracing")));
            let r = if can {
                r.on_hover_text(tr!("A locked guide layer under your artwork shows the video as you scrub and play. The video stays a file on this computer and is never in the movie or an export"))
            } else {
                r.on_disabled_hover_text(tr!("This file doesn't say how long it is, so it can only come in as frames"))
            };
            if r.clicked() {
                d.mode = Mode::Reference;
            }
        });
    });
    let last = probe.duration.map_or(86_400.0, |s| (s - 0.01).max(0.0));
    form_row(ui, FORM_LABEL_DIALOG, tr!("Start at"), |ui| {
        ui.add(egui::DragValue::new(&mut d.start).range(0.0..=last).speed(0.05).max_decimals(2).suffix(" s")).on_hover_text(tr!("Seconds into the video to begin at")).named(tr!("Start at (seconds)"));
    });
    if d.mode == Mode::Frames {
        form_row(ui, FORM_LABEL_DIALOG, tr!("Length"), |ui| {
            ui.add(egui::DragValue::new(&mut d.length).range(0.0..=86_400.0).speed(0.05).max_decimals(2).suffix(" s")).on_hover_text(tr!("Seconds of video to take. 0 takes it to its end")).named(tr!("Length (seconds, 0 for all)"));
            if d.length <= 0.0 {
                ui.label(text(Type::Caption, tr!("to the end")).color(T.text_dim));
            }
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Frame step"), |ui| {
            ui.add(egui::DragValue::new(&mut d.step).range(1..=video::MAX_STEP).speed(0.1)).on_hover_text(tr!("One picture every this many frames of the document. 2 animates on twos: half the pictures, each held for two frames")).named(tr!("Frame step"));
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Largest size"), |ui| {
            ui.add(egui::DragValue::new(&mut d.max_edge).range(16..=video::MAX_DECODE_EDGE).speed(8.0).suffix(" px")).on_hover_text(tr!("The pictures' longest edge. A smaller video is never enlarged; smaller pictures make a smaller document")).named(tr!("Largest size (pixels)"));
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Sound"), |ui| {
            ui.add_enabled_ui(probe.audio, |ui| {
                let mut on = d.audio && probe.audio;
                crate::theme::checkbox(ui, &mut on, tr!("Import the sound track")).on_hover_text(tr!("The video's sound goes on a layer of its own as a stream sound, in time with the frames"));
                if probe.audio {
                    d.audio = on;
                }
            });
        });
    }
    // Told up front: what comes in, what it costs, and where it is cut short.
    let p = plan(app, d, probe);
    let from = app.s.frame + 1;
    ui.add_space(4.0);
    match d.mode {
        Mode::Frames => {
            let n = p.frames.unwrap_or(p.limit);
            let size = format!("{} × {}", p.width, p.height);
            let line = if p.frames.is_some() {
                trn!("{n} picture of {size} from frame {from}, about {memory} in memory while you edit.", "{n} pictures of {size} from frame {from}, about {memory} in memory while you edit.", u64::from(n), size = size, from = from, memory = megabytes(p.memory))
            } else {
                trn!("Up to {n} picture of {size} from frame {from}, at most {memory} in memory while you edit.", "Up to {n} pictures of {size} from frame {from}, at most {memory} in memory while you edit.", u64::from(n), size = size, from = from, memory = megabytes(p.memory))
            };
            form_note(ui, FORM_LABEL_DIALOG, text(Type::Caption, line));
            if p.truncated {
                let secs = format!("{:.1}", f64::from(n) * f64::from(d.step) / f64::from(app.s.doc.stage.fps.max(1.0)));
                form_note(
                    ui,
                    FORM_LABEL_DIALOG,
                    text(Type::Caption, trn!("The video is longer than one import takes: only the first {n} picture ({secs} s) comes in. Raise the frame step, lower the size, shorten the length, or use a reference layer.", "The video is longer than one import takes: only the first {n} pictures ({secs} s) come in. Raise the frame step, lower the size, shorten the length, or use a reference layer.", u64::from(n), secs = secs)).color(T.error),
                );
            } else if p.memory > COMFORTABLE {
                form_note(ui, FORM_LABEL_DIALOG, text(Type::Caption, tr!("That is a large document. A reference layer shows the same video without keeping its frames.")).color(T.text_dim));
            }
        }
        Mode::Reference => {
            let frames = probe.duration.map_or(0.0, |s| ((s - d.start).max(0.0) * f64::from(app.s.doc.stage.fps)).ceil());
            form_note(
                ui,
                FORM_LABEL_DIALOG,
                text(Type::Caption, tr!("A locked guide layer under the layer you are on shows the video for {frames} frames from frame {from}. Nothing is added to the document but the file's name; frames are read as you go and kept within {memory}.", frames = frames, from = from, memory = megabytes(video::DEFAULT_CACHE_BYTES))),
            );
        }
    }
}

/// The reference videos' pictures for movie frame `frame` of the stage picture `doc`, asking
/// for the ones that aren't decoded yet when `ask` (the frame on screen; frames rendered ahead
/// only take what is there). Pictures on their way bring a repaint; a video that can't be
/// shown is said once in the status bar.
pub fn stage_frames(app: &mut App, ctx: &egui::Context, doc: &anicraft_engine::doc::Document, frame: u32, ask: bool) -> Option<Arc<Vec<RefFrame>>> {
    let shown = video::shown(&app.video, doc, frame, ask);
    if !ask {
        return shown.frames;
    }
    if shown.pending {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
    match shown.failed {
        Some((_, why)) if app.video_failed.as_deref() != Some(why.as_str()) => {
            app.status_err(tr!("The reference video can't be shown: {why}", why = crate::i18n::t_dyn(&why)));
            app.video_failed = Some(why);
        }
        Some(_) => {}
        None => app.video_failed = None,
    }
    shown.frames
}
