//! Modify › Document Settings (Ctrl+J, #505): the stage's size, frame rate and colour in one
//! small dialog, as Animate's Document Settings. OK runs one `doc.set_stage`, one undo step;
//! Cancel changes nothing. The same values are in Properties while nothing is selected, where
//! they change live.

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{FORM_LABEL_DIALOG, Type, form_row, text, with_type};
use anicraft_engine::doc::Stage;
use egui::Key;
use serde_json::json;

/// The largest stage side and frame rate offered (as Properties offers them).
pub const MAX_EDGE: u32 = 8192;
pub const MAX_FPS: f32 = 120.0;

/// The dialog while it is open: what OK would set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub background: [u8; 3],
    /// The width field takes the keyboard on the first frame.
    focus: bool,
}

impl DocSettings {
    fn of(stage: &Stage) -> DocSettings {
        let c = stage.background.sanitized();
        let b = |v: f32| (v * 255.0).round() as u8;
        DocSettings { width: stage.width, height: stage.height, fps: stage.fps, background: [b(c.r), b(c.g), b(c.b)], focus: true }
    }
}

/// Opens the dialog on the document's settings.
pub fn open(app: &mut App) {
    app.playing = false;
    app.doc_settings = Some(DocSettings::of(&app.s.doc.stage));
}

/// What the dialog's values ask `doc.set_stage` for, brought into range.
pub fn params(d: &DocSettings) -> serde_json::Value {
    let [r, g, b] = d.background;
    json!({
        "width": d.width.clamp(1, MAX_EDGE),
        "height": d.height.clamp(1, MAX_EDGE),
        "fps": if d.fps.is_finite() { d.fps.clamp(1.0, MAX_FPS) } else { 24.0 },
        "background": format!("#{r:02x}{g:02x}{b:02x}"),
    })
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(d) = &mut app.doc_settings else { return };
    let (mut submit, mut cancel) = (false, false);
    crate::windows::new(ctx, tr!("Document Settings")).id(egui::Id::new("doc_settings")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
        ui.set_min_width(340.0);
        let entered = |ui: &egui::Ui, r: &egui::Response| r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        form_row(ui, FORM_LABEL_DIALOG, tr!("Stage size"), |ui| {
            let w = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.width).range(1..=MAX_EDGE).suffix(" px"))).named(tr!("Stage width")).on_hover_text(tr!("The stage's width, in pixels"));
            if d.focus {
                w.request_focus();
                d.focus = false;
            }
            submit |= entered(ui, &w);
            let h = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.height).range(1..=MAX_EDGE).suffix(" px"))).named(tr!("Stage height")).on_hover_text(tr!("The stage's height, in pixels"));
            submit |= entered(ui, &h);
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Frame rate"), |ui| {
            let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.fps).range(1.0..=MAX_FPS).suffix(" fps").speed(0.1).max_decimals(2))).named(tr!("Frame rate")).on_hover_text(tr!("Frames shown each second"));
            submit |= entered(ui, &r);
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Stage colour"), |ui| {
            crate::colour::button(ui, &mut d.background).named(tr!("Stage colour"));
        });
        ui.label(text(Type::Caption, tr!("The artwork keeps its place and size: the stage changes around it.")));
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
        app.doc_settings = None;
    }
}

/// OK: one `doc.set_stage` with the dialog's values (nothing is recorded when they are the
/// document's already).
fn accept(app: &mut App) {
    let Some(d) = app.doc_settings.take() else { return };
    app.request("doc.set_stage", params(&d));
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::Document;

    fn app(ctx: &egui::Context) -> App {
        App::with_context(ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")))
    }

    /// #505: Modify › Document Settings ran `doc.set_stage` with nothing to set, so nothing
    /// happened. The menu item (and Ctrl+J) opens the dialog; OK sets the stage in one step.
    #[test]
    fn the_menu_item_opens_the_dialog_and_ok_sets_the_stage_in_one_step() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let before = app.s.doc.stage;
        app.invoke("doc.set_stage");
        let d = app.doc_settings.as_mut().expect("Modify › Document Settings opens its dialog");
        assert_eq!((d.width, d.height, d.fps), (before.width, before.height, before.fps), "it starts from the document");
        assert!(app.modal_open());
        // Drawn, it changes nothing until OK.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert_eq!(app.s.doc.stage, before);
        let d = app.doc_settings.as_mut().expect("still open");
        (d.width, d.height, d.fps, d.background) = (1920, 1080, 30.0, [0x10, 0x20, 0x30]);
        accept(&mut app);
        assert!(app.doc_settings.is_none());
        let st = app.s.doc.stage;
        assert_eq!((st.width, st.height, st.fps, st.background.to_hex().as_str()), (1920, 1080, 30.0, "#102030"));
        assert_eq!(app.s.undo_label(), Some("Document Settings"));
        app.run("edit.undo", json!({}));
        assert_eq!(app.s.doc.stage, before, "one undo step");
        // Cancel (closing the dialog) leaves the document alone.
        open(&mut app);
        if let Some(d) = app.doc_settings.as_mut() {
            d.width = 64;
        }
        app.doc_settings = None;
        assert_eq!(app.s.doc.stage, before);
    }

    #[test]
    fn values_out_of_range_are_brought_into_it() {
        let d = DocSettings { width: 0, height: 99_999, fps: f32::NAN, background: [255, 0, 128], focus: false };
        assert_eq!(params(&d), json!({"width": 1, "height": 8192, "fps": 24.0f32, "background": "#ff0080"}));
        let d = DocSettings { fps: 900.0, ..d };
        assert_eq!(params(&d)["fps"], json!(120.0f32));
    }

    /// Animate's key for Document Settings.
    #[test]
    fn document_settings_has_animates_shortcut() {
        let spec = anicraft_engine::find_command("doc.set_stage").expect("the command exists");
        assert_eq!(spec.shortcut, Some("Ctrl+J"));
        assert_eq!(spec.menu, "Modify");
    }
}
