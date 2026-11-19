//! The timeline's parenting view (ADR 0113): a column in the layer list where each layer has a
//! dot, joined to its parent layer's dot by a line in its colour.
//!
//! Drag from a layer's dot to another layer to parent it; click the dot (or focus it and press
//! Enter or Space) to pick the parent from a menu, or remove it. The same menu is in every
//! layer's context menu, parenting view open or not. Everything runs `layer.set_parent`, so the
//! layer stays where it is on stage and the change is one undo step.

use crate::a11y;
use crate::app::App;
use crate::icons::{self, Icon};
use crate::menus;
use crate::theme::{T, to_color32};
use anicraft_engine::doc::{Layer, LayerId, Timeline};
use egui::{Pos2, Rect, Sense, Stroke, StrokeKind, WidgetType, pos2, vec2};
use serde_json::json;

/// Width of one step of the tree, and the most steps drawn (deeper chains share the last).
const LANE: f32 = 9.0;
const MAX_LANES: usize = 6;
/// The dot's radius.
const DOT: f32 = 4.5;

/// Where the parent column is in the layer list.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub left: f32,
    pub right: f32,
}

impl Column {
    /// How wide the column is for `tl`: as wide as its deepest chain needs.
    pub fn width(tl: &Timeline) -> f32 {
        let lanes = tl.layers.iter().map(|l| tl.rig_ancestors(l.id).len()).max().unwrap_or(0).min(MAX_LANES - 1) + 1;
        2.0 * DOT + 8.0 + lanes as f32 * LANE
    }

    /// The column for `tl`, ending at `right`.
    pub fn new(tl: &Timeline, right: f32) -> Column {
        Column { left: right - Column::width(tl), right }
    }

    fn dot(&self, tl: &Timeline, layer: LayerId, y: f32) -> Pos2 {
        let lane = tl.rig_ancestors(layer).len().min(MAX_LANES - 1);
        pos2(self.left + DOT + 4.0 + lane as f32 * LANE, y)
    }
}

/// A layer's dot as drawn this frame.
#[derive(Clone, Copy, Debug)]
pub struct Dot {
    pub layer: LayerId,
    pub at: Pos2,
}

/// The toggle beside the "Layers" heading: opens and closes the parenting view.
pub fn head_toggle(app: &mut App, ui: &mut egui::Ui, painter: &egui::Painter, rect: Rect) {
    let on = app.prefs.timeline_parenting;
    let r = ui.interact(rect, ui.id().with("parenting_view"), Sense::click());
    if r.hovered() || on {
        painter.rect_filled(rect, T.rc(), if on { T.accent_dim } else { T.hover });
    }
    icons::paint(painter, Rect::from_center_size(rect.center(), vec2(15.0, 15.0)), Icon::Parent, if on { T.accent_text } else { T.text_dim });
    a11y::control(ui, &r, WidgetType::Checkbox, tr!("Parenting view"), Some(on));
    if r.on_hover_text(tr!("Parenting View: show which layer each layer follows. Drag from a layer's dot to another layer to parent it; click the dot to pick or remove the parent")).clicked() {
        toggle_view(app);
    }
}

/// Opens or closes the parenting view (kept with the preferences).
pub fn toggle_view(app: &mut App) {
    let on = !app.prefs.timeline_parenting;
    app.prefs.timeline_parenting = on;
    app.save_prefs();
    if on {
        app.status_ok(tr!("Parenting view: drag from a layer's dot to another layer, or click the dot to pick its parent"));
    }
}

/// What a screen reader hears for a layer's dot.
fn dot_name(tl: &Timeline, layer: &Layer) -> String {
    match layer.rig_parent.and_then(|p| tl.layer(p)) {
        Some(p) => tr!("Parent of {name}: {parent}", name = layer.name, parent = p.name),
        None => tr!("Parent of {name}: none", name = layer.name),
    }
}

/// One row of the parent column: the dot's target, its name for assistive technology, the
/// start of a drag, and its menu. `None` for a layer that can neither follow nor lead.
pub fn row(app: &mut App, ui: &mut egui::Ui, tl: &Timeline, layer: &Layer, col: Column, y: f32, size: f32) -> Option<Dot> {
    if !layer.can_follow() && !layer.can_lead() {
        return None;
    }
    let at = col.dot(tl, layer.id, y);
    let hit = Rect::from_center_size(at, vec2(size, size));
    let r = ui.interact(hit, ui.id().with(("parent_dot", layer.id.0)), Sense::click_and_drag());
    a11y::control(ui, &r, WidgetType::Button, &dot_name(tl, layer), None);
    if r.drag_started_by(egui::PointerButton::Primary) && layer.can_follow() {
        app.tl_parent_drag = Some(layer.id);
    }
    let tip = match layer.rig_parent.and_then(|p| tl.layer(p)) {
        Some(p) => tr!("{name} follows {parent}. Drag to another layer to change its parent; click to pick or remove it", name = layer.name, parent = p.name),
        None if layer.can_follow() => tr!("Drag to another layer to make it the parent of {name}; click to pick one", name = layer.name),
        None => tr!("{name} can lead other layers: drag from their dots to this layer", name = layer.name),
    };
    let r = r.on_hover_text(tip);
    let id = layer.id;
    let shown = egui::Popup::menu(&r).id(egui::Id::new(("layer_parent", id.0))).show(|ui| menu(app, ui, id));
    if let Some(m) = shown {
        ui.ctx().accesskit_node_builder(m.response.id, |node| {
            node.set_role(egui::accesskit::Role::Menu);
            node.set_label(tr!("Parent layer"));
        });
    }
    Some(Dot { layer: id, at })
}

