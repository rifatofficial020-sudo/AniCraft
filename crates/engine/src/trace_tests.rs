//! Trace Bitmap and Convert to Bitmap (#463).

use super::*;
use crate::Session;
use anicraft_doc::{BitmapId, Document, ElementKind, Paint};
use anicraft_geom::{Affine, Path};
use serde_json::{Value, json};
use std::f32::consts::PI;
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// Pixels from a function of each pixel's centre.
fn image(w: u32, h: u32, f: impl Fn(f32, f32) -> [u8; 4]) -> Pixels {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&f(x as f32 + 0.5, y as f32 + 0.5));
        }
    }
    Pixels { width: w, height: h, rgba }
}

const RED: [u8; 4] = [220, 30, 40, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

fn circle(w: u32, r: f32, outside: [u8; 4]) -> Pixels {
    let c = w as f32 / 2.0;
    image(w, w, |x, y| if (x - c).powi(2) + (y - c).powi(2) < r * r { RED } else { outside })
}

/// The area a path fills (non-zero), from its flattened outline.
fn area(p: &Path, m: &Affine) -> f32 {
    p.flatten(m, 0.02)
        .iter()
        .map(|l| {
            let pts = &l.points;
            (0..pts.len()).map(|i| pts[i].cross(pts[(i + 1) % pts.len()])).sum::<f32>() / 2.0
        })
        .sum::<f32>()
        .abs()
}

fn settings(fit: CurveFit) -> TraceSettings {
    TraceSettings { curve_fit: fit, ..TraceSettings::default() }
}

#[test]
fn a_circle_on_clear_pixels_is_one_round_shape_of_its_area() {
    let px = circle(64, 20.0, CLEAR);
    for fit in CurveFit::ALL {
        let out = trace(&px, &settings(fit)).unwrap();
        assert_eq!(out.len(), 1, "{fit:?}: one shape, the clear pixels make none");
        let a = area(&out[0].path, &Affine::IDENTITY);
        assert!((a - PI * 400.0).abs() < PI * 400.0 * 0.03, "{fit:?}: area {a}");
        assert_eq!(out[0].color, Rgba::new(220.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0, 1.0));
    }
    let curved = trace(&px, &TraceSettings::default()).unwrap();
    let cmds = &curved[0].path.cmds;
    assert!(cmds.iter().any(|c| matches!(c, PathCmd::QuadTo(..))), "smooth outlines are quadratic curves");
    assert!(!cmds.iter().any(|c| matches!(c, PathCmd::CubicTo(..))), "and never cubic, as Animate's shapes");
    assert!(cmds.len() < 40, "a circle takes a handful of curves, not a staircase: {}", cmds.len());
    let pixels = trace(&px, &settings(CurveFit::Pixels)).unwrap();
    let count = px.rgba.chunks(4).filter(|p| p[3] == 255).count() as f32;
    assert_eq!(area(&pixels[0].path, &Affine::IDENTITY), count, "Pixels follows the pixel edges exactly");
    assert!(pixels[0].path.cmds.iter().all(|c| !matches!(c, PathCmd::QuadTo(..))));
}

#[test]
fn two_colours_are_two_fills_that_tile_without_gaps() {
    let px = circle(48, 15.0, WHITE);
    for fit in CurveFit::ALL {
        let out = trace(&px, &settings(fit)).unwrap();
        assert_eq!(out.len(), 2, "{fit:?}");
        let total: f32 = out.iter().map(|t| area(&t.path, &Affine::IDENTITY)).sum();
        assert!((total - 48.0 * 48.0).abs() < 0.5, "{fit:?}: the background's hole is the circle exactly: {total}");
        let disc = out.iter().find(|t| t.color.r < 0.9).map(|t| area(&t.path, &Affine::IDENTITY)).unwrap();
        assert!((disc - PI * 225.0).abs() < PI * 225.0 * 0.04, "{fit:?}: {disc}");
    }
    // Halves of two colours, side by side.
    let halves = image(30, 20, |x, _| if x < 12.0 { [20, 60, 200, 255] } else { [30, 180, 60, 255] });
    let out = trace(&halves, &TraceSettings::default()).unwrap();
    assert_eq!(out.len(), 2);
    let mut areas: Vec<f32> = out.iter().map(|t| area(&t.path, &Affine::IDENTITY)).collect();
    areas.sort_by(f32::total_cmp);
    assert!((areas[0] - 240.0).abs() < 0.01 && (areas[1] - 360.0).abs() < 0.01, "{areas:?}");
}

#[test]
fn the_colour_threshold_and_minimum_area_decide_the_regions() {
    // A horizontal ramp: one colour at a high threshold, bands at a low one.
    let ramp = image(40, 10, |x, _| {
        let v = (x * 6.0) as u8;
        [v, v, v, 255]
    });
    let loose = TraceSettings { color_threshold: 500, ..TraceSettings::default() };
    assert_eq!(trace(&ramp, &loose).unwrap().len(), 1);
    let strict = TraceSettings { color_threshold: 10, min_area: 1, ..TraceSettings::default() };
    assert!(trace(&ramp, &strict).unwrap().len() >= 10);
    // Specks: single dark pixels on white join the white unless the minimum area is 1.
    let specks = image(20, 20, |x, y| if (x as u32 % 5 == 2) && (y as u32 % 7 == 3) { [0, 0, 0, 255] } else { WHITE });
    assert_eq!(trace(&specks, &TraceSettings::default()).unwrap().len(), 1);
    let fine = TraceSettings { min_area: 1, ..TraceSettings::default() };
    assert_eq!(trace(&specks, &fine).unwrap().len(), 1 + 4 * 3);
    // A checkerboard: pixels that touch only at corners are separate shapes, and still tile.
    let board = image(4, 4, |x, y| if (x as u32 + y as u32).is_multiple_of(2) { [0, 0, 0, 255] } else { WHITE });
    let exact = TraceSettings { color_threshold: 1, min_area: 1, curve_fit: CurveFit::Pixels, corners: Corners::Normal };
    let out = trace(&board, &exact).unwrap();
    assert_eq!(out.len(), 16);
    assert!(out.iter().all(|t| (area(&t.path, &Affine::IDENTITY) - 1.0).abs() < 1e-4));
}

#[test]
fn corners_follow_the_corner_threshold_and_squares_keep_theirs() {
    let square = image(40, 40, |x, y| if (10.0..30.0).contains(&x) && (10.0..30.0).contains(&y) { RED } else { CLEAR });
    for corners in Corners::ALL {
        let out = trace(&square, &TraceSettings { corners, ..TraceSettings::default() }).unwrap();
        assert_eq!(out.len(), 1);
        assert!((area(&out[0].path, &Affine::IDENTITY) - 400.0).abs() < 0.5, "{corners:?}: a square stays square");
        let anchors: Vec<Vec2> = out[0].path.anchors().into_iter().map(|(_, a)| a).collect();
        for c in [(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)] {
            assert!(anchors.iter().any(|a| a.distance(Vec2::new(c.0, c.1)) < 1e-3), "{corners:?}: a corner at {c:?}");
        }
    }
}

#[test]
fn bad_images_are_refused_and_never_panic() {
    let wrong = Pixels { width: 4, height: 4, rgba: vec![0; 10] };
    assert!(trace(&wrong, &TraceSettings::default()).is_err());
    let empty = Pixels { width: 0, height: 0, rgba: Vec::new() };
    assert!(trace(&empty, &TraceSettings::default()).is_err());
    let one = image(1, 1, |_, _| RED);
    let out = trace(&one, &TraceSettings { min_area: 1000, ..TraceSettings::default() }).unwrap();
    assert_eq!(out.len(), 1);
    assert!(trace(&image(5, 5, |_, _| CLEAR), &TraceSettings::default()).unwrap().is_empty());
}

// ----- the commands -----------------------------------------------------------------------------

/// A session with `px` in the Library, placed at (100, 50) scaled 2×, selected.
fn placed(px: &Pixels) -> (Session, BitmapId, u64) {
    let mut s = Session::new(Document::default());
    let id = BitmapId(s.doc.alloc_id());
    let b = anicraft_doc::Bitmap::from_rgba(id, "art.png", px.width, px.height, px.rgba.clone()).unwrap();
    s.doc.bitmaps.push(Arc::new(b));
    let el = run(&mut s, "library.place", json!({"bitmap": id.0}))["element"].as_u64().unwrap();
    run(&mut s, "element.set_transform", json!({"x": 100, "y": 50, "sx": 2, "sy": 2}));
    (s, id, el)
}

fn shapes(s: &Session) -> Vec<Arc<anicraft_doc::Element>> {
    s.visible_elements().into_iter().filter(|e| matches!(&e.kind, ElementKind::Shape(sh) if matches!(sh.fill, Some(Paint::Solid(_))))).collect()
}

fn shape_area(e: &anicraft_doc::Element) -> f32 {
    match &e.kind {
        ElementKind::Shape(sh) => area(&sh.path, &e.transform),
        _ => 0.0,
    }
}

#[test]
fn trace_bitmap_replaces_the_bitmap_with_raw_shapes_in_one_undo_step() {
    let (mut s, _, el) = placed(&circle(40, 12.0, CLEAR));
    let before = s.doc.clone();
    let out = run(&mut s, "modify.trace_bitmap", json!({}));
    assert_eq!(out["shapes"], 1);
    let els = s.visible_elements();
    assert!(!els.iter().any(|e| e.id.0 == el), "the bitmap is gone");
    let made = shapes(&s);
    assert_eq!(made.len(), 1);
    let ElementKind::Shape(sh) = &made[0].kind else { panic!("not a solid shape") };
    assert!(sh.merge, "a raw shape, as Animate's trace makes");
    assert!(sh.stroke.is_none());
    let a = shape_area(&made[0]);
    assert!((a - 4.0 * PI * 144.0).abs() < 4.0 * PI * 144.0 * 0.04, "scaled with the instance: {a}");
    let b = s.selection_bounds();
    assert!((b.center().x - 140.0).abs() < 1.5 && (b.center().y - 90.0).abs() < 1.5, "where the bitmap was: {b:?}");
    assert_eq!(s.selection.on(s.layer), vec![made[0].id], "the result is selected");
    assert_eq!(s.undo_label(), Some("Trace Bitmap…"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc, before, "one undo step brings the bitmap back");
}

#[test]
fn trace_bitmap_takes_the_dialog_settings_and_checks_them() {
    let px = image(30, 20, |x, _| if x < 12.0 { [20, 60, 200, 255] } else { [30, 180, 60, 255] });
    let (mut s, _, _) = placed(&px);
    for bad in [
        json!({"color_threshold": 0}),
        json!({"color_threshold": 501}),
        json!({"min_area": 1001}),
        json!({"curve_fit": "wobbly"}),
        json!({"corner_threshold": "lots"}),
    ] {
        assert!(s.execute("modify.trace_bitmap", &bad).is_err(), "{bad}");
    }
    run(&mut s, "modify.trace_bitmap", json!({"color_threshold": 50, "min_area": 4, "curve_fit": "pixels", "corner_threshold": "few"}));
    let made = shapes(&s);
    assert_eq!(made.len(), 2, "two colours, two fills");
    let total: f32 = made.iter().map(|e| shape_area(e)).sum();
    assert!((total - 4.0 * 600.0).abs() < 0.5, "{total}");
    // Nothing traceable selected.
    assert!(s.execute("modify.trace_bitmap", &json!({})).is_err());
    run(&mut s, "edit.deselect", json!({}));
    assert!(s.execute("modify.trace_bitmap", &json!({})).is_err());
}

#[test]
fn traced_shapes_merge_with_raw_shapes_they_touch() {
    let (mut s, _, _) = placed(&image(10, 10, |_, _| RED));
    run(&mut s, "edit.deselect", json!({}));
    run(&mut s, "draw.rect", json!({"x": 90, "y": 40, "w": 30, "h": 30, "fill": "#2040c0", "stroke": null, "object": false}));
    let bitmap = s
        .visible_elements()
        .into_iter()
        .find(|e| matches!(&e.kind, ElementKind::Shape(sh) if matches!(sh.fill, Some(Paint::Bitmap { .. }))))
        .unwrap();
    run(&mut s, "element.select", json!({"ids": [bitmap.id.0]}));
    run(&mut s, "modify.trace_bitmap", json!({}));
    let made = shapes(&s);
    let total: f32 = made.iter().map(|e| shape_area(e)).sum();
    assert!((total - (900.0 + 400.0 - 400.0)).abs() < 1.0, "the red square cuts into the blue one under it: {total}");
    assert!(made.iter().all(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.merge)));
}

#[test]
fn convert_to_bitmap_rasterizes_the_selection_into_the_library() {
    let mut s = Session::new(Document::default());
    run(&mut s, "draw.ellipse", json!({"x": 10.25, "y": 20.5, "w": 40, "h": 40, "fill": "#dd2030", "stroke": null, "object": true}));
    let before = s.doc.clone();
    let out = run(&mut s, "modify.convert_to_bitmap", json!({}));
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(41), Some(41)), "{out}");
    assert_eq!(out["name"], "Bitmap 1");
    let bitmap = s.doc.bitmap(BitmapId(out["bitmap"].as_u64().unwrap())).unwrap();
    assert_eq!((bitmap.width, bitmap.height), (41, 41));
    let els = s.visible_elements();
    assert_eq!(els.len(), 1, "the ellipse became the placed bitmap");
    assert_eq!(els[0].id.0, out["element"].as_u64().unwrap());
    assert_eq!(els[0].transform, Affine::translate(10.0, 20.0), "on whole pixels");
    assert_eq!(s.selection.on(s.layer), vec![els[0].id]);
    let px = bitmap.pixels().unwrap();
    assert_eq!(px.at(20, 20), [221.0 / 255.0, 32.0 / 255.0, 48.0 / 255.0, 1.0], "the fill inside");
    assert_eq!(px.at(0, 0)[3], 0.0, "clear outside");
    // It looks the same on the stage.
    let centre = crate::cmds_shape::sample_at(&s, anicraft_geom::Vec2::new(30.25, 40.5), 1.0).unwrap();
    assert_eq!(centre, [221, 32, 48]);
    // A second one gets the next name; one undo step takes each back.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc, before);
    assert!(s.execute("modify.convert_to_bitmap", &json!({})).is_ok());
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 10, "h": 10, "fill": "#000000", "object": true}));
    assert_eq!(run(&mut s, "modify.convert_to_bitmap", json!({}))["name"], "Bitmap 2");
    run(&mut s, "edit.deselect", json!({}));
    assert!(s.execute("modify.convert_to_bitmap", &json!({})).is_err(), "nothing selected");
}

