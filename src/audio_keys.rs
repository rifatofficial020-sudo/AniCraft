//! Convert Audio to Keyframes and Auto Lip-Sync (ADR 0114): one dialog for the two commands
//! that let a sound on the timeline drive an object on another layer.
//!
//! It names the sound and the frames it is heard on, asks for the layer and the object that
//! get the keys and for the settings (the property and its range, threshold and smoothing; or
//! the mouth for each sound), and shows what they give while they are changed: a graph of the
//! sound's loudness frame by frame with the threshold, the value the keys follow and a tick
//! where each key lands (for lip-sync, a row per mouth shape), and the keys themselves on the
//! stage and the timeline. The preview is the command itself run inside a gesture that every
//! change takes back and runs again; OK closes the gesture (one undo step) and Cancel takes it
//! back, as the plug-in dialog does.

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{self, FORM_LABEL_DIALOG, T, TARGET, Type, font, form_row, text, with_type};
use anicraft_engine::cmds_audio_keys::{self as keys, DEFAULT_SMOOTHING, DEFAULT_THRESHOLD, KeyProperty, LevelSettings, MAX_STEP, Output, Source};
use anicraft_engine::doc::audio::analysis::{Channel, LipOptions, MAX_HOLD, MAX_SMOOTHING, Viseme};
use anicraft_engine::doc::{Element, ElementKind, LayerKind, Symbol, SymbolKind};
use egui::{Align2, Key, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

/// The engine's English for what the dialog lists, for the catalogs: translated where drawn.
#[cfg_attr(not(test), allow(dead_code))]
pub const TEXTS: [&str; 21] = [
    tr_noop!("Scale"),
    tr_noop!("Scale X"),
    tr_noop!("Scale Y"),
    tr_noop!("Alpha"),
    tr_noop!("X offset"),
    tr_noop!("Y offset"),
    tr_noop!("Rotation"),
    tr_noop!("First frame"),
    tr_noop!("Both channels"),
    tr_noop!("Left channel"),
    tr_noop!("Right channel"),
    tr_noop!("Rest"),
    tr_noop!("Open"),
    tr_noop!("Wide"),
    tr_noop!("Round"),
    tr_noop!("Closed"),
    tr_noop!("Silence: the mouth at rest"),
    tr_noop!("An open jaw: ah, as in father and cat"),
    tr_noop!("Spread lips: ee and e, and the hiss of s and t"),
    tr_noop!("Rounded lips: oh, oo and w"),
    tr_noop!("Lips together: m, b, p and the short stops between syllables"),
];

/// Which of the two commands the dialog runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keys,
    Lips,
}

impl Kind {
    pub fn command(self) -> &'static str {
        match self {
            Kind::Keys => "sound.to_keyframes",
            Kind::Lips => "sound.lip_sync",
        }
    }

    /// The undo step's name (the engine's English, translated where shown).
    fn step(self) -> &'static str {
        match self {
            Kind::Keys => tr_noop!("Convert Audio to Keyframes"),
            Kind::Lips => tr_noop!("Auto Lip-Sync"),
        }
    }
}

/// What the dialog was last given, offered again the next time (view state).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub property: KeyProperty,
    /// The property's value in silence and at the loudest frame.
    pub min: f32,
    pub max: f32,
    pub threshold: f32,
    pub smoothing: u32,
    pub channel: Channel,
    pub step: u32,
    pub lip_threshold: f32,
    pub hold: u32,
}

impl Default for Settings {
    fn default() -> Self {
        let (min, max) = KeyProperty::Scale.defaults();
        let lips = LipOptions::default();
        Settings { property: KeyProperty::Scale, min, max, threshold: DEFAULT_THRESHOLD, smoothing: DEFAULT_SMOOTHING, channel: Channel::Both, step: 1, lip_threshold: lips.threshold, hold: lips.hold }
    }
}

/// What the graph draws: the sound's loudness, and what the settings make of it.
#[derive(Clone, Debug, PartialEq)]
struct Graph {
    from: u32,
    raw: Vec<f32>,
    /// Keys: the gated, smoothed level the value follows.
    shaped: Vec<f32>,
    /// Keys: where each key lands (frames from `from`).
    ticks: Vec<u32>,
    /// Lips: each frame's mouth.
    mouths: Vec<Viseme>,
}

/// The dialog while it is open.
pub struct Dialog {
    pub kind: Kind,
    /// The sound read: its layer and its keyframe's first frame.
    sound: (u64, u32),
    /// The layer and the object that get the keys.
    pub layer: u64,
    pub element: Option<u64>,
    pub set: Settings,
    /// Keys go on the motion tween's curve instead of keyframes.
    pub motion: bool,
    /// The symbol frame of each mouth, in `Viseme::ALL`'s order; `None` until one is changed
    /// (the engine then picks them by the symbol's frame labels).
    pub mouths: Option<[u32; 5]>,
    pub preview: bool,
    /// What the stage shows now: the params applied, and what the command answered.
    applied: Option<Value>,
    answer: Option<Value>,
    pub error: Option<String>,
    /// The frame the stage shows (the playhead, moved from the graph).
    frame: u32,
    /// The graph, for the settings it was worked out from.
    graph: Option<(Value, Graph)>,
}

fn is_graphic(app: &App, e: &Element) -> bool {
    matches!(&e.kind, ElementKind::Symbol { symbol, .. } if app.s.doc.symbol(*symbol).is_some_and(|s| s.kind == SymbolKind::Graphic))
}

/// The objects on layer `id` that could take the keys at `frame` (as the engine finds them: on
/// the keyframe there, or the layer's last one before it).
fn objects(app: &App, id: u64, frame: u32, kind: Kind) -> Vec<(u64, String)> {
    let Some(layer) = app.s.timeline().layers.iter().find(|l| l.id.0 == id) else { return Vec::new() };
    if matches!(layer.kind, LayerKind::Folder | LayerKind::Armature | LayerKind::Camera) {
        return Vec::new();
    }
    let k = layer.keyframe_at(frame).or_else(|| layer.keyframes.iter().rev().find(|k| k.start <= frame).map(|k| &**k));
    k.map_or_else(Vec::new, |k| k.elements.iter().filter(|e| kind == Kind::Keys || is_graphic(app, e)).map(|e| (e.id.0, anicraft_engine::element_label(&app.s.doc, e))).collect())
}

