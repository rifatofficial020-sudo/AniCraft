use super::*;
use crate::Style;

fn fonts() -> Option<&'static Fonts> {
    let f = Fonts::get();
    if f.families().is_empty() {
        eprintln!("no fonts on this computer: rich text layout tests skipped");
        return None;
    }
    Some(f)
}

fn run(chars: usize, size: f32, paint: u32) -> Run {
    Run { chars, style: RunStyle { size, paint, ..RunStyle::default() } }
}

fn lay(f: &Fonts, text: &str, runs: &[Run], paras: &[ParaStyle], width: Option<f32>) -> Layout {
    f.layout_rich(&RichText { text, runs, paragraphs: paras, width }, true).unwrap()
}

#[test]
fn one_run_is_the_plain_layout() {
    let Some(f) = fonts() else { return };
    let plain = f.layout_glyphs("Hello there\nworld", &Style { size: 24.0, letter_spacing: 1.0, line_spacing: 3.0, width: Some(90.0), ..Style::default() }).unwrap();
    let runs = [Run { chars: 17, style: RunStyle { size: 24.0, letter_spacing: 1.0, ..RunStyle::default() } }];
    let rich = lay(f, "Hello there\nworld", &runs, &[ParaStyle { line_spacing: 3.0, ..ParaStyle::default() }], Some(90.0));
    assert_eq!(rich, plain);
    assert_eq!(rich.pieces.len(), 1);
    assert_eq!(rich.pieces[0].1, rich.outline);
}

#[test]
fn paints_cut_the_outline_into_pieces() {
    let Some(f) = fonts() else { return };
    let l = lay(f, "abcdef", &[run(2, 20.0, 7), run(2, 20.0, 3), run(2, 20.0, 7)], &[], None);
    assert_eq!(l.pieces.iter().map(|p| p.0).collect::<Vec<_>>(), vec![7, 3], "in the order paints first appear");
    let joined: Vec<PathCmd> = l.pieces.iter().flat_map(|p| p.1.cmds.clone()).collect();
    assert_eq!(joined, l.outline.cmds, "the outline is the pieces one after another");
    assert_eq!(l.glyphs.iter().map(|g| g.paint).collect::<Vec<_>>(), vec![7, 7, 3, 3, 7, 7]);
    // The same glyphs as one run: paints don't move anything.
    let one = lay(f, "abcdef", &[run(6, 20.0, 0)], &[], None);
    assert_eq!((one.width, one.height), (l.width, l.height));
    assert_eq!(one.glyphs.iter().map(|g| g.x).collect::<Vec<_>>(), l.glyphs.iter().map(|g| g.x).collect::<Vec<_>>());
}

/// What the default face says a superscript is raised by at `size` (pixels).
fn expected_superscript_raise(f: &Fonts, size: f32) -> f32 {
    let family = f.resolve("").unwrap();
    let q = fontdb::Query { families: &[fontdb::Family::Name(&family)], ..fontdb::Query::default() };
    let id = f.db.query(&q).unwrap();
    f.db.with_face_data(id, |d, i| {
        let font = skrifa::FontRef::from_index(d, i).unwrap();
        let em = f32::from(skrifa::MetadataProvider::metrics(&font, skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default()).units_per_em);
        let o = font.os2().map(|o| f32::from(o.y_superscript_y_offset()) / em).unwrap_or(0.0);
        if (0.05..=1.0).contains(&o) { o * size } else { 0.35 * size }
    })
    .unwrap()
}

#[test]
fn a_superscript_is_smaller_and_raised_by_the_fonts_offset() {
    let Some(f) = fonts() else { return };
    let sup = RunStyle { size: 40.0, shift: Shift::Superscript, ..RunStyle::default() };
    let sub = RunStyle { shift: Shift::Subscript, ..sup.clone() };
    let l = lay(f, "x2y2", &[run(1, 40.0, 0), Run { chars: 1, style: sup }, run(1, 40.0, 0), Run { chars: 1, style: sub }], &[], None);
    let plain = lay(f, "x2y2", &[run(4, 40.0, 0)], &[], None);
    let b = |l: &Layout, i: usize| l.glyphs[i].outline.bounds();
    // Every glyph shares the line's baseline; the shift is in the glyph's own outline.
    assert!(l.glyphs.iter().all(|g| g.baseline == l.ascent));
    let (up, down, normal) = (b(&l, 1), b(&l, 3), b(&plain, 1));
    assert!(up.height() < normal.height() * 0.9 && up.height() > normal.height() * 0.3, "smaller: {up:?} vs {normal:?}");
    // Raised by the font's offset: the digit's bottom (on its own baseline, within the
    // outline's overshoot) is that far above the line's.
    let raise = -up.max.y;
    let want = expected_superscript_raise(f, 40.0);
    assert!((raise - want).abs() < 1.0, "raised {raise}, the font says {want}");
    assert!(down.max.y > 0.5, "a subscript drops below the baseline: {down:?}");
    // Its advance is smaller too.
    assert!(l.glyphs[1].advance < plain.glyphs[1].advance * 0.9);
}

