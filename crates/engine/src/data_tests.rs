//! Data tables and bindings end to end (ADR 0118): import and re-import, inline tables, binding,
//! what `evaluate` shows, baking to keyframes, the Library, undo, the file and plug-ins.

use crate::{Session, format};
use anicraft_doc::{BoundProperty, DataId, Document, ElementId, ElementKind, LibraryItem, LoopMode, evaluate};
use anicraft_geom::Affine;
use serde_json::{Value, json};
use std::path::Path;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn fails(s: &mut Session, id: &str, p: Value) -> String {
    match s.execute(id, &p) {
        Ok(v) => panic!("{id} {p} should fail, gave {v}"),
        Err(e) => e.to_string(),
    }
}

/// Runs `f` on a thread of its own with the in-memory file store.
fn in_memory(f: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        crate::vfs::use_memory();
        f();
    })
    .join()
    .unwrap();
}

/// A document whose first layer runs 40 frames and holds one instance (selected) of a graphic
/// symbol: a 20 × 100 bar. The instance's id.
fn bar_doc() -> (Session, u64) {
    let mut s = Session::new(Document::default());
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 20, "h": 100, "fill": "#3366cc", "object": true}));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Bar", "kind": "graphic"}));
    run(&mut s, "timeline.insert_frame", json!({"frame": 39}));
    let id = s.selection.ids().next().expect("the instance is selected").0;
    (s, id)
}

fn table(s: &mut Session, name: &str, columns: Value) -> u64 {
    run(s, "data.set_table", json!({"name": name, "columns": columns}))["table"].as_u64().unwrap()
}

/// What every frame of the movie shows: each item's transform and opacity.
fn movie(s: &Session) -> Vec<Vec<(Affine, f32, f32)>> {
    (0..s.doc.frame_count())
        .map(|f| {
            let dl = evaluate(&s.doc, f);
            dl.items.iter().map(|i| (i.transform, dl.effective_alpha(i), i.path.bounds().width())).collect()
        })
        .collect()
}

fn same_movie(a: &[Vec<(Affine, f32, f32)>], b: &[Vec<(Affine, f32, f32)>]) {
    assert_eq!(a.len(), b.len());
    for (f, (x, y)) in a.iter().zip(b).enumerate() {
        assert_eq!(x.len(), y.len(), "frame {f}");
        for ((ma, aa, wa), (mb, ab, wb)) in x.iter().zip(y) {
            assert!(ma.m.iter().zip(mb.m).all(|(p, q)| (p - q).abs() < 1e-3), "frame {f}: {ma:?} vs {mb:?}");
            assert!((aa - ab).abs() < 1e-4 && wa == wb, "frame {f}");
        }
    }
}

