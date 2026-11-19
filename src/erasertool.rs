//! The Eraser (E) and the Eyedropper (I), ADR 0034: view state over `draw.erase`, `draw.faucet`
//! and `paint.pick`. An eraser drag collects samples like the Brush, shows the stage colour
//! where it goes and sends one command when the pointer lifts, so a stroke is one undo step.

use crate::app::{App, Drag, Tool};
use crate::brushtool::{BrushOptions, draw_sweep, draw_tip, points, shape_key};
use crate::pathtools::Xf;
use crate::theme::to_color32;
use anicraft_engine::cmds_erase::EraseMode;
use anicraft_engine::doc::ElementKind;
use anicraft_engine::geom::Vec2;
use anicraft_engine::geom::brush::TipShape;
use egui::Pos2;
use serde_json::json;

/// The Eraser's options (view state: the tool's, not the document's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EraserOptions {
    /// Diameter (or side), stage units.
    pub size: f32,
    pub shape: TipShape,
    pub mode: EraseMode,
    /// The Faucet: a click deletes a whole fill or line.
    pub faucet: bool,
}

impl Default for EraserOptions {
    fn default() -> Self {
        EraserOptions { size: 16.0, shape: TipShape::Round, mode: EraseMode::Normal, faucet: false }
    }
}

impl EraserOptions {
    /// The tip as the Brush draws it (for the shared previews).
    fn tip(&self) -> BrushOptions {
        BrushOptions { size: self.size, shape: self.shape, ..BrushOptions::default() }
    }
}

/// Starts an eraser stroke at stage point `p` (the eraser ignores pen pressure).
pub fn start(app: &mut App, p: Vec2) -> Drag {
    app.pen_pressure = None;
    app.brush_samples = vec![(p, None)];
    Drag::Eraser
}

/// Sends the stroke: one command, one undo step.
pub fn commit(app: &mut App, xf: Xf) {
    let samples = std::mem::take(&mut app.brush_samples);
    if samples.is_empty() {
        return;
    }
    let o = app.eraser;
    let params = json!({"points": points(&samples), "size": o.size, "shape": shape_key(o.shape), "mode": o.mode.key(), "unit": xf.unit()});
    let undo = app.undo_key();
    // What the stroke passed over and kept (ADR 0093): symbols and drawing objects, which open
    // in place with a double-click so their own lines and fills can be erased.
    let kept = |v: &serde_json::Value| (v["kept"]["symbols"].as_u64().unwrap_or(0), v["kept"]["objects"].as_u64().unwrap_or(0));
    match app.run("draw.erase", params) {
        Some(v) if v["erased"] == true => match kept(&v) {
            (0, 0) => app.status_ok(tr!("Erased. {undo} to undo.", undo = undo)),
            _ => app.status_ok(tr!("Erased the lines and fills. {undo} to undo. Symbols and drawing objects are kept: double-click one with the Selection tool to erase inside it.", undo = undo)),
        },
        Some(v) if kept(&v) != (0, 0) => {
            let k = kept(&v);
            app.status_ok(match k {
                (_, 0) => tr!("Nothing erased: on the stage the Eraser keeps symbols. Double-click one with the Selection tool to erase inside it."),
                (0, _) => tr!("Nothing erased: the Eraser keeps drawing objects. Double-click one with the Selection tool to erase inside it."),
                _ => tr!("Nothing erased: the Eraser keeps symbols and drawing objects. Double-click one with the Selection tool to erase inside it."),
            });
        }
        Some(v) => {
            // The engine's reason is English from below the shell.
            let why = v["reason"].as_str().map_or(std::borrow::Cow::Borrowed(tr!("nothing to erase there")), crate::i18n::t_dyn);
            app.status_ok(tr!("Nothing erased: {why}. The eraser works on lines and fills drawn with Object Drawing off; drawing objects and symbols are kept.", why = why));
        }
        None => {}
    }
}

