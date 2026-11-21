//! Font embedding (ADR 0107): whether a face's licence bits allow its glyphs to travel with an
//! export, and the glyphs of chosen characters as outlines, for exports that draw text the
//! author didn't type (a text field a script fills in the HTML page).
//!
//! The permission is the OpenType `OS/2` table's `fsType` field, read as the specification
//! describes it ("OS/2 — fsType", "Embedding licensing rights"). The three cases and their rule
//! are the ones proposed for the shared Craft text library (craft-libs issue #82), so this
//! function can be swapped for the shared one without a change in behaviour.

use crate::{Fonts, Pen};
use anicraft_geom::Path;
use skrifa::MetadataProvider as _;
use skrifa::raw::TableProvider as _;

/// Most characters embedded from one face.
pub const MAX_EMBED_CHARS: usize = 1024;

/// The em size glyph tables are drawn at (pixels): outlines and advances scale by
/// `size / EMBED_EM`.
pub const EMBED_EM: f32 = 1000.0;

/// What a face's licence bits allow an export to do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Embedding {
    /// Its glyphs may be embedded, and only the ones used.
    Subset,
    /// It may be embedded, but only whole ("no subsetting").
    Whole,
    /// It may not be embedded ("restricted licence"), or only as bitmaps.
    Forbidden,
}

impl Embedding {
    /// The permission in an `OS/2` `fsType` value. Restricted (`0x0002`) forbids embedding
    /// unless Preview & Print (`0x0004`) or Editable (`0x0008`) is also set (older fonts set
    /// several; the least restrictive applies); bitmap embedding only (`0x0200`) forbids
    /// outlines; no subsetting (`0x0100`) allows only the whole font. A font without the bits
    /// (or without an `OS/2` table) is installable: anything goes.
    pub fn from_fs_type(fs_type: u16) -> Embedding {
        const RESTRICTED: u16 = 0x0002;
        const PREVIEW_PRINT: u16 = 0x0004;
        const EDITABLE: u16 = 0x0008;
        const NO_SUBSETTING: u16 = 0x0100;
        const BITMAP_ONLY: u16 = 0x0200;
        if (fs_type & RESTRICTED != 0 && fs_type & (PREVIEW_PRINT | EDITABLE) == 0) || fs_type & BITMAP_ONLY != 0 {
            Embedding::Forbidden
        } else if fs_type & NO_SUBSETTING != 0 {
            Embedding::Whole
        } else {
            Embedding::Subset
        }
    }
}

/// One character's glyph in a [`GlyphTable`].
#[derive(Clone, Debug, PartialEq)]
pub struct TableGlyph {
    pub ch: char,
    /// How far the pen moves past it, at [`EMBED_EM`].
    pub advance: f32,
    /// Its outline at [`EMBED_EM`], the pen position on the baseline at the origin (y down).
    pub outline: Path,
}

/// The glyphs of chosen characters in one face, unshaped: one glyph per character, no kerning
/// or ligatures.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphTable {
    /// The family used (the one asked for, or the fallback).
    pub family: String,
    pub embedding: Embedding,
    /// The face's ascent and line height (ascent + descent + line gap) at [`EMBED_EM`].
    pub ascent: f32,
    pub line_height: f32,
    /// The characters the face has, in the order asked for. Empty when `embedding` doesn't
    /// allow a subset.
    pub glyphs: Vec<TableGlyph>,
    /// Characters asked for that the face has no glyph for.
    pub missing: usize,
}

