//! Text on a path (ADR 0070): attaching text to a shape's outline, by selection or by picking,
//! moving it along, flipping and releasing it, edits keeping it on the path, undo, the file.

use crate::Session;
use anicraft_doc::{ElementId, ElementKind, TextBlock};
use serde_json::{Value, json};
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn fonts() -> bool {
    let ok = !anicraft_text::Fonts::get().families().is_empty();
    if !ok {
        eprintln!("no fonts on this computer: text on a path tests skipped");
    }
    ok
}

fn text_of(s: &Session, id: ElementId) -> (anicraft_geom::Affine, Arc<TextBlock>) {
    let e = s.visible_elements().into_iter().find(|e| e.id == id).unwrap();
    match &e.kind {
        ElementKind::Text(t) => (e.transform, t.clone()),
        other => panic!("not text: {other:?}"),
    }
}

/// A session with "Along the line" and a 400 px line from (100, 300) to (500, 300).
fn text_and_line() -> (Session, ElementId, ElementId) {
    let mut s = Session::default();
    let text = ElementId(run(&mut s, "text.create", json!({"text": "Along the line", "x": 50, "y": 50, "size": 24}))["element"].as_u64().unwrap());
    let line = ElementId(run(&mut s, "draw.line", json!({"x0": 100, "y0": 300, "x1": 500, "y1": 300}))["element"].as_u64().unwrap());
    (s, text, line)
}

#[test]
fn text_attaches_to_a_selected_shape_and_sits_on_it() {
    if !fonts() {
        return;
    }
    let (mut s, text, line) = text_and_line();
    run(&mut s, "element.select", json!({"ids": [text.0, line.0]}));
    let steps = s.history().undo_len();
    let v = run(&mut s, "text.attach_path", json!({}));
    assert_eq!(s.history().undo_len(), steps + 1, "one undo step");
    assert!((v["length"].as_f64().unwrap() - 400.0).abs() < 0.5, "{v}");
    assert_eq!(v["offset"], 0.0, "left-aligned text starts at the path's start");
    let (xf, t) = text_of(&s, text);
    let on = t.on_path.as_ref().expect("on a path");
    // In stage space the glyphs stand on the line, starting at its start.
    let b = t.outline.bounds().transformed(&xf);
    assert!(b.max.y <= 300.0 + 8.0 && b.max.y > 295.0 && b.min.y < 290.0, "standing on y 300 (descenders a little below): {b:?}");
    assert!((b.min.x - 100.0).abs() < 4.0, "{b:?}");
    let path = on.path.transformed(&xf).bounds();
    assert!((path.min.x - 100.0).abs() < 1e-3 && (path.max.x - 500.0).abs() < 1e-3);
    // The box holds the path and the glyphs from the origin.
    let local = t.outline.bounds().union(&on.path.bounds());
    assert!(local.min.x.abs() < 1e-3 && local.min.y.abs() < 1e-3 && (local.width() - t.box_width).abs() < 1e-3 && (local.height() - t.box_height).abs() < 1e-3, "{local:?}");
    // The shape is left as it is, and only the text is selected.
    assert!(s.visible_elements().iter().any(|e| e.id == line));
    assert_eq!(s.selection.on(s.layer), vec![text]);
    // Undo puts the text back in its lines.
    s.undo().unwrap();
    assert!(text_of(&s, text).1.on_path.is_none());
}

#[test]
fn offset_flip_alignment_and_edits_keep_text_on_its_path() {
    if !fonts() {
        return;
    }
    let (mut s, text, line) = text_and_line();
    run(&mut s, "element.select", json!({"ids": [text.0]}));
    // Picking the shape at a stage point (the stage's pick mode does this).
    run(&mut s, "text.attach_path", json!({"x": 300, "y": 300}));
    let left = |s: &Session| {
        let (xf, t) = text_of(s, text);
        t.outline.bounds().transformed(&xf)
    };
    let at0 = left(&s);
    run(&mut s, "text.path", json!({"offset": 100}));
    let at100 = left(&s);
    assert!((at100.min.x - at0.min.x - 100.0).abs() < 0.5, "{at0:?} → {at100:?}");
    // Flipped: from the other end, below the line.
    run(&mut s, "text.path", json!({"flip": true}));
    let f = left(&s);
    assert!(f.min.y > 290.0 && f.max.y > 310.0 && (f.max.x - 400.0).abs() < 1.0, "{f:?}");
    run(&mut s, "text.path", json!({"flip": false, "offset": 200}));
    // Centred text is centred on its offset.
    run(&mut s, "text.edit", json!({"align": "center"}));
    let c = left(&s);
    assert!((c.center().x - 300.0).abs() < 3.0, "centred on x 300: {c:?}");
    // Editing the words or the size keeps it on the path, where it was.
    run(&mut s, "text.edit", json!({"text": "Hi", "size": 40}));
    let (xf, t) = text_of(&s, text);
    assert_eq!(t.text, "Hi");
    assert!(t.on_path.is_some());
    assert!((t.on_path.as_ref().unwrap().path.transformed(&xf).bounds().min.x - 100.0).abs() < 1e-3, "the path doesn't move");
    assert!((left(&s).center().x - 300.0).abs() < 3.0);
    // Text on a path is static text.
    assert!(s.execute("text.field", &json!({"type": "dynamic"})).is_err());
    // Released: back to a line of text where it started.
    let before = left(&s);
    run(&mut s, "text.release_path", json!({}));
    let (_, t) = text_of(&s, text);
    assert!(t.on_path.is_none());
    let after = left(&s);
    assert!((after.min.x - before.min.x).abs() < 6.0 && (after.min.y - before.min.y).abs() < 12.0, "{before:?} → {after:?}");
    assert!(s.execute("text.path", &json!({"offset": 5})).is_err(), "no text on a path selected");
    let _ = line;
}

