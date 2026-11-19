//! Copy in one document, paste in another (ADR 0112): the Library items come along.

use crate::Session;
use anicraft_doc::{ElementKind, Paint, SymbolId};
use serde_json::{Value, json};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A document with one symbol, "Box", a square of `colour` at (100, 100), selected.
fn with_box(colour: &str) -> Session {
    let mut s = Session::default();
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": colour, "stroke": null}));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Box"}));
    s
}

fn px(s: &Session, x: u32, y: u32) -> [f32; 4] {
    anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().pixel(x, y).unwrap()
}

fn placed(s: &Session) -> Vec<SymbolId> {
    s.elements_on(s.layer).iter().filter_map(|e| if let ElementKind::Symbol { symbol, .. } = e.kind { Some(symbol) } else { None }).collect()
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

#[test]
fn a_symbol_pasted_into_another_document_comes_along_renamed_on_a_clash() {
    let mut a = with_box("#ff0000");
    let mut b = with_box("#0000ff");
    // Each document counts its own ids: the two symbols have the same one.
    assert_eq!(a.doc.library[0].id, b.doc.library[0].id);
    run(&mut a, "edit.copy", json!({}));
    b.take_shared(&mut a);
    assert!(a.clipboard.is_empty(), "the copy went with the editor");
    let before = b.history_len();
    run(&mut b, "edit.paste", json!({"x": 400, "y": 300}));
    let names: Vec<&str> = b.doc.library.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Box", "Box 2"], "its own Box is untouched, the other came under a new name");
    assert_ne!(b.doc.library[0].id, b.doc.library[1].id);
    assert_eq!(placed(&b), vec![b.doc.library[0].id, b.doc.library[1].id], "the pasted instance places the one that came");
    assert_eq!(px(&b, 150, 150), BLUE, "this document's box");
    assert_eq!(px(&b, 400, 300), RED, "the pasted one looks as it did where it was copied");
    // One undo step takes the instance and the Library item away again.
    assert_eq!(b.history_len(), before + 1);
    run(&mut b, "edit.undo", json!({}));
    assert_eq!(b.doc.library.len(), 1);
    // Ids stay unique: nothing made later takes one the pasted symbol uses inside.
    run(&mut b, "edit.redo", json!({}));
    let inside: Vec<u64> = b.doc.library[1].timeline.layers.iter().flat_map(|l| std::iter::once(l.id.0).chain(l.keyframes.iter().flat_map(|k| k.elements.iter().map(|e| e.id.0)))).collect();
    assert!(inside.iter().all(|id| *id < b.doc.next_id), "{inside:?} < {}", b.doc.next_id);
}

#[test]
fn pasting_the_same_symbol_twice_brings_it_once() {
    let mut a = with_box("#ff0000");
    let mut b = with_box("#0000ff");
    run(&mut a, "edit.copy", json!({}));
    b.take_shared(&mut a);
    run(&mut b, "edit.paste", json!({"x": 400, "y": 300}));
    run(&mut b, "edit.paste", json!({"x": 500, "y": 300}));
    assert_eq!(b.doc.library.len(), 2, "one Box 2, placed twice");
    let ids = placed(&b);
    assert_eq!(ids.len(), 3);
    assert_eq!(ids[1], ids[2]);
    // Changed here since: the next paste brings the original again rather than the changed one.
    b.doc.library[1] = std::sync::Arc::new(anicraft_doc::Symbol { name: "Mine".into(), ..(*b.doc.library[1]).clone() });
    run(&mut b, "edit.paste", json!({"x": 300, "y": 300}));
    let names: Vec<&str> = b.doc.library.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Box", "Mine", "Box 2"]);
    // Undone, the item is gone, and a paste brings it back rather than pointing at nothing.
    run(&mut b, "edit.undo", json!({}));
    run(&mut b, "edit.paste", json!({"x": 300, "y": 300}));
    assert!(placed(&b).iter().all(|id| b.doc.symbol(*id).is_some()));
}

#[test]
fn symbols_inside_symbols_and_groups_follow() {
    let mut a = with_box("#ff0000");
    // Box inside Outer, Outer inside a group.
    run(&mut a, "modify.convert_to_symbol", json!({"name": "Outer"}));
    run(&mut a, "draw.rect", json!({"x": 300, "y": 100, "w": 50, "h": 50, "fill": "#00ff00", "stroke": null}));
    run(&mut a, "edit.select_all", json!({}));
    run(&mut a, "modify.group", json!({}));
    run(&mut a, "edit.copy", json!({}));
    assert_eq!(a.clipboard.carried.symbols.len(), 2, "both symbols travel");
    let mut b = with_box("#0000ff");
    run(&mut b, "edit.delete", json!({}));
    // Something else under the ids the copied symbols have.
    run(&mut b, "draw.rect", json!({"x": 0, "y": 0, "w": 20, "h": 20, "fill": "#000000", "stroke": null}));
    run(&mut b, "modify.convert_to_symbol", json!({"name": "Dot"}));
    b.take_shared(&mut a);
    run(&mut b, "edit.paste_in_place", json!({}));
    assert_eq!(b.doc.library.len(), 4, "Box, Dot, and the two that came");
    assert_eq!(px(&b, 150, 150), RED, "the box inside Outer inside the group");
    // Every reference resolves, and to a symbol that came (not to this document's own).
    let uses = b.doc.library_uses();
    let came: Vec<SymbolId> = b.doc.library.iter().filter(|s| s.name == "Outer" || s.name == "Box 2").map(|s| s.id).collect();
    assert_eq!(came.len(), 2);
    assert!(came.iter().all(|id| uses.get(&anicraft_doc::LibraryItem::Symbol(*id)) == Some(&1)), "{uses:?}");
}

#[test]
fn a_paste_into_the_same_document_uses_what_is_there() {
    let mut a = with_box("#ff0000");
    run(&mut a, "edit.copy", json!({}));
    run(&mut a, "edit.paste", json!({"x": 400, "y": 300}));
    assert_eq!(a.doc.library.len(), 1);
    let ids = placed(&a);
    assert_eq!(ids[0], ids[1]);
    // After New in the same session (the CLI's one document), the copy still pastes.
    run(&mut a, "file.new", json!({"confirm": true}));
    run(&mut a, "edit.paste", json!({"x": 400, "y": 300}));
    assert_eq!(a.doc.library.len(), 1);
    assert_eq!(px(&a, 400, 300), RED);
}

#[test]
fn a_bitmap_fill_comes_along_once() {
    let dir = std::env::temp_dir().join(format!("anicraft-transfer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("dot.png");
    let rgba: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    std::fs::write(&file, anicraft_render::encode_png(4, 4, &rgba).unwrap()).unwrap();
    let mut a = Session::default();
    run(&mut a, "file.import", json!({"path": file.display().to_string()}));
    run(&mut a, "edit.select_all", json!({}));
    run(&mut a, "edit.copy", json!({}));
    assert_eq!(a.clipboard.carried.bitmaps.len(), 1);
    // The other document already has a picture under that id.
    let mut b = Session::default();
    let other = dir.join("other.png");
    std::fs::write(&other, anicraft_render::encode_png(4, 4, &[0u8, 0, 255, 255].repeat(16)).unwrap()).unwrap();
    run(&mut b, "file.import", json!({"path": other.display().to_string()}));
    let _ = std::fs::remove_dir_all(&dir);
    b.take_shared(&mut a);
    run(&mut b, "edit.paste", json!({"x": 300, "y": 300}));
    run(&mut b, "edit.paste", json!({"x": 350, "y": 300}));
    assert_eq!(b.doc.bitmaps.len(), 2, "the red picture came, once");
    let fills: Vec<_> = b
        .doc
        .timeline(0)
        .layers
        .iter()
        .flat_map(|l| b.elements_on(l.id))
        .filter_map(|e| if let ElementKind::Shape(s) = &e.kind && let Some(Paint::Bitmap { bitmap, .. }) = &s.fill { Some(*bitmap) } else { None })
        .collect();
    assert_eq!(fills.len(), 3);
    assert!(fills.iter().all(|id| b.doc.bitmap(*id).is_some()));
    assert_eq!(px(&b, 300, 300), RED);
}

#[test]
fn copied_frames_bring_their_symbols() {
    let mut a = with_box("#ff0000");
    run(&mut a, "timeline.copy_frames", json!({"frame": 0, "count": 1}));
    let mut b = with_box("#0000ff");
    b.take_shared(&mut a);
    run(&mut b, "layer.new", json!({}));
    run(&mut b, "timeline.paste_frames", json!({"frame": 0}));
    assert_eq!(b.doc.library.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Box", "Box 2"]);
    let uses = b.doc.library_uses();
    assert!(b.doc.library.iter().all(|s| uses.get(&anicraft_doc::LibraryItem::Symbol(s.id)) == Some(&1)), "{uses:?}");
}

#[test]
fn taking_the_editors_share_leaves_each_document_its_own() {
    let mut a = with_box("#ff0000");
    let mut b = Session::default();
    a.style.stroke_width = 7.0;
    let (a_state, a_steps, a_sel) = (a.state, a.history_len(), a.selection.len());
    assert_ne!(a.origin, b.origin);
    b.take_shared(&mut a);
    assert_eq!(b.style.stroke_width, 7.0, "the tools stay as they were");
    assert_eq!((a.state, a.history_len(), a.selection.len()), (a_state, a_steps, a_sel));
    assert!(b.doc.library.is_empty() && b.history_len() == 0);
    // New and Open make it another document: a copy from before is from elsewhere.
    let was = a.origin;
    run(&mut a, "file.new", json!({"confirm": true}));
    assert_ne!(a.origin, was);
}
