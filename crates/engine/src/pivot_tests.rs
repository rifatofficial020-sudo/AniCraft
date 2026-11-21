//! The transformation point an instance keeps, the box Free Transform draws round one
//! element, and layer opacity (ADR 0111; issues #302, #280, #178, #183, #175, #279).

use crate::Session;
use anicraft_doc::{ElementId, evaluate_layer_elements};
use anicraft_geom::{Affine, Rect, Vec2};
use serde_json::{Value, json};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A 100 × 20 arm as a symbol instance, its box from (100, 100) to (200, 120), selected. Its
/// registration point is the box's centre (150, 110).
fn arm() -> (Session, ElementId) {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 20, "fill": "#222222", "stroke": null, "object": true}));
    let id = ElementId(run(&mut s, "modify.convert_to_symbol", json!({"name": "Arm"}))["instance"].as_u64().unwrap());
    assert_eq!(s.element_bounds(id), Rect::new(100.0, 100.0, 200.0, 120.0));
    (s, id)
}

fn el(s: &Session, id: ElementId) -> std::sync::Arc<anicraft_doc::Element> {
    s.visible_elements().into_iter().find(|e| e.id == id).expect("the element is there")
}

/// Where the instance's kept point is on the stage.
fn point(s: &Session, id: ElementId) -> Vec2 {
    let e = el(s, id);
    e.transform.apply(e.pivot().expect("the instance keeps a point"))
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.distance(b) < 0.01
}

#[test]
fn the_point_starts_at_the_centre_and_is_kept_in_the_instances_own_space() {
    let (mut s, id) = arm();
    assert_eq!(s.transformation_point(), Some((Vec2::new(150.0, 110.0), false)), "the centre of the box until it is moved");
    assert_eq!(run(&mut s, "modify.transformation_point", json!({"x": 100, "y": 110})), json!({"set": 1, "x": 100.0, "y": 110.0}));
    assert_eq!(s.transformation_point(), Some((Vec2::new(100.0, 110.0), true)));
    // Kept relative to the registration point (150, 110), so it travels with the instance.
    assert_eq!(el(&s, id).pivot(), Some(Vec2::new(-50.0, 0.0)));
    run(&mut s, "element.translate", json!({"dx": 30, "dy": -10}));
    assert!(near(point(&s, id), Vec2::new(130.0, 100.0)));
    // Outside the box is fine (#178), and a scaled, turned instance still gets the stage point asked for.
    run(&mut s, "modify.transform", json!({"a": 0.0, "b": 2.0, "c": -2.0, "d": 0.0, "e": 400.0, "f": 0.0}));
    run(&mut s, "modify.transformation_point", json!({"x": -500, "y": 900}));
    assert!(near(point(&s, id), Vec2::new(-500.0, 900.0)));
    // Back to the centre.
    assert_eq!(run(&mut s, "modify.reset_transformation_point", json!({})), json!({"reset": 1}));
    assert_eq!(el(&s, id).pivot(), None);
    assert!(!s.transformation_point().unwrap().1);
    // Every change is one undo step.
    run(&mut s, "edit.undo", json!({}));
    assert!(near(point(&s, id), Vec2::new(-500.0, 900.0)));
}

#[test]
fn only_an_instance_keeps_a_point_and_bad_points_are_refused() {
    let mut s = Session::new(anicraft_doc::Document::default());
    assert!(s.execute("modify.transformation_point", &json!({"x": 1, "y": 2})).is_err(), "nothing selected");
    assert!(s.execute("modify.reset_transformation_point", &json!({})).is_err());
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 50, "h": 50, "object": true}));
    let e = s.execute("modify.transformation_point", &json!({"x": 1, "y": 2})).unwrap_err().to_string();
    assert!(e.contains("symbol instance"), "{e}");
    let (mut s, id) = arm();
    for bad in [json!({}), json!({"x": 1}), json!({"x": "a", "y": 2}), json!({"x": 1e30, "y": 0}), json!({"x": null, "y": null})] {
        assert!(s.execute("modify.transformation_point", &bad).is_err(), "{bad}");
    }
    assert_eq!(el(&s, id).pivot(), None, "a refused point changes nothing");
    assert!(!s.can_undo() || el(&s, id).pivot().is_none());
}