#[test]
fn bad_requests_change_nothing() {
    if !fonts() {
        return;
    }
    let (mut s, text, line) = text_and_line();
    // No shape anywhere selected or there.
    run(&mut s, "element.select", json!({"ids": [text.0]}));
    assert!(s.execute("text.attach_path", &json!({})).is_err());
    assert!(s.execute("text.attach_path", &json!({"x": 5, "y": 5})).is_err());
    assert!(s.execute("text.attach_path", &json!({"x": 5})).is_err());
    assert!(s.execute("text.attach_path", &json!({"path": line.0, "offset": 1e9})).is_err());
    // Only the shape selected: no text.
    run(&mut s, "element.select", json!({"ids": [line.0]}));
    assert!(s.execute("text.attach_path", &json!({})).is_err());
    assert!(text_of(&s, text).1.on_path.is_none());
    // By id works from another layer too.
    run(&mut s, "layer.new", json!({}));
    let other = ElementId(run(&mut s, "draw.ellipse", json!({"x": 100, "y": 100, "w": 200, "h": 200}))["element"].as_u64().unwrap());
    let base = s.timeline().layers.last().unwrap().id;
    run(&mut s, "layer.select", json!({"layer": base.0}));
    run(&mut s, "element.select", json!({"ids": [text.0]}));
    run(&mut s, "text.attach_path", json!({"path": other.0}));
    assert!(text_of(&s, text).1.on_path.is_some());
}

#[test]
fn text_on_a_path_is_saved_and_older_files_open_without_it() {
    if !fonts() {
        return;
    }
    let (mut s, text, line) = text_and_line();
    run(&mut s, "element.select", json!({"ids": [text.0, line.0]}));
    run(&mut s, "text.attach_path", json!({"offset": 12.5, "flip": true}));
    let (_, t) = text_of(&s, text);
    let json = serde_json::to_string(&*t).unwrap();
    assert!(json.contains("\"on_path\"") && json.contains("\"offset\":12.5") && json.contains("\"flip\":true"), "{json}");
    let back: TextBlock = serde_json::from_str(&json).unwrap();
    assert_eq!(back, *t);
    // The whole document round-trips through the file format.
    let saved = crate::format::to_json(&s.doc).unwrap();
    let doc = crate::format::from_json(&saved).unwrap();
    assert_eq!(doc, s.doc);
    // A block saved before text on a path opens as plain text.
    let mut v: Value = serde_json::from_str(&json).unwrap();
    v.as_object_mut().unwrap().remove("on_path");
    assert!(serde_json::from_value::<TextBlock>(v).unwrap().on_path.is_none());
    // It renders from its outlines alone: the same pixels without the font.
    let a = anicraft_render::render_frame(&s.doc, 0, 0.5).unwrap();
    assert!(a.data.iter().any(|p| p[0] < 0.5), "the text draws");
}

/// The kinds of an outline's commands, in order: the same glyphs in the same order give the
/// same list wherever they stand.
fn kinds(t: &TextBlock) -> Vec<u8> {
    use anicraft_geom::PathCmd as C;
    t.outline
        .cmds
        .iter()
        .map(|c| match c {
            C::MoveTo(_) => 0,
            C::LineTo(_) => 1,
            C::QuadTo(..) => 2,
            C::CubicTo(..) => 3,
            C::Close => 4,
        })
        .collect()
}

