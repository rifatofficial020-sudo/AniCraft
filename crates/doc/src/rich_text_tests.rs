use super::*;
use crate::{Rgba, TextAlign, TextBlock};

fn st(size: f32) -> CharStyle {
    CharStyle { size, ..CharStyle::default() }
}
fn run(chars: u32, size: f32) -> TextRun {
    TextRun { chars, style: st(size) }
}
fn sizes(runs: &[TextRun]) -> Vec<(u32, f32)> {
    runs.iter().map(|r| (r.chars, r.style.size)).collect()
}

#[test]
fn typing_continues_the_style_before_the_caret() {
    let runs = vec![run(3, 10.0), run(3, 20.0)];
    // Inside a run, at its end, and at the very start (the first character's style).
    assert_eq!(sizes(&type_into_runs(&runs, 1, 1, 2)), vec![(5, 10.0), (3, 20.0)]);
    assert_eq!(sizes(&type_into_runs(&runs, 3, 3, 1)), vec![(4, 10.0), (3, 20.0)], "after the last small character: small");
    assert_eq!(sizes(&type_into_runs(&runs, 0, 0, 1)), vec![(4, 10.0), (3, 20.0)]);
    assert_eq!(sizes(&type_into_runs(&runs, 6, 6, 1)), vec![(3, 10.0), (4, 20.0)]);
    // Typing over a selection that crosses runs takes the style before it.
    assert_eq!(sizes(&type_into_runs(&runs, 2, 5, 1)), vec![(3, 10.0), (1, 20.0)]);
    // Deleting a whole run joins its neighbours when they match.
    let three = vec![run(2, 10.0), run(2, 20.0), run(2, 10.0)];
    assert_eq!(sizes(&replace_runs(&three, 2, 4, Vec::new())), vec![(4, 10.0)]);
    // Deleting everything leaves one empty run in the first style.
    assert_eq!(sizes(&replace_runs(&three, 0, 6, Vec::new())), vec![(0, 10.0)]);
}

#[test]
fn styling_a_range_splits_and_merges_runs() {
    let runs = vec![run(10, 10.0)];
    let big = restyle_runs(&runs, 2, 5, |s| s.size = 30.0);
    assert_eq!(sizes(&big), vec![(2, 10.0), (3, 30.0), (5, 10.0)]);
    // Restyling it back joins the runs again.
    assert_eq!(sizes(&restyle_runs(&big, 2, 5, |s| s.size = 10.0)), vec![(10, 10.0)]);
    // A range across runs; an attribute set on a part keeps the others.
    let both = restyle_runs(&big, 4, 7, |s| s.bold = true);
    assert_eq!(both.iter().map(|r| (r.chars, r.style.size, r.style.bold)).collect::<Vec<_>>(), vec![(2, 10.0, false), (2, 30.0, false), (1, 30.0, true), (2, 10.0, true), (3, 10.0, false)]);
    // Out-of-range and backwards ranges are clamped, never a panic.
    assert_eq!(sizes(&restyle_runs(&runs, 8, 99, |s| s.size = 1.0)), vec![(8, 10.0), (2, 1.0)]);
    assert_eq!(sizes(&restyle_runs(&runs, 7, 3, |s| s.size = 1.0)), vec![(10, 10.0)]);
}

#[test]
fn a_selection_reports_what_it_shares_and_what_is_mixed() {
    let mut runs = vec![run(3, 10.0), run(3, 10.0)];
    runs[1].style.bold = true;
    runs[1].style.color = Rgba::hex(0xff0000);
    let s = summarize(&runs, 1, 5);
    assert_eq!((s.size, s.bold, s.color, s.font.as_deref()), (Some(10.0), None, None, Some("")));
    let caret = summarize(&runs, 4, 4);
    assert_eq!(caret.bold, Some(true), "a caret shows the style typing there would use");
    assert_eq!(summarize(&runs, 0, 3).bold, Some(false));
}