impl Fonts {
    /// The glyphs of `chars` (at most [`MAX_EMBED_CHARS`], duplicates and control characters
    /// skipped) in the face `family`, `bold`, `italic` resolves to, if its licence bits allow a
    /// subset of it to be embedded ([`Embedding::Subset`]); otherwise the table comes back
    /// without glyphs, saying why. `None`: no usable font.
    pub fn glyph_table(&self, family: &str, bold: bool, italic: bool, chars: &str) -> Option<GlyphTable> {
        let family = self.resolve(family)?;
        let q = fontdb::Query {
            families: &[fontdb::Family::Name(&family)],
            weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
            stretch: fontdb::Stretch::Normal,
            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
        };
        let id = self.db.query(&q)?;
        self.db
            .with_face_data(id, |data, index| {
                let font = skrifa::FontRef::from_index(data, index).ok()?;
                let embedding = Embedding::from_fs_type(font.os2().map_or(0, |o| o.fs_type()));
                let size = skrifa::instance::Size::new(EMBED_EM);
                let loc = skrifa::instance::LocationRef::default();
                let m = skrifa::MetadataProvider::metrics(&font, size, loc);
                let mut table = GlyphTable { family: family.clone(), embedding, ascent: m.ascent, line_height: (m.ascent - m.descent + m.leading).max(EMBED_EM * 0.5), glyphs: Vec::new(), missing: 0 };
                if embedding != Embedding::Subset {
                    return Some(table);
                }
                let charmap = font.charmap();
                let outlines = font.outline_glyphs();
                let advances = font.glyph_metrics(size, loc);
                let mut seen = std::collections::BTreeSet::new();
                for ch in chars.chars().filter(|c| !c.is_control()).take(MAX_EMBED_CHARS * 4) {
                    if table.glyphs.len() >= MAX_EMBED_CHARS || !seen.insert(ch) {
                        continue;
                    }
                    let Some(gid) = charmap.map(ch) else {
                        table.missing += 1;
                        continue;
                    };
                    let mut outline = Path::new();
                    if let Some(g) = outlines.get(gid) {
                        let mut pen = Pen { cmds: &mut outline.cmds, x: 0.0, y: 0.0 };
                        let _ = g.draw(skrifa::outline::DrawSettings::unhinted(size, loc), &mut pen);
                    }
                    let advance = advances.advance_width(gid).filter(|a| a.is_finite()).unwrap_or(0.0);
                    table.glyphs.push(TableGlyph { ch, advance, outline });
                }
                Some(table)
            })
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_type_bits_read_as_the_specification_says() {
        use Embedding::*;
        // (fsType, what it allows): installable, each level alone, the modifiers, and old fonts
        // with several levels set (the least restrictive applies).
        for (bits, want) in [
            (0x0000, Subset),
            (0x0002, Forbidden),
            (0x0004, Subset),
            (0x0008, Subset),
            (0x0006, Subset),
            (0x000a, Subset),
            (0x000e, Subset),
            (0x0100, Whole),
            (0x0104, Whole),
            (0x0108, Whole),
            (0x0102, Forbidden),
            (0x0200, Forbidden),
            (0x0204, Forbidden),
            (0x0300, Forbidden),
            (0x0001, Subset),
            (0xfc00, Subset),
            (0xffff, Forbidden),
        ] {
            assert_eq!(Embedding::from_fs_type(bits), want, "{bits:#06x}");
        }
    }

    #[test]
    fn a_glyph_table_holds_the_characters_asked_for_once_each() {
        let fonts = Fonts::get();
        if fonts.families().is_empty() {
            eprintln!("no fonts on this computer: glyph table test skipped");
            return;
        }
        assert!(Fonts::from_faces(Vec::new()).glyph_table("", false, false, "a").is_none());
        let t = fonts.glyph_table("", false, false, "Score: 0123456789 Score\u{7}\n").expect("the default face");
        assert!(!t.family.is_empty());
        if t.embedding != Embedding::Subset {
            assert!(t.glyphs.is_empty(), "a face that can't be subset gives no glyphs");
            return;
        }
        let chars: String = t.glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(chars.chars().count() + t.missing, "Score: 0123456789".chars().count(), "each character once, controls skipped: {chars:?}");
        assert!(t.ascent > 500.0 && t.ascent < 1500.0 && t.line_height >= t.ascent, "{} {}", t.ascent, t.line_height);
        // Advances and outlines are the layout's own, at the table's em.
        let l = fonts.layout_glyphs("S0", &crate::Style { size: EMBED_EM, ..crate::Style::default() }).unwrap();
        for (g, ch) in l.glyphs.iter().zip(['S', '0']) {
            let e = t.glyphs.iter().find(|x| x.ch == ch).expect("in the table");
            assert_eq!(e.outline, g.outline, "{ch}");
        }
        let zero = t.glyphs.iter().find(|x| x.ch == '0').unwrap();
        let nine = t.glyphs.iter().find(|x| x.ch == '9').unwrap();
        assert!(zero.advance > 100.0 && (zero.advance - nine.advance).abs() < zero.advance, "{}", zero.advance);
        let space = t.glyphs.iter().find(|x| x.ch == ' ').unwrap();
        assert!(space.outline.cmds.is_empty() && space.advance > 0.0, "a space has an advance and no outline");
        // The cap holds whatever is asked for.
        let many: String = (0x4e00u32..0x4e00 + 5000).filter_map(char::from_u32).chain("abc".chars()).collect();
        let big = fonts.glyph_table("", false, false, &many).unwrap();
        assert!(big.glyphs.len() <= MAX_EMBED_CHARS);
    }
}