#[test]
fn rotating_turns_the_instance_about_its_point() {
    // The point at the arm's left end (100, 110): Rotate 90° CW swings the arm down from it.
    let (mut s, id) = arm();
    run(&mut s, "modify.transformation_point", json!({"x": 100, "y": 110}));
    run(&mut s, "modify.rotate_cw", json!({}));
    assert!(near(point(&s, id), Vec2::new(100.0, 110.0)), "the point stays put");
    let b = s.element_bounds(id);
    assert!((b.min.x - 90.0).abs() < 0.01 && (b.max.x - 110.0).abs() < 0.01 && (b.min.y - 110.0).abs() < 0.01 && (b.max.y - 210.0).abs() < 0.01, "{b:?}");
    // Flipping mirrors it across the point.
    run(&mut s, "modify.flip_v", json!({}));
    let b = s.element_bounds(id);
    assert!((b.min.y - 10.0).abs() < 0.01 && (b.max.y - 110.0).abs() < 0.01, "{b:?}");
    // Without a point of its own it turns about the centre of its box, as before.
    let (mut s, id) = arm();
    run(&mut s, "modify.rotate_cw", json!({}));
    assert!(near(s.element_bounds(id).center(), Vec2::new(150.0, 110.0)));
}

#[test]
fn the_properties_fields_turn_and_scale_about_the_point() {
    let (mut s, id) = arm();
    run(&mut s, "modify.transformation_point", json!({"x": 100, "y": 110}));
    // Rotation 30°: analytic, the registration point (50 px along the arm) swings round the point.
    run(&mut s, "element.set_transform", json!({"rotation": 30}));
    let (sin, cos) = 30f32.to_radians().sin_cos();
    assert!(near(point(&s, id), Vec2::new(100.0, 110.0)));
    assert!(near(el(&s, id).transform.translation(), Vec2::new(100.0 + 50.0 * cos, 110.0 + 50.0 * sin)));
    assert!((el(&s, id).transform.decompose().rotation.to_degrees() - 30.0).abs() < 1e-3);
    // Scale: the point stays, the rotation is kept.
    run(&mut s, "element.set_transform", json!({"sx": 2, "sy": 0.5}));
    assert!(near(point(&s, id), Vec2::new(100.0, 110.0)));
    let d = el(&s, id).transform.decompose();
    assert!((d.sx - 2.0).abs() < 1e-4 && (d.sy - 0.5).abs() < 1e-4 && (d.rotation.to_degrees() - 30.0).abs() < 1e-3);
    // A position places the registration point, as always.
    run(&mut s, "element.set_transform", json!({"x": 10, "y": 20}));
    assert!(near(el(&s, id).transform.translation(), Vec2::new(10.0, 20.0)));
    // No point: about the registration point, as before.
    let (mut s, id) = arm();
    run(&mut s, "element.set_transform", json!({"rotation": 90}));
    assert!(near(el(&s, id).transform.translation(), Vec2::new(150.0, 110.0)));
}

#[test]
fn dragging_the_point_is_one_undo_step() {
    let (mut s, id) = arm();
    s.begin_gesture("Move Transformation Point");
    for i in 0..30 {
        run(&mut s, "modify.transformation_point", json!({"x": 150.0 - i as f32, "y": 110.0 + i as f32}));
    }
    s.end_gesture();
    assert!(near(point(&s, id), Vec2::new(121.0, 139.0)));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(el(&s, id).pivot(), None, "one undo takes the whole drag back");
    assert!(!s.can_undo() || s.execute("edit.undo", &json!({})).is_ok());
}