#[test]
fn lines_wrap_across_runs_and_make_room_for_the_tallest() {
    let Some(f) = fonts() else { return };
    // "aaa " small, "BBB ccc" big: the wrap falls inside the big run.
    let text = "aaa BBB ccc";
    let small = lay(f, "aaa ", &[run(4, 20.0, 0)], &[], None);
    let big_word = lay(f, "BBB", &[run(3, 40.0, 0)], &[], None);
    let space = lay(f, "a a", &[run(3, 20.0, 0)], &[], None);
    let w = small.width + (space.width - 2.0 * lay(f, "a", &[run(1, 20.0, 0)], &[], None).width) + big_word.width + 2.0;
    let l = lay(f, text, &[run(4, 20.0, 0), run(7, 40.0, 1)], &[], Some(w));
    assert_eq!(l.lines, 2, "{w}");
    assert_eq!((l.carets[0].start, l.carets[0].end, l.carets[1].start, l.carets[1].end), (0, 8, 8, 11));
    // The first line's baseline is the big run's ascent (the tallest on it), not the small one's.
    let big = lay(f, "B", &[run(1, 40.0, 0)], &[], None);
    assert!((l.ascent - big.ascent).abs() < 1e-3, "{} vs {}", l.ascent, big.ascent);
    assert!(l.carets[1].baseline > l.carets[0].baseline + big.line_height * 0.9);
    // A line of only small text after a big one keeps its own height.
    let two = lay(f, "BIG\nsmall\nsmall", &[run(4, 60.0, 0), run(11, 15.0, 0)], &[], None);
    let (a, b, c) = (&two.carets[0], &two.carets[1], &two.carets[2]);
    assert!((b.top - a.height).abs() < 1e-3 && (c.top - b.top - b.height).abs() < 1e-3);
    assert!(a.height > b.height * 3.0 && (b.height - c.height).abs() < 1e-3, "{} {} {}", a.height, b.height, c.height);
    let s = lay(f, "s", &[run(1, 15.0, 0)], &[], None);
    assert!((two.height - (c.baseline + s.height - s.ascent)).abs() < 0.01);
}

#[test]
fn paragraphs_align_indent_and_keep_their_margins() {
    let Some(f) = fonts() else { return };
    let p = |align, indent, left, right| ParaStyle { align, indent, left_margin: left, right_margin: right, line_spacing: 0.0 };
    let l = lay(f, "a\nb\nc", &[run(5, 20.0, 0)], &[p(Align::Left, 30.0, 10.0, 0.0), p(Align::Right, 0.0, 0.0, 25.0), p(Align::Center, 0.0, 40.0, 0.0)], Some(200.0));
    let x = |i: usize| l.glyphs[i].x;
    assert!((x(0) - 40.0).abs() < 1e-3, "indent and left margin: {}", x(0));
    let bw = l.glyphs[1].advance;
    assert!((x(1) - (200.0 - 25.0 - bw)).abs() < 1e-3, "right margin: {}", x(1));
    let cw = l.glyphs[2].advance;
    assert!((x(2) - (40.0 + (160.0 - cw) * 0.5)).abs() < 1e-3, "centred in what the margins leave: {}", x(2));
    // An indent only moves a paragraph's first line, and the margins narrow its wrap.
    let words = "one two three four five six seven eight";
    let wrapped = lay(f, words, &[run(words.chars().count(), 20.0, 0)], &[p(Align::Left, 20.0, 15.0, 15.0)], Some(160.0));
    assert!(wrapped.lines >= 3);
    let starts: Vec<f32> = wrapped.carets.iter().map(|c| c.stops[0].1).collect();
    assert!((starts[0] - 35.0).abs() < 1e-3 && starts[1..].iter().all(|s| (s - 15.0).abs() < 1e-3), "{starts:?}");
    assert!(wrapped.outline.bounds().max.x <= 160.0 - 15.0 + 1.0, "{:?}", wrapped.outline.bounds());
    // Without a width, the box is the widest line with its margins.
    let free = lay(f, "ab", &[run(2, 20.0, 0)], &[p(Align::Left, 5.0, 10.0, 20.0)], None);
    let line = free.glyphs[1].x + free.glyphs[1].advance - free.glyphs[0].x;
    assert!((free.width - (35.0 + line)).abs() < 1e-3, "{}", free.width);
    // Per-paragraph line spacing.
    let spaced = lay(f, "a\nb\nc", &[run(5, 20.0, 0)], &[ParaStyle { line_spacing: 30.0, ..ParaStyle::default() }, ParaStyle::default()], None);
    let tight = lay(f, "a\nb\nc", &[run(5, 20.0, 0)], &[], None);
    assert!((spaced.height - tight.height - 30.0).abs() < 0.01, "only the first line is spaced: {} vs {}", spaced.height, tight.height);
}

