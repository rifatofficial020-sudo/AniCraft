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
