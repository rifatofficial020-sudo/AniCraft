//! Lines of text that mixes directions (ADR 0130), on shaped clusters and independent of fonts.
//!
//! The order is the one the Unicode bidi algorithm (UAX #9) prescribes: levels are resolved for
//! the paragraph, text is shaped and broken into lines in reading order, and only then is each
//! line put in the order it is displayed ([`place`]). A cluster (a letter with its marks, or a
//! ligature) is never split, by a line break or by reordering.
//!
//! Breaking lines in reading order and never inside a cluster is from outside PR #39
//! (SoldierPePe); the levels and the reordering are `craft_text::bidi`.

use crate::CaretBox;
use craft_text::bidi;
use std::collections::HashMap;
use std::ops::Range;

/// One shaped cluster of a paragraph, in reading order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cluster {
    /// Its glyphs (a range of the paragraph's glyphs, in the order the shaper gave them).
    pub glyphs: Range<usize>,
    /// The characters it covers (of the whole text): from `start` up to `end`.
    pub start: u32,
    pub end: u32,
    /// Its first character, and whether that is white space.
    pub first: char,
    pub space: bool,
    pub advance: f32,
    /// Its embedding level: odd is right to left.
    pub level: u8,
}

/// A paragraph's lines as ranges of its clusters, in reading order: wrapped after the last
/// space before the room is used up (or between the clusters of a word wider than the line),
/// with the spaces at a line's end left out. `room(first)` is the width a line may take
/// (`first`: the paragraph's first line); `None` keeps the paragraph on one line. There is
/// always at least one line.
pub(crate) fn break_lines(clusters: &[Cluster], room: &dyn Fn(bool) -> Option<f32>) -> Vec<Range<usize>> {
    let trimmed = |start: usize, mut end: usize| {
        while end > start && clusters.get(end - 1).is_some_and(|c| c.space) {
            end -= 1;
        }
        start..end
    };
    let mut lines = Vec::new();
    let (mut start, mut x, mut break_at) = (0usize, 0.0f32, None::<usize>);
    for (i, c) in clusters.iter().enumerate() {
        if let Some(w) = room(lines.is_empty())
            && !c.space
            && x + c.advance > w
            && i > start
        {
            let end = break_at.unwrap_or(i);
            lines.push(trimmed(start, end));
            start = end;
            x = clusters.get(start..i).map_or(0.0, |rest| rest.iter().map(|r| r.advance).sum());
            break_at = None;
        }
        x += c.advance;
        if c.space {
            break_at = Some(i + 1);
        }
    }
    lines.push(trimmed(start, clusters.len()));
    lines
}

/// One line's clusters in the order they are displayed, left to right (rules L1 and L2), with
/// where each one stands: `(cluster, box)`, the cluster as an index into `clusters` and the
/// box's x from the line's left end. `line` is the line's range of `clusters` and `para_level`
/// its paragraph's embedding level.
pub(crate) fn place(clusters: &[Cluster], line: Range<usize>, para_level: u8) -> Vec<(usize, CaretBox)> {
    let on_line = clusters.get(line.clone()).unwrap_or(&[]);
    let mixed = para_level > 0 || on_line.iter().any(|c| c.level > 0);
    let (order, levels): (Vec<usize>, Vec<u8>) = if mixed {
        let chars: Vec<char> = on_line.iter().map(|c| c.first).collect();
        let resolved: Vec<u8> = on_line.iter().map(|c| c.level).collect();
        let levels = bidi::line_levels(&chars, &resolved, para_level);
        (bidi::visual_order(&levels), levels)
    } else {
        ((0..on_line.len()).collect(), vec![0; on_line.len()])
    };
    let mut x = 0.0f32;
    let mut out = Vec::with_capacity(order.len());
    for k in order {
        let Some(c) = on_line.get(k) else { continue };
        let rtl = levels.get(k).is_some_and(|l| l % 2 == 1);
        out.push((line.start + k, CaretBox { start: c.start, end: c.end, x0: x, x1: x + c.advance, rtl }));
        x += c.advance;
    }
    out
}

