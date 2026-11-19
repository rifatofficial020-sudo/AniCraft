//! Moving a layer anywhere in the stack (`layer.move`, the timeline's drag, #407) and the
//! buttons above the layer names that show, lock or outline every layer (#422).

use crate::Session;
use anicraft_doc::{LayerId, LayerKind, MAX_LAYER_DEPTH};
use serde_json::{Value, json};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn new_layer(s: &mut Session, name: &str) -> LayerId {
    LayerId(run(s, "layer.new", json!({"name": name}))["layer"].as_u64().unwrap())
}

fn new_folder(s: &mut Session, name: &str) -> LayerId {
    LayerId(run(s, "layer.new_folder", json!({"name": name}))["layer"].as_u64().unwrap())
}

/// The stack top down, each name indented by a dot per level of nesting.
fn stack(s: &Session) -> Vec<String> {
    let tl = s.timeline();
    (0..tl.layers.len()).map(|i| format!("{}{}", ".".repeat(tl.depth(i)), tl.layers[i].name)).collect()
}

/// Five plain layers, top down: E, D, C, B, A.
fn five() -> (Session, [LayerId; 5]) {
    let mut s = Session::default();
    let a = s.layer;
    run(&mut s, "layer.rename", json!({"name": "A"}));
    let b = new_layer(&mut s, "B");
    let c = new_layer(&mut s, "C");
    let d = new_layer(&mut s, "D");
    let e = new_layer(&mut s, "E");
    assert_eq!(stack(&s), ["E", "D", "C", "B", "A"]);
    (s, [a, b, c, d, e])
}

#[test]
fn a_layer_moves_any_distance_in_one_undo_step() {
    let (mut s, [a, b, _, _, e]) = five();
    // Up past three layers: just above D... no: A lands just above E, the top.
    let r = run(&mut s, "layer.move", json!({"layer": a.0, "before": e.0}));
    assert_eq!(r["index"], 0);
    assert_eq!(stack(&s), ["A", "E", "D", "C", "B"]);
    // One step in the history, and the active layer is still the one it was.
    let active = s.layer;
    s.undo().unwrap();
    assert_eq!(stack(&s), ["E", "D", "C", "B", "A"]);
    assert_eq!(s.layer, active);
    s.redo().unwrap();
    // Down: E to the bottom (no target), then B just above it.
    run(&mut s, "layer.move", json!({"layer": e.0}));
    assert_eq!(stack(&s), ["A", "D", "C", "B", "E"]);
    run(&mut s, "layer.move", json!({"layer": a.0, "before": e.0}));
    assert_eq!(stack(&s), ["D", "C", "B", "A", "E"]);
    // Above itself, or above its neighbour below: nothing changes, and nothing breaks.
    let before = stack(&s);
    run(&mut s, "layer.move", json!({"layer": b.0, "before": b.0}));
    run(&mut s, "layer.move", json!({"layer": b.0, "before": a.0}));
    assert_eq!(stack(&s), before);
    // Without a layer, the active one moves.
    run(&mut s, "layer.select", json!({"layer": b.0}));
    run(&mut s, "layer.move", json!({}));
    assert_eq!(stack(&s).last().map(String::as_str), Some("B"));
}