#[test]
fn convert_to_bitmap_takes_a_selection_on_several_layers_onto_the_topmost() {
    let mut s = Session::new(Document::default());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 20, "h": 20, "fill": "#ff0000", "stroke": null, "object": true}));
    run(&mut s, "layer.new", json!({"name": "Top"}));
    run(&mut s, "draw.rect", json!({"x": 10, "y": 10, "w": 20, "h": 20, "fill": "#0000ff", "stroke": null, "object": true}));
    let top = s.layer;
    run(&mut s, "edit.select_all", json!({}));
    assert_eq!(s.selected_layers().len(), 2);
    let out = run(&mut s, "modify.convert_to_bitmap", json!({}));
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(30), Some(30)));
    assert_eq!(s.layer, top);
    let all: usize = s.timeline().layers.iter().map(|l| s.elements_on(l.id).len()).sum();
    assert_eq!(all, 1, "one placed bitmap left, on the top layer");
    let px = s.doc.bitmap(BitmapId(out["bitmap"].as_u64().unwrap())).unwrap().pixels().unwrap();
    assert_eq!(px.at(15, 15), [0.0, 0.0, 1.0, 1.0], "the top layer's square in front");
    assert_eq!(px.at(5, 5), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px.at(25, 5)[3], 0.0);
}