#[test]
fn underlines_run_under_their_glyphs_in_their_paint() {
    let Some(f) = fonts() else { return };
    let u = RunStyle { size: 30.0, underline: true, paint: 1, ..RunStyle::default() };
    let l = lay(f, "ab cd", &[run(3, 30.0, 0), Run { chars: 2, style: u }], &[], None);
    let plain = lay(f, "ab cd", &[run(3, 30.0, 0), run(2, 30.0, 1)], &[], None);
    let (with, without) = (&l.pieces[1].1, &plain.pieces[1].1);
    assert_eq!(with.cmds.len(), without.cmds.len() + 5, "one bar: a rectangle");
    let bar = Path { cmds: with.cmds[without.cmds.len()..].to_vec() }.bounds();
    assert!(bar.min.y > l.ascent - 1.0 && bar.max.y < l.height + 1.0, "under the baseline: {bar:?}");
    assert!((bar.min.x - l.glyphs[3].x).abs() < 1e-3 && (bar.max.x - l.width).abs() < 1e-3, "{bar:?}");
}

#[test]
fn hostile_runs_are_capped_and_sanitized() {
    let Some(f) = fonts() else { return };
    // Thousands of runs: capped, the rest joins the last.
    let text = "x".repeat(MAX_RUNS + 500);
    let runs: Vec<Run> = (0..MAX_RUNS + 500).map(|i| run(1, 10.0 + (i % 7) as f32, i as u32 % 3)).collect();
    let t = std::time::Instant::now();
    let l = f.layout_rich(&RichText { text: &text, runs: &runs, paragraphs: &[], width: Some(400.0) }, false).unwrap();
    eprintln!("{} runs laid out in {:?}", MAX_RUNS, t.elapsed());
    assert_eq!(l.families.len(), MAX_RUNS);
    assert!(l.width.is_finite() && l.height.is_finite());
    // As many paragraphs as runs, empty ones included: linear, not paragraphs × runs.
    let text = "y

".repeat(MAX_RUNS / 2);
    let t = std::time::Instant::now();
    let l = f.layout_rich(&RichText { text: &text, runs: &runs, paragraphs: &[], width: None }, false).unwrap();
    eprintln!("{} paragraphs laid out in {:?}", MAX_RUNS, t.elapsed());
    assert_eq!(l.lines, MAX_RUNS + 1);
    // Hostile sizes, spacing, margins and lengths: clamped, never a panic.
    let bad = [
        Run { chars: usize::MAX, style: RunStyle { size: f32::NAN, letter_spacing: f32::INFINITY, ..RunStyle::default() } },
        Run { chars: 0, style: RunStyle { size: 1e30, ..RunStyle::default() } },
    ];
    let p = [ParaStyle { indent: f32::NEG_INFINITY, left_margin: -5.0, right_margin: 1e20, line_spacing: f32::NAN, align: Align::Center }];
    let l = f.layout_rich(&RichText { text: "a b\nc", runs: &bad, paragraphs: &p, width: Some(f32::NAN) }, true).unwrap();
    assert!(l.width.is_finite() && l.height.is_finite());
    // Runs longer than the text, shorter than it, or none.
    for runs in [vec![run(100, 20.0, 0)], vec![run(1, 20.0, 0)], vec![]] {
        let l = f.layout_rich(&RichText { text: "abc", runs: &runs, paragraphs: &[], width: None }, true).unwrap();
        assert_eq!(l.glyphs.len(), 3);
    }
    assert_eq!(f.layout_rich(&RichText { text: &"x".repeat(MAX_CHARS + 1), runs: &[], paragraphs: &[], width: None }, false), Err(TextError::TooLong));
}