/// The layers with something the keys could go on.
fn layers(app: &App, frame: u32, kind: Kind) -> Vec<(u64, String)> {
    app.s.timeline().layers.iter().filter(|l| !objects(app, l.id.0, frame, kind).is_empty()).map(|l| (l.id.0, l.name.clone())).collect()
}

/// The sounds on the timeline the dialog can read: (layer, keyframe start, "name on layer").
fn sounds(app: &App) -> Vec<((u64, u32), String)> {
    let doc = &app.s.doc;
    let mut out = Vec::new();
    for l in &app.s.timeline().layers {
        for k in &l.keyframes {
            let Some(fs) = k.sound.as_ref().filter(|f| f.sync != anicraft_engine::doc::SoundSync::Stop) else { continue };
            let name = doc.sound(fs.sound).map_or_else(String::new, |s| s.name.clone());
            out.push(((l.id.0, k.start), tr!("{sound} on {layer}, frame {frame}", sound = name, layer = l.name, frame = k.start + 1)));
        }
    }
    out
}

/// Whether the sound the dialog reads is not a Stream sound (ADR 0114): it keeps time with the
/// frames only while the movie plays straight through, so the dialog says so.
fn can_drift(app: &App, sound: (u64, u32)) -> bool {
    let key = app.s.timeline().layers.iter().find(|l| l.id.0 == sound.0).and_then(|l| l.keyframes.iter().find(|k| k.start == sound.1));
    key.and_then(|k| k.sound.as_ref()).is_some_and(|f| f.sync != anicraft_engine::doc::SoundSync::Stream)
}

/// The sound the dialog reads, as the engine finds it.
fn source(app: &App, sound: (u64, u32)) -> Option<Source> {
    keys::source(&app.s, &json!({"sound_layer": sound.0, "sound_frame": sound.1})).ok()
}

/// Opens the dialog on the sound at the playhead, with the layer and object the selection
/// names (or the first that fit). Says why when there is nothing to read or to animate.
pub fn open(app: &mut App, kind: Kind) {
    let src = match keys::source(&app.s, &json!({})) {
        Ok(src) => src,
        Err(e) => {
            app.status_err(e.to_string());
            return;
        }
    };
    let candidates = layers(app, src.from, kind);
    if candidates.is_empty() {
        app.status_err(match kind {
            Kind::Keys => tr!("Put the object to animate on a layer first: the sound's loudness then keys its scale, alpha, position or rotation."),
            Kind::Lips => tr!("Auto Lip-Sync needs an instance of a graphic symbol with a frame for each mouth shape (File › New from Template has a Lip-sync mouth chart)."),
        });
        return;
    }
    // The selection's layer when it fits, else the active layer, else the first that fits.
    let fits = |id: u64| candidates.iter().any(|(c, _)| *c == id);
    let selected = app.s.selection.first().map(|(l, e)| (l.0, e.0)).filter(|(l, _)| fits(*l));
    let layer = selected.map(|(l, _)| l).or(Some(app.s.layer.0).filter(|l| fits(*l))).or(candidates.first().map(|(c, _)| *c)).unwrap_or(0);
    let list = objects(app, layer, src.from, kind);
    let element = selected.map(|(_, e)| e).filter(|e| list.iter().any(|(id, _)| id == e)).or(list.first().map(|(id, _)| *id));
    let motion = kind == Kind::Keys && app.s.timeline().layers.iter().find(|l| l.id.0 == layer).and_then(|l| l.keyframe_at(src.from)).is_some_and(|k| k.motion.is_some());
    app.playing = false;
    app.s.begin_gesture(kind.step());
    let frame = app.s.frame.clamp(src.from, src.to);
    let mut set = app.audio_keys_last;
    if motion && set.property == KeyProperty::FirstFrame {
        (set.property, (set.min, set.max)) = (KeyProperty::Scale, KeyProperty::Scale.defaults());
    }
    app.audio_keys = Some(Dialog { kind, sound: (src.layer.0, src.keyframe), layer, element, set, motion, mouths: None, preview: true, applied: None, answer: None, error: None, frame, graph: None });
    fit_first_frame(app);
    preview(app);
}

/// The graphic symbol the dialog's object is an instance of.
fn mouth_symbol<'a>(app: &'a App, d: &Dialog) -> Option<&'a Symbol> {
    let layer = app.s.timeline().layers.iter().find(|l| l.id.0 == d.layer)?;
    let el = layer.keyframes.iter().flat_map(|k| k.elements.iter()).find(|e| Some(e.id.0) == d.element)?;
    match &el.kind {
        ElementKind::Symbol { symbol, .. } => app.s.doc.symbol(*symbol).filter(|s| s.kind == SymbolKind::Graphic),
        _ => None,
    }
}

/// Keeps a First frame range inside the symbol's frames (and off objects that have none).
fn fit_first_frame(app: &mut App) {
    let Some(d) = &app.audio_keys else { return };
    if d.kind != Kind::Keys || d.set.property != KeyProperty::FirstFrame {
        return;
    }
    let frames = mouth_symbol(app, d).map(|s| s.timeline.frame_count().max(1));
    let Some(d) = &mut app.audio_keys else { return };
    match frames {
        Some(n) => {
            let last = (n - 1) as f32;
            (d.set.min, d.set.max) = (d.set.min.clamp(0.0, last), d.set.max.clamp(0.0, last));
            d.motion = false;
        }
        None => (d.set.property, (d.set.min, d.set.max)) = (KeyProperty::Scale, KeyProperty::Scale.defaults()),
    }
}

/// The command's params for the dialog's values.
pub fn params(d: &Dialog) -> Value {
    let mut p = json!({"sound_layer": d.sound.0, "sound_frame": d.sound.1, "layer": d.layer});
    if let Some(e) = d.element {
        p["element"] = json!(e);
    }
    match d.kind {
        Kind::Keys => {
            let r = d.set.property.range();
            p["property"] = json!(d.set.property.key());
            p["min"] = json!(d.set.min.clamp(*r.start(), *r.end()));
            p["max"] = json!(d.set.max.clamp(*r.start(), *r.end()));
            p["threshold"] = json!(d.set.threshold.clamp(0.0, 0.99));
            p["smoothing"] = json!(d.set.smoothing.min(MAX_SMOOTHING));
            p["channel"] = json!(d.set.channel.key());
            p["step"] = json!(d.set.step.clamp(1, MAX_STEP));
            p["output"] = json!(if d.motion { Output::Motion.key() } else { Output::Keyframes.key() });
        }
        Kind::Lips => {
            p["threshold"] = json!(d.set.lip_threshold.clamp(0.0, 0.99));
            p["hold"] = json!(d.set.hold.clamp(1, MAX_HOLD));
            if let Some(m) = d.mouths {
                p["mouths"] = Value::Object(Viseme::ALL.iter().zip(m).map(|(v, f)| (v.key().to_string(), json!(f))).collect());
            }
        }
    }
    p
}