#[test]
fn the_point_commands_edit_the_path_a_text_follows() {
    if !fonts() {
        return;
    }
    let (mut s, text, line) = text_and_line();
    run(&mut s, "element.select", json!({"ids": [text.0, line.0]}));
    run(&mut s, "text.attach_path", json!({}));
    let (xf0, before) = text_of(&s, text);
    let steps = s.history().undo_len();
    // A drag of the path's end point, 100 px down in three moves: one undo step, live each move.
    s.begin_gesture("Move Points");
    for _ in 0..4 {
        run(&mut s, "path.move_points", json!({"id": text.0, "points": [[1, 0]], "dx": 0, "dy": 25}));
    }
    s.end_gesture();
    assert_eq!(s.history().undo_len(), steps + 1, "one undo step per drag");
    let (xf, t) = text_of(&s, text);
    let on = t.on_path.as_ref().unwrap();
    // The path's ends in stage space: the start where it was, the end 100 lower.
    let stage = on.path.transformed(&xf);
    let (a, b) = (stage.point(0, 0).unwrap(), stage.point(1, 0).unwrap());
    assert!((a - anicraft_geom::Vec2::new(100.0, 300.0)).length() < 1e-2 && (b - anicraft_geom::Vec2::new(500.0, 400.0)).length() < 1e-2, "{a:?} {b:?}");
    // The glyphs follow: the same glyphs in the same order, now running downhill.
    assert_eq!(kinds(&t), kinds(&before), "glyph order kept");
    assert_eq!((t.text.as_str(), t.runs.len()), (before.text.as_str(), before.runs.len()));
    let (b0, b1) = (before.outline.bounds().transformed(&xf0), t.outline.bounds().transformed(&xf));
    assert!(b1.height() > b0.height() + 10.0, "{b0:?} then {b1:?}");
    // The box still holds the path and the glyphs from the origin.
    let local = t.outline.bounds().union(&on.path.bounds());
    assert!(local.min.x.abs() < 1e-3 && local.min.y.abs() < 1e-3, "{local:?}");
    // A path squeezed to no length is refused and the text stays as it was.
    let kept = text_of(&s, text).1;
    let e = s.execute("path.move_points", &json!({"id": text.0, "points": [[1, 0]], "dx": -400, "dy": -100}));
    assert!(e.is_err(), "{e:?}");
    assert_eq!(*text_of(&s, text).1, *kept);
    // A point added on the path, made smooth, and deleted again.
    let v = run(&mut s, "path.add_point", json!({"id": text.0, "x": 300, "y": 350, "slop": 3}));
    let cmd = v["cmd"].as_u64().unwrap();
    assert_eq!(text_of(&s, text).1.on_path.as_ref().unwrap().path.cmds.len(), 3);
    run(&mut s, "path.move_points", json!({"id": text.0, "points": [[cmd, 0]], "dx": 0, "dy": -80}));
    run(&mut s, "path.convert_point", json!({"id": text.0, "cmd": cmd, "smooth": true}));
    assert_eq!(kinds(&text_of(&s, text).1), kinds(&before));
    run(&mut s, "path.delete_points", json!({"id": text.0, "cmds": [cmd]}));
    assert_eq!(text_of(&s, text).1.on_path.as_ref().unwrap().path.cmds.len(), 2);
    // Plain text still has no points.
    let plain = ElementId(run(&mut s, "text.create", json!({"text": "plain"}))["element"].as_u64().unwrap());
    let e = s.execute("path.move_points", &json!({"id": plain.0, "points": [[0, 0]], "dx": 1, "dy": 1})).unwrap_err().to_string();
    assert!(e.contains("attach it to a path"), "{e}");
    // Undo walks back through it all to lines of text.
    while s.history().undo_len() > 0 {
        s.undo().unwrap();
    }
}