/// The Faucet: deletes the whole fill or line under the click.
pub fn faucet(app: &mut App, p: Vec2, zoom: f32) {
    if let Some(v) = app.run("draw.faucet", json!({"x": p.x, "y": p.y, "slop": 4.0 / zoom.max(1e-3)})) {
        let undo = app.undo_key();
        app.status_ok(if v["kind"] == "line" { tr!("Deleted the line. {undo} to undo.", undo = undo) } else { tr!("Deleted the fill. {undo} to undo.", undo = undo) });
    }
}

/// Double-clicking the Eraser tool: erase everything on the stage at this frame (asks first).
pub fn erase_everything(app: &mut App) {
    // The engine asks only when there is something to erase.
    if app.s.confirmation("draw.erase_all", &json!({})).is_none() {
        app.status_ok(tr!("Nothing to erase: the visible, unlocked layers are empty at this frame."));
        return;
    }
    app.request("draw.erase_all", json!({}));
}

/// The stroke as it goes (in the stage colour, as if erased) and the tip under the pointer.
pub fn draw_overlay(app: &App, painter: &egui::Painter, xf: Xf, hover: Option<Pos2>) {
    if app.tool != Tool::Eraser {
        return;
    }
    let tip = app.eraser.tip();
    if app.drag == Drag::Eraser {
        let bg = to_color32(anicraft_engine::doc::Rgba { a: 1.0, ..app.s.doc.stage.background });
        draw_sweep(painter, xf, &tip, &app.brush_samples, bg);
    }
    if let Some(h) = hover
        && !app.eraser.faucet
    {
        draw_tip(painter, xf, &tip, xf.to_stage(h));
    }
}

/// The Eyedropper's click. It picks the fill or line under the point into the tool settings
/// (Shift: into the other swatch only). As in Animate, it then becomes the Paint Bucket (a
/// fill) or the Ink Bottle (a line), and with shapes selected it gives them the picked style.
pub fn eyedropper(app: &mut App, p: Vec2, shift: bool, zoom: f32) {
    let Some(v) = app.run("paint.pick", json!({"x": p.x, "y": p.y, "other": shift, "slop": 3.0 / zoom.max(1e-3)})) else { return };
    let color = v["color"].as_str().unwrap_or("").to_string();
    let into_fill = v["into"] == "fill";
    if shift {
        app.status_ok(if into_fill { tr!("Picked {color} into the fill colour.", color = color) } else { tr!("Picked {color} into the stroke colour.", color = color) });
        return;
    }
    // One whole sentence per kind of paint, so each translates as a sentence.
    let paint = v["paint"].as_str();
    let width = v["width"].as_f64().map_or(String::new(), |w| format!("{w:.1}"));
    // The selected shapes on every layer (ADR 0044).
    let shapes = app.s.selected_elements().iter().filter(|(_, e)| matches!(e.kind, ElementKind::Shape(_))).count();
    if shapes > 0 {
        if let Some(r) = app.run("paint.apply_style", json!({"fill": into_fill, "stroke": !into_fill})) {
            let n = r["changed"].as_u64().unwrap_or(0);
            let undo = app.undo_key();
            app.status_ok(match paint {
                Some("linear") => trn!("Applied the picked linear gradient to {n} shape. {undo} to undo.", "Applied the picked linear gradient to {n} shapes. {undo} to undo.", n, undo = undo),
                Some("radial") => trn!("Applied the picked radial gradient to {n} shape. {undo} to undo.", "Applied the picked radial gradient to {n} shapes. {undo} to undo.", n, undo = undo),
                Some("bitmap") => trn!("Applied the picked bitmap fill to {n} shape. {undo} to undo.", "Applied the picked bitmap fill to {n} shapes. {undo} to undo.", n, undo = undo),
                _ if into_fill => trn!("Applied the picked fill {color} to {n} shape. {undo} to undo.", "Applied the picked fill {color} to {n} shapes. {undo} to undo.", n, color = color, undo = undo),
                _ => trn!("Applied the picked line {color}, {width} px to {n} shape. {undo} to undo.", "Applied the picked line {color}, {width} px to {n} shapes. {undo} to undo.", n, color = color, width = width, undo = undo),
            });
        }
    } else {
        let picked = match paint {
            Some("linear") => tr!("Picked the linear gradient.").to_string(),
            Some("radial") => tr!("Picked the radial gradient.").to_string(),
            Some("bitmap") => tr!("Picked the bitmap fill.").to_string(),
            _ if into_fill => tr!("Picked the fill {color}.", color = color),
            _ => tr!("Picked the line {color}, {width} px.", color = color, width = width),
        };
        let next = if into_fill { tr!("Click a fill or an enclosed area to pour it.") } else { tr!("Click a shape to give it this line.") };
        app.status_ok(format!("{picked} {next}"));
    }
    app.tool = if into_fill { Tool::PaintBucket } else { Tool::InkBottle };
}