/// Where the caret stands on a line (see [`crate::CaretLine::stops`]): each cluster's first
/// character at the edge the cluster is read from (its left when it reads left to right, its
/// right otherwise), and the boundary after the line's last cluster at that cluster's other
/// edge. `boxes` are the line's, x from the box's left edge; `empty` is where the caret stands
/// on a line without clusters.
pub(crate) fn stops(boxes: &[CaretBox], empty: (u32, f32)) -> Vec<(u32, f32)> {
    let mut stops: Vec<(u32, f32)> = boxes.iter().map(|b| (b.start, if b.rtl { b.x1 } else { b.x0 })).collect();
    stops.sort_by_key(|s| s.0);
    stops.dedup_by_key(|s| s.0);
    match boxes.iter().max_by_key(|b| b.start) {
        Some(last) => stops.push((last.end.max(last.start + 1), if last.rtl { last.x0 } else { last.x1 })),
        None => stops.push(empty),
    }
    stops
}

/// The script each character is shaped in: its own, or for characters every script shares
/// (spaces, digits, punctuation, marks) the script before them (after them at the start).
/// `None` throughout when no character has a script of its own.
pub(crate) struct Scripts {
    known: HashMap<char, Option<harfrust::Script>>,
}

impl Scripts {
    pub(crate) fn new() -> Self {
        Scripts { known: HashMap::new() }
    }

    /// A character's own script (`None`: common to all, inherited or unknown).
    fn own(&mut self, c: char) -> Option<harfrust::Script> {
        if c.is_ascii() {
            return c.is_ascii_alphabetic().then_some(harfrust::script::LATIN);
        }
        *self.known.entry(c).or_insert_with(|| {
            // harfrust keeps its script table to itself: a buffer of one character says it.
            let mut one = harfrust::UnicodeBuffer::new();
            one.add(c, 0);
            one.guess_segment_properties();
            let s = one.script();
            (s != harfrust::script::UNKNOWN && s != harfrust::script::COMMON && s != harfrust::script::INHERITED).then_some(s)
        })
    }