#[test]
fn import_reads_files_and_importing_again_keeps_bindings() {
    in_memory(|| {
        let (mut s, el) = bar_doc();
        crate::vfs::put(Path::new("dir/core.csv"), b"sample,age,pinus,betula\nA,100,10,1\nB,300,30,3\nC,200,20,2\n".to_vec()).unwrap();
        let v = run(&mut s, "data.import_table", json!({"path": "dir/core.csv", "key": "age"}));
        assert_eq!((v["name"].as_str(), v["rows"].as_u64(), v["key"].as_str(), v["replaced"].as_bool()), (Some("core"), Some(3), Some("age"), Some(false)));
        assert_eq!(v["columns"], json!(["age", "pinus", "betula"]));
        assert_eq!(v["skipped"], json!(["sample"]));
        assert_eq!(v["source"], "core.csv", "the file's name, never its folder");
        let id = v["table"].as_u64().unwrap();
        // Keyed: the rows are in the key's order.
        let got = run(&mut s, "data.get", json!({"table": "CORE"}));
        assert_eq!(got["columns"][0]["values"], json!([100.0, 200.0, 300.0]));
        assert_eq!(got["columns"][1]["values"], json!([10.0, 20.0, 30.0]));
        run(&mut s, "data.bind", json!({"property": "scale_y", "table": "core", "column": "pinus"}));
        run(&mut s, "data.bind", json!({"property": "x", "table": id, "column": "betula"}));
        // Again, revised: the same table (by its name), its bindings and its key stay.
        crate::vfs::put(Path::new("dir/core.csv"), b"age,pinus\n100,50\n200,60\n".to_vec()).unwrap();
        let v = run(&mut s, "data.import_table", json!({"path": "dir/core.csv"}));
        assert_eq!((v["table"].as_u64(), v["replaced"].as_bool(), v["bindings"].as_u64(), v["key"].as_str()), (Some(id), Some(true), Some(2), Some("age")));
        assert_eq!(v["missing"], json!(["betula"]), "a binding whose column went rests");
        let b = run(&mut s, "data.bindings", json!({"element": el}));
        assert_eq!(b["bindings"].as_array().unwrap().len(), 2);
        assert_eq!(b["bindings"][0]["live"], false);
        assert_eq!((b["bindings"][1]["live"].as_bool(), b["bindings"][1]["value"].as_f64()), (Some(true), Some(50.0)));
        // Under another name, or into a table named by id.
        let other = run(&mut s, "data.import_table", json!({"path": "dir/core.csv", "name": "Second"}));
        assert_ne!(other["table"].as_u64(), Some(id));
        crate::vfs::put(Path::new("new.json"), br#"[{"age": 1, "pinus": 2}]"#.to_vec()).unwrap();
        let v = run(&mut s, "data.import_table", json!({"path": "new.json", "table": id}));
        assert_eq!((v["table"].as_u64(), v["name"].as_str(), v["rows"].as_u64(), v["source"].as_str()), (Some(id), Some("core"), Some(1), Some("new.json")));
        // Refused: no file, no numbers, a key that isn't there, a format that isn't one.
        assert!(fails(&mut s, "data.import_table", json!({"path": "missing.csv"})).contains("I/O"));
        crate::vfs::put(Path::new("text.csv"), b"a,b\nx,y\n".to_vec()).unwrap();
        assert!(fails(&mut s, "data.import_table", json!({"path": "text.csv"})).contains("no numbers"));
        assert!(fails(&mut s, "data.import_table", json!({"path": "dir/core.csv", "name": "Third", "key": "depth"})).contains("no column 'depth'"));
        assert!(fails(&mut s, "data.import_table", json!({"path": "dir/core.csv", "format": "xlsx"})).contains("unknown format"));
        assert_eq!(s.doc.data.tables.len(), 2, "a refused import leaves nothing behind");
    });
}

#[test]
fn inline_tables_in_every_shape() {
    let mut s = Session::new(Document::default());
    let a = table(&mut s, "A", json!({"x": [1, 2, null], "y": [4, 5, 6]}));
    let got = run(&mut s, "data.get", json!({"table": a}));
    assert_eq!(got["columns"], json!([{"name": "x", "values": [1.0, 2.0, null]}, {"name": "y", "values": [4.0, 5.0, 6.0]}]));
    let b = table(&mut s, "B", json!([{"name": "v", "values": [1, 2]}]));
    assert_eq!(run(&mut s, "data.get", json!({"table": b, "from": 1}))["columns"][0]["values"], json!([2.0]));
    let c = run(&mut s, "data.set_table", json!({"name": "C", "columns": ["t", "v"], "rows": [[0, 1], [10, 2]], "key": "t"}));
    assert_eq!((c["rows"].as_u64(), c["key"].as_str()), (Some(2), Some("t")));
    // Setting a table that is there replaces it.
    let again = run(&mut s, "data.set_table", json!({"name": "a", "columns": {"x": [9]}}));
    assert_eq!((again["table"].as_u64(), again["replaced"].as_bool(), again["rows"].as_u64()), (Some(a), Some(true), Some(1)));
    let l = run(&mut s, "data.list", json!({}));
    assert_eq!(l["tables"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["A", "B", "C"]);
    // One cell, and the key.
    run(&mut s, "data.set_cell", json!({"table": "C", "column": "t", "row": 0, "value": 20}));
    assert_eq!(run(&mut s, "data.get", json!({"table": "C"}))["columns"][1]["values"], json!([2.0, 1.0]), "a changed key moves its row");
    run(&mut s, "data.set_cell", json!({"table": "C", "column": "v", "row": 1, "value": null}));
    run(&mut s, "data.set_key", json!({"table": "C", "key": null}));
    assert_eq!(run(&mut s, "data.list", json!({}))["tables"][2]["key"], Value::Null);
    for (p, why) in [
        (json!({"columns": {"x": [1]}}), "'name' is required"),
        (json!({"name": "D"}), "'columns' is required"),
        (json!({"name": "D", "columns": 5}), "'columns' must be"),
        (json!({"name": "D", "columns": {"x": ["a"]}}), "no numbers"),
        (json!({"name": "D", "columns": {"x": [1]}, "rows": [[1]]}), "column names"),
        (json!({"name": "a/b", "columns": {"x": [1]}}), "/"),
        (json!({"name": "D", "columns": {"x": [1]}, "key": "y"}), "no column 'y'"),
        (json!({"table": 999, "columns": {"x": [1]}}), "no data table with id"),
    ] {
        let e = fails(&mut s, "data.set_table", p.clone());
        assert!(e.contains(why), "{p}: {e}");
    }
    assert!(fails(&mut s, "data.set_cell", json!({"table": "C", "column": "v", "row": 9, "value": 1})).contains("2 rows"));
    assert!(fails(&mut s, "data.set_cell", json!({"table": "C", "column": "v", "row": 0, "value": "x"})).contains("finite number"));
    assert!(fails(&mut s, "data.get", json!({"table": "nope"})).contains("no data table named"));
}

#[test]
fn a_binding_drives_the_instance_and_follows_the_table() {
    let (mut s, el) = bar_doc();
    let t = table(&mut s, "pollen", json!({"pinus": [100, 200, 50], "x": [0, 10, 20]}));
    let own = evaluate(&s.doc, 0).items[0].transform;
    let v = run(&mut s, "data.bind", json!({"property": "scale_y", "table": "pollen", "column": "PINUS"}));
    assert_eq!((v["start"].as_u64(), v["frames"].as_u64(), v["column"].as_str(), v["value"].as_f64()), (Some(0), Some(3), Some("pinus"), Some(100.0)), "one frame per row, from the instance's first frame");
    let sy = |s: &Session, f| evaluate(&s.doc, f).items[0].transform.m[3];
    assert!((sy(&s, 0) - 1.0).abs() < 1e-5 && (sy(&s, 1) - 2.0).abs() < 1e-5 && (sy(&s, 2) - 0.5).abs() < 1e-5);
    assert!((sy(&s, 30) - 0.5).abs() < 1e-5, "held after the span");
    // Editing one number changes the frame; nothing else is touched.
    run(&mut s, "data.set_cell", json!({"table": t, "column": "pinus", "row": 1, "value": 400}));
    assert!((sy(&s, 1) - 4.0).abs() < 1e-5);
    // A span of its own, with scale and offset; rebinding a property replaces its binding.
    run(&mut s, "data.bind", json!({"property": "scale_y", "table": t, "column": "pinus", "element": el, "start": 10, "frames": 21, "scale": 0.5, "offset": 100, "interpolation": "step"}));
    assert_eq!(s.doc.data.bindings.len(), 1);
    assert!((sy(&s, 0) - 1.5).abs() < 1e-5 && (sy(&s, 19) - 1.5).abs() < 1e-5 && (sy(&s, 20) - 3.0).abs() < 1e-5 && (sy(&s, 30) - 1.25).abs() < 1e-5);
    // Unbind: as placed again. Undo brings the binding back.
    run(&mut s, "data.unbind", json!({}));
    assert_eq!(evaluate(&s.doc, 20).items[0].transform, own);
    s.undo().unwrap();
    assert!((sy(&s, 20) - 3.0).abs() < 1e-5);
    // What is refused.
    for (p, why) in [
        (json!({"table": t, "column": "pinus"}), "'property' is required"),
        (json!({"property": "skew", "table": t, "column": "pinus"}), "unknown property"),
        (json!({"property": "x", "column": "pinus"}), "'table' is required"),
        (json!({"property": "x", "table": t, "column": "quercus"}), "it has: pinus, x"),
        (json!({"property": "x", "table": t, "column": "x", "element": 999_999}), "no object with id"),
        (json!({"property": "x", "table": t, "column": "x", "frames": 1_000_000}), "between 1 and"),
        (json!({"property": "x", "table": t, "column": "x", "scale": 1e300}), "finite number"),
        (json!({"property": "x", "table": t, "column": "x", "interpolation": "cubic"}), "unknown interpolation"),
    ] {
        let e = fails(&mut s, "data.bind", p.clone());
        assert!(e.contains(why), "{p}: {e}");
    }
    // Only symbol instances; only a graphic shows a frame picked by data.
    run(&mut s, "draw.rect", json!({"x": 300, "y": 100, "w": 20, "h": 20, "object": true}));
    assert!(fails(&mut s, "data.bind", json!({"property": "x", "table": t, "column": "x"})).contains("symbol instances"));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Clip", "kind": "movie_clip"}));
    assert!(fails(&mut s, "data.bind", json!({"property": "frame", "table": t, "column": "x"})).contains("graphic"));
    run(&mut s, "data.bind", json!({"property": "alpha", "table": t, "column": "x"}));
    s.selection = Default::default();
    assert!(fails(&mut s, "data.unbind", json!({})).contains("nothing is selected"));
    assert!(fails(&mut s, "data.unbind", json!({"element": el, "property": "rotation"})).contains("no data binding"));
}

#[test]
fn a_keyed_table_maps_onto_the_frames_the_instance_is_on() {
    let (mut s, _) = bar_doc();
    table(&mut s, "ages", json!({"age": [0, 1000, 4000], "v": [100, 200, 500]}));
    run(&mut s, "data.set_key", json!({"table": "ages", "key": "age"}));
    let v = run(&mut s, "data.bind", json!({"property": "scale_x", "table": "ages", "column": "v"}));
    assert_eq!((v["start"].as_u64(), v["frames"].as_u64()), (Some(0), Some(40)));
    let sx = |s: &Session, f| evaluate(&s.doc, f).items[0].transform.m[0];
    assert!((sx(&s, 0) - 1.0).abs() < 1e-5 && (sx(&s, 39) - 5.0).abs() < 1e-5);
    // Backwards, old to young.
    run(&mut s, "data.bind", json!({"property": "scale_x", "table": "ages", "column": "v", "from": 4000, "to": 0}));
    assert!((sx(&s, 0) - 5.0).abs() < 1e-5 && (sx(&s, 39) - 1.0).abs() < 1e-5);
}

#[test]
fn bake_writes_keyframes_and_every_frame_looks_as_it_did() {
    // Step data on a static span: a keyframe where the value changes.
    let (mut s, el) = bar_doc();
    table(&mut s, "d", json!({"v": [100, 100, 250, 250, 250, 40], "a": [100, 80, 60, 40, 20, 0]}));
    run(&mut s, "data.bind", json!({"property": "scale_y", "table": "d", "column": "v", "interpolation": "step"}));
    let live = movie(&s);
    let v = run(&mut s, "data.bake", json!({}));
    assert_eq!((v["keys"].as_u64(), v["removed"].as_u64()), (Some(2), Some(1)));
    assert!(s.doc.data.bindings.is_empty());
    assert_eq!(s.timeline().layers[0].keyframes.iter().map(|k| k.start).collect::<Vec<_>>(), [0, 2, 5]);
    same_movie(&live, &movie(&s));
    // One undo step brings the binding (and the single keyframe) back.
    s.undo().unwrap();
    assert_eq!((s.doc.data.bindings.len(), s.timeline().layers[0].keyframes.len()), (1, 1));

    // Linear data over more frames than rows, two properties: a keyframe a frame while it moves.
    run(&mut s, "data.bind", json!({"property": "scale_y", "table": "d", "column": "v", "frames": 21}));
    run(&mut s, "data.bind", json!({"property": "alpha", "table": "d", "column": "a", "start": 5, "frames": 11}));
    let live = movie(&s);
    // Only one of them: the other stays live.
    let v = run(&mut s, "data.bake", json!({"property": "alpha"}));
    assert_eq!((v["keys"].as_u64(), v["removed"].as_u64()), (Some(10), Some(1)));
    assert_eq!(s.doc.data.bindings.iter().map(|b| b.property).collect::<Vec<_>>(), [BoundProperty::ScaleY]);
    same_movie(&live, &movie(&s));
    run(&mut s, "data.bake", json!({"element": el}));
    same_movie(&live, &movie(&s));
    assert!(fails(&mut s, "data.bake", json!({"element": el})).contains("no data binding"));
}

#[test]
fn bake_inside_a_classic_tween_and_with_a_looping_graphic() {
    let (mut s, el) = bar_doc();
    // The instance tweens across the stage over frames 0..20 and holds to 39.
    run(&mut s, "timeline.insert_keyframe", json!({"frame": 20}));
    run(&mut s, "timeline.goto", json!({"frame": 20}));
    s.select_only([ElementId(el)]);
    run(&mut s, "element.set_transform", json!({"x": 400, "y": 300}));
    run(&mut s, "timeline.create_tween", json!({"frame": 0}));
    table(&mut s, "d", json!({"v": [100, 300, 50, 200]}));
    run(&mut s, "data.bind", json!({"property": "scale_y", "table": "d", "column": "v", "element": el, "start": 4, "frames": 30}));
    let live = movie(&s);
    assert!((live[10][0].0.m[4] - live[0][0].0.m[4]).abs() > 50.0, "the tween still moves it");
    run(&mut s, "data.bake", json!({"element": el}));
    same_movie(&live, &movie(&s));

    // A looping graphic of three frames (20, 60 and 90 wide): after the bake cuts its span,
    // each new keyframe carries on from the frame it was showing.
    let (mut s, el) = bar_doc();
    let sym = s.doc.library[0].id.0;
    run(&mut s, "symbol.edit", json!({"symbol": sym}));
    for f in 1..3u32 {
        run(&mut s, "timeline.insert_blank_keyframe", json!({"frame": f}));
        run(&mut s, "timeline.goto", json!({"frame": f}));
        run(&mut s, "draw.rect", json!({"x": 0, "y": 0, "w": 30 * (f + 1), "h": 10, "object": true}));
    }
    run(&mut s, "symbol.exit", json!({}));
    assert!(matches!(&s.timeline().layers[0].keyframes[0].elements[0].kind, ElementKind::Symbol { loop_mode: LoopMode::Loop, .. }));
    table(&mut s, "d", json!({"x": [0, 0, 0, 0, 50, 50, 50, 90]}));
    run(&mut s, "data.bind", json!({"property": "x", "table": "d", "column": "x", "element": el, "interpolation": "step"}));
    let live = movie(&s);
    let widths: Vec<f32> = live.iter().take(6).map(|f| f[0].2).collect();
    assert_ne!(widths[0], widths[1], "the graphic loops: {widths:?}");
    assert_eq!(run(&mut s, "data.bake", json!({"element": el}))["keys"], 2);
    same_movie(&live, &movie(&s));

    // The frame a graphic shows, picked by a column and baked.
    let (mut s2, el2) = (s, el);
    s2.undo().unwrap();
    run(&mut s2, "data.unbind", json!({"element": el2}));
    table(&mut s2, "f", json!({"level": [2, 0, 1, 1]}));
    run(&mut s2, "data.bind", json!({"property": "frame", "table": "f", "column": "level", "element": el2, "interpolation": "step"}));
    let live = movie(&s2);
    assert_eq!(live.iter().take(5).map(|f| f[0].2).collect::<Vec<_>>(), [90.0, 20.0, 60.0, 60.0, 60.0]);
    run(&mut s2, "data.bake", json!({"element": el2}));
    same_movie(&live, &movie(&s2));
}

#[test]
fn bake_refuses_what_it_would_break() {
    let (mut s, el) = bar_doc();
    table(&mut s, "d", json!({"v": [100, 300]}));
    run(&mut s, "data.bind", json!({"property": "scale_y", "table": "d", "column": "v", "frames": 20}));
    let layer = s.layer.0;
    run(&mut s, "layer.toggle_lock", json!({"layer": layer, "on": true}));
    assert!(fails(&mut s, "data.bake", json!({"element": el})).contains("locked"));
    run(&mut s, "layer.toggle_lock", json!({"layer": layer, "on": false}));
    run(&mut s, "timeline.create_motion_tween", json!({"frame": 0}));
    let before = s.doc.clone();
    assert!(fails(&mut s, "data.bake", json!({"element": el})).contains("motion tween"));
    assert_eq!(s.doc, before, "a refused bake changes nothing");
    // A binding inside a symbol bakes once that symbol is open.
    let (mut s, el) = bar_doc();
    table(&mut s, "d", json!({"v": [100, 300]}));
    run(&mut s, "data.bind", json!({"property": "scale_y", "table": "d", "column": "v", "frames": 20}));
    s.select_only([ElementId(el)]);
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Outer", "kind": "graphic"}));
    assert!(fails(&mut s, "data.bake", json!({"element": el})).contains("another timeline"));
}

#[test]
fn tables_are_library_items() {
    let (mut s, _) = bar_doc();
    let t = table(&mut s, "pollen", json!({"v": [1, 2]}));
    run(&mut s, "data.bind", json!({"property": "x", "table": t, "column": "v"}));
    let items = run(&mut s, "library.items", json!({}));
    let row = items["items"].as_array().unwrap().iter().find(|i| i["data"] == t).expect("the table is listed").clone();
    assert_eq!((row["kind"].as_str(), row["rows"].as_u64(), row["columns"].as_u64(), row["uses"].as_u64(), row["unused"].as_bool()), (Some("data"), Some(2), Some(1), Some(1), Some(false)));
    // Rename (bindings follow: they name the table by id), move into a folder, refuse a clash.
    run(&mut s, "library.rename", json!({"data": t, "name": "Pollen 2026"}));
    let f = run(&mut s, "library.new_folder", json!({"name": "Data"}))["folder"].as_u64().unwrap();
    run(&mut s, "library.move", json!({"items": [{"data": t}], "to": f}));
    assert_eq!(s.doc.entry_path(anicraft_doc::LibraryEntry::Item(LibraryItem::Data(DataId(t)))), "Data/Pollen 2026");
    table(&mut s, "other", json!({"v": [1]}));
    assert!(fails(&mut s, "library.rename", json!({"data": t, "name": "OTHER"})).contains("already named"));
    assert_eq!(run(&mut s, "data.bindings", json!({}))["bindings"][0]["table_name"], "Pollen 2026");
    // A table can't be placed on the stage.
    assert!(fails(&mut s, "library.place", json!({"data": t})).contains("isn't artwork") || fails(&mut s, "library.place", json!({"items": [{"data": t}]})).contains("isn't artwork"));
    // Deleting the folder's table takes its bindings with it; undo brings both back.
    let v = run(&mut s, "library.delete", json!({"items": [{"folder": f}], "confirm": true}));
    assert_eq!((v["deleted"].as_u64(), v["uses"].as_u64()), (Some(1), Some(1)));
    assert!(s.doc.data.bindings.is_empty() && s.doc.data.tables.len() == 1);
    s.undo().unwrap();
    assert_eq!((s.doc.data.bindings.len(), s.doc.data.tables.len()), (1, 2));
    assert_eq!(run(&mut s, "data.remove", json!({"table": "Pollen 2026"}))["uses"], 1);
    // Unused tables go with Delete Unused Items.
    assert_eq!(run(&mut s, "library.unused", json!({}))["items"], json!([{"data": s.doc.data.tables[0].id.0}]));
}

#[test]
fn data_is_saved_and_older_files_stay_the_same() {
    let (mut s, _) = bar_doc();
    let plain = format::to_json(&s.doc).unwrap();
    assert!(!plain.contains("\"data\""), "a document without data writes no data field");
    table(&mut s, "pollen", json!({"age": [3, 1, 2], "v": [1.5, null, -2e-3]}));
    run(&mut s, "data.set_key", json!({"table": "pollen", "key": "age"}));
    run(&mut s, "data.bind", json!({"property": "rotation", "table": "pollen", "column": "v", "scale": 2, "interpolation": "step"}));
    let text = format::to_json(&s.doc).unwrap();
    assert_eq!(format::version_for(&s.doc), format::BASE_VERSION, "data needs no newer file version");
    let back = format::from_json(&text).unwrap();
    assert_eq!(back, s.doc);
    assert_eq!(movie(&Session::new(back)), movie(&s));
}

#[test]
fn plug_ins_may_use_tables_but_not_read_files() {
    use crate::cmds_plugin::plugin_may_call;
    for id in ["data.set_table", "data.list", "data.get", "data.set_cell", "data.set_key", "data.remove", "data.bind", "data.unbind", "data.bindings", "data.bake"] {
        assert!(crate::find_command(id).is_some(), "{id}");
        assert!(plugin_may_call(id).is_ok(), "{id}");
    }
    assert!(plugin_may_call("data.import_table").is_err());
}

#[test]
fn hostile_params_never_panic() {
    let (mut s, el) = bar_doc();
    table(&mut s, "d", json!({"v": [1, 2, 3]}));
    let junk = [
        Value::Null,
        json!([]),
        json!({"table": -1, "column": 7, "property": 3, "element": "x"}),
        json!({"table": "d", "column": "v", "property": "x", "element": el, "start": u64::MAX, "frames": 0, "from": 1e308, "to": -1e308, "scale": f64::MAX}),
        json!({"table": "d", "column": "v", "property": "frame", "element": el, "frames": 100_000, "from": -1e9, "to": 1e9, "scale": 1e9, "offset": -1e9}),
        json!({"table": "d", "column": "v", "row": u64::MAX, "value": 1e308, "key": 5, "from": u64::MAX, "count": u64::MAX}),
        json!({"name": "x".repeat(10_000), "columns": {"": [1e308, -1e308, null, "x", [1], {"a": 1}]}, "key": ""}),
        json!({"name": "n", "columns": (0..600).map(|i| json!({"name": format!("c{i}"), "values": [i]})).collect::<Vec<_>>()}),
        json!({"name": "n", "columns": ["a"], "rows": (0..2_000).map(|i| json!([i, i, i])).collect::<Vec<_>>()}),
        json!({"path": "", "format": "json", "table": "d"}),
        json!({"path": "\u{0}/../..", "name": "/", "key": null}),
    ];
    for c in crate::commands().into_iter().filter(|c| c.id.starts_with("data.")) {
        for p in &junk {
            let _ = s.execute(c.id, p);
            for f in [0, 1, 39] {
                assert!(evaluate(&s.doc, f).items.iter().all(|i| i.transform.is_finite()), "{} {p}", c.id);
            }
        }
    }
}

/// The numbers in ADR 0118 (`cargo test -p anicraft-engine data_timings -- --ignored --nocapture`).
#[test]
#[ignore = "prints timings; not a check"]
fn data_timings() {
    use std::time::Instant;
    // Import: 100 000 rows of 10 columns.
    let mut csv = String::from("age,a,b,c,d,e,f,g,h,i\n");
    for r in 0..100_000 {
        csv.push_str(&format!("{r},{}.5,{},{},{},{},{},{},{},{}\n", r % 97, r % 13, r * 3, r % 7, r % 11, r % 5, r % 3, r % 2, r % 17));
    }
    let t = Instant::now();
    let p = crate::data_import::parse(csv.as_bytes(), "big.csv", None).unwrap();
    println!("import: {} rows x {} columns from {:.1} MB of CSV in {:.1} ms", p.rows(), p.columns.len(), csv.len() as f64 / 1e6, t.elapsed().as_secs_f64() * 1e3);
    // Evaluate: 200 instances, each bound to a 1 000-row keyed table.
    let (mut s, first) = bar_doc();
    let sym = s.doc.library[0].id.0;
    let mut bars = vec![first];
    for i in 1..200 {
        bars.push(run(&mut s, "library.place", json!({"symbol": sym, "x": 10 + i * 4, "y": 200}))["element"].as_u64().unwrap());
    }
    let frames = s.doc.frame_count();
    let time = |s: &Session| {
        let t = Instant::now();
        for round in 0..5 {
            for f in 0..frames {
                std::hint::black_box(evaluate(&s.doc, (f + round) % frames));
            }
        }
        t.elapsed().as_secs_f64() * 1e6 / f64::from(frames * 5)
    };
    let plain = time(&s);
    let age: Vec<u32> = (0..1_000).collect();
    let v: Vec<f64> = (0..1_000).map(|i| 50.0 + f64::from(i % 100)).collect();
    run(&mut s, "data.set_table", json!({"name": "t", "columns": [{"name": "age", "values": age}, {"name": "v", "values": v}], "key": "age"}));
    for el in &bars {
        run(&mut s, "data.bind", json!({"element": el, "property": "scale_y", "table": "t", "column": "v"}));
    }
    println!("evaluate, 200 instances: {plain:.0} us a frame unbound, {:.0} us with 200 bindings on a 1 000-row keyed table", time(&s));
    let t = Instant::now();
    s.select_only(bars.iter().map(|b| ElementId(*b)));
    let v = run(&mut s, "data.bake", json!({}));
    println!("bake: {} keyframes for 200 instances in {:.1} ms", v["keys"], t.elapsed().as_secs_f64() * 1e3);
}