// Mixed directions and fallback faces (ADR 0130), on fonts made in memory (`testfont`): every
// letter is a box half an em wide, so at size 20 a letter is 10 pixels.

use crate::testfont::{self, LETTER_HEIGHT, NOTDEF_HEIGHT};

const LATIN: &str = "abcdefghijklmnopqrstuvwxyz0123456789 .-";
/// Alef, bet, gimel, dalet, he.
const HEBREW: &str = "\u{05D0}\u{05D1}\u{05D2}\u{05D3}\u{05D4} ";

fn both() -> Fonts {
    Fonts::from_faces([testfont::font("Test Both", &format!("{LATIN}{HEBREW}"), 500)])
}

fn set(f: &Fonts, family: &str, text: &str, width: Option<f32>) -> Layout {
    let runs = [Run { chars: usize::MAX, style: RunStyle { family: family.into(), size: 20.0, ..RunStyle::default() } }];
    f.layout_rich(&RichText { text, runs: &runs, paragraphs: &[], width }, true).unwrap()
}

/// The characters of the glyphs on line `row`, left to right.
fn shown(l: &Layout, row: usize) -> Vec<u32> {
    let baseline = l.carets[row].baseline;
    let mut on: Vec<&PlacedGlyph> = l.glyphs.iter().filter(|g| g.baseline == baseline).collect();
    on.sort_by(|a, b| a.x.total_cmp(&b.x));
    on.iter().map(|g| g.index).collect()
}

/// How many of a layout's glyphs are "notdef" boxes.
fn boxes(l: &Layout) -> usize {
    let notdef = 20.0 * f32::from(NOTDEF_HEIGHT) / f32::from(testfont::EM);
    let letter = 20.0 * f32::from(LETTER_HEIGHT) / f32::from(testfont::EM);
    let tall = |g: &&PlacedGlyph| g.outline.bounds().height();
    assert!(l.glyphs.iter().filter(|g| !g.outline.cmds.is_empty()).all(|g| (tall(&g) - notdef).abs() < 0.01 || (tall(&g) - letter).abs() < 0.01));
    l.glyphs.iter().filter(|g| !g.outline.cmds.is_empty() && (tall(g) - notdef).abs() < 0.01).count()
}

#[test]
fn a_right_to_left_word_in_left_to_right_text_reads_from_its_right() {
    let f = both();
    // a b _ alef bet gimel _ c d: the Hebrew word is displayed gimel, bet, alef.
    let l = set(&f, "Test Both", "ab \u{05D0}\u{05D1}\u{05D2} cd", None);
    assert_eq!(shown(&l, 0), vec![0, 1, 2, 5, 4, 3, 6, 7, 8]);
    assert_eq!((l.width, l.missing, boxes(&l)), (90.0, 0, 0));
    let c = &l.carets[0];
    assert!(!c.rtl);
    // The caret before alef is at the word's right end, after gimel at its left end.
    assert_eq!((c.x_at(3), c.x_at(4), c.x_at(5), c.x_at(6)), (60.0, 50.0, 40.0, 60.0));
    assert_eq!(c.boxes.len(), 9);
    assert!(c.boxes.windows(2).all(|w| w[0].x1 == w[1].x0), "boxes run left to right without gaps");
    // Text that reads one way is laid out as it always was.
    let plain = set(&f, "Test Both", "ab cd", None);
    assert_eq!(shown(&plain, 0), vec![0, 1, 2, 3, 4]);
    assert_eq!(plain.carets[0].stops, vec![(0, 0.0), (1, 10.0), (2, 20.0), (3, 30.0), (4, 40.0), (5, 50.0)]);
}