/// Shows the dialog's values on the stage and the timeline (or the untouched document).
fn preview(app: &mut App) {
    let Some(d) = &app.audio_keys else { return };
    let (kind, p, on, frame) = (d.kind, params(d), d.preview, d.frame);
    app.s.revert_gesture();
    let result = if on { app.s.execute(kind.command(), &p).map(Some) } else { Ok(None) };
    // Taking the gesture back took the playhead back too.
    app.s.set_frame(frame);
    if let Some(d) = &mut app.audio_keys {
        match result {
            Ok(v) => {
                d.applied = on.then_some(p);
                d.answer = v;
                d.error = None;
            }
            Err(e) => {
                d.applied = None;
                d.answer = None;
                d.error = Some(e.to_string());
            }
        }
    }
}

/// The graph for the dialog's settings, worked out again only when they change.
fn graph(app: &App, d: &mut Dialog) -> Option<Graph> {
    let p = params(d);
    if let Some((for_p, g)) = &d.graph
        && *for_p == p
    {
        return Some(g.clone());
    }
    let src = source(app, d.sound)?;
    let g = match d.kind {
        Kind::Keys => {
            let lv = keys::levels(&src, &LevelSettings { threshold: d.set.threshold, smoothing: d.set.smoothing, channel: d.set.channel }).ok()?;
            let output = if d.motion { Output::Motion } else { Output::Keyframes };
            let ticks = keys::planned_keys(d.set.property, d.set.min, d.set.max, d.set.step, output, &lv.shaped).into_iter().map(|(f, _)| f).collect();
            Graph { from: lv.from, raw: lv.raw, shaped: lv.shaped, ticks, mouths: Vec::new() }
        }
        Kind::Lips => {
            let lv = keys::levels(&src, &LevelSettings { threshold: d.set.lip_threshold, smoothing: 0, channel: Channel::Both }).ok()?;
            let mouths = keys::visemes(&src, &LipOptions { threshold: d.set.lip_threshold, hold: d.set.hold }).ok()?;
            Graph { from: lv.from, raw: lv.raw, shaped: Vec::new(), ticks: Vec::new(), mouths }
        }
    };
    d.graph = Some((p, g.clone()));
    Some(g)
}

/// Height of the loudness graph, and of one mouth's row under it.
const GRAPH_H: f32 = 96.0;
const ROW_H: f32 = 14.0;
/// Width of the mouth names' column beside the rows.
const NAMES_W: f32 = 56.0;

/// Draws the graph; the frame clicked or dragged to, if any.
fn draw_graph(ui: &mut egui::Ui, d: &Dialog, g: &Graph) -> Option<u32> {
    let lips = d.kind == Kind::Lips;
    let rows = if lips { Viseme::ALL.len() as f32 * ROW_H + 4.0 } else { 0.0 };
    let (area, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), GRAPH_H + rows), Sense::click_and_drag());
    let name = match d.kind {
        Kind::Keys => tr!("Preview: the sound's loudness on each frame, the threshold, the value the keys follow and where the keys land. Click or drag to show a frame on the stage"),
        Kind::Lips => tr!("Preview: the sound's loudness on each frame and the mouth each frame gets. Click or drag to show a frame on the stage"),
    };
    crate::a11y::surface(ui, &resp, name);
    // A gutter beside the plot: the mouths' names, or the two ends of the property's range.
    let left = NAMES_W;
    let plot = Rect::from_min_max(pos2(area.left() + left, area.top()), pos2(area.right(), area.top() + GRAPH_H));
    let p = ui.painter_at(area);
    p.rect_filled(plot, theme::SQUARE, T.field);
    let n = g.raw.len().max(1);
    let per = plot.width() / n as f32;
    let x_of = |i: f32| plot.left() + i * per;
    let (base, room) = (plot.bottom() - 8.0, plot.height() - 14.0);
    let y_of = |level: f32| base - level.clamp(0.0, 1.0) * room;
    // The loudness: a bar a frame (a column a pixel when the frames are narrower than that).
    let cols = (plot.width().floor() as usize).clamp(1, n);
    for c in 0..cols {
        let (a, b) = (c * n / cols, ((c + 1) * n / cols).max(c * n / cols + 1));
        let level = g.raw.iter().skip(a).take(b - a).copied().fold(0.0f32, f32::max);
        if level > 0.0 {
            let bar = Rect::from_min_max(pos2(x_of(a as f32), y_of(level)), pos2((x_of(b as f32) - if per >= 4.0 { 1.0 } else { 0.0 }).max(x_of(a as f32) + 1.0), base));
            p.rect_filled(bar, theme::SQUARE, T.waveform);
        }
    }
    // The threshold, across.
    let th = if lips { d.set.lip_threshold } else { d.set.threshold };
    p.line_segment([pos2(plot.left(), y_of(th)), pos2(plot.right(), y_of(th))], Stroke::new(1.0, T.text_dim));
    let label = p.layout_no_wrap(tr!("Threshold").to_string(), font(Type::DataSmall), T.text);
    let at = pos2(plot.right() - 4.0 - label.size().x, (y_of(th) - 3.0 - label.size().y).max(plot.top() + 1.0));
    p.rect_filled(Rect::from_min_size(at, label.size()).expand(2.0), theme::SQUARE, T.field);
    p.galley(at, label, T.text);
    // The value the keys follow, and a tick where each key lands.
    if !lips {
        let line: Vec<egui::Pos2> = g.shaped.iter().enumerate().flat_map(|(i, v)| if d.motion { vec![pos2(x_of(i as f32 + 0.5), y_of(*v))] } else { vec![pos2(x_of(i as f32), y_of(*v)), pos2(x_of(i as f32 + 1.0), y_of(*v))] }).collect();
        p.add(egui::Shape::line(line, Stroke::new(1.5, T.text)));
        for f in &g.ticks {
            let x = x_of(*f as f32 + if d.motion { 0.5 } else { 0.0 });
            p.line_segment([pos2(x, base + 2.0), pos2(x, plot.bottom())], Stroke::new(1.0, T.text_dim));
        }
        // The range's two ends, as the property's values.
        let unit = d.set.property.unit();
        let shown = |v: f32| if d.set.property == KeyProperty::FirstFrame { format!("{:.0}", v + 1.0) } else { format!("{v:.0}{unit}") };
        p.text(pos2(plot.left() - 6.0, y_of(1.0)), Align2::RIGHT_CENTER, shown(d.set.max), font(Type::DataSmall), T.text_dim);
        p.text(pos2(plot.left() - 6.0, base), Align2::RIGHT_CENTER, shown(d.set.min), font(Type::DataSmall), T.text_dim);
    }
    // Lip-sync: a row per mouth, filled on the frames that get it.
    if lips {
        for (r, v) in Viseme::ALL.iter().enumerate() {
            let top = plot.bottom() + 4.0 + r as f32 * ROW_H;
            let row = Rect::from_min_max(pos2(plot.left(), top), pos2(plot.right(), top + ROW_H - 2.0));
            p.rect_filled(row, theme::SQUARE, T.field);
            p.text(pos2(area.left(), row.center().y), Align2::LEFT_CENTER, crate::i18n::t(v.label()), font(Type::Caption), T.text_dim);
            let mut i = 0;
            while i < g.mouths.len() {
                let run = g.mouths.iter().skip(i).take_while(|m| g.mouths.get(i) == Some(*m)).count().max(1);
                if g.mouths.get(i) == Some(v) {
                    p.rect_filled(Rect::from_min_max(pos2(x_of(i as f32), row.top()), pos2((x_of((i + run) as f32) - 1.0).max(x_of(i as f32) + 1.0), row.bottom())), theme::SQUARE, T.accent);
                }
                i += run;
            }
        }
    }
    // The frame the stage shows.
    let at = d.frame.saturating_sub(g.from) as f32 + 0.5;
    p.line_segment([pos2(x_of(at), area.top()), pos2(x_of(at), area.bottom())], Stroke::new(1.0, T.playhead));
    let picked = (resp.clicked() || resp.dragged()).then(|| resp.interact_pointer_pos()).flatten()?;
    let i = (((picked.x - plot.left()) / per).floor().max(0.0) as usize).min(n - 1);
    Some(g.from + i as u32)
}

