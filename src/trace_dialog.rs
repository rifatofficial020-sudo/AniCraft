//! Modify › Bitmap › Trace Bitmap… (#463): Animate's dialog. Colour threshold, minimum area,
//! curve fit and corner threshold; OK runs one `modify.trace_bitmap`, one undo step. What was
//! entered last is offered again (view state).

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{FORM_LABEL_DIALOG, Type, form_row, text, with_type};
use anicraft_engine::trace::{Corners, CurveFit, TraceSettings};
use egui::Key;
use serde_json::json;

/// The dialog while it is open.
pub struct TraceDialog {
    pub values: TraceSettings,
    focus: bool,
}

/// Opens the dialog for the selection. Without a selected bitmap the engine says why instead.
pub fn open(app: &mut App) {
    if !has_bitmap(app) {
        app.request("modify.trace_bitmap", json!({}));
        return;
    }
    app.playing = false;
    app.trace = Some(TraceDialog { values: app.trace_last, focus: true });
}

/// Whether a placed bitmap (or a shape filled with one) is selected.
pub fn has_bitmap(app: &App) -> bool {
    use anicraft_engine::doc::{ElementKind, Paint};
    app.s.selected_elements().iter().any(|(_, e)| matches!(&e.kind, ElementKind::Shape(sh) if matches!(sh.fill, Some(Paint::Bitmap { .. }))))
}

/// The params a dialog's values run.
pub fn params(v: &TraceSettings) -> serde_json::Value {
    json!({
        "color_threshold": v.color_threshold.clamp(1, 500),
        "min_area": v.min_area.clamp(1, 1000),
        "curve_fit": v.curve_fit.key(),
        "corner_threshold": v.corners.key(),
    })
}

fn fit_label(c: CurveFit) -> &'static str {
    match c {
        CurveFit::Pixels => tr!("Pixels"),
        CurveFit::VeryTight => tr!("Very Tight"),
        CurveFit::Tight => tr!("Tight"),
        CurveFit::Normal => tr!("Normal"),
        CurveFit::Smooth => tr!("Smooth"),
        CurveFit::VerySmooth => tr!("Very Smooth"),
    }
}

fn corner_label(c: Corners) -> &'static str {
    match c {
        Corners::Many => tr!("Many corners"),
        Corners::Normal => tr!("Normal"),
        Corners::Few => tr!("Few corners"),
    }
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(d) = &mut app.trace else { return };
    let (mut submit, mut cancel) = (false, false);
    crate::windows::new(ctx, tr!("Trace Bitmap"))
        .id(egui::Id::new("trace_bitmap"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0])
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            form_row(ui, FORM_LABEL_DIALOG, tr!("Colour threshold"), |ui| {
                let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.values.color_threshold).range(1..=500).speed(1.0)))
                    .named(tr!("Colour threshold"))
                    .on_hover_text(tr!("Neighbouring pixels whose colours differ by less than this are one colour: higher makes fewer shapes"));
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
            form_row(ui, FORM_LABEL_DIALOG, tr!("Minimum area"), |ui| {
                let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.values.min_area).range(1..=1000).suffix(" px").speed(0.5)))
                    .named(tr!("Minimum area"))
                    .on_hover_text(tr!("Areas of colour smaller than this many pixels join their neighbour"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
            form_row(ui, FORM_LABEL_DIALOG, tr!("Curve fit"), |ui| {
                egui::ComboBox::from_id_salt("trace_curve_fit")
                    .width(180.0)
                    .selected_text(fit_label(d.values.curve_fit))
                    .show_ui(ui, |ui| {
                        for c in CurveFit::ALL {
                            ui.selectable_value(&mut d.values.curve_fit, c, fit_label(c));
                        }
                    })
                    .response
                    .on_hover_text(tr!("How closely the outlines follow the pixels: Pixels follows them exactly, Very Smooth rounds them most"))
                    .set_name(tr!("Curve fit"));
            });
            form_row(ui, FORM_LABEL_DIALOG, tr!("Corner threshold"), |ui| {
                egui::ComboBox::from_id_salt("trace_corners")
                    .width(180.0)
                    .selected_text(corner_label(d.values.corners))
                    .show_ui(ui, |ui| {
                        for c in Corners::ALL {
                            ui.selectable_value(&mut d.values.corners, c, corner_label(c));
                        }
                    })
                    .response
                    .on_hover_text(tr!("Whether sharp turns in the outlines stay corners or are smoothed"))
                    .set_name(tr!("Corner threshold"));
            });
            ui.label(text(
                Type::Caption,
                tr!("The selected bitmaps become filled shapes, one for each area of colour. Pixels under half opacity stay empty."),
            ));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button(tr!("OK")).clicked() {
                    submit = true;
                }
                if ui.button(tr!("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
            });
        });
    if submit {
        accept(app);
    } else if cancel {
        app.trace = None;
    }
}

/// OK: traces with the dialog's values and remembers them for the next time.
fn accept(app: &mut App) {
    let Some(d) = app.trace.take() else { return };
    app.trace_last = d.values;
    app.request("modify.trace_bitmap", params(&d.values));
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::{Bitmap, BitmapId, Document, ElementKind, Paint};
    use std::sync::Arc;

    fn app(ctx: &egui::Context) -> App {
        let mut s = Session::new(Document::default());
        let id = BitmapId(s.doc.alloc_id());
        let rgba: Vec<u8> = (0..16 * 16).flat_map(|i| if i % 16 < 8 { [200, 20, 20, 255] } else { [20, 20, 200, 255] }).collect();
        s.doc.bitmaps.push(Arc::new(Bitmap::from_rgba(id, "two.png", 16, 16, rgba).unwrap()));
        let mut app = App::with_context(ctx, s, Some(std::path::PathBuf::from("unused.png")));
        app.run("library.place", json!({"bitmap": id.0}));
        app
    }

    fn fills(app: &App) -> usize {
        app.s.visible_elements().iter().filter(|e| matches!(&e.kind, ElementKind::Shape(sh) if matches!(sh.fill, Some(Paint::Solid(_))))).count()
    }

    #[test]
    fn the_dialog_traces_with_its_values_and_remembers_them() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        open(&mut app);
        let d = app.trace.as_mut().expect("the dialog opens on a selected bitmap");
        assert_eq!(d.values, TraceSettings::default(), "Animate's defaults the first time");
        d.values.curve_fit = CurveFit::Pixels;
        d.values.min_area = 4;
        let before = app.s.doc.clone();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert_eq!(app.s.doc, before, "drawing the dialog changes nothing");
        accept(&mut app);
        assert!(app.trace.is_none());
        assert_eq!(fills(&app), 2);
        assert_eq!(app.s.undo_label(), Some("Trace Bitmap…"));
        assert_eq!(app.trace_last.curve_fit, CurveFit::Pixels);
        app.run("edit.undo", json!({}));
        assert_eq!(fills(&app), 0, "one undo step");
        open(&mut app);
        assert_eq!(app.trace.as_ref().map(|d| d.values.min_area), Some(4), "offered again");
        // Nothing to trace: no dialog; the engine says why.
        app.trace = None;
        app.run("edit.deselect", json!({}));
        open(&mut app);
        assert!(app.trace.is_none());
        assert_eq!(
            params(&TraceSettings { color_threshold: 900, min_area: 0, ..TraceSettings::default() }),
            json!({"color_threshold": 500, "min_area": 1, "curve_fit": "normal", "corner_threshold": "normal"})
        );
    }
}