/// From outside PR #39: a number inside right-to-left text keeps its digits in order, and a
/// wrapped right-to-left paragraph starts with its first words.
#[test]
fn a_right_to_left_paragraph_runs_from_the_right_and_wraps_in_reading_order() {
    let f = both();
    // alef bet _ 1 2 _ gimel dalet, displayed: dalet gimel _ 1 2 _ bet alef.
    let l = set(&f, "Test Both", "\u{05D0}\u{05D1} 12 \u{05D2}\u{05D3}", None);
    assert_eq!(shown(&l, 0), vec![7, 6, 5, 3, 4, 2, 1, 0]);
    let c = &l.carets[0];
    assert!(c.rtl);
    assert_eq!((c.x_at(0), c.x_at(8)), (l.width, 0.0), "the paragraph starts at its right end and ends at its left");
    // Wrapped: the first line holds the first word, each line displayed right to left.
    let w = set(&f, "Test Both", "\u{05D0}\u{05D1} \u{05D2}\u{05D3}", Some(25.0));
    assert_eq!(w.lines, 2);
    assert_eq!((shown(&w, 0), shown(&w, 1)), (vec![1, 0], vec![4, 3]));
    assert_eq!((w.carets[0].start, w.carets[0].end, w.carets[1].start, w.carets[1].end), (0, 3, 3, 5));
    // Each paragraph has its own direction.
    let two = set(&f, "Test Both", "ab\n\u{05D0}\u{05D1}", None);
    assert_eq!((two.carets[0].rtl, two.carets[1].rtl), (false, true));
    assert_eq!(shown(&two, 1), vec![4, 3]);
}

#[test]
fn styled_runs_keep_their_style_through_reordering() {
    let f = both();
    // The first run ends inside the Hebrew word: alef is its last letter.
    let style = |paint| RunStyle { family: "Test Both".into(), size: 20.0, underline: paint == 2, paint, ..RunStyle::default() };
    let runs = [Run { chars: 3, style: style(1) }, Run { chars: 2, style: style(2) }];
    let l = f.layout_rich(&RichText { text: "ab\u{05D0}\u{05D1}\u{05D2}", runs: &runs, paragraphs: &[], width: None }, true).unwrap();
    assert_eq!(shown(&l, 0), vec![0, 1, 4, 3, 2]);
    let paint_at = |i: u32| l.glyphs.iter().find(|g| g.index == i).unwrap().paint;
    assert_eq!([0, 1, 2, 3, 4].map(paint_at), [1, 1, 1, 2, 2]);
    // The underline is under the second run's two letters, which are displayed first.
    let piece = &l.pieces.iter().find(|p| p.0 == 2).unwrap().1;
    let b = piece.bounds();
    assert!(b.min.x >= 20.0 && b.max.x <= 40.0, "{b:?}");
}

/// From outside PR #40: letters the chosen face lacks come from a fallback face, the block
/// keeps its family, and a character no face has is counted.
#[test]
fn letters_the_face_lacks_come_from_a_fallback_face() {
    // A listed fallback family wins over another face that has the letters.
    let f = Fonts::from_faces([testfont::font("Test Latin", LATIN, 500), testfont::font("Noto Sans Hebrew", HEBREW, 800), testfont::font("Aaa Hebrew", HEBREW, 600)]);
    let l = set(&f, "Test Latin", "ab \u{05D0}\u{05D1}", None);
    assert_eq!((l.missing, boxes(&l)), (0, 0));
    assert_eq!((l.family.as_str(), l.families.clone(), l.fallbacks.clone()), ("Test Latin", vec!["Test Latin".to_string()], vec!["Noto Sans Hebrew".to_string()]));
    assert_eq!(l.width, 30.0 + 32.0, "the Hebrew letters are as wide as the fallback face makes them");
    assert_eq!(shown(&l, 0), vec![0, 1, 2, 4, 3]);
    // The line is the chosen face's line, whatever the fallback's metrics are.
    assert_eq!(l.line_height, set(&f, "Test Latin", "ab", None).line_height);
    // No listed family: any installed face with the letter.
    let wide = Fonts::from_faces([testfont::font("Test Latin", LATIN, 500), testfont::font("Zzz Hebrew", HEBREW, 600)]);
    let l = set(&wide, "Test Latin", "ab \u{05D0}\u{05D1}", None);
    assert_eq!((l.missing, l.fallbacks.clone(), l.width), (0, vec!["Zzz Hebrew".to_string()], 30.0 + 24.0));
    // A character no face has is counted and drawn as the chosen face's box; invisible format
    // characters are neither.
    let none = set(&f, "Test Latin", "a\u{05E9}", None);
    assert_eq!((none.missing, boxes(&none), none.fallbacks.len()), (1, 1, 0));
    let hidden = set(&f, "Test Latin", "a\u{200D}b\u{200F}", None);
    assert_eq!((hidden.missing, boxes(&hidden)), (0, 0));
    // Text the face has all of asks no other face.
    assert_eq!(set(&f, "Test Latin", "abc 123", None).fallbacks, Vec::<String>::new());
}

