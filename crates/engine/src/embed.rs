//! Embedded glyphs for the HTML export (ADR 0107): the outlines of the characters text fields
//! ask to embed, read from this computer's fonts where their licence bits allow, so text a
//! script sets in a dynamic field is drawn in the author's font in any browser.
//!
//! The export itself (`anicraft_render::html`) has no fonts: it draws what it is handed. A
//! face whose `OS/2` `fsType` forbids embedding, or allows it only whole, is left out and
//! named, and the page draws that field with the browser's fonts as before.

use anicraft_doc::{Document, ElementKind, TextKind, Timeline};
use anicraft_render::html::EmbeddedFace;
use anicraft_text::{Embedding, Fonts};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Most faces one page embeds.
pub const MAX_EMBEDDED_FACES: usize = 32;

/// What an export embeds, and what it says about it.
#[derive(Clone, Debug, Default)]
pub struct Embedded {
    pub faces: Vec<EmbeddedFace>,
    /// Lines for the export's report, with a count each.
    pub notes: Vec<(String, usize)>,
    /// The same as data: every face asked for, whether it was embedded and why not.
    pub summary: Value,
}

/// `(family, bold, italic)` → the characters asked for, over every field of the document.
fn wanted(doc: &Document) -> (BTreeMap<(String, bool, bool), String>, usize) {
    fn walk(tl: &Timeline, depth: usize, out: &mut BTreeMap<(String, bool, bool), String>, inputs: &mut usize) {
        if depth > anicraft_doc::MAX_SYMBOL_DEPTH {
            return;
        }
        for l in &tl.layers {
            for k in &l.keyframes {
                for e in &k.elements {
                    match &e.kind {
                        ElementKind::Text(t) => {
                            let Some(f) = t.field.as_ref().filter(|f| !f.embed.is_empty()) else { continue };
                            // An input field is a native element in the page: the browser
                            // draws what is typed.
                            if f.kind == TextKind::Input {
                                *inputs += 1;
                                continue;
                            }
                            out.entry((t.font.clone(), t.bold, t.italic)).or_default().push_str(&f.embed);
                        }
                        ElementKind::Group { members } => walk(members, depth + 1, out, inputs),
                        _ => {}
                    }
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    let mut inputs = 0;
    for s in &doc.scenes {
        walk(&s.timeline, 0, &mut out, &mut inputs);
    }
    for s in &doc.library {
        walk(&s.timeline, 0, &mut out, &mut inputs);
    }
    (out, inputs)
}

/// The faces `doc`'s fields ask to embed, with this computer's fonts.
pub fn embedded_faces(doc: &Document) -> Embedded {
    embedded_with(doc, Fonts::get())
}

/// [`embedded_faces`] with a given font library.
pub fn embedded_with(doc: &Document, fonts: &Fonts) -> Embedded {
    let (wanted, inputs) = wanted(doc);
    let mut out = Embedded::default();
    let mut rows: Vec<Value> = Vec::new();
    let (mut glyphs, mut refused, mut missing_faces, mut missing_chars) = (0usize, 0usize, 0usize, 0usize);
    for ((family, bold, italic), chars) in wanted.into_iter().take(MAX_EMBEDDED_FACES) {
        let chars = anicraft_doc::embed_chars(&chars);
        let table = fonts.glyph_table(&family, bold, italic, &chars).filter(|t| t.family.eq_ignore_ascii_case(family.trim()));
        let row = |state: &str, n: usize| json!({"family": family, "bold": bold, "italic": italic, "embedded": state == "embedded", "state": state, "glyphs": n});
        let Some(table) = table else {
            // The font isn't on this computer (a fallback's glyphs would be the wrong face).
            missing_faces += 1;
            rows.push(row("not installed", 0));
            continue;
        };
        match table.embedding {
            Embedding::Subset => {
                glyphs += table.glyphs.len();
                missing_chars += table.missing;
                rows.push(row("embedded", table.glyphs.len()));
                out.faces.push(EmbeddedFace {
                    family: family.clone(),
                    bold,
                    italic,
                    ascent: table.ascent,
                    line_height: table.line_height,
                    glyphs: table.glyphs.into_iter().map(|g| (g.ch, g.advance, Arc::new(g.outline))).collect(),
                });
            }
            Embedding::Whole => {
                refused += 1;
                rows.push(row("its licence allows embedding only the whole font", 0));
            }
            Embedding::Forbidden => {
                refused += 1;
                rows.push(row("its licence doesn't allow embedding", 0));
            }
        }
    }
    let mut note = |text: &str, n: usize| {
        if n > 0 {
            out.notes.push((text.to_string(), n));
        }
    };
    note("glyph outlines embedded for text fields (drawn without kerning or ligatures)", glyphs);
    note("fonts not embedded because their licence doesn't allow it (those fields use the browser's fonts)", refused);
    note("fonts to embed that aren't installed on this computer (those fields use the browser's fonts)", missing_faces);
    note("characters to embed that the font doesn't have", missing_chars);
    note("input fields with characters to embed (what the viewer types is drawn by the browser)", inputs);
    out.summary = Value::Array(rows);
    out
}