#[test]
fn line_breaks_set_further_lines_parallel_to_the_path() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let one = ElementId(run(&mut s, "text.create", json!({"text": "First", "x": 50, "y": 50, "size": 24}))["element"].as_u64().unwrap());
    let line = ElementId(run(&mut s, "draw.line", json!({"x0": 100, "y0": 300, "x1": 500, "y1": 300}))["element"].as_u64().unwrap());
    run(&mut s, "element.select", json!({"ids": [one.0]}));
    run(&mut s, "text.attach_path", json!({"path": line.0}));
    let (xf, t1) = text_of(&s, one);
    let b1 = t1.outline.bounds().transformed(&xf);
    // The same block with a second line: the first line stays, the second is a line height
    // below the path, starting at the same place.
    let fonts = anicraft_text::Fonts::get();
    let lh = fonts.layout("First", &anicraft_text::Style { size: 24.0, family: t1.font.clone(), ..Default::default() }).unwrap().line_height;
    run(&mut s, "text.edit", json!({"text": "First\nFloor"}));
    let (xf, t2) = text_of(&s, one);
    assert_eq!(t2.text, "First\nFloor", "the line break is kept");
    let b2 = t2.outline.bounds().transformed(&xf);
    assert!((b2.min.y - b1.min.y).abs() < 0.05 && (b2.min.x - b1.min.x).abs() < 0.05, "the first line didn't move: {b1:?} {b2:?}");
    assert!((b2.max.y - (b1.max.y + lh)).abs() < 1.5, "the second line is one line height lower: {b1:?} {b2:?} {lh}");
    // Line spacing moves it further; flipping puts both lines on the path's other side.
    run(&mut s, "text.edit", json!({"line_spacing": 10}));
    let (xf, t3) = text_of(&s, one);
    let b3 = t3.outline.bounds().transformed(&xf);
    assert!((b3.max.y - b2.max.y - 10.0).abs() < 0.05, "{b2:?} {b3:?}");
    run(&mut s, "text.path", json!({"flip": true, "offset": 200}));
    let (xf, t4) = text_of(&s, one);
    let b4 = t4.outline.bounds().transformed(&xf);
    assert!(b4.max.y > 300.0 + 10.0 && b4.min.y < 300.0 - lh, "hanging below the line, its second line above it: {b4:?}");
    // Released, it is two lines of text again.
    run(&mut s, "text.release_path", json!({}));
    let (_, t5) = text_of(&s, one);
    assert!(t5.on_path.is_none() && t5.text == "First\nFloor" && t5.box_height > lh * 1.8);
}

/// Hostile parameters on the new surfaces (ADR 0107) return errors, never panic, and leave a
/// document that still evaluates and saves.
#[test]
fn hostile_point_edits_embeds_and_change_events_never_panic() {
    if !fonts() {
        return;
    }
    let (mut s, text, line) = text_and_line();
    run(&mut s, "element.select", json!({"ids": [text.0]}));
    run(&mut s, "text.edit", json!({"text": "a\n\n\nb\n".repeat(40)}));
    run(&mut s, "element.select", json!({"ids": [text.0, line.0]}));
    run(&mut s, "text.attach_path", json!({}));
    let id = text.0;
    let wild = [json!(null), json!(-1), json!(1e308), json!(u64::MAX), json!("x"), json!([]), json!({}), json!(f64::MIN_POSITIVE)];
    let mut tried = 0;
    for a in &wild {
        for b in &wild {
            for p in [
                json!({"id": id, "points": [[a, b]], "dx": b, "dy": a}),
                json!({"id": id, "points": [[1, 0]], "dx": a, "dy": b}),
                json!({"id": id, "points": a, "dx": 1, "dy": 1, "smooth": b}),
            ] {
                let _ = s.execute("path.move_points", &p);
                tried += 1;
            }
            let _ = s.execute("path.add_point", &json!({"id": id, "x": a, "y": b, "slop": a}));
            let _ = s.execute("path.delete_points", &json!({"id": id, "cmds": [a, b]}));
            let _ = s.execute("path.convert_point", &json!({"id": id, "cmd": a, "smooth": b}));
            let _ = s.execute("text.path", &json!({"offset": a, "flip": b}));
            let _ = s.execute("text.field", &json!({"embed": a, "type": b}));
            let _ = s.execute("actions.set", &json!({"element": id, "event": "change", "actions": a}));
            let _ = s.execute("actions.clear", &json!({"element": a, "event": "change"}));
        }
    }
    assert!(tried > 100);
    // Far moves that stay in range, a path folded onto itself, many lines: still text on a path.
    for (dx, dy) in [(1e5, -1e5), (-1e5, 1e5), (-399.0, 0.0), (0.5, 0.5)] {
        let _ = s.execute("path.move_points", &json!({"id": id, "points": [[1, 0]], "dx": dx, "dy": dy}));
    }
    let (_, t) = text_of(&s, text);
    assert!(t.on_path.is_some());
    assert!(t.outline.cmds.iter().all(|c| match c {
        anicraft_geom::PathCmd::MoveTo(p) | anicraft_geom::PathCmd::LineTo(p) => p.is_finite(),
        _ => true,
    }));
    let saved = crate::format::to_json(&s.doc).unwrap();
    assert_eq!(crate::format::from_json(&saved).unwrap(), s.doc);
    assert!(anicraft_render::render_frame(&s.doc, 0, 0.25).is_ok());
    // Plug-ins may call what artists can: the namespaces already have their policy.
    for cmd in ["path.move_points", "text.field", "text.path", "actions.set"] {
        let _ = crate::cmds_plugin::plugin_may_call(cmd);
    }
}