/// A drop-down of `(id, name)` choices, told apart from the dialog's others by `widget`; the id
/// picked.
fn choice(ui: &mut egui::Ui, widget: &str, name: &str, current: u64, list: &[(u64, String)], tip: &str) -> Option<u64> {
    let mut picked = None;
    let shown = list.iter().find(|(id, _)| *id == current).map_or_else(String::new, |(_, n)| n.clone());
    egui::ComboBox::from_id_salt(egui::Id::new(("audio_keys_choice", widget))).width(theme::fit(ui, 220.0)).selected_text(text(Type::Body, shown)).show_ui(ui, |ui| {
        for (id, n) in list {
            if ui.selectable_label(*id == current, text(Type::Body, n.clone())).clicked() && *id != current {
                picked = Some(*id);
            }
        }
    })
    .response
    .on_hover_text(tip)
    .set_name(name);
    picked
}

/// The dialog, when open.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut d) = app.audio_keys.take() else { return };
    // Its sound went away (it can't while the dialog holds the editor, but never draw stale).
    let Some(src) = source(app, d.sound) else {
        app.s.revert_gesture();
        app.s.end_gesture();
        return;
    };
    let g = graph(app, &mut d);
    let layer_list = layers(app, src.from, d.kind);
    let object_list = objects(app, d.layer, src.from, d.kind);
    let sound_list = sounds(app);
    let symbol = mouth_symbol(app, &d).map(|s| (s.timeline.frame_count().max(1), s.clone()));
    let stereo = src.sound.channels == 2;
    let drifts = can_drift(app, d.sound);
    let mut changed = false;
    let (mut ok, mut cancel) = (false, false);
    let mut retarget = false;
    let id = egui::Id::new(match d.kind {
        Kind::Keys => "audio_keys",
        Kind::Lips => "lip_sync",
    });
    // Beside the stage, with no dimming, so the preview stays in view.
    let area = egui::Modal::default_area(id).anchor(egui::Align2::RIGHT_TOP, [-10.0, 72.0]);
    egui::Modal::new(id).area(area).backdrop_color(egui::Color32::TRANSPARENT).show(ctx, |ui| {
        ui.set_width(430.0);
        ui.label(text(Type::Title, match d.kind {
            Kind::Keys => tr!("Convert Audio to Keyframes"),
            Kind::Lips => tr!("Auto Lip-Sync"),
        }));
        ui.label(text(Type::Caption, match d.kind {
            Kind::Keys => tr!("The sound's loudness on each frame keys a property of an object."),
            Kind::Lips => tr!("The speech picks a mouth for each frame: a first pass to polish by hand."),
        }));
        ui.add_space(6.0);
        // ---- the sound, and what it drives ----
        form_row(ui, FORM_LABEL_DIALOG, tr!("Sound"), |ui| {
            if sound_list.len() > 1 {
                let list: Vec<(u64, String)> = sound_list.iter().enumerate().map(|(i, (_, n))| (i as u64, n.clone())).collect();
                let current = sound_list.iter().position(|(at, _)| *at == d.sound).unwrap_or(0) as u64;
                if let Some(i) = choice(ui, "audio_keys_sound", tr!("Sound"), current, &list, tr!("The sound that is read: any sound on this timeline's keyframes"))
                    && let Some((at, _)) = sound_list.get(i as usize)
                {
                    d.sound = *at;
                    retarget = true;
                }
            } else {
                ui.label(text(Type::Body, src.sound.name.clone()));
            }
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Frames"), |ui| {
            ui.label(text(Type::Data, tr!("{from} to {to}", from = src.from + 1, to = src.to + 1)));
        });
        if drifts {
            // The keys are right for the frames; what is heard follows them only with Stream.
            theme::form_note(ui, FORM_LABEL_DIALOG, text(Type::Caption, tr!("After a timeline jump the keys can drift from this sound. Stream sync keeps them locked.")));
        }
        form_row(ui, FORM_LABEL_DIALOG, tr!("Layer"), |ui| {
            if let Some(l) = choice(ui, "audio_keys_layer", tr!("Layer that gets the keys"), d.layer, &layer_list, tr!("The layer whose object the sound drives: the keys are written on it")) {
                d.layer = l;
                retarget = true;
            }
        });
        if object_list.len() > 1 {
            form_row(ui, FORM_LABEL_DIALOG, tr!("Object"), |ui| {
                if let Some(e) = choice(ui, "audio_keys_object", tr!("Object that gets the keys"), d.element.unwrap_or(0), &object_list, tr!("The object on the layer that the sound drives")) {
                    d.element = Some(e);
                    d.mouths = None;
                    changed = true;
                }
            });
        }
        ui.add_space(4.0);
        match d.kind {
            Kind::Keys => changed |= key_settings(ui, &mut d, stereo, symbol.as_ref().map(|(n, _)| *n)),
            Kind::Lips => {
                let answer = d.answer.clone();
                changed |= lip_settings(ui, &mut d, symbol.as_ref().map(|(_, s)| s), answer);
            }
        }
        // ---- the preview ----
        ui.add_space(6.0);
        if let Some(g) = &g
            && let Some(f) = draw_graph(ui, &d, g)
        {
            d.frame = f;
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(text(Type::Label, tr!("Frame")));
            let mut f = d.frame + 1;
            if with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut f).range(src.from + 1..=src.to + 1).speed(0.2))).named(tr!("Frame shown on the stage")).changed() {
                d.frame = f.saturating_sub(1).clamp(src.from, src.to);
            }
            ui.add_space(8.0);
            let summary = match (&d.error, d.answer.as_ref().and_then(|a| a["keys"].as_u64())) {
                (Some(_), _) => String::new(),
                (None, Some(n)) if d.motion => trn!("{n} key on the motion tween's curve", "{n} keys on the motion tween's curve", n as usize),
                (None, Some(n)) => trn!("{n} keyframe", "{n} keyframes", n as usize),
                (None, None) => tr!("Preview is off").to_string(),
            };
            ui.label(text(Type::Caption, summary));
        });
        if theme::checkbox(ui, &mut d.preview, tr!("Preview on the stage and the timeline")).changed() {
            changed = true;
        }
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            ui.label(text(Type::Caption, e.clone()).color(T.error));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(tr!("Cancel")).min_size(vec2(96.0, TARGET + 4.0))).clicked() {
                cancel = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let run = egui::Button::new(text(Type::Body, tr!("OK")).strong()).fill(T.accent_dim).min_size(vec2(96.0, TARGET + 4.0));
                if ui.add(run).clicked() {
                    ok = true;
                }
            });
        });
        ui.add_space(2.0);
        ui.label(text(Type::Caption, tr!("Enter applies · Esc cancels · one undo step")));
    });
    let (esc, enter) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::Escape), i.consume_key(egui::Modifiers::NONE, Key::Enter)));
    cancel |= esc;
    ok |= enter && !cancel;
    let frame = d.frame;
    app.audio_keys = Some(d);
    if retarget {
        retargeted(app);
        changed = true;
    }
    if changed {
        fit_first_frame(app);
        preview(app);
    } else if app.s.frame != frame {
        app.s.set_frame(frame);
    }
    if cancel {
        close(app, false);
        app.status_ok(tr!("Cancelled"));
    } else if ok {
        finish(app);
    }
}