#[test]
fn line_breaks_split_and_join_paragraph_styles() {
    let c = ParagraphStyle { align: TextAlign::Center, ..ParagraphStyle::default() };
    let r = ParagraphStyle { align: TextAlign::Right, indent: 4.0, ..ParagraphStyle::default() };
    let paras = vec![c, r];
    // A line break in the first paragraph: both halves centred.
    assert_eq!(edit_paragraphs(&paras, "ab\ncd", 1, 1, "\n"), vec![c, c, r]);
    // Joining the two keeps the first's style.
    assert_eq!(edit_paragraphs(&paras, "ab\ncd", 2, 3, ""), vec![c]);
    // Pasting two lines into the second.
    assert_eq!(edit_paragraphs(&paras, "ab\ncd", 4, 4, "x\ny\nz"), vec![c, r, r, r]);
    assert_eq!(paragraphs_in("ab\ncd\nef", 1, 4), 0..=1);
    assert_eq!(paragraphs_in("ab\ncd\nef", 7, 7), 2..=2);
}

#[test]
fn one_edit_turns_old_text_into_new() {
    assert_eq!(diff_chars("hello", "help me"), (3, 5, 7));
    assert_eq!(diff_chars("abc", "abc"), (3, 3, 3));
    assert_eq!(diff_chars("aaa", "aa"), (2, 3, 2), "the common end never overlaps the common start");
    assert_eq!(diff_chars("", "é!"), (0, 0, 2));
    assert_eq!(diff_chars("xÿz", "xz"), (1, 2, 1));
}

#[test]
fn a_block_keeps_runs_only_when_characters_differ() {
    let mut b = TextBlock { text: "Hello".into(), size: 20.0, ..TextBlock::default() };
    b.set_styles(vec![TextRun { chars: 5, style: CharStyle { size: 20.0, ..CharStyle::default() } }], vec![ParagraphStyle::default()]);
    assert!(b.runs.is_empty() && b.paragraphs.is_empty(), "one style: no runs");
    let red = CharStyle { color: Rgba::hex(0xff0000), shift: BaselineShift::Superscript, ..b.char_style() };
    b.set_styles(vec![TextRun { chars: 2, style: red.clone() }, TextRun { chars: 3, style: b.char_style() }], Vec::new());
    assert_eq!(b.runs.len(), 2);
    assert_eq!((b.color, b.shift), (red.color, BaselineShift::Superscript), "the block's fields are its first run's");
    assert_eq!(b.styled_runs().iter().map(|r| r.chars).sum::<u32>(), 5);
    // Too many runs are capped; lengths past the text are cut.
    let many: Vec<TextRun> = (0..MAX_TEXT_RUNS as u32 + 50).map(|i| TextRun { chars: 1, style: CharStyle { size: 10.0 + (i % 2) as f32, ..CharStyle::default() } }).collect();
    let mut long = TextBlock { text: "x".repeat(MAX_TEXT_RUNS + 50), ..TextBlock::default() };
    long.set_styles(many, Vec::new());
    assert_eq!(long.runs.len(), MAX_TEXT_RUNS);
    assert_eq!(long.runs.iter().map(|r| r.chars as usize).sum::<usize>(), MAX_TEXT_RUNS + 50);
    // Hostile styles are clamped.
    let mut h = TextBlock { text: "ab".into(), ..TextBlock::default() };
    h.set_styles(vec![TextRun { chars: u32::MAX, style: CharStyle { size: f32::NAN, letter_spacing: 1e9, url: "u".repeat(5000), ..CharStyle::default() } }], vec![ParagraphStyle { indent: f32::INFINITY, left_margin: -3.0, ..ParagraphStyle::default() }; 9]);
    assert_eq!((h.size, h.letter_spacing, h.url.len(), h.indent, h.left_margin), (32.0, 1000.0, MAX_URL, 0.0, 0.0));
    assert!(h.paragraphs.is_empty(), "one paragraph");
}