#[test]
fn a_tween_made_with_the_tools_swings_about_the_point() {
    // Key the arm at frame 0, turn it 90° about its left end at frame 10, tween between.
    let (mut s, id) = arm();
    run(&mut s, "modify.transformation_point", json!({"x": 100, "y": 110}));
    run(&mut s, "timeline.insert_frame", json!({"count": 10}));
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    run(&mut s, "timeline.insert_keyframe", json!({}));
    run(&mut s, "element.select", json!({"ids": [id.0]}));
    run(&mut s, "modify.rotate_cw", json!({}));
    run(&mut s, "timeline.goto", json!({"frame": 0}));
    run(&mut s, "timeline.create_tween", json!({}));
    let layer = s.doc.timeline(s.scene).layer(s.layer).unwrap().clone();
    for f in 0..=10 {
        let (_, els) = evaluate_layer_elements(&layer, f).unwrap();
        let e = els.iter().find(|e| e.id == id).unwrap();
        let at = e.transform.apply(e.pivot().unwrap());
        assert!(near(at, Vec2::new(100.0, 110.0)), "frame {f}: the hinge is at {at:?}");
        assert!((e.transform.decompose().rotation.to_degrees() - 9.0 * f as f32).abs() < 0.01, "frame {f}");
    }
}

#[test]
fn one_element_shows_its_own_box_turned_and_several_show_an_upright_one() {
    let (mut s, id) = arm();
    assert_eq!(s.transform_frame(), Some((Affine::translate(150.0, 110.0), Rect::new(-50.0, -10.0, 50.0, 10.0))));
    // Turned 30° about its centre: the same box, under the instance's transform (#175).
    let turn = Affine::translate(150.0, 110.0).mul(&Affine::rotate(30f32.to_radians())).mul(&Affine::translate(-150.0, -110.0));
    let [a, b, c, d, e, f] = turn.m;
    run(&mut s, "modify.transform", json!({"a": a, "b": b, "c": c, "d": d, "e": e, "f": f}));
    let (frame, own) = s.transform_frame().unwrap();
    assert_eq!(frame, el(&s, id).transform);
    assert!(near(own.min, Vec2::new(-50.0, -10.0)) && near(own.max, Vec2::new(50.0, 10.0)), "{own:?}");
    assert!((frame.decompose().rotation.to_degrees() - 30.0).abs() < 1e-3);
    // The upright bounds are bigger: the turned box hugs the arm.
    let upright = s.selection_bounds();
    assert!(upright.max.y - upright.min.y > 60.0);
    // A stretch along the box's own x axis (what its side handle does) keeps the turn and
    // changes only the arm's length: no skew appears.
    let along = frame.mul(&Affine::scale(1.5, 1.0)).mul(&frame.inverse().unwrap());
    let [a, b, c, d, e, f] = along.m;
    run(&mut s, "modify.transform", json!({"a": a, "b": b, "c": c, "d": d, "e": e, "f": f}));
    let dec = el(&s, id).transform.decompose();
    assert!((dec.sx - 1.5).abs() < 1e-4 && (dec.sy - 1.0).abs() < 1e-4 && dec.skew.abs() < 1e-4 && (dec.rotation.to_degrees() - 30.0).abs() < 1e-3, "{dec:?}");
    // Two elements: the upright bounds of both, and their centre as the point.
    run(&mut s, "draw.rect", json!({"x": 300, "y": 300, "w": 40, "h": 40, "object": true}));
    run(&mut s, "edit.select_all", json!({}));
    let (frame, b) = s.transform_frame().unwrap();
    assert_eq!((frame, b), (Affine::IDENTITY, s.selection_bounds()));
    assert_eq!(s.transformation_point(), Some((b.center(), false)));
    run(&mut s, "edit.deselect", json!({}));
    assert_eq!((s.transform_frame(), s.transformation_point()), (None, None));
}