/// After the sound or the layer changed: the object is one of the layer's (the first when the
/// one before isn't there), the frame is one of the sound's, and the mouths are the symbol's.
fn retargeted(app: &mut App) {
    let Some(d) = &app.audio_keys else { return };
    let Some(src) = source(app, d.sound) else { return };
    let candidates = layers(app, src.from, d.kind);
    let layer = if candidates.iter().any(|(c, _)| *c == d.layer) { d.layer } else { candidates.first().map_or(d.layer, |(c, _)| *c) };
    let list = objects(app, layer, src.from, d.kind);
    let element = d.element.filter(|e| list.iter().any(|(id, _)| id == e)).or(list.first().map(|(id, _)| *id));
    let motion = d.kind == Kind::Keys && app.s.timeline().layers.iter().find(|l| l.id.0 == layer).and_then(|l| l.keyframe_at(src.from)).is_some_and(|k| k.motion.is_some());
    if let Some(d) = &mut app.audio_keys {
        (d.layer, d.element, d.mouths, d.motion) = (layer, element, None, motion);
        d.frame = d.frame.clamp(src.from, src.to);
    }
}

/// The settings of Convert Audio to Keyframes; whether one changed.
fn key_settings(ui: &mut egui::Ui, d: &mut Dialog, stereo: bool, symbol_frames: Option<u32>) -> bool {
    let mut changed = false;
    form_row(ui, FORM_LABEL_DIALOG, tr!("Property"), |ui| {
        egui::ComboBox::from_id_salt("audio_keys_property").width(theme::fit(ui, 160.0)).selected_text(text(Type::Body, crate::i18n::t(d.set.property.label()))).show_ui(ui, |ui| {
            for p in KeyProperty::ALL {
                // A first frame is a graphic symbol's.
                let fits = p != KeyProperty::FirstFrame || symbol_frames.is_some();
                let r = ui.add_enabled(fits, egui::Button::selectable(p == d.set.property, text(Type::Body, crate::i18n::t(p.label())))).on_disabled_hover_text(tr!("The object is not an instance of a graphic symbol"));
                if r.clicked() && p != d.set.property {
                    d.set.property = p;
                    (d.set.min, d.set.max) = p.defaults();
                    if p == KeyProperty::FirstFrame {
                        d.set.max = symbol_frames.map_or(1.0, |n| (n - 1) as f32);
                    }
                    changed = true;
                }
            }
        })
        .response
        .set_name(tr!("Property the sound drives"));
    });
    let p = d.set.property;
    let r = p.range();
    // A symbol's frames are counted from 1 here, as in Properties.
    let shift = if p == KeyProperty::FirstFrame { 1.0 } else { 0.0 };
    let (lo, hi) = match (p, symbol_frames) {
        (KeyProperty::FirstFrame, Some(n)) => (1.0, n as f32),
        _ => (*r.start() + shift, *r.end() + shift),
    };
    let unit = match p.unit() {
        "" => String::new(),
        u => format!(" {u}"),
    };
    let (speed, decimals) = if p == KeyProperty::FirstFrame { (0.1, 0) } else { (0.5, 1) };
    for (label, tip, name, v) in [
        (tr!("In silence"), tr!("The property's value where nothing sounds (or the sound is under the threshold)"), tr!("Value in silence"), &mut d.set.min),
        (tr!("At the loudest"), tr!("The property's value on the sound's loudest frame"), tr!("Value at the loudest frame"), &mut d.set.max),
    ] {
        form_row(ui, FORM_LABEL_DIALOG, label, |ui| {
            let mut shown = *v + shift;
            if with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut shown).range(lo..=hi).speed(speed).max_decimals(decimals).suffix(unit.clone()))).named(name).on_hover_text(tip).changed() {
                *v = shown - shift;
                changed = true;
            }
        });
    }
    form_row(ui, FORM_LABEL_DIALOG, tr!("Threshold"), |ui| {
        let mut pct = f64::from(d.set.threshold) * 100.0;
        let r = with_type(ui, Type::Data, |ui| crate::a11y::slider(ui, &mut pct, 0.0..=99.0, tr!("Threshold"), crate::a11y::SliderLook { suffix: " %", max_decimals: Some(0), ..Default::default() }));
        if r.on_hover_text(tr!("Frames quieter than this share of the loudest frame count as silence: raise it to ignore background noise")).changed() {
            d.set.threshold = (pct / 100.0) as f32;
            changed = true;
        }
    });
    form_row(ui, FORM_LABEL_DIALOG, tr!("Smoothing"), |ui| {
        let r = with_type(ui, Type::Data, |ui| crate::a11y::slider(ui, &mut d.set.smoothing, 0..=MAX_SMOOTHING, tr!("Smoothing"), crate::a11y::SliderLook { suffix: " f", max_decimals: Some(0), ..Default::default() }));
        changed |= r.on_hover_text(tr!("Each frame becomes the average of itself and this many frames either side: higher is calmer, 0 follows every flick")).changed();
    });
    if stereo {
        form_row(ui, FORM_LABEL_DIALOG, tr!("Channel"), |ui| {
            egui::ComboBox::from_id_salt("audio_keys_channel").width(theme::fit(ui, 160.0)).selected_text(text(Type::Body, crate::i18n::t(d.set.channel.label()))).show_ui(ui, |ui| {
                for c in Channel::ALL {
                    if ui.selectable_label(c == d.set.channel, text(Type::Body, crate::i18n::t(c.label()))).clicked() && c != d.set.channel {
                        d.set.channel = c;
                        changed = true;
                    }
                }
            })
            .response
            .set_name(tr!("Channel measured"));
        });
    }
    form_row(ui, FORM_LABEL_DIALOG, tr!("Key every"), |ui| {
        let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.set.step).range(1..=MAX_STEP).speed(0.1).suffix(" f"))).named(tr!("Frames between keys"));
        changed |= r.on_hover_text(tr!("1 keys every frame that changes; 2 animates on twos, and so on")).changed();
    });
    form_row(ui, FORM_LABEL_DIALOG, tr!("Write"), |ui| {
        ui.vertical(|ui| {
            changed |= ui.radio_value(&mut d.motion, false, tr!("Keyframes")).on_hover_text(tr!("A keyframe on the layer wherever the value changes, as if you had set each by hand")).changed();
            let curve = ui.add_enabled(p != KeyProperty::FirstFrame, egui::RadioButton::new(d.motion, tr!("Motion tween curve")));
            let curve = curve.on_hover_text(tr!("The property's curve in the motion tween on the span (one is made for a lone object): the span stays whole, and the Motion Editor shows the curve")).on_disabled_hover_text(tr!("A symbol's first frame is set with keyframes"));
            if curve.clicked() && !d.motion {
                d.motion = true;
                changed = true;
            }
        });
    });
    changed
}