/// The colour picker's eyedropper on the stage (ADR 0109, #288): while a picker popup is open,
/// the pointer over the stage is an eyedropper. A chip beside it shows the colour drawn under
/// it (`paint.sample`: fills, gradients, bitmaps, lines and effects, as the CPU renderer draws
/// them), and a click hands that colour to the picker, which takes it and closes. Escape closes
/// the picker without picking, like any popup. `at` is the pointer on screen and in the space
/// being edited; `zoom` is screen points per unit there.
pub fn picker_dropper(app: &mut App, ui: &egui::Ui, resp: &egui::Response, picker: egui::Id, at: Option<(Pos2, Vec2)>, zoom: f32) {
    let Some((screen, p)) = at.filter(|_| resp.hovered()) else { return };
    let ctx = ui.ctx();
    crate::colour::over_stage(ctx);
    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    // One screen pixel there; sampled again only when the pointer, the view or the document
    // changes.
    let scale = (zoom * ctx.pixels_per_point()).clamp(0.01, 64.0);
    let key = (app.s.state, p.x.to_bits(), p.y.to_bits(), scale.to_bits());
    let cache = egui::Id::new("anicraft.stage.picker_dropper");
    type Sampled = ((anicraft_engine::StateId, u32, u32, u32), [u8; 3]);
    let known: Option<Sampled> = ctx.data(|d| d.get_temp(cache));
    let rgb = match known.filter(|(k, _)| *k == key) {
        Some((_, rgb)) => Some(rgb),
        None => {
            let sampled = app.s.execute("paint.sample", &json!({"x": p.x, "y": p.y, "scale": scale})).ok().and_then(|v| {
                let byte = |k: &str| v[k].as_u64().and_then(|b| u8::try_from(b).ok());
                Some([byte("r")?, byte("g")?, byte("b")?])
            });
            if let Some(rgb) = sampled {
                ctx.data_mut(|d| d.insert_temp::<Sampled>(cache, (key, rgb)));
            }
            sampled
        }
    };
    let Some(rgb) = rgb else { return };
    // The chip: the colour that a click takes, below and right of the pointer.
    let chip = egui::Rect::from_min_size(screen + egui::vec2(14.0, 14.0), egui::vec2(20.0, 20.0));
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, cache));
    let colour = anicraft_engine::doc::Rgba::new(f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0, 1.0);
    painter.rect_filled(chip.expand(1.0), crate::theme::T.rc(), crate::theme::T.text);
    painter.rect_filled(chip, crate::theme::T.rc(), to_color32(colour));
    painter.rect_stroke(chip, crate::theme::T.rc(), egui::Stroke::new(1.0, crate::theme::T.panel), egui::StrokeKind::Inside);
    if ui.input(|i| i.pointer.primary_clicked()) {
        crate::colour::pick_from_stage(ctx, picker, rgb);
        app.status_ok(tr!("Picked {colour} from the stage.", colour = crate::colour::to_hex([rgb[0], rgb[1], rgb[2], 255], false)));
    }
}