#[test]
fn the_point_survives_a_save_and_copies_with_the_instance() {
    let dir = std::env::temp_dir().join(format!("anicraft-pivot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("arm.anicraft");
    let (mut s, id) = arm();
    run(&mut s, "modify.transformation_point", json!({"x": 100, "y": 110}));
    run(&mut s, "layer.set_opacity", json!({"opacity": 40}));
    run(&mut s, "file.save", json!({"path": path.display().to_string()}));
    let mut t = Session::default();
    run(&mut t, "file.open", json!({"path": path.display().to_string()}));
    assert_eq!(s.doc, t.doc);
    assert_eq!(el(&t, id).pivot(), Some(Vec2::new(-50.0, 0.0)));
    assert_eq!(t.doc.timeline(0).layers[0].opacity, 0.4);
    let _ = std::fs::remove_dir_all(&dir);
    // A copy keeps the point.
    run(&mut s, "edit.copy", json!({}));
    run(&mut s, "edit.paste_in_place", json!({}));
    let copy = s.selected_elements().into_iter().map(|(_, e)| e).find(|e| e.id != id).expect("the pasted instance is selected");
    assert_eq!(copy.pivot(), Some(Vec2::new(-50.0, 0.0)));
}

#[test]
fn layer_opacity_is_set_read_checked_and_undone() {
    let (mut s, _) = arm();
    let layer = s.layer;
    assert_eq!(run(&mut s, "layer.properties", json!({}))["opacity"], json!(100.0));
    assert_eq!(run(&mut s, "layer.set_opacity", json!({"opacity": 37.5})), json!({"layer": layer.0, "opacity": 37.5}));
    assert_eq!(s.doc.timeline(0).layers[0].opacity, 0.375);
    assert_eq!(run(&mut s, "layer.properties", json!({}))["opacity"], json!(37.5));
    // What the stage and every export draw: the layer is one group at that opacity.
    let dl = s.display_list();
    assert_eq!(dl.groups.len(), 1);
    assert_eq!(dl.groups[0].alpha, 0.375);
    assert!(anicraft_doc::evaluate(&s.doc, 0).groups.iter().any(|g| g.alpha == 0.375));
    // With the other Layer Properties, as one step.
    let v = run(&mut s, "layer.set_properties", json!({"opacity": 80, "name": "Faded"}));
    assert_eq!((v["opacity"].clone(), v["name"].clone()), (json!(80.0), json!("Faded")));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.timeline(0).layers[0].opacity, 0.375);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.timeline(0).layers[0].opacity, 1.0);
    for bad in [json!({}), json!({"opacity": -1}), json!({"opacity": 100.5}), json!({"opacity": "half"}), json!({"opacity": 50, "layer": 999_999})] {
        assert!(s.execute("layer.set_opacity", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("layer.set_properties", &json!({"opacity": 500})).is_err());
    assert_eq!(s.doc.timeline(0).layers[0].opacity, 1.0);
    // 0 and 100 are the ends.
    run(&mut s, "layer.set_opacity", json!({"opacity": 0}));
    assert_eq!(s.display_list().groups[0].alpha, 0.0);
    run(&mut s, "layer.set_opacity", json!({"opacity": 100}));
    assert!(s.display_list().groups.is_empty());
}

/// #466: a bitmap imported to the stage turns about the centre of its box, as in Animate, not
/// about its top-left corner (where the bitmap's own origin is).
#[test]
fn an_imported_bitmap_turns_about_its_centre() {
    let dir = std::env::temp_dir().join(format!("anicraft-pivot-bitmap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("photo.png");
    std::fs::write(&png, anicraft_render::encode_png(40, 20, &[255u8; 40 * 20 * 4]).unwrap()).unwrap();
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "file.import", json!({"path": png.display().to_string(), "x": 100, "y": 50}));
    std::fs::remove_dir_all(&dir).ok();
    run(&mut s, "edit.select_all", json!({}));
    assert_eq!(s.selection_bounds(), Rect::new(100.0, 50.0, 140.0, 70.0));
    // Free Transform's point: the centre of the box, and not one the artist moved.
    let (at, moved) = s.transformation_point().expect("the bitmap is selected");
    assert!(near(at, Vec2::new(120.0, 60.0)) && !moved, "{at:?} {moved}");
    // The origin modes that have a point for a bitmap give the same one.
    for mode in [crate::TransformOrigin::Selection, crate::TransformOrigin::Center, crate::TransformOrigin::Registration] {
        let origins = crate::cmds_modify::transform_origins(&s, mode);
        assert!(matches!(origins.as_slice(), [(_, p)] if near(*p, Vec2::new(120.0, 60.0))), "{mode:?}: {:?}", origins.iter().map(|o| o.1).collect::<Vec<_>>());
    }
    // A quarter turn keeps the centre where it is: a 20 × 40 box round the same point.
    run(&mut s, "modify.rotate_cw", json!({}));
    let b = s.selection_bounds();
    assert!(near(b.min, Vec2::new(110.0, 40.0)) && near(b.max, Vec2::new(130.0, 80.0)), "{b:?}");
}