/// The settings of Auto Lip-Sync; whether one changed.
fn lip_settings(ui: &mut egui::Ui, d: &mut Dialog, symbol: Option<&Symbol>, answer: Option<Value>) -> bool {
    let mut changed = false;
    form_row(ui, FORM_LABEL_DIALOG, tr!("Silence below"), |ui| {
        let mut pct = f64::from(d.set.lip_threshold) * 100.0;
        let r = with_type(ui, Type::Data, |ui| crate::a11y::slider(ui, &mut pct, 0.0..=99.0, tr!("Silence below"), crate::a11y::SliderLook { suffix: " %", max_decimals: Some(0), ..Default::default() }));
        if r.on_hover_text(tr!("Frames quieter than this share of the loudest frame are silence: the mouth rests, or closes for a short stop")).changed() {
            d.set.lip_threshold = (pct / 100.0) as f32;
            changed = true;
        }
    });
    form_row(ui, FORM_LABEL_DIALOG, tr!("Hold each mouth"), |ui| {
        let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.set.hold).range(1..=MAX_HOLD).speed(0.1).suffix(" f"))).named(tr!("Fewest frames a mouth is held"));
        changed |= r.on_hover_text(tr!("The fewest frames a mouth is held, so it doesn't flicker; lips together are kept however short")).changed();
    });
    // Each mouth's frame of the symbol: by its frame labels until one is changed.
    let Some(sym) = symbol else { return changed };
    let frames = sym.timeline.frame_count().max(1);
    let label_of = |f: u32| sym.timeline.layers.iter().flat_map(|l| l.keyframes.iter()).find(|k| k.start == f && !k.label.is_empty()).map(|k| k.label.clone());
    let frame_name = |f: u32| match label_of(f) {
        Some(l) => tr!("{frame}: {label}", frame = f + 1, label = l),
        None => format!("{}", f + 1),
    };
    let current = d.mouths.or_else(|| {
        let m = answer.as_ref()?.get("mouths")?;
        let mut out = [0u32; 5];
        for (slot, v) in out.iter_mut().zip(Viseme::ALL) {
            *slot = m.get(v.key())?.as_u64()? as u32;
        }
        Some(out)
    });
    let current = current.unwrap_or_else(|| keys::default_mouths(sym));
    ui.add_space(2.0);
    ui.label(text(Type::Label, tr!("Mouths: the frame of {symbol} for each sound", symbol = sym.name)));
    let list: Vec<(u64, String)> = (0..frames.min(500)).map(|f| (u64::from(f), frame_name(f))).collect();
    for (i, v) in Viseme::ALL.iter().enumerate() {
        form_row(ui, FORM_LABEL_DIALOG, crate::i18n::t(v.label()), |ui| {
            let now = current.get(i).copied().unwrap_or(0);
            let name = tr!("Frame for the {mouth} mouth", mouth = crate::i18n::t(v.label()));
            let name = name.as_str();
            if let Some(f) = choice(ui, v.key(), name, u64::from(now), &list, crate::i18n::t(v.hint())) {
                let mut m = current;
                if let Some(slot) = m.get_mut(i) {
                    *slot = f as u32;
                }
                d.mouths = Some(m);
                changed = true;
            }
        });
    }
    changed
}

