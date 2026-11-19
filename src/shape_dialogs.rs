//! Modify › Shape › Expand Fill… and Soften Fill Edges… (ADR 0109): Animate's two small dialogs.
//! Distance, direction and (for a soft edge) the number of steps; OK runs one
//! `modify.expand_fill` or `modify.soften_fill_edges`, one undo step. What was entered last is
//! offered again (view state), so a run of fills gets the same edge.

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{FORM_LABEL_DIALOG, Type, form_row, text, with_type};
use anicraft_engine::cmds_shape::{DEFAULT_STEPS, MAX_DISTANCE};
use anicraft_engine::geom::outline::MAX_SOFTEN_STEPS;
use egui::Key;
use serde_json::json;

/// Which dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Expand,
    Soften,
}

impl Kind {
    fn command(self) -> &'static str {
        match self {
            Kind::Expand => "modify.expand_fill",
            Kind::Soften => "modify.soften_fill_edges",
        }
    }
}

/// What a dialog holds, and what the next one starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    /// Stage pixels.
    pub distance: f32,
    pub steps: u32,
    pub inset: bool,
}

impl Default for Values {
    /// Animate's: 4 px, 4 steps, Expand.
    fn default() -> Self {
        Values { distance: 4.0, steps: DEFAULT_STEPS, inset: false }
    }
}

/// The dialog while it is open.
pub struct ShapeDialog {
    pub kind: Kind,
    pub values: Values,
    /// The distance field takes the keyboard on the first frame.
    focus: bool,
}

/// What each dialog was last given.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Last {
    pub expand: Values,
    pub soften: Values,
}

/// Opens the dialog for the selection. With nothing selected the engine says so instead.
pub fn open(app: &mut App, kind: Kind) {
    if app.s.selection.is_empty() {
        app.request(kind.command(), json!({"distance": 1}));
        return;
    }
    app.playing = false;
    let values = match kind {
        Kind::Expand => app.shape_last.expand,
        Kind::Soften => app.shape_last.soften,
    };
    app.shape_dialog = Some(ShapeDialog { kind, values, focus: true });
}

/// The command and params a dialog's values run.
pub fn params(kind: Kind, v: &Values) -> serde_json::Value {
    let mut p = json!({"distance": v.distance.clamp(0.05, MAX_DISTANCE), "direction": if v.inset { "inset" } else { "expand" }});
    if kind == Kind::Soften {
        p["steps"] = json!(v.steps.clamp(1, MAX_SOFTEN_STEPS));
    }
    p
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(d) = &mut app.shape_dialog else { return };
    let (mut submit, mut cancel) = (false, false);
    let (title, id) = match d.kind {
        Kind::Expand => (tr!("Expand Fill"), "expand_fill"),
        Kind::Soften => (tr!("Soften Fill Edges"), "soften_fill_edges"),
    };
    crate::windows::new(ctx, title).id(egui::Id::new(id)).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
        ui.set_min_width(340.0);
        form_row(ui, FORM_LABEL_DIALOG, tr!("Distance"), |ui| {
            let tip = match d.kind {
                Kind::Expand => tr!("How far the fill's edge moves, in stage pixels"),
                Kind::Soften => tr!("How wide the soft edge is, in stage pixels"),
            };
            let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.values.distance).range(0.05..=MAX_DISTANCE).suffix(" px").speed(0.25).max_decimals(2))).named(tr!("Distance")).on_hover_text(tip);
            if d.focus {
                r.request_focus();
                d.focus = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
        });
        if d.kind == Kind::Soften {
            form_row(ui, FORM_LABEL_DIALOG, tr!("Steps"), |ui| {
                let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.values.steps).range(1..=MAX_SOFTEN_STEPS).speed(0.1)))
                    .named(tr!("Number of steps"))
                    .on_hover_text(tr!("How many bands the soft edge is made of: more are smoother, and more shapes to draw"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
        }
        form_row(ui, FORM_LABEL_DIALOG, tr!("Direction"), |ui| {
            ui.vertical(|ui| {
                let (out, inward) = match d.kind {
                    Kind::Expand => (tr!("The fill grows by the distance; its corners round"), tr!("The fill shrinks by the distance; its corners stay sharp")),
                    Kind::Soften => (tr!("The soft edge is added round the fill"), tr!("The soft edge is taken from the fill, which keeps its size")),
                };
                ui.radio_value(&mut d.values.inset, false, tr!("Expand")).on_hover_text(out);
                ui.radio_value(&mut d.values.inset, true, tr!("Inset")).on_hover_text(inward);
            });
        });
        if d.kind == Kind::Soften {
            ui.label(text(Type::Caption, tr!("Each step is a band of the fill's colour, fainter the farther out. Works best on a fill without an outline.")));
        }
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
        app.shape_dialog = None;
    }
}