    /// The scripts of one paragraph's characters.
    pub(crate) fn of(&mut self, chars: &[char]) -> Vec<Option<harfrust::Script>> {
        let mut out: Vec<Option<harfrust::Script>> = chars.iter().map(|c| self.own(*c)).collect();
        let mut last = out.iter().flatten().next().copied();
        for s in &mut out {
            match s {
                Some(own) => last = Some(*own),
                None => *s = last,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clusters of one character each, 10 wide: capitals are right to left (level 1, or the
    /// level given after them in `levels`), a space is a space.
    fn clusters(text: &str, levels: &[u8]) -> Vec<Cluster> {
        text.chars().enumerate().map(|(i, c)| Cluster { glyphs: i..i + 1, start: i as u32, end: i as u32 + 1, first: c, space: c == ' ', advance: 10.0, level: levels.get(i).copied().unwrap_or(0) }).collect()
    }

    fn shown(cl: &[Cluster], line: Range<usize>, para: u8) -> String {
        place(cl, line, para).iter().map(|(k, _)| cl[*k].first).collect()
    }

    /// From outside PR #39: lines are found in reading order, after the last space that fits
    /// or between the clusters of a word wider than the line.
    #[test]
    fn lines_break_in_reading_order() {
        let wide = |w: f32| move |_: bool| Some(w);
        assert_eq!(break_lines(&clusters("ab cd ef", &[]), &wide(50.0)), vec![0..5, 6..8]);
        assert_eq!(break_lines(&clusters("abcdefgh", &[]), &wide(30.0)), vec![0..3, 3..6, 6..8]);
        assert_eq!(break_lines(&clusters("ab cd", &[]), &|_| None), vec![0..5]);
        assert_eq!(break_lines(&[], &wide(10.0)), vec![0..0], "an empty paragraph is one empty line");
        assert_eq!(break_lines(&clusters("ab   ", &[]), &wide(100.0)), vec![0..2], "spaces at the end are left out");
        // A cluster wider than the line stays whole, on a line of its own.
        let mut big = clusters("abc", &[]);
        big[1].advance = 500.0;
        assert_eq!(break_lines(&big, &wide(15.0)), vec![0..1, 1..2, 2..3]);
        // The first line's room can differ (an indent).
        let lines = break_lines(&clusters("aa bb cc dd", &[]), &|first| Some(if first { 25.0 } else { 55.0 }));
        assert_eq!(lines, vec![0..2, 3..8, 9..11]);
    }

    #[test]
    fn a_line_is_displayed_in_visual_order() {
        // Left to right throughout: as written.
        let ltr = clusters("abc def", &[]);
        assert_eq!(shown(&ltr, 0..7, 0), "abc def");
        // A right-to-left word inside left-to-right text is reversed in place.
        let mixed = clusters("ab CDE fg", &[0, 0, 0, 1, 1, 1, 0, 0, 0]);
        assert_eq!(shown(&mixed, 0..9, 0), "ab EDC fg");
        // A number (level 2) inside a right-to-left paragraph keeps its digits in order, and the
        // paragraph runs from the right: its first word is the rightmost.
        let rtl = clusters("ABC 123 DEF", &[1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1]);
        assert_eq!(shown(&rtl, 0..11, 1), "FED 123 CBA");
        // Each line is reordered on its own: the first line holds the first words.
        assert_eq!(shown(&rtl, 0..7, 1), "123 CBA");
        assert_eq!(shown(&rtl, 8..11, 1), "FED");
        // Boxes run left to right without gaps, each as wide as its cluster.
        let boxes: Vec<CaretBox> = place(&mixed, 0..9, 0).into_iter().map(|p| p.1).collect();
        assert!(boxes.windows(2).all(|w| w[0].x1 == w[1].x0) && boxes[0].x0 == 0.0 && boxes[8].x1 == 90.0);
        assert_eq!(boxes.iter().map(|b| b.rtl).collect::<Vec<_>>(), [false, false, false, true, true, true, false, false, false]);
        assert_eq!(boxes[3].start, 5, "the word's last letter is its leftmost");
        assert_eq!(place(&ltr, 3..3, 0), Vec::new());
        assert_eq!(place(&ltr, 5..99, 0), Vec::new(), "a range outside the paragraph places nothing");
    }

    #[test]
    fn the_caret_stands_where_each_cluster_is_read_from() {
        // "ab" then "CDE" right to left: a b E D C. Before C (boundary 2) is the line's right
        // end; after E (boundary 5, the line's end) is E's left edge, where "ab" ends.
        let cl = clusters("abCDE", &[0, 0, 1, 1, 1]);
        let boxes: Vec<CaretBox> = place(&cl, 0..5, 0).into_iter().map(|p| p.1).collect();
        assert_eq!(stops(&boxes, (0, 0.0)), vec![(0, 0.0), (1, 10.0), (2, 50.0), (3, 40.0), (4, 30.0), (5, 20.0)]);
        // Left to right, the stops are the clusters' left edges and the line's end.
        let ltr: Vec<CaretBox> = place(&clusters("abc", &[]), 0..3, 0).into_iter().map(|p| p.1).collect();
        assert_eq!(stops(&ltr, (0, 0.0)), vec![(0, 0.0), (1, 10.0), (2, 20.0), (3, 30.0)]);
        assert_eq!(stops(&[], (7, 4.0)), vec![(7, 4.0)]);
        // A cluster of several characters has one stop, and the line's end is after all of it.
        let lig = [CaretBox { start: 0, end: 3, x0: 0.0, x1: 12.0, rtl: false }];
        assert_eq!(stops(&lig, (0, 0.0)), vec![(0, 0.0), (3, 12.0)]);
    }

    #[test]
    fn shared_characters_take_the_script_beside_them() {
        let mut s = Scripts::new();
        let of = |s: &mut Scripts, t: &str| s.of(&t.chars().collect::<Vec<_>>());
        assert!(of(&mut s, "12 - 3").iter().all(Option::is_none));
        let latin = Some(harfrust::script::LATIN);
        assert_eq!(of(&mut s, "(a 1)"), vec![latin; 5], "before the first letter: the script after");
        let mixed = of(&mut s, "ab \u{05D0}\u{05D1} 12");
        assert_eq!(&mixed[..3], &[latin; 3]);
        assert!(mixed[3].is_some() && mixed[3] != latin, "Hebrew letters have their own script");
        assert_eq!(&mixed[4..], &[mixed[3]; 4], "the space and digits after them are shaped with them");
        assert_eq!(of(&mut s, ""), Vec::new());
    }
}
