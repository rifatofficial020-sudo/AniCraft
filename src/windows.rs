//! Floating windows: every `egui::Window` the shell opens comes from [`new`], which keeps it
//! inside the work area below the menu bar, so no dialog's title bar covers File, Edit, View
//! (a window dragged up stops under the bar; one placed or anchored over it is moved down).
//! `guidelines_tests` checks that no other code opens a window directly.

/// Where the area below the menu bar is kept for this frame.
fn key() -> egui::Id {
    egui::Id::new("anicraft_work_area")
}

/// Records the work area: the window's content below the menu bar (`bar_bottom`).
pub fn set_work_area(ctx: &egui::Context, bar_bottom: f32) {
    let mut r = ctx.content_rect();
    if bar_bottom.is_finite() {
        r.min.y = bar_bottom.clamp(r.min.y, r.max.y);
    }
    ctx.data_mut(|d| d.insert_temp(key(), r));
}

/// The area floating windows stay in (the whole window before the menu bar has been laid out).
pub fn work_area(ctx: &egui::Context) -> egui::Rect {
    ctx.data(|d| d.get_temp::<egui::Rect>(key())).unwrap_or_else(|| ctx.content_rect())
}

/// A floating window kept below the menu bar.
pub fn new(ctx: &egui::Context, title: impl Into<egui::WidgetText>) -> egui::Window<'static> {
    egui::Window::new(title).constrain_to(work_area(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame of a 800 × 600 window with a 24 px bar, a window asked to sit at the very
    /// top, and returns where it was drawn.
    fn frame(ctx: &egui::Context, at: egui::Pos2) -> egui::Rect {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
        let mut shown = egui::Rect::NOTHING;
        let mut out = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            set_work_area(&ctx, 24.0);
            if let Some(r) = new(&ctx, "Custom Ease").id(egui::Id::new("t")).current_pos(at).show(&ctx, |ui| {
                ui.label("graph");
            }) {
                shown = r.response.rect;
            }
        });
        out.textures_delta.clear();
        shown
    }

    #[test]
    fn windows_never_open_over_the_menu_bar() {
        let ctx = egui::Context::default();
        for at in [egui::pos2(100.0, 0.0), egui::pos2(100.0, -50.0), egui::pos2(-20.0, 10.0)] {
            let r = frame(&ctx, at);
            let r = frame(&ctx, at).union(r);
            assert!(r.min.y >= 24.0 - 0.5, "{at:?} → {r:?}");
            assert!(r.min.x >= -0.5);
        }
        // Lower down, a window stays where it was put.
        let lower = egui::Context::default();
        frame(&lower, egui::pos2(100.0, 200.0));
        let r = frame(&lower, egui::pos2(100.0, 200.0));
        assert!((r.min.y - 200.0).abs() < 1.0, "{r:?}");
        // Before a bar was laid out, the whole window is the area.
        assert_eq!(work_area(&egui::Context::default()), egui::Context::default().content_rect());
    }
}