#[test]
fn layers_move_into_within_and_out_of_folders() {
    let (mut s, [a, b, c, d, e]) = five();
    run(&mut s, "layer.select", json!({"layer": e.0}));
    let f = new_folder(&mut s, "F");
    assert_eq!(stack(&s), ["F", "E", "D", "C", "B", "A"]);
    // Onto the folder: first inside it, each drop above the last.
    run(&mut s, "layer.move", json!({"layer": a.0, "into": f.0}));
    run(&mut s, "layer.move", json!({"layer": c.0, "into": f.0}));
    assert_eq!(stack(&s), ["F", ".C", ".A", "E", "D", "B"]);
    let r = run(&mut s, "layer.move", json!({"layer": c.0, "into": f.0}));
    assert_eq!(r["parent"], f.0, "already first in the folder: stays there");
    assert_eq!(stack(&s), ["F", ".C", ".A", "E", "D", "B"]);
    // Between two layers of the folder: inside it too.
    run(&mut s, "layer.move", json!({"layer": b.0, "before": a.0}));
    assert_eq!(stack(&s), ["F", ".C", ".B", ".A", "E", "D"]);
    // Below the folder's last layer: still inside (a layer right under the folder joins it
    // without moving a row); below a layer outside it: out again.
    run(&mut s, "layer.move", json!({"layer": e.0, "after": a.0}));
    assert_eq!(stack(&s), ["F", ".C", ".B", ".A", ".E", "D"]);
    run(&mut s, "layer.move", json!({"layer": e.0, "after": f.0}));
    assert_eq!(stack(&s), ["F", ".C", ".B", ".A", "E", "D"], "after the folder: below everything in it");
    run(&mut s, "layer.move", json!({"layer": d.0, "after": c.0}));
    assert_eq!(stack(&s), ["F", ".C", ".D", ".B", ".A", "E"]);
    run(&mut s, "layer.move", json!({"layer": d.0, "after": e.0}));
    assert_eq!(stack(&s), ["F", ".C", ".B", ".A", "E", "D"]);
    // Above a layer outside the folder: out of it.
    let r = run(&mut s, "layer.move", json!({"layer": b.0, "before": d.0}));
    assert_eq!(r["parent"], Value::Null);
    assert_eq!(stack(&s), ["F", ".C", ".A", "E", "B", "D"]);
    // The folder moves with what is in it, to the bottom and back above a layer.
    run(&mut s, "layer.move", json!({"layer": f.0}));
    assert_eq!(stack(&s), ["E", "B", "D", "F", ".C", ".A"]);
    run(&mut s, "layer.move", json!({"layer": f.0, "before": b.0}));
    assert_eq!(stack(&s), ["E", "F", ".C", ".A", "B", "D"]);
    // A folder inside a folder, with its layers one level deeper.
    run(&mut s, "layer.select", json!({"layer": d.0}));
    let g = new_folder(&mut s, "G");
    run(&mut s, "layer.move", json!({"layer": f.0, "into": g.0}));
    assert_eq!(stack(&s), ["E", "B", "G", ".F", "..C", "..A", "D"]);
    // Never into itself or into something inside it.
    assert!(s.execute("layer.move", &json!({"layer": g.0, "into": f.0})).is_err());
    assert!(s.execute("layer.move", &json!({"layer": g.0, "into": g.0})).is_err());
    let before = stack(&s);
    run(&mut s, "layer.move", json!({"layer": g.0, "before": c.0}));
    assert_eq!(stack(&s), before, "above one of its own layers: where it is");
    // Bad input is refused, and the stack is untouched.
    for p in [json!({"layer": 999_999}), json!({"before": 999_999}), json!({"into": 999_999}), json!({"into": a.0}), json!({"before": a.0, "into": f.0}), json!({"before": a.0, "after": b.0}), json!({"after": 999_999}), json!({"before": "x"})] {
        assert!(s.execute("layer.move", &p).is_err(), "{p}");
    }
    assert_eq!(stack(&s), before);
}

#[test]
fn masks_keep_their_layers_and_take_only_normal_ones() {
    let (mut s, [a, b, c, d, e]) = five();
    run(&mut s, "layer.toggle_mask", json!({"layer": c.0}));
    assert_eq!(stack(&s), ["E", "D", "C", ".B", "A"], "B is masked by C");
    // The mask moves with its masked layer.
    run(&mut s, "layer.move", json!({"layer": c.0, "before": e.0}));
    assert_eq!(stack(&s), ["C", ".B", "E", "D", "A"]);
    // A normal layer dropped above the masked layer is masked too; dropped elsewhere it is free.
    run(&mut s, "layer.move", json!({"layer": a.0, "before": b.0}));
    assert_eq!(stack(&s), ["C", ".A", ".B", "E", "D"]);
    assert_eq!(s.timeline().layer(a).unwrap().parent, Some(c));
    run(&mut s, "layer.move", json!({"layer": a.0, "before": d.0}));
    assert_eq!(s.timeline().layer(a).unwrap().parent, None);
    // A folder can't go under a mask: refused, with the stack as it was.
    run(&mut s, "layer.select", json!({"layer": d.0}));
    let f = new_folder(&mut s, "F");
    let before = stack(&s);
    assert!(s.execute("layer.move", &json!({"layer": f.0, "before": b.0})).is_err());
    assert_eq!(stack(&s), before);
    // Every link still holds (a repair changes nothing).
    let mut tl = s.timeline().clone();
    tl.repair_masks();
    assert_eq!(tl.layers.iter().map(|l| l.parent).collect::<Vec<_>>(), s.timeline().layers.iter().map(|l| l.parent).collect::<Vec<_>>());
}

