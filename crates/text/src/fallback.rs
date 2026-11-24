//! Fallback faces (ADR 0130): letters a run's own face has no glyph for are drawn from another
//! face on this computer, cluster by cluster (a letter with its marks), so the rest of the block
//! keeps its face. The faces tried first are [`FALLBACK_FAMILIES`]; after them, any installed
//! face that has the character.
//!
//! The list, the rule that a cluster moves whole and the rule that invisible format characters
//! are never "missing" come from outside PR #40 (SoldierPePe), rebuilt here on shaped clusters.

use crate::Fonts;
use skrifa::MetadataProvider as _;
use std::collections::HashMap;
use std::sync::Arc;

/// Families tried, in order, for characters a run's own face doesn't have: faces with wide
/// coverage that come with Windows, macOS and common Linux desktops, then the system faces for
/// Chinese, Japanese, Korean, Indic and Thai text, then symbols.
pub const FALLBACK_FAMILIES: &[&str] = &[
    "Segoe UI",
    "Arial",
    "Tahoma",
    "Geeza Pro",
    "Arial Unicode MS",
    "DejaVu Sans",
    "Noto Sans",
    "Noto Sans Arabic",
    "Noto Naskh Arabic",
    "Noto Sans Hebrew",
    "Yu Gothic",
    "Meiryo",
    "Hiragino Sans",
    "Noto Sans CJK JP",
    "Microsoft YaHei",
    "Microsoft JhengHei",
    "PingFang SC",
    "Noto Sans CJK SC",
    "Malgun Gothic",
    "Apple SD Gothic Neo",
    "Noto Sans CJK KR",
    "Nirmala UI",
    "Kohinoor Devanagari",
    "Noto Sans Devanagari",
    "Leelawadee UI",
    "Thonburi",
    "Noto Sans Thai",
    "Segoe UI Symbol",
    "Apple Symbols",
    "Noto Sans Symbols",
    "Noto Sans Symbols 2",
    "Droid Sans Fallback",
];

/// Most fallback faces one block draws with; clusters past it stay in their run's face.
pub const MAX_FALLBACK_FACES: usize = 16;

/// Most characters one layout searches every installed face for (after the list): each search
/// opens up to [`crate::MAX_FALLBACK_SEARCH`] faces once, and its answer is remembered.
pub(crate) const MAX_WIDE_SEARCHES: usize = 16;

/// Most answers of the wide search remembered.
const MAX_REMEMBERED: usize = 4096;

/// Whether a character is drawn: not a space, a line break or another control, and not an
/// invisible format character (joiners, direction marks and embeddings, variation selectors,
/// the byte-order mark, the soft hyphen, tags), which fonts often don't map and which draw as
/// nothing.
pub fn needs_glyph(c: char) -> bool {
    let invisible = matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0000}'..='\u{E0FFF}');
    !c.is_control() && !c.is_whitespace() && !invisible
}

/// Which of `candidates` faces draws a cluster whose own face lacks part of it. `cluster` is
/// its drawn characters, `own` how many of them its own face has, and `has(f, c)` whether
/// candidate `f` has `c`. The first candidate with every character wins, so a letter and its
/// marks are shaped together; when none has them all, the one with the most, if that is more
/// than the cluster's own face has (the earliest of equals). `None`: it stays where it is.
pub(crate) fn pick_face(cluster: &[char], own: usize, candidates: usize, has: &mut dyn FnMut(usize, char) -> bool) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for f in 0..candidates {
        let n = cluster.iter().filter(|c| has(f, **c)).count();
        if n == cluster.len() && n > 0 {
            return Some(f);
        }
        if n > own && best.is_none_or(|b| n > b.1) {
            best = Some((f, n));
        }
    }
    best.map(|b| b.0)
}

/// Which characters which faces have, asked once each.
pub(crate) struct Coverage<'f> {
    fonts: &'f Fonts,
    memo: HashMap<(fontdb::ID, char), bool>,
}

impl<'f> Coverage<'f> {
    pub(crate) fn new(fonts: &'f Fonts) -> Self {
        Coverage { fonts, memo: HashMap::new() }
    }

    pub(crate) fn has(&mut self, id: fontdb::ID, c: char) -> bool {
        let fonts = self.fonts;
        *self.memo.entry((id, c)).or_insert_with(|| fonts.covers(id, c))
    }
}

