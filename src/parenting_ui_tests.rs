//! The timeline's parenting view (ADR 0113), driven by pointer events and by assistive
//! technology: a dot dragged to another layer parents it, a cycle is refused, and every dot is a
//! named button whose menu picks the parent without a pointer.

use crate::app::App;
use anicraft_engine::doc::LayerId;
use egui::accesskit::{Action, ActionRequest, Node, Role, TreeId};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

/// The arm sample with its links removed (the limbs where they stand), parenting view open.
fn opened() -> (App, egui::Context) {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut session = anicraft_engine::Session::new(anicraft_engine::doc::sample::arm());
    let ids: Vec<u64> = session.timeline().layers.iter().map(|l| l.id.0).collect();
    session.execute("layer.remove_parent", &json!({"layers": ids})).expect("unparented");
    let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
    app.prefs.timeline_parenting = true;
    settle(&ctx, &mut app);
    (app, ctx)
}

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) -> Vec<(egui::accesskit::NodeId, Node)> {
    let mut all = vec![Event::ModifiersChanged(Modifiers::NONE)];
    all.extend(events);
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 940.0))), events: all, ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| app.frame(ui));
    out.textures_delta.clear();
    out.platform_output.accesskit_update.map(|u| u.nodes).unwrap_or_default()
}

fn settle(ctx: &egui::Context, app: &mut App) -> Vec<(egui::accesskit::NodeId, Node)> {
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = frame(ctx, app, Vec::new());
    }
    nodes
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn drag(ctx: &egui::Context, app: &mut App, from: Pos2, to: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true)]);
    for i in 1..=10 {
        frame(ctx, app, vec![Event::PointerMoved(from + (to - from) * (i as f32 / 10.0))]);
    }
    frame(ctx, app, vec![button(to, false)]);
    settle(ctx, app);
}

fn layer(app: &App, name: &str) -> LayerId {
    app.s.timeline().layers.iter().find(|l| l.name == name).map(|l| l.id).expect("the layer")
}

fn parent(app: &App, name: &str) -> Option<String> {
    let tl = app.s.timeline();
    tl.layer(layer(app, name)).and_then(|l| l.rig_parent).and_then(|p| tl.layer(p)).map(|l| l.name.clone())
}

fn dot(app: &App, name: &str) -> Pos2 {
    let id = layer(app, name);
    app.tl_parent_dots.iter().find(|d| d.layer == id).map(|d| d.at).expect("its dot shows")
}

/// A point on the layer's name.
fn on_row(app: &App, name: &str) -> Pos2 {
    let geo = app.drop.timeline.as_ref().expect("the timeline is drawn");
    let id = layer(app, name);
    let r = geo.rows.iter().find(|(_, l)| *l == id).map(|(r, _)| *r).expect("its row shows");
    pos2(r.left() + 60.0, r.center().y)
}

/// Where the layer's object shows on stage at the playhead.
fn shown(app: &App, name: &str) -> anicraft_engine::geom::Affine {
    let id = layer(app, name);
    app.s.display_list().items.iter().find(|i| i.layer == id).map(|i| i.transform).expect("it shows")
}

#[test]
fn a_dot_dragged_to_another_layer_parents_it_in_one_undo_step() {
    let (mut app, ctx) = opened();
    assert_eq!(app.tl_parent_dots.len(), 3, "every layer with artwork has a dot");
    let before = shown(&app, "Forearm");
    let steps = app.s.info()["history"]["undo"].as_u64().unwrap_or(0);
    let (from, to) = (dot(&app, "Forearm"), on_row(&app, "Upper arm"));
    drag(&ctx, &mut app, from, to);
    assert_eq!(parent(&app, "Forearm").as_deref(), Some("Upper arm"));
    assert_eq!(app.s.info()["history"]["undo"].as_u64().unwrap_or(0), steps + 1, "one step");
    // It stays where it was on stage.
    let after = shown(&app, "Forearm");
    assert!(before.m.iter().zip(after.m).all(|(a, b)| (a - b).abs() < 1e-2), "{before:?} → {after:?}");
    // A chain: the hand onto the forearm; its dot moves one lane in.
    let (from, to) = (dot(&app, "Hand"), on_row(&app, "Forearm"));
    drag(&ctx, &mut app, from, to);
    assert_eq!(parent(&app, "Hand").as_deref(), Some("Forearm"));
    assert!(dot(&app, "Hand").x > dot(&app, "Forearm").x && dot(&app, "Forearm").x > dot(&app, "Upper arm").x, "the tree steps in with each link");
    assert!(app.tl_parent_drag.is_none());
}