#[test]
fn convert_to_bitmap_then_trace_comes_back_to_the_drawing() {
    let mut s = Session::new(Document::default());
    run(&mut s, "draw.ellipse", json!({"x": 40, "y": 30, "w": 60, "h": 60, "fill": "#1a8f4a", "stroke": null, "object": true}));
    run(&mut s, "modify.convert_to_bitmap", json!({}));
    run(&mut s, "modify.trace_bitmap", json!({"curve_fit": "tight"}));
    let made = shapes(&s);
    assert_eq!(made.len(), 1, "the circle's anti-aliased edge joins it or stays clear");
    let a = shape_area(&made[0]);
    assert!((a - PI * 900.0).abs() < PI * 900.0 * 0.02, "{a}");
    let b = s.selection_bounds();
    assert!((b.min.x - 40.0).abs() < 1.0 && (b.max.y - 90.0).abs() < 1.0, "{b:?}");
    let ElementKind::Shape(sh) = &made[0].kind else { panic!("not a solid shape") };
    let Some(Paint::Solid(c)) = sh.fill else { panic!("not a solid shape") };
    assert!((c.r - 0x1a as f32 / 255.0).abs() < 0.02 && (c.g - 0x8f as f32 / 255.0).abs() < 0.02, "{c:?}");
}