/// From outside PR #40: a mark its letter's face lacks takes the letter along to a face with
/// both, so the two are shaped together.
#[test]
fn a_letter_goes_with_its_mark_to_the_fallback_face() {
    let f = Fonts::from_faces([testfont::font("Test Latin", LATIN, 500), testfont::font("Noto Sans", "e\u{0301}", 800)]);
    let l = set(&f, "Test Latin", "e\u{0301}x", None);
    assert_eq!((l.missing, boxes(&l), l.fallbacks.clone()), (0, 0, vec!["Noto Sans".to_string()]));
    let cluster: Vec<&PlacedGlyph> = l.glyphs.iter().filter(|g| g.index == 0).collect();
    assert_eq!(cluster.len(), 2, "the letter and its mark are one cluster");
    assert!(cluster.iter().any(|g| g.advance == 16.0), "the letter is the fallback face's");
    assert_eq!(l.glyphs.iter().find(|g| g.index == 2).unwrap().advance, 10.0, "the next letter stays in its face");
    // The caret doesn't stop between a letter and its mark.
    assert_eq!(l.carets[0].stops.iter().map(|s| s.0).collect::<Vec<_>>(), vec![0, 2, 3]);
}

#[test]
fn hostile_directions_lay_out_without_panicking() {
    let f = both();
    let nested = format!("{}ab\u{05D0}{}", "\u{2067}".repeat(300), "\u{2069}".repeat(300));
    let texts = ["\u{202E}abc\u{202C}", "\u{05D0}\n\n\u{05D1}\r\n", "a\u{2029}\u{05D0}", "\u{0301}\u{0301}", "\u{200F}", "\u{05D0} \u{05D1}  ", " \u{05D0}", "1\u{05D0}2\u{05D1}3", nested.as_str(), "\u{202B}ab\u{05D0}\n\u{202C}\u{05D1}cd"];
    for text in texts {
        for width in [None, Some(1.0), Some(35.0)] {
            let l = set(&f, "Test Both", text, width);
            assert!(l.width.is_finite() && l.height.is_finite());
            assert!(l.glyphs.iter().all(|g| g.x.is_finite() && g.advance.is_finite()), "{text:?}");
            assert_eq!(l.carets.len(), l.lines);
            for c in &l.carets {
                assert!(c.start <= c.end && !c.stops.is_empty() && c.stops.iter().all(|s| s.1.is_finite() && (c.start..=c.end).contains(&s.0)), "{text:?} {c:?}");
                assert!(c.boxes.iter().all(|b| b.start < b.end && b.x0 <= b.x1), "{text:?} {c:?}");
            }
            // Every drawn character is on exactly one line.
            let mut seen: Vec<u32> = l.carets.iter().flat_map(|c| c.boxes.iter().map(|b| b.start)).collect();
            let n = seen.len();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), n, "{text:?}");
        }
    }
}

/// With this computer's fonts: Arabic letters join, a number inside them keeps its digits in
/// order at the line's left, and a face without Arabic letters borrows them (outside PRs #39
/// and #40). Skipped where no installed face has Arabic letters.
#[test]
fn arabic_with_a_number_reads_the_right_way_round_in_any_face() {
    let Some(f) = fonts() else { return };
    let l = f.layout_glyphs("\u{0645}\u{0631}\u{062D}\u{0628}\u{0627} 2026", &Style { size: 40.0, ..Style::default() }).unwrap();
    if l.missing > 0 {
        eprintln!("no face here has Arabic letters: skipped");
        return;
    }
    let x_of = |i: u32| l.glyphs.iter().filter(|g| g.index == i).map(|g| g.x).fold(f32::INFINITY, f32::min);
    assert!(x_of(6) < x_of(7) && x_of(7) < x_of(8) && x_of(8) < x_of(9), "the digits read left to right");
    assert!(x_of(9) < x_of(4) && x_of(4) < x_of(0), "the number is left of the word, whose first letter is rightmost");
    assert!(l.carets[0].rtl);
    // Every family draws it: one without Arabic letters says which family lent them.
    for family in f.families().iter().take(40) {
        let Ok(in_family) = f.layout("\u{0628}\u{064A}\u{062A} ab", &Style { family: family.clone(), size: 40.0, ..Style::default() }) else { continue };
        assert_eq!(in_family.family, *family);
        assert_eq!(in_family.missing, 0, "{family}: the letters it lacks come from a fallback face");
    }
}