impl Fonts {
    /// The faces of [`FALLBACK_FAMILIES`] on this computer in a weight and slant, in order
    /// (found once for each).
    pub(crate) fn fallback_list(&self, bold: bool, italic: bool) -> Arc<[fontdb::ID]> {
        if let Ok(known) = self.lists.lock()
            && let Some(list) = known.get(&(bold, italic))
        {
            return list.clone();
        }
        let mut out: Vec<fontdb::ID> = Vec::new();
        for name in FALLBACK_FAMILIES {
            let q = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                stretch: fontdb::Stretch::Normal,
                style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
            };
            if let Some(id) = self.db.query(&q)
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        let out: Arc<[fontdb::ID]> = out.into();
        if let Ok(mut known) = self.lists.lock() {
            known.insert((bold, italic), out.clone());
        }
        out
    }

    /// Any installed face with a glyph for `c`: regular faces in family order (so the answer is
    /// the same from run to run), found once and remembered. `None` when no face has it.
    pub(crate) fn any_face_with(&self, c: char) -> Option<fontdb::ID> {
        if let Ok(known) = self.wide.lock()
            && let Some(found) = known.get(&c)
        {
            return *found;
        }
        let regular = |f: &&fontdb::FaceInfo| f.style == fontdb::Style::Normal && (300..=500).contains(&f.weight.0);
        let mut faces: Vec<(&str, fontdb::ID)> = self.db.faces().filter(regular).filter_map(|f| f.families.first().map(|(n, _)| (n.as_str(), f.id))).filter(|(n, _)| !n.starts_with('.')).collect();
        faces.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let found = faces.into_iter().take(crate::MAX_FALLBACK_SEARCH).map(|(_, id)| id).find(|id| self.db.with_face_data(*id, |data, index| skrifa::FontRef::from_index(data, index).is_ok_and(|f| f.charmap().map(c).is_some() && f.outline_glyphs().format().is_some())).unwrap_or(false));
        if let Ok(mut known) = self.wide.lock()
            && known.len() < MAX_REMEMBERED
        {
            known.insert(c, found);
        }
        found
    }

    /// Whether face `id` has a glyph for `c`: read from the font once and remembered, so text
    /// that keeps needing a fallback (every keystroke lays the block out again) doesn't open
    /// the fallback faces each time.
    pub(crate) fn covers(&self, id: fontdb::ID, c: char) -> bool {
        if let Ok(known) = self.covered.lock()
            && let Some(has) = known.get(&(id, c))
        {
            return *has;
        }
        let has = self.db.with_face_data(id, |data, index| crate::face_has(data, index, c)).unwrap_or(false);
        if let Ok(mut known) = self.covered.lock()
            && known.len() < MAX_REMEMBERED * 16
        {
            known.insert((id, c), has);
        }
        has
    }

    /// A face's family name.
    pub(crate) fn family_of(&self, id: fontdb::ID) -> String {
        self.db.face(id).and_then(|f| f.families.first().map(|(n, _)| n.clone())).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Faces as the characters they have.
    fn pick(cluster: &str, own: &str, faces: &[&str]) -> Option<usize> {
        let drawn: Vec<char> = cluster.chars().filter(|c| needs_glyph(*c)).collect();
        let had = drawn.iter().filter(|c| own.contains(**c)).count();
        pick_face(&drawn, had, faces.len(), &mut |f, c| faces.get(f).is_some_and(|s| s.contains(c)))
    }

    /// From outside PR #40: a letter and its marks go together to the first face with all of
    /// them, even when an earlier face has the letter.
    #[test]
    fn a_cluster_moves_whole_to_the_first_face_with_all_of_it() {
        assert_eq!(pick("e\u{0301}\u{0323}", "e\u{0301}", &["e\u{0323}", "e\u{0301}\u{0323}"]), Some(1));
        assert_eq!(pick("\u{05D0}", "", &["abc", "\u{05D0}\u{05D1}", "\u{05D0}"]), Some(1), "the earliest face that has it");
        // No face has all three: the one with the most, when that beats the cluster's own face.
        assert_eq!(pick("q\u{0301}\u{0323}", "q", &["q\u{0323}", "\u{0301}"]), Some(0));
        assert_eq!(pick("q\u{0301}\u{0323}", "q\u{0301}", &["q\u{0323}", "\u{0301}"]), None, "no better than its own face: it stays");
        assert_eq!(pick("\u{10FFFD}", "", &["abc", "def"]), None, "a character no face has");
        assert_eq!(pick("", "", &["abc"]), None);
    }

    #[test]
    fn invisible_format_characters_need_no_glyph() {
        for c in ['\u{200C}', '\u{200D}', '\u{200F}', '\u{202B}', '\u{2067}', '\u{FE0F}', '\u{FEFF}', '\u{00AD}', ' ', '\n', '\t'] {
            assert!(!needs_glyph(c), "{c:?}");
        }
        for c in ['a', '\u{05D0}', '\u{0627}', '\u{0301}', '1', '-'] {
            assert!(needs_glyph(c), "{c:?}");
        }
    }
}