/// The parent picker: no parent, then every layer, the ones this layer can't follow greyed.
pub fn menu(app: &mut App, ui: &mut egui::Ui, layer: LayerId) {
    ui.set_min_width(180.0);
    let tl = app.s.timeline().clone();
    let Some(me) = tl.layer(layer) else { return };
    if !me.can_follow() {
        ui.add_enabled(false, egui::Button::new(tr!("This kind of layer can't follow a parent")));
        return;
    }
    if menus::check_item(ui, tr!("No Parent"), me.rig_parent.is_none(), true) && me.rig_parent.is_some() {
        app.run("layer.remove_parent", json!({"layer": layer.0}));
    }
    ui.separator();
    for cand in tl.layers.iter().filter(|l| l.id != layer && l.can_lead()) {
        let current = me.rig_parent == Some(cand.id);
        let ok = current || tl.rig_check(layer, cand.id).is_ok();
        if menus::check_item(ui, &cand.name, current, ok) && !current {
            app.run("layer.set_parent", json!({"layer": layer.0, "parent": cand.id.0}));
        }
    }
}

/// After the rows: the tree's lines and dots, and a dot being dragged to its new parent.
/// `rows` are the layer rows on screen; `left` is the layer list's area.
pub fn finish(app: &mut App, ui: &mut egui::Ui, painter: &egui::Painter, tl: &Timeline, dots: &[Dot], rows: &[(Rect, LayerId)], left: Rect) {
    let at = |id: LayerId| dots.iter().find(|d| d.layer == id).map(|d| d.at);
    let p = painter.with_clip_rect(left);
    // Lines first, each in the follower's colour: down (or up) the parent's lane, then across.
    for d in dots {
        let Some(layer) = tl.layer(d.layer) else { continue };
        let Some(parent) = layer.rig_parent else { continue };
        let stroke = Stroke::new(1.5, to_color32(layer.color));
        match at(parent) {
            Some(from) => {
                p.line_segment([from, pos2(from.x, d.at.y)], stroke);
                p.line_segment([pos2(from.x, d.at.y), d.at], stroke);
            }
            // Its parent's row isn't showing (scrolled away, or in a closed folder): a stub.
            None => {
                p.line_segment([pos2(d.at.x - LANE, d.at.y), d.at], stroke);
            }
        }
    }
    for d in dots {
        let Some(layer) = tl.layer(d.layer) else { continue };
        let c = to_color32(layer.color);
        let linked = layer.rig_parent.is_some() || tl.layers.iter().any(|l| l.rig_parent == Some(layer.id));
        if linked {
            p.circle_filled(d.at, DOT, c);
            p.circle_stroke(d.at, DOT, Stroke::new(1.0, T.panel));
        } else {
            // A free layer: a ring, so a rig's dots stand out.
            p.circle_filled(d.at, DOT, T.panel);
            p.circle_stroke(d.at, DOT - 0.5, Stroke::new(1.5, if layer.can_follow() { c } else { T.text_faint }));
        }
    }
    let Some(dragged) = app.tl_parent_drag else { return };
    let (pointer, down, cancel) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.key_pressed(egui::Key::Escape)));
    let Some(from) = at(dragged).filter(|_| !cancel) else {
        app.tl_parent_drag = None;
        return;
    };
    let over = pointer.filter(|q| left.contains(*q)).and_then(|q| rows.iter().find(|(r, _)| q.y >= r.top() && q.y < r.bottom())).map(|(r, id)| (*r, *id)).filter(|(_, id)| *id != dragged);
    if !down {
        app.tl_parent_drag = None;
        // The engine says why when the layer can't follow that one (shown in the status bar).
        if let Some((_, target)) = over {
            app.run("layer.set_parent", json!({"layer": dragged.0, "parent": target.0}));
        }
        return;
    }
    let ok = over.is_some_and(|(_, t)| tl.rig_check(dragged, t).is_ok());
    ui.ctx().set_cursor_icon(if ok { egui::CursorIcon::Grabbing } else if over.is_some() { egui::CursorIcon::NoDrop } else { egui::CursorIcon::Grabbing });
    if let Some((r, _)) = over.filter(|_| ok) {
        let band = Rect::from_min_max(pos2(left.left(), r.top()), pos2(left.right(), r.bottom()));
        painter.rect_filled(band, 0.0, T.accent.gamma_multiply(0.22));
        painter.rect_stroke(band, 0.0, Stroke::new(1.0, T.accent), StrokeKind::Inside);
    }
    if let Some(q) = pointer {
        let c = tl.layer(dragged).map_or(T.accent, |l| to_color32(l.color));
        painter.line_segment([from, q], Stroke::new(2.0, c));
        painter.circle_filled(q, 3.0, c);
    }
    ui.ctx().request_repaint();
}