/// OK: runs the command with the dialog's values and remembers them for the next time.
fn accept(app: &mut App) {
    let Some(d) = app.shape_dialog.take() else { return };
    match d.kind {
        Kind::Expand => app.shape_last.expand = d.values,
        Kind::Soften => app.shape_last.soften = d.values,
    }
    app.request(d.kind.command(), params(d.kind, &d.values));
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::{Document, ElementKind};

    fn app(ctx: &egui::Context) -> App {
        let mut app = App::with_context(ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        app.run("draw.rect", json!({"x": 100, "y": 100, "w": 200, "h": 100, "fill": "#3366cc", "stroke": null, "object": true}));
        app
    }

    fn shapes(app: &App) -> usize {
        app.s.visible_elements().iter().filter(|e| matches!(e.kind, ElementKind::Shape(_))).count()
    }

    #[test]
    fn the_dialogs_run_their_commands_and_remember_what_was_entered() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        open(&mut app, Kind::Expand);
        let d = app.shape_dialog.as_mut().expect("the dialog opens on a selection");
        assert_eq!(d.values, Values::default(), "Animate's defaults the first time");
        d.values = Values { distance: 12.0, steps: 9, inset: true };
        accept(&mut app);
        assert!(app.shape_dialog.is_none());
        let b = app.s.selection_bounds();
        assert!((b.width() - 176.0).abs() < 0.01 && (b.height() - 76.0).abs() < 0.01, "inset by 12: {b:?}");
        assert_eq!(app.s.undo_label(), Some("Expand Fill…"));
        // The next Expand Fill offers the same; Soften Fill Edges keeps its own.
        open(&mut app, Kind::Expand);
        assert_eq!(app.shape_dialog.as_ref().map(|d| d.values), Some(Values { distance: 12.0, steps: 9, inset: true }));
        app.shape_dialog = None;
        open(&mut app, Kind::Soften);
        let d = app.shape_dialog.as_mut().expect("the dialog opens");
        assert_eq!(d.values, Values::default());
        d.values.steps = 3;
        d.values.distance = 9.0;
        accept(&mut app);
        assert_eq!(shapes(&app), 4, "the fill and three bands");
        assert_eq!(app.shape_last.soften.steps, 3);
        app.run("edit.undo", json!({}));
        assert_eq!(shapes(&app), 1, "one undo step");
    }

    #[test]
    fn the_dialogs_draw_without_changing_anything_and_need_a_selection() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        for kind in [Kind::Expand, Kind::Soften] {
            open(&mut app, kind);
            let before = app.s.doc.clone();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
            out.textures_delta.clear();
            assert!(app.shape_dialog.is_some());
            assert_eq!(app.s.doc, before);
            app.shape_dialog = None;
        }
        // Nothing selected: no dialog; the engine says why.
        app.run("edit.deselect", json!({}));
        open(&mut app, Kind::Soften);
        assert!(app.shape_dialog.is_none());
        // Values out of range are brought into it.
        assert_eq!(params(Kind::Soften, &Values { distance: 0.0, steps: 900, inset: false }), json!({"distance": 0.05f32, "direction": "expand", "steps": 50}));
        assert_eq!(params(Kind::Expand, &Values { distance: 5000.0, steps: 2, inset: true }), json!({"distance": 1000.0f32, "direction": "inset"}));
    }
}
