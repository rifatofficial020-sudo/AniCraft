//! The Oval tool's default outline has eight nodes, like Animate's.

use crate::Session;
use anicraft_doc::ElementKind;
use serde_json::json;

#[test]
fn default_ellipse_has_eight_nodes() {
    let mut s = Session::new(anicraft_doc::Document::default());
    s.execute("draw.ellipse", &json!({"x": 10.0, "y": 20.0, "w": 100.0, "h": 60.0})).unwrap();
    let counts: Vec<usize> = s
        .visible_elements()
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Shape(sh) => Some(sh.path.editable_anchors().len()),
            _ => None,
        })
        .collect();
    assert_eq!(counts, vec![8]);
}