#[test]
fn a_drop_on_a_follower_or_on_nothing_changes_nothing() {
    let (mut app, ctx) = opened();
    let (from, to) = (dot(&app, "Forearm"), on_row(&app, "Upper arm"));
    drag(&ctx, &mut app, from, to);
    let doc = app.s.doc.clone();
    // The upper arm can't follow the forearm that follows it: refused, and said so.
    let (from, to) = (dot(&app, "Upper arm"), on_row(&app, "Forearm"));
    drag(&ctx, &mut app, from, to);
    assert_eq!(app.s.doc, doc);
    assert!(app.status.as_ref().is_some_and(|s| s.error && s.text.contains("follows")), "{:?}", app.status.as_ref().map(|s| s.text.clone()));
    // Released away from the layers, or back on itself: nothing.
    let from = dot(&app, "Hand");
    drag(&ctx, &mut app, from, pos2(900.0, 300.0));
    let (from, to) = (dot(&app, "Hand"), on_row(&app, "Hand"));
    drag(&ctx, &mut app, from, to);
    assert_eq!(app.s.doc, doc);
    assert!(app.tl_parent_drag.is_none());
}

#[test]
fn the_view_closes_and_the_column_goes() {
    let (mut app, ctx) = opened();
    crate::parenting::toggle_view(&mut app);
    settle(&ctx, &mut app);
    assert!(!app.prefs.timeline_parenting && app.tl_parent_dots.is_empty());
}

fn named<'a>(nodes: &'a [(egui::accesskit::NodeId, Node)], label: &str) -> Option<&'a (egui::accesskit::NodeId, Node)> {
    nodes.iter().find(|(_, n)| n.label().is_some_and(|l| l.trim() == label))
}

#[test]
fn every_dot_is_a_named_button_and_its_menu_picks_the_parent_without_a_pointer() {
    let (mut app, ctx) = opened();
    let nodes = settle(&ctx, &mut app);
    let toggle = named(&nodes, "Parenting view").expect("the view's toggle is named");
    assert_eq!(toggle.1.role(), Role::CheckBox);
    for l in ["Hand", "Forearm", "Upper arm"] {
        let n = named(&nodes, &format!("Parent of {l}: none")).unwrap_or_else(|| panic!("no dot named for {l}"));
        assert_eq!(n.1.role(), Role::Button);
        assert!(n.1.supports_action(Action::Focus) && n.1.supports_action(Action::Click), "{l}: reachable and pressable by keyboard");
    }
    // Pressing the hand's dot opens its menu; choosing the forearm there parents it.
    let request = |id, action| Event::AccessKitActionRequest(ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None });
    let hand = named(&nodes, "Parent of Hand: none").map(|n| n.0).expect("the dot");
    frame(&ctx, &mut app, vec![request(hand, Action::Click)]);
    let nodes = settle(&ctx, &mut app);
    assert!(named(&nodes, "✔ No Parent").is_some(), "the menu is open, No Parent ticked");
    let pick = named(&nodes, "Forearm").map(|n| n.0).expect("the forearm is offered");
    frame(&ctx, &mut app, vec![request(pick, Action::Click)]);
    let nodes = settle(&ctx, &mut app);
    assert_eq!(parent(&app, "Hand").as_deref(), Some("Forearm"));
    assert!(named(&nodes, "Parent of Hand: Forearm").is_some(), "the dot says what it follows");
}
