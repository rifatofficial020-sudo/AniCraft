//! Transformation-origin modes for several selected objects (ADR 0109, issue #296): the whole
//! selection about one point, or each object about its own registration point or centre.

use crate::{Session, TransformOrigin, transform_origins};
use anicraft_doc::{ElementId, ElementKind};
use anicraft_geom::{Affine, Rect, Vec2};
use serde_json::{Value, json};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A clock hand: an instance of a 100 × 10 bar whose registration point is at its left end,
/// placed with that end at `(x, y)`.
fn hand(s: &mut Session, x: f32, y: f32, name: &str) -> ElementId {
    run(s, "draw.rect", json!({"x": x - 50.0, "y": y - 5.0, "w": 100, "h": 10, "fill": "#222222", "stroke": null, "object": true}));
    let id = ElementId(run(s, "modify.convert_to_symbol", json!({"name": name}))["instance"].as_u64().unwrap());
    // Inside, the bar moves so the registration point (the centre it was made about) is its end.
    run(s, "symbol.edit", json!({"id": id.0}));
    run(s, "edit.select_all", json!({}));
    run(s, "element.translate", json!({"dx": 50, "dy": 0}));
    run(s, "symbol.exit", json!({}));
    id
}

fn transform_of(s: &Session, id: ElementId) -> Affine {
    s.selected_elements().into_iter().chain(s.visible_elements().into_iter().map(|e| (s.layer, e))).find(|(_, e)| e.id == id).map(|(_, e)| e.transform).expect("the element is there")
}

fn near(a: Vec2, b: Vec2) -> bool {
    a.distance(b) < 0.01
}

/// Two hands hinged at (100, 100) and (300, 200), both pointing right, both selected.
fn two_hands() -> (Session, ElementId, ElementId) {
    let mut s = Session::new(anicraft_doc::Document::default());
    let a = hand(&mut s, 100.0, 100.0, "A");
    let b = hand(&mut s, 300.0, 200.0, "B");
    run(&mut s, "edit.select_all", json!({}));
    assert!(near(transform_of(&s, a).translation(), Vec2::new(100.0, 100.0)) && near(transform_of(&s, b).translation(), Vec2::new(300.0, 200.0)));
    assert_eq!(s.element_bounds(a), Rect::new(100.0, 95.0, 200.0, 105.0), "the bar runs from its hinge");
    (s, a, b)
}

#[test]
fn the_mode_is_set_cycled_and_checked() {
    let mut s = Session::new(anicraft_doc::Document::default());
    assert_eq!(s.style.transform_origin, TransformOrigin::Selection, "Animate's behaviour by default");
    assert_eq!(run(&mut s, "modify.transform_origin", json!({})), json!({"mode": "registration"}));
    assert_eq!(run(&mut s, "modify.transform_origin", json!({})), json!({"mode": "center"}));
    assert_eq!(run(&mut s, "modify.transform_origin", json!({})), json!({"mode": "selection"}));
    assert_eq!(run(&mut s, "modify.transform_origin", json!({"mode": "center"})), json!({"mode": "center"}));
    assert_eq!(s.style.transform_origin, TransformOrigin::Center);
    assert!(s.execute("modify.transform_origin", &json!({"mode": "pivot"})).is_err());
    assert!(s.execute("modify.transform_origin", &json!({"mode": 3})).is_err());
    assert!(!s.can_undo(), "tool state, not a document change");
    for m in TransformOrigin::ALL {
        assert_eq!(TransformOrigin::from_key(m.key()), Some(m));
    }
}