#[test]
fn fills_cut_the_outline_by_colour() {
    let mut b = TextBlock { text: "ab".into(), ..TextBlock::default() };
    let red = Rgba::hex(0xff0000);
    b.set_outline(vec![(Rgba::BLACK, String::new(), Path::rect(0.0, 0.0, 1.0, 1.0)), (red, "https://example.org".into(), Path::rect(2.0, 0.0, 1.0, 1.0))]);
    assert_eq!(b.fills.len(), 2);
    assert_eq!(b.outline.cmds.len(), 10);
    let parts = b.fill_paths();
    assert_eq!(parts.len(), 2);
    assert_eq!((parts[1].0, parts[1].1), (red, "https://example.org"));
    assert_eq!(*parts[1].2, Path::rect(2.0, 0.0, 1.0, 1.0));
    // Cut once: drawing again shares the pieces.
    assert!(Arc::ptr_eq(&b.fill_paths()[0].2, &parts[0].2));
    // One piece in the block's colour keeps no fills.
    b.set_outline(vec![(Rgba::BLACK, String::new(), Path::rect(0.0, 0.0, 1.0, 1.0))]);
    assert!(b.fills.is_empty());
    assert_eq!(b.fill_paths().len(), 1);
    // Counts that don't add up never panic: the last fill takes the rest.
    let odd = TextBlock { fills: vec![TextFill { color: red, url: String::new(), cmds: 99 }, TextFill { color: red, url: String::new(), cmds: 0 }], outline: Arc::new(Path::rect(0.0, 0.0, 1.0, 1.0)), ..TextBlock::default() };
    assert_eq!(odd.fill_paths().iter().map(|p| p.2.cmds.len()).collect::<Vec<_>>(), vec![5, 0]);
}

#[test]
fn older_text_saves_byte_for_byte_and_runs_round_trip() {
    // A block as files before styled text wrote it: nothing new is written.
    let old = r#"{"text":"Hi","font":"Inter","size":20.0,"bold":false,"italic":false,"color":{"r":0.0,"g":0.0,"b":0.0,"a":1.0},"align":"left","letter_spacing":0.0,"line_spacing":0.0,"width":null,"outline":"","box_width":10.0,"box_height":12.0}"#;
    let b: TextBlock = serde_json::from_str(old).unwrap();
    assert!(b.runs.is_empty() && b.paragraphs.is_empty() && b.fills.is_empty());
    assert_eq!(serde_json::to_string(&b).unwrap(), old);
    // Runs, paragraphs and fills come back as they were.
    let mut rich = b.clone();
    rich.set_styles(vec![TextRun { chars: 1, style: CharStyle { underline: true, url: "a".into(), ..b.char_style() } }, TextRun { chars: 1, style: b.char_style() }], Vec::new());
    rich.text = "H\ni".into();
    rich.set_styles(rich.runs.clone(), vec![ParagraphStyle::default(), ParagraphStyle { align: TextAlign::Right, right_margin: 3.0, ..ParagraphStyle::default() }]);
    let json = serde_json::to_string(&rich).unwrap();
    let back: TextBlock = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rich);
    assert!(json.contains("\"runs\"") && json.contains("\"paragraphs\""));
}

#[test]
fn links_that_run_code_are_dropped() {
    assert_eq!(safe_url(" https://example.org/a "), Some("https://example.org/a".into()));
    assert_eq!(safe_url("mailto:me@example.org"), Some("mailto:me@example.org".into()));
    assert_eq!(safe_url("page.html#top"), Some("page.html#top".into()));
    assert_eq!(safe_url("JavaScript:alert(1)"), None);
    assert_eq!(safe_url(" java	script:alert(1)"), None, "control characters don't hide a scheme");
    assert_eq!(safe_url("data:text/html,x"), None);
    assert_eq!(safe_url("/a:b"), Some("/a:b".into()), "a colon in a path is no scheme");
}
