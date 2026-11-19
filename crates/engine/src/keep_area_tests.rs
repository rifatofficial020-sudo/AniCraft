//! Keep area (ADR 0115, issue #408) through the command registry: classic and motion tweens,
//! the keys a transform records while an axis follows, undo and the file round trip.

use super::*;
use anicraft_doc::Property;
use serde_json::json;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// Scale X, scale Y and their product for the one element at `frame`.
fn scale(s: &mut Session, frame: u32) -> (f32, f32, f32) {
    s.set_frame(frame);
    let d = s.visible_elements().first().map(|e| e.transform.decompose()).expect("an element");
    (d.sx, d.sy, d.sx * d.sy)
}

/// A square that a classic tween takes from 100 x 100 % to 200 x 50 % over 12 frames.
fn squashed() -> Session {
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40}));
    run(&mut s, "timeline.insert_keyframe", json!({"frame": 12}));
    s.set_frame(12);
    run(&mut s, "edit.select_all", json!({}));
    run(&mut s, "element.set_transform", json!({"sx": 2.0, "sy": 0.5}));
    s.set_frame(0);
    run(&mut s, "timeline.create_tween", json!({}));
    s
}

#[test]
fn a_classic_tween_keeps_its_area_on_every_frame_and_off_is_what_it_was() {
    let mut s = squashed();
    let before: Vec<_> = (0..=12).map(|f| scale(&mut s, f)).collect();
    assert!((before[6].2 - 1.125).abs() < 1e-4, "the bulge of issue #408: {:?}", before[6]);
    s.set_frame(3);
    assert_eq!(run(&mut s, "timeline.set_keep_area", json!({"keep_area": true})), json!({"keep_area": true, "tween": "classic"}));
    for f in 0..=12 {
        let (x, y, a) = scale(&mut s, f);
        assert!((a - 1.0).abs() < 2e-5, "frame {f}: {x} x {y} = {a}");
    }
    let mid = scale(&mut s, 6);
    assert!((mid.0 - 2f32.sqrt()).abs() < 1e-4 && (mid.1 - 0.5f32.sqrt()).abs() < 1e-4, "{mid:?}");
    // It survives a change of ease and of kind, and the file.
    run(&mut s, "timeline.set_easing", json!({"easing": "QuadIn", "frame": 0}));
    run(&mut s, "timeline.create_shape_tween", json!({"frame": 0}));
    run(&mut s, "timeline.create_tween", json!({"frame": 0}));
    assert!((scale(&mut s, 5).2 - 1.0).abs() < 2e-5);
    let back = format::from_json(&format::to_json(&s.doc).expect("save")).expect("load");
    assert!(back.scenes[0].timeline.layers[0].keyframes[0].tween.as_ref().is_some_and(|t| t.keep_area));
    // One undo step each; off again is bit for bit the tween of before.
    run(&mut s, "timeline.set_easing", json!({"easing": "Linear", "frame": 0}));
    run(&mut s, "timeline.set_keep_area", json!({"keep_area": false, "frame": 0}));
    assert_eq!((0..=12).map(|f| scale(&mut s, f)).collect::<Vec<_>>(), before);
    assert!(!format::to_json(&s.doc).expect("save").contains("keep_area"));
    run(&mut s, "edit.undo", json!({}));
    assert!((scale(&mut s, 6).2 - 1.0).abs() < 2e-5, "undo puts Keep area back");
}

#[test]
fn keep_area_needs_a_tween_and_a_true_or_false() {
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40}));
    assert!(s.execute("timeline.set_keep_area", &json!({"keep_area": true})).is_err(), "no tween on the span");
    run(&mut s, "timeline.create_motion_tween", json!({}));
    assert!(s.execute("timeline.set_keep_area", &json!({})).is_err());
    assert!(s.execute("timeline.set_keep_area", &json!({"keep_area": "yes"})).is_err());
    assert_eq!(run(&mut s, "timeline.set_keep_area", json!({"keep_area": true}))["tween"], "motion");
    assert_eq!(run(&mut s, "motion.info", json!({}))["keep_area"], true);
}

#[test]
fn on_a_motion_tween_the_other_axis_squashes_by_itself_until_it_is_keyed() {
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 40, "h": 40}));
    run(&mut s, "timeline.create_motion_tween", json!({}));
    run(&mut s, "timeline.set_keep_area", json!({"keep_area": true}));
    // Stretch the width at frame 12: only Scale X is keyed, and the height follows.
    s.set_frame(12);
    run(&mut s, "element.set_transform", json!({"sx": 2.0}));
    let m = |s: &Session| s.active_layer().and_then(|l| l.keyframes.first()).and_then(|k| k.motion.clone()).expect("motion tween");
    assert!(m(&s).track(Property::ScaleY).is_none());
    for f in 0..=23 {
        let (x, y, a) = scale(&mut s, f);
        assert!((a - 1.0).abs() < 1e-4, "frame {f}: {x} x {y} = {a}");
    }
    assert!((scale(&mut s, 12).1 - 0.5).abs() < 1e-4);
    // A transform handle dragged on: each step sets the width and puts the height back where
    // the drag found it (100 %). That isn't an edit of the height, which goes on following.
    s.set_frame(12);
    run(&mut s, "element.set_transform", json!({"sx": 2.5, "sy": 1.0}));
    assert!(m(&s).track(Property::ScaleY).is_none(), "{:?}", m(&s).tracks);
    let at = scale(&mut s, 12);
    assert!((at.0 - 2.5).abs() < 1e-4 && (at.1 - 0.4).abs() < 1e-4, "{at:?}");
    // Setting the height to something of its own keys it: both curves are the animator's now,
    // geometric between their keys.
    s.set_frame(12);
    run(&mut s, "element.set_transform", json!({"sy": 0.8}));
    assert!(m(&s).track(Property::ScaleY).is_some());
    let at = scale(&mut s, 12);
    assert!((at.0 - 2.5).abs() < 1e-4 && (at.1 - 0.8).abs() < 1e-4, "{at:?}");
    let mid = scale(&mut s, 6);
    assert!((mid.0 - 2.5f32.sqrt()).abs() < 1e-3 && (mid.1 - 0.8f32.sqrt()).abs() < 1e-3, "{mid:?}");
    // The file keeps it.
    let back = format::from_json(&format::to_json(&s.doc).expect("save")).expect("load");
    assert!(back.scenes[0].timeline.layers[0].keyframes[0].motion.as_ref().is_some_and(|m| m.keep_area));
}