#[test]
fn each_hand_turns_about_its_own_hinge_or_its_own_centre_or_they_turn_as_one() {
    let quarter = json!({"a": 0, "b": 1, "c": -1, "d": 0, "e": 0, "f": 0});
    let with = |origin: &str| {
        let mut p = quarter.clone();
        p["origin"] = json!(origin);
        p
    };
    // Individual pivots: the hinges stay, each bar now hangs down from its hinge.
    let (mut s, a, b) = two_hands();
    assert_eq!(run(&mut s, "modify.transform", with("registration")), json!({"transformed": 2}));
    assert!(near(transform_of(&s, a).translation(), Vec2::new(100.0, 100.0)) && near(transform_of(&s, b).translation(), Vec2::new(300.0, 200.0)));
    assert_eq!(s.element_bounds(a), Rect::new(95.0, 100.0, 105.0, 200.0));
    assert_eq!(s.element_bounds(b), Rect::new(295.0, 200.0, 305.0, 300.0));
    // Individual centres: each bar turns on its own middle; its hinge swings round it.
    let (mut s, a, b) = two_hands();
    run(&mut s, "modify.transform", with("center"));
    assert_eq!(s.element_bounds(a), Rect::new(145.0, 50.0, 155.0, 150.0));
    assert_eq!(s.element_bounds(b), Rect::new(345.0, 150.0, 355.0, 250.0));
    assert!(near(transform_of(&s, a).translation(), Vec2::new(150.0, 50.0)));
    // The selection (the default, and what the matrix says without `origin`): one arrangement.
    for p in [with("selection"), quarter.clone()] {
        let (mut s, a, b) = two_hands();
        run(&mut s, "modify.transform", p);
        assert!(near(transform_of(&s, a).translation(), Vec2::new(-100.0, 100.0)) && near(transform_of(&s, b).translation(), Vec2::new(-200.0, 300.0)));
    }
    let (mut s, ..) = two_hands();
    assert!(s.execute("modify.transform", &with("hinge")).is_err());
    // One undo step each.
    let (mut s, a, _) = two_hands();
    run(&mut s, "modify.transform", with("registration"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.element_bounds(a), Rect::new(100.0, 95.0, 200.0, 105.0));
}

#[test]
fn a_move_stays_a_move_and_an_own_origin_transform_leaves_the_move_out() {
    let (mut s, a, b) = two_hands();
    // Free Transform sends a move without `origin`: both go the same way.
    run(&mut s, "modify.transform", json!({"a": 1, "b": 0, "c": 0, "d": 1, "e": 30, "f": -10}));
    assert!(near(transform_of(&s, a).translation(), Vec2::new(130.0, 90.0)) && near(transform_of(&s, b).translation(), Vec2::new(330.0, 190.0)));
    // With an own origin only the scale, rotation and skew count: twice as long, in place.
    run(&mut s, "modify.transform", json!({"a": 2, "b": 0, "c": 0, "d": 1, "e": 999, "f": 999, "origin": "registration"}));
    assert_eq!(s.element_bounds(a), Rect::new(130.0, 85.0, 330.0, 95.0));
    assert_eq!(s.element_bounds(b), Rect::new(330.0, 185.0, 530.0, 195.0));
}

#[test]
fn a_drag_applied_a_little_at_a_time_ends_where_one_transform_would() {
    for mode in ["registration", "center"] {
        let (mut once, a, b) = two_hands();
        let (mut steps, ..) = two_hands();
        let turn = |deg: f32| {
            let r = Affine::rotate(deg.to_radians()).mul(&Affine::scale(1.0 + deg / 180.0, 1.0));
            json!({"a": r.m[0], "b": r.m[1], "c": r.m[2], "d": r.m[3], "e": 0, "f": 0, "origin": mode})
        };
        // What Free Transform sends: the change since the last frame, thirty times.
        let total = |deg: f32| Affine::rotate(deg.to_radians()).mul(&Affine::scale(1.0 + deg / 180.0, 1.0));
        run(&mut once, "modify.transform", turn(90.0));
        let mut applied = Affine::IDENTITY;
        for i in 1..=30 {
            let now = total(3.0 * i as f32);
            let delta = now.mul(&applied.inverse().unwrap());
            run(&mut steps, "modify.transform", json!({"a": delta.m[0], "b": delta.m[1], "c": delta.m[2], "d": delta.m[3], "e": 0, "f": 0, "origin": mode}));
            applied = now;
        }
        for id in [a, b] {
            let (x, y) = (transform_of(&once, id), transform_of(&steps, id));
            assert!(x.m.iter().zip(y.m).all(|(p, q)| (p - q).abs() < 0.02), "{mode}: {x:?} in one, {y:?} in steps");
        }
    }
}

#[test]
fn shapes_and_text_turn_about_their_centres_and_a_raw_drawing_stays_one_drawing() {
    let mut s = Session::new(anicraft_doc::Document::default());
    // A drawing object, and a raw rectangle with its outline (a fill and four lines).
    run(&mut s, "draw.rect", json!({"x": 20, "y": 20, "w": 100, "h": 40, "fill": "#ff0000", "stroke": null, "object": true}));
    run(&mut s, "draw.rect", json!({"x": 300, "y": 100, "w": 100, "h": 40, "fill": "#0000ff", "stroke": "#000000", "stroke_width": 2, "object": false}));
    run(&mut s, "edit.select_all", json!({}));
    assert!(s.selection.len() >= 6, "the object and the drawing's pieces");
    for mode in [TransformOrigin::Registration, TransformOrigin::Center] {
        let origins = transform_origins(&s, mode);
        assert_eq!(origins.len(), 2, "two objects: {origins:?}");
        let mut at: Vec<Vec2> = origins.iter().map(|(_, p)| *p).collect();
        at.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert!(near(at[0], Vec2::new(70.0, 40.0)) && near(at[1], Vec2::new(350.0, 120.0)), "their centres: {at:?}");
        assert_eq!(origins.iter().map(|(items, _)| items.len()).sum::<usize>(), s.selection.len());
    }
    assert_eq!(transform_origins(&s, TransformOrigin::Selection).len(), 2, "the same objects, whatever the mode");
    run(&mut s, "modify.transform_origin", json!({"mode": "registration"}));
    // Modify › Transform follows the mode: each turns a quarter on its own centre.
    run(&mut s, "modify.rotate_cw", json!({}));
    let raw: Vec<_> = s.visible_elements().into_iter().filter(|e| matches!(&e.kind, ElementKind::Shape(sh) if sh.merge)).collect();
    assert!(raw.len() >= 5, "the drawing's pieces");
    let object = s.visible_elements().into_iter().find(|e| matches!(&e.kind, ElementKind::Shape(sh) if !sh.merge)).unwrap();
    assert_eq!(s.element_bounds(object.id), Rect::new(50.0, -10.0, 90.0, 90.0));
    let drawing = raw.iter().fold(Rect::EMPTY, |r, e| r.union(&s.element_bounds(e.id)));
    assert!(near(drawing.center(), Vec2::new(350.0, 120.0)) && (drawing.width() - 42.0).abs() < 0.1 && (drawing.height() - 102.0).abs() < 0.1, "{drawing:?}");
    // `origin` overrides the mode: the selection flips as one about its shared centre.
    let before = s.selection_bounds();
    run(&mut s, "modify.flip_h", json!({"origin": "selection"}));
    let after = s.selection_bounds();
    assert!(near(before.center(), after.center()) && (before.width() - after.width()).abs() < 0.01);
    assert!(s.element_bounds(object.id).min.x > 300.0, "the object is now on the right");
    assert!(s.execute("modify.flip_v", &json!({"origin": 7})).is_err());
}

#[test]
fn own_origins_work_across_layers_and_refuse_what_would_collapse() {
    let (mut s, a, _) = two_hands();
    run(&mut s, "layer.new", json!({}));
    let c = hand(&mut s, 200.0, 300.0, "C");
    run(&mut s, "edit.select_all", json!({}));
    assert_eq!(s.selected_layers().len(), 2);
    let v = run(&mut s, "modify.rotate_ccw", json!({"origin": "registration"}));
    assert!(v.to_string().contains("transformed"), "{v}");
    let up = s.element_bounds(a);
    assert!(near(up.min, Vec2::new(95.0, 0.0)) && near(up.max, Vec2::new(105.0, 100.0)), "{up:?}");
    assert!(near(transform_of(&s, c).translation(), Vec2::new(200.0, 300.0)));
    assert_eq!(s.selection.len(), 3, "still selected");
    // A flattening matrix is refused and nothing changes.
    let before = s.doc.clone();
    assert!(s.execute("modify.transform", &json!({"a": 1, "b": 0, "c": 0, "d": 0, "e": 0, "f": 0, "origin": "center"})).is_err());
    assert_eq!(s.doc, before);
    run(&mut s, "edit.deselect", json!({}));
    assert!(s.execute("modify.transform", &json!({"a": 2, "b": 0, "c": 0, "d": 2, "e": 0, "f": 0, "origin": "center"})).is_err());
    assert!(s.execute("modify.rotate_cw", &json!({"origin": "registration"})).is_err());
}