/// Closes the dialog: `keep` keeps what the stage shows as one undo step, else it is taken back.
fn close(app: &mut App, keep: bool) {
    if !keep {
        app.s.revert_gesture();
    }
    app.s.end_gesture();
    if let Some(d) = app.audio_keys.take() {
        app.audio_keys_last = d.set;
    }
}

/// OK: makes sure the document holds the final values, then closes the step.
pub fn finish(app: &mut App) {
    let Some(d) = &mut app.audio_keys else { return };
    if d.applied.as_ref() != Some(&params(d)) {
        d.preview = true;
        preview(app);
    }
    let Some(d) = &app.audio_keys else { return };
    if d.error.is_some() {
        return;
    }
    let (kind, n, frame) = (d.kind, d.answer.as_ref().and_then(|a| a["keys"].as_u64()).unwrap_or(0) as usize, d.frame);
    close(app, true);
    app.s.set_frame(frame);
    let undo = app.undo_key();
    app.status_ok(match kind {
        Kind::Keys => trn!("{n} key from the sound. {undo} to undo.", "{n} keys from the sound. {undo} to undo.", n, undo = undo),
        Kind::Lips => trn!("{n} mouth keyframe from the speech. {undo} to undo.", "{n} mouth keyframes from the speech. {undo} to undo.", n, undo = undo),
    });
}

/// Points the open dialog at `layer` and shows `frame` (screenshots).
pub fn aim(app: &mut App, layer: u64, frame: u32) {
    let Some(d) = &mut app.audio_keys else { return };
    (d.layer, d.frame) = (layer, frame);
    retargeted(app);
    fit_first_frame(app);
    preview(app);
}

