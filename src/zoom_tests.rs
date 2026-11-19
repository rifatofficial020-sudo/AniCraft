//! The zoom edit-bar field (#43): an editable percentage with a separate presets arrow,
//! driven frame by frame like the retime tests.

use crate::app::App;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, vec2};

fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>, modifiers: Modifiers) {
    let mut all = vec![Event::ModifiersChanged(modifiers)];
    all.extend(events);
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 940.0))), events: all, ..Default::default() };
    ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
}

fn settle(ctx: &egui::Context, app: &mut App) {
    for _ in 0..3 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
}

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

fn click(ctx: &egui::Context, app: &mut App, at: Pos2, modifiers: Modifiers) {
    frame(ctx, app, vec![Event::PointerMoved(at)], modifiers);
    frame(ctx, app, vec![button(at, true, modifiers)], modifiers);
    frame(ctx, app, vec![button(at, false, modifiers)], modifiers);
    settle(ctx, app);
}

fn key(ctx: &egui::Context, app: &mut App, key: egui::Key, modifiers: Modifiers) {
    frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], modifiers);
    frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }], modifiers);
    settle(ctx, app);
}

fn type_text(ctx: &egui::Context, app: &mut App, s: &str) {
    frame(ctx, app, vec![Event::Text(s.to_owned())], Modifiers::NONE);
    settle(ctx, app);
}

fn select_all(ctx: &egui::Context, app: &mut App) {
    frame(ctx, app, vec![Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL }], Modifiers::CTRL);
    frame(ctx, app, vec![Event::Key { key: egui::Key::A, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::CTRL }], Modifiers::CTRL);
    settle(ctx, app);
}

fn clear_field(ctx: &egui::Context, app: &mut App) {
    select_all(ctx, app);
    // Selection or not (Select All is ⌘A on macOS, so Ctrl+A selects nothing there), this
    // leaves the field empty: the seed is 3-4 digits, deleted on both sides of the cursor.
    for key in [egui::Key::Backspace, egui::Key::Delete] {
        for _ in 0..8 {
            frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
            frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }], Modifiers::NONE);
        }
    }
    settle(ctx, app);
}

fn zoom_rects(ctx: &egui::Context) -> (Rect, Rect) {
    let field = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("zoom_field"))).expect("the zoom field drew");
    let arrow = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("zoom_arrow"))).expect("the zoom arrow drew");
    (field, arrow)
}

#[test]
fn zoom_field_types_a_value_and_rejects_garbage() {
    let ctx = egui::Context::default();
    let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
    let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
    app.view.fit = false;
    app.view.zoom = 1.0;
    settle(&ctx, &mut app);
    app.view.fit = false;
    app.view.zoom = 1.0;
    settle(&ctx, &mut app);

    // Clicking the text focuses it without changing the zoom (no presets menu from the text).
    let (field, arrow) = zoom_rects(&ctx);
    let before = app.view.zoom;
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    assert!((app.view.zoom - before).abs() < 1e-6, "focusing the field leaves zoom alone");
    // Clicking the arrow alone does not change the zoom either (it only opens the presets).
    click(&ctx, &mut app, arrow.center(), Modifiers::NONE);
    assert!((app.view.zoom - before).abs() < 1e-6, "the presets arrow alone zooms nothing");
    key(&ctx, &mut app, egui::Key::Escape, Modifiers::NONE);

    // Type a custom value and press Enter: the stage zooms there.
    let (field, _) = zoom_rects(&ctx);
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    clear_field(&ctx, &mut app);
    type_text(&ctx, &mut app, "150");
    key(&ctx, &mut app, egui::Key::Enter, Modifiers::NONE);
    assert!((app.view.zoom - 1.5).abs() < 0.01, "typed 150% zooms to 1.5, got {}", app.view.zoom);

    // Garbage reverts to the current zoom without changing anything.
    let (field, _) = zoom_rects(&ctx);
    click(&ctx, &mut app, field.center(), Modifiers::NONE);
    clear_field(&ctx, &mut app);
    type_text(&ctx, &mut app, "abc");
    key(&ctx, &mut app, egui::Key::Enter, Modifiers::NONE);
    assert!((app.view.zoom - 1.5).abs() < 0.01, "invalid text keeps zoom at 1.5, got {}", app.view.zoom);
}