#[test]
fn folders_never_nest_past_the_limit_and_the_camera_stays_on_top() {
    let mut s = Session::default();
    let art = s.layer;
    // A chain of folders as deep as allowed: each new one goes into the last.
    let mut chain = vec![new_folder(&mut s, "F0")];
    for i in 1..MAX_LAYER_DEPTH + 2 {
        let f = new_folder(&mut s, &format!("F{i}"));
        let inner = *chain.last().unwrap();
        if s.execute("layer.move", &json!({"layer": f.0, "into": inner.0})).is_ok() {
            chain.push(f);
        }
    }
    let tl = s.timeline();
    let deepest = (0..tl.layers.len()).map(|i| tl.depth(i)).max().unwrap();
    assert!(deepest < MAX_LAYER_DEPTH, "depth {deepest}");
    assert!(chain.len() < MAX_LAYER_DEPTH + 2, "the deepest moves were refused");
    // A whole chain can't go one deeper either: refused, not flattened.
    let before = stack(&s);
    let inner = *chain.last().unwrap();
    run(&mut s, "layer.select", json!({"layer": art.0}));
    let extra = new_folder(&mut s, "Extra");
    run(&mut s, "layer.select", json!({"layer": art.0}));
    let extra_child = new_folder(&mut s, "Extra child");
    run(&mut s, "layer.move", json!({"layer": extra_child.0, "into": extra.0}));
    let with_extra = stack(&s);
    assert!(s.execute("layer.move", &json!({"layer": extra.0, "into": inner.0})).is_err());
    assert_eq!(stack(&s), with_extra);
    assert!(with_extra.len() > before.len());

    // The camera layer is always the top row; nothing moves it or lands above it.
    run(&mut s, "camera.add", json!({}));
    let cam = s.timeline().layers.iter().find(|l| l.kind == LayerKind::Camera).map(|l| l.id).unwrap();
    assert!(s.execute("layer.move", &json!({"layer": cam.0})).is_err());
    run(&mut s, "layer.move", json!({"layer": art.0, "before": cam.0}));
    assert_eq!(s.timeline().layers.first().map(|l| l.id), Some(cam));
    assert_eq!(s.timeline().layers.get(1).map(|l| l.id), Some(art), "as high as a layer goes: just under the camera");
}

#[test]
fn the_buttons_above_the_layers_show_lock_and_outline_every_layer() {
    let (mut s, [a, b, _, _, e]) = five();
    let state = |s: &Session, f: fn(&anicraft_doc::Layer) -> bool| s.timeline().layers.iter().map(|l| f(l)).collect::<Vec<_>>();
    let visible = |s: &Session| state(s, |l| l.visible);
    let locked = |s: &Session| state(s, |l| l.locked);
    let outline = |s: &Session| state(s, |l| l.outline);

    // All showing: the eye hides them all; again: shows them all.
    let r = run(&mut s, "layer.toggle_visible", json!({"all": true}));
    assert_eq!((r["on"].as_bool(), r["changed"].as_u64()), (Some(false), Some(5)));
    assert_eq!(visible(&s), [false; 5]);
    run(&mut s, "layer.toggle_visible", json!({"all": true}));
    assert_eq!(visible(&s), [true; 5]);
    // Some hidden: one click gets them alike (all hidden), and it is one undo step.
    run(&mut s, "layer.toggle_visible", json!({"layer": b.0}));
    let r = run(&mut s, "layer.toggle_visible", json!({"all": true}));
    assert_eq!(r["changed"], 4, "only the layers that change are touched");
    assert_eq!(visible(&s), [false; 5]);
    s.undo().unwrap();
    assert_eq!(visible(&s), [true, true, true, false, true]);
    // An explicit state.
    run(&mut s, "layer.toggle_visible", json!({"all": true, "on": true}));
    assert_eq!(visible(&s), [true; 5]);

    // The padlock: locks all while any is unlocked, then unlocks all.
    run(&mut s, "layer.toggle_lock", json!({"layer": e.0}));
    run(&mut s, "layer.toggle_lock", json!({"all": true}));
    assert_eq!(locked(&s), [true; 5]);
    run(&mut s, "layer.toggle_lock", json!({"all": true}));
    assert_eq!(locked(&s), [false; 5]);
    // The square: all as outlines, then all as artwork.
    run(&mut s, "layer.toggle_outline", json!({"all": true}));
    assert_eq!(outline(&s), [true; 5]);
    run(&mut s, "layer.toggle_outline", json!({"all": true}));
    assert_eq!(outline(&s), [false; 5]);

    // One layer still toggles, and takes an explicit state.
    run(&mut s, "layer.toggle_lock", json!({"layer": a.0, "on": true}));
    run(&mut s, "layer.toggle_lock", json!({"layer": a.0, "on": true}));
    assert!(s.timeline().layer(a).unwrap().locked);

    // The camera is left alone: hiding every layer never turns the camera off.
    run(&mut s, "camera.add", json!({}));
    run(&mut s, "layer.toggle_visible", json!({"all": true}));
    let cam = s.timeline().layers.iter().find(|l| l.kind == LayerKind::Camera).unwrap();
    assert!(cam.visible);
    assert!(s.timeline().layers.iter().filter(|l| l.kind != LayerKind::Camera).all(|l| !l.visible));
    assert!(s.execute("layer.toggle_visible", &json!({"all": "yes"})).is_err());
}