/// Deterministic speckle: no two neighbours alike.
fn speckle(x: f32, y: f32) -> [u8; 4] {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    [h as u8, (h >> 8) as u8, (h >> 16) as u8, 255]
}

#[test]
fn speckle_is_refused_in_a_sentence_before_it_costs_memory() {
    // 1448 x 1448 pixels of noise at threshold 1: more areas than a trace may grow. It stops
    // at the limit (the areas are counted as they are found), and says what to change.
    let px = image(1448, 1448, speckle);
    let err = trace(&px, &TraceSettings { color_threshold: 1, min_area: 1000, ..TraceSettings::default() }).unwrap_err().to_string();
    assert!(err.contains("separate areas of colour") && err.contains("raise the colour threshold"), "{err}");
    // Under the limit the same speckle is traced: small areas join until each has the minimum
    // area, and the shapes still tile the whole image.
    let px = image(96, 96, speckle);
    let out = trace(&px, &TraceSettings { color_threshold: 1, min_area: 64, curve_fit: CurveFit::Pixels, ..TraceSettings::default() }).unwrap();
    assert!(!out.is_empty() && out.len() <= 96 * 96 / 64, "{} shapes", out.len());
    let total: f32 = out.iter().map(|t| area(&t.path, &Affine::IDENTITY)).sum();
    assert!((total - 96.0 * 96.0).abs() < 1.0, "the shapes cover {total} of 9216 pixels");
    // And the refusal for too many shapes still names both settings.
    let err = trace(&image(512, 512, speckle), &TraceSettings { color_threshold: 1, min_area: 1, ..TraceSettings::default() }).unwrap_err().to_string();
    assert!(err.contains("at most 20000"), "{err}");
}