/// Cancel, from outside the dialog (a document closing under it).
pub fn cancel(app: &mut App) {
    if app.audio_keys.is_some() {
        close(app, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::{Document, LoopMode};

    /// A second of sound at 24 kHz: twelve silent frames, six of a 480 Hz tone, six silent.
    fn burst() -> Vec<f32> {
        (0..24_000).map(|i| if (12_000..18_000).contains(&i) { 0.5 * (std::f32::consts::TAU * 480.0 * i as f32 / 24_000.0).sin() } else { 0.0 }).collect()
    }

    fn import(app: &mut App, tag: &str, samples: &[f32]) {
        let path = std::env::temp_dir().join(format!("anicraft-audio-keys-ui-{tag}-{}.wav", std::process::id()));
        std::fs::write(&path, anicraft_audio::wav::encode(24_000, 1, samples, anicraft_audio::wav::Bits::Int16)).expect("the test sound is written");
        app.run("file.import", json!({"path": path.display().to_string()}));
        app.run("sound.set", json!({"frame": 0, "sync": "stream"}));
        let _ = std::fs::remove_file(&path);
    }

    /// A square on the first layer and the burst on a layer above it.
    fn app(ctx: &egui::Context, tag: &str) -> (App, u64) {
        let mut app = App::with_context(ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        app.run("doc.set_stage", json!({"fps": 24}));
        app.run("draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": "#3366cc", "stroke": null, "object": true}));
        let art = app.s.layer.0;
        import(&mut app, tag, &burst());
        (app, art)
    }

    fn starts(app: &App, layer: u64) -> Vec<u32> {
        app.s.timeline().layers.iter().find(|l| l.id.0 == layer).map(|l| l.keyframes.iter().map(|k| k.start).collect()).unwrap_or_default()
    }

    #[test]
    fn the_dialog_previews_live_and_ok_is_one_undo_step() {
        let ctx = egui::Context::default();
        let (mut app, art) = app(&ctx, "live");
        let before = app.s.doc.clone();
        open(&mut app, Kind::Keys);
        let d = app.audio_keys.as_mut().expect("the dialog opens on the sound at the playhead");
        assert_eq!((d.layer, d.kind, d.motion), (art, Kind::Keys, false), "the layer with the square is offered");
        // The preview is on the timeline at once.
        assert!(starts(&app, art).len() > 1, "{:?}", starts(&app, art));
        // A change shows at once: no smoothing, keys only where the burst starts and stops.
        let d = app.audio_keys.as_mut().expect("open");
        (d.set.smoothing, d.set.threshold, d.set.max) = (0, 0.1, 200.0);
        preview(&mut app);
        assert_eq!(starts(&app, art), vec![0, 12, 18]);
        assert_eq!(app.audio_keys.as_ref().and_then(|d| d.answer.as_ref().and_then(|a| a["keys"].as_u64())), Some(3));
        // Drawing it changes nothing, and the graph marks the three keys.
        let shown = app.s.doc.clone();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.s.doc == shown && app.audio_keys.is_some());
        let g = app.audio_keys.as_ref().and_then(|d| d.graph.as_ref().map(|(_, g)| g.clone())).expect("the graph");
        assert_eq!((g.raw.len(), g.ticks.clone()), (24, vec![0, 12, 18]));
        assert!(g.raw.iter().enumerate().all(|(i, v)| (*v > 0.9) == (12..18).contains(&i)));
        // Preview off: the document as it was, the dialog still open.
        app.audio_keys.as_mut().expect("open").preview = false;
        preview(&mut app);
        assert!(app.s.doc == before);
        // OK applies the values whatever the preview showed: one undo step, values remembered.
        finish(&mut app);
        assert!(app.audio_keys.is_none());
        assert_eq!(starts(&app, art), vec![0, 12, 18]);
        assert_eq!(app.s.undo_label(), Some("Convert Audio to Keyframes"));
        assert_eq!((app.audio_keys_last.smoothing, app.audio_keys_last.max), (0, 200.0));
        app.run("edit.undo", json!({}));
        assert!(app.s.doc == before, "one undo step");
    }

    /// ADR 0114's follow-up: both dialogs say when the sound isn't a Stream sound, and say
    /// nothing when it is.
    #[test]
    fn a_sound_that_is_not_stream_synced_gets_a_note_about_drift() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (mut app, _) = app(&ctx, "drift");
        let sound = app.s.layer.0;
        let noted = |app: &mut App, ctx: &egui::Context| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(app, ui.ctx()));
            out.textures_delta.clear();
            out.platform_output.accesskit_update.is_some_and(|t| t.nodes.iter().any(|(_, n)| n.value().is_some_and(|v| v.contains("Stream sync keeps them locked"))))
        };
        for kind in [Kind::Keys, Kind::Lips] {
            app.run("sound.set", json!({"frame": 0, "sync": "stream"}));
            open(&mut app, kind);
            // Auto Lip-Sync opens only with a graphic symbol to drive; the note is in the part
            // of the dialog both share, so Convert Audio to Keyframes covers it here.
            let Some(at) = app.audio_keys.as_ref().map(|d| d.sound) else {
                assert_eq!(kind, Kind::Lips, "Convert Audio to Keyframes opens on the square");
                continue;
            };
            assert_eq!(at.0, sound);
            assert!(!can_drift(&app, at) && !noted(&mut app, &ctx), "{kind:?}: a stream sound stays in time");
            cancel(&mut app);
            for sync in ["event", "start"] {
                app.run("sound.set", json!({"frame": 0, "sync": sync}));
                open(&mut app, kind);
                assert!(can_drift(&app, at) && noted(&mut app, &ctx), "{kind:?}, {sync}");
                cancel(&mut app);
            }
        }
        assert!(!can_drift(&app, (9999, 0)), "no such sound: nothing to say");
    }

    #[test]
    fn cancel_takes_the_preview_back_and_leaves_no_step() {
        let ctx = egui::Context::default();
        let (mut app, art) = app(&ctx, "cancel");
        let (before, steps) = (app.s.doc.clone(), app.s.history_len());
        open(&mut app, Kind::Keys);
        assert!(starts(&app, art).len() > 1);
        // Moving the frame shown is not an edit, and survives the next preview.
        app.audio_keys.as_mut().expect("open").frame = 14;
        preview(&mut app);
        assert_eq!(app.s.frame, 14);
        cancel(&mut app);
        assert!(app.audio_keys.is_none() && app.s.doc == before && app.s.history_len() == steps && !app.s.in_gesture());
        // Nothing to animate: no dialog, and the reason in the status bar.
        let mut bare = App::with_context(&ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        open(&mut bare, Kind::Keys);
        assert!(bare.audio_keys.is_none() && bare.status.as_ref().is_some_and(|s| s.error), "no sound");
        import(&mut bare, "bare", &burst());
        open(&mut bare, Kind::Lips);
        assert!(bare.audio_keys.is_none() && bare.status.as_ref().is_some_and(|s| s.error), "no mouth symbol");
        assert!(!bare.s.in_gesture());
    }

    #[test]
    fn lip_sync_offers_the_mouth_chart_and_its_labelled_frames() {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        app.run("file.new_from_template", json!({"template": "lip-sync"}));
        let mouth = app.s.timeline().layers.iter().find(|l| l.name == "Mouth").map(|l| l.id.0).expect("the Mouth layer");
        // "ah" for six frames after twelve of silence.
        let voice: Vec<f32> = (0..24_000).map(|i| if (12_000..18_000).contains(&i) { let t = i as f32 / 24_000.0 * std::f32::consts::TAU; 0.6 * (730.0 * t).sin() + 0.3 * (1090.0 * t).sin() } else { 0.0 }).collect();
        import(&mut app, "lips", &voice);
        open(&mut app, Kind::Lips);
        let d = app.audio_keys.as_ref().expect("the dialog opens");
        assert_eq!((d.layer, d.error.clone()), (mouth, None), "the layer with the mouth symbol, not the guide's");
        assert_eq!(d.answer.as_ref().map(|a| a["mouths"].clone()), Some(json!({"rest": 0, "open": 1, "wide": 2, "round": 3, "closed": 8})));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let g = app.audio_keys.as_ref().and_then(|d| d.graph.as_ref().map(|(_, g)| g.clone())).expect("the graph");
        assert!(g.mouths.iter().enumerate().all(|(i, m)| (*m == Viseme::Open) == (12..18).contains(&i)), "{:?}", g.mouths);
        // Another frame for the open mouth shows at once.
        app.audio_keys.as_mut().expect("open").mouths = Some([0, 6, 2, 3, 8]);
        preview(&mut app);
        let shown = |app: &App, f: u32| app.s.timeline().layers.iter().find(|l| l.id.0 == mouth).and_then(|l| l.keyframe_at(f)).and_then(|k| k.elements.first().map(|e| e.kind.clone()));
        assert!(matches!(shown(&app, 14), Some(ElementKind::Symbol { first_frame: 6, loop_mode: LoopMode::SingleFrame, .. })));
        finish(&mut app);
        assert_eq!(app.s.undo_label(), Some("Auto Lip-Sync"));
        assert_eq!(starts(&app, mouth), vec![0, 12, 18]);
    }

    #[test]
    fn every_label_the_dialog_lists_is_in_the_catalog_table() {
        let listed = KeyProperty::ALL.map(KeyProperty::label).into_iter().chain(Channel::ALL.map(Channel::label)).chain(Viseme::ALL.map(Viseme::label)).chain(Viseme::ALL.map(Viseme::hint));
        for s in listed {
            assert!(TEXTS.contains(&s), "{s:?} is listed for translation");
        }
    }

    #[test]
    fn values_out_of_range_are_brought_into_it() {
        let d = Dialog { kind: Kind::Keys, sound: (1, 0), layer: 2, element: Some(3), set: Settings { property: KeyProperty::Alpha, min: -40.0, max: 900.0, threshold: 4.0, smoothing: 99, channel: Channel::Left, step: 0, lip_threshold: 9.0, hold: 0 }, motion: true, mouths: None, preview: true, applied: None, answer: None, error: None, frame: 0, graph: None };
        assert_eq!(params(&d), json!({"sound_layer": 1, "sound_frame": 0, "layer": 2, "element": 3, "property": "alpha", "min": 0.0f32, "max": 100.0f32, "threshold": 0.99f32, "smoothing": 24, "channel": "left", "step": 1, "output": "motion"}));
        let d = Dialog { kind: Kind::Lips, mouths: Some([0, 1, 2, 3, 4]), element: None, ..d };
        assert_eq!(params(&d), json!({"sound_layer": 1, "sound_frame": 0, "layer": 2, "threshold": 0.99f32, "hold": 1, "mouths": {"rest": 0, "open": 1, "wide": 2, "round": 3, "closed": 4}}));
    }
}
