use super::*;
use crate::{Rgba, TextAlign};

fn block(text: &str, f: TextField) -> TextBlock {
    TextBlock { text: text.into(), font: "Mono".into(), size: 10.0, bold: false, italic: false, color: Rgba::BLACK, align: TextAlign::Left, letter_spacing: 0.0, line_spacing: 0.0, width: None, outline: Arc::new(Path::new()), box_width: 54.0, box_height: 24.0, field: Some(f), on_path: None, ..Default::default() }
}

/// Runs edits on a field as the player does: each change of text lays it out again (10 px per
/// character, 10 px lines) and keeps the caret in view.
struct Rig {
    block: TextBlock,
    field: TextField,
    st: FieldState,
}

impl Rig {
    fn new(text: &str, kind: TextKind, lines: LineType) -> Rig {
        let field = TextField { lines, ..TextField::new(kind) };
        let mut r = Rig { block: block(text, field.clone()), field, st: FieldState::default() };
        r.relayout();
        r
    }
    fn relayout(&mut self) {
        let shown = self.field.shown(self.st.value(&self.block));
        self.st.look = Some(Arc::new(mono_look(&shown, 10.0, 10.0, self.field.wrap_width(self.block.box_width))));
        scroll_to_caret(&self.block, &self.field, &mut self.st);
    }
    fn apply(&mut self, ch: Changed) {
        if ch.text {
            self.relayout();
        } else {
            scroll_to_caret(&self.block, &self.field, &mut self.st);
        }
    }
    fn ed(&mut self) -> (Editor<'_>, Option<Arc<FieldLook>>) {
        let look = self.st.look.clone();
        (Editor { block: &self.block, field: &self.field, st: &mut self.st }, look)
    }
    fn key(&mut self, e: Edit, shift: bool) -> &mut Self {
        let (mut ed, look) = self.ed();
        let ch = ed.key(e, shift, look.as_deref());
        self.apply(ch);
        self
    }
    fn typed(&mut self, s: &str) -> &mut Self {
        for c in s.chars() {
            let (mut ed, _) = self.ed();
            let ch = ed.insert(&c.to_string());
            self.apply(ch);
        }
        self
    }
    fn click(&mut self, x: f32, y: f32, extend: bool) -> &mut Self {
        let (mut ed, look) = self.ed();
        let ch = ed.point(Vec2::new(x, y), extend, look.as_deref());
        self.apply(ch);
        self
    }
    fn text(&self) -> &str {
        self.st.value(&self.block)
    }
}

#[test]
fn typing_moves_the_caret_and_keys_edit_around_it() {
    let mut r = Rig::new("", TextKind::Input, LineType::Single);
    r.typed("hello");
    assert_eq!((r.text(), r.st.caret, r.st.anchor), ("hello", 5, 5));
    r.key(Edit::Left, false).key(Edit::Left, false).typed("X");
    assert_eq!((r.text(), r.st.caret), ("helXlo", 4));
    r.key(Edit::Backspace, false);
    assert_eq!((r.text(), r.st.caret), ("hello", 3));
    r.key(Edit::Delete, false);
    assert_eq!((r.text(), r.st.caret), ("helo", 3));
    r.key(Edit::Home, false);
    assert_eq!(r.st.caret, 0);
    r.key(Edit::Backspace, false);
    assert_eq!(r.text(), "helo", "nothing before the caret");
    r.key(Edit::End, false).key(Edit::Delete, false);
    assert_eq!((r.text(), r.st.caret), ("helo", 4), "nothing after it");
    // Enter types nothing in a one-line field; control characters never go in.
    r.key(Edit::Enter, false).typed("\u{7}\t");
    assert_eq!(r.text(), "helo");
}

#[test]
fn shift_selects_and_typing_replaces_the_selection() {
    let mut r = Rig::new("abcdef", TextKind::Input, LineType::Single);
    r.click(20.0, 5.0, false);
    assert_eq!(r.st.caret, 2, "a click puts the caret at the nearest boundary");
    r.key(Edit::Right, true).key(Edit::Right, true);
    assert_eq!(r.st.selection(), (2, 4));
    assert_eq!(r.st.selected(&r.block), "cd");
    // An arrow without Shift collapses the selection to its side.
    r.key(Edit::Left, false);
    assert_eq!((r.st.caret, r.st.anchor), (2, 2));
    r.key(Edit::End, true);
    assert_eq!(r.st.selection(), (2, 6));
    r.typed("Z");
    assert_eq!((r.text(), r.st.caret), ("abZ", 3));
    // Dragging extends from where the press was; Backspace deletes the selection.
    r.click(0.0, 5.0, false).click(19.0, 5.0, true);
    assert_eq!(r.st.selection(), (0, 2));
    r.key(Edit::Backspace, false);
    assert_eq!((r.text(), r.st.caret), ("Z", 0));
    r.key(Edit::SelectAll, false);
    assert_eq!(r.st.selection(), (0, 1));
}

#[test]
fn multiline_fields_take_enter_and_move_between_lines() {
    let mut r = Rig::new("", TextKind::Input, LineType::MultilineNoWrap);
    r.typed("abc").key(Edit::Enter, false).typed("de");
    assert_eq!(r.text(), "abc\nde");
    assert_eq!(r.st.caret, 6);
    r.key(Edit::Up, false);
    assert_eq!(r.st.caret, 2, "up keeps the column");
    r.key(Edit::End, false);
    assert_eq!(r.st.caret, 3);
    r.key(Edit::Down, false);
    assert_eq!(r.st.caret, 6, "down to the shorter line's end");
    r.key(Edit::Down, false);
    assert_eq!(r.st.caret, 6, "down from the last line goes to the end");
    r.key(Edit::Up, false).key(Edit::Up, false);
    assert_eq!(r.st.caret, 0, "up from the first line goes to the start");
    // Clicking on the second line.
    r.click(17.0, 14.0, false);
    assert_eq!(r.st.caret, 6, "x 17 on line 2 is nearest the boundary after d");
}

#[test]
fn max_chars_and_passwords() {
    let mut r = Rig::new("", TextKind::Input, LineType::Password);
    r.field.max_chars = 4;
    r.typed("secret");
    assert_eq!(r.text(), "secr", "at most max_chars");
    let shown = r.field.shown(r.text());
    assert_eq!(shown, "****");
    // A paste is cut to the room left, and single-line fields drop its line breaks.
    let mut r = Rig::new("ab", TextKind::Input, LineType::Single);
    r.field.max_chars = 6;
    r.key(Edit::End, false);
    let (mut ed, _) = r.ed();
    let ch = ed.insert("c\nd\nefgh");
    r.apply(ch);
    assert_eq!(r.text(), "abcdef");
}

#[test]
fn dynamic_fields_select_but_never_change() {
    let mut r = Rig::new("read only", TextKind::Dynamic, LineType::Single);
    r.typed("x").key(Edit::Backspace, false).key(Edit::Delete, false);
    assert_eq!(r.text(), "read only");
    assert!(r.st.text.is_none(), "untouched");
    r.key(Edit::SelectAll, false);
    assert_eq!(r.st.selected(&r.block), "read only");
}

#[test]
fn one_line_fields_scroll_to_keep_the_caret_in_view() {
    // The box shows 50 px of text (54 less the gutters): five characters.
    let mut r = Rig::new("", TextKind::Input, LineType::Single);
    r.typed("abcdefgh");
    assert_eq!(r.st.scroll.x, 30.0, "the caret at 80 px shows at the right edge");
    r.key(Edit::Home, false);
    assert_eq!(r.st.scroll.x, 0.0);
    // Multiline: lines wrap and it scrolls down instead (the box shows two 10 px lines).
    let mut r = Rig::new("", TextKind::Input, LineType::Multiline);
    r.typed("aaaaabbbbbccccc");
    assert_eq!(r.st.look.as_ref().map(|l| l.lines.len()), Some(3));
    assert_eq!((r.st.scroll.x, r.st.scroll.y), (0.0, 10.0));
}

#[test]
fn script_text_is_capped_and_takes_actionscript_line_ends() {
    let f = TextField::new(TextKind::Dynamic);
    assert_eq!(fit(&f, "a\rb\r\nc"), "a\nb\nc");
    assert_eq!(chars(&fit(&f, &"x".repeat(MAX_FIELD_CHARS + 10))), MAX_FIELD_CHARS as u32);
    assert_eq!(slice("héllo", 1, 3), "él");
    assert_eq!(slice("héllo", 4, 99), "o");
    assert_eq!(byte_at("héllo", 2), 3);
}

#[test]
fn restrict_reads_as_actionscript_writes_it() {
    let ok = |r: &str, s: &str| s.chars().filter(|c| allowed(r, *c)).collect::<String>();
    assert_eq!(ok("A-Z 0-9", "Ab 9-z"), "A 9");
    assert_eq!(ok("^a-z", "aBc1"), "B1", "everything but lower case");
    assert_eq!(ok("A-Z^Q", "PQR"), "PR", "capitals, but not Q");
    assert_eq!(ok("^a-z^m", "almz"), "m", "excluded, then let back in");
    assert_eq!(ok("0-9\\-", "1-2a"), "1-2", "an escaped hyphen is a character");
    assert_eq!(ok("\\^\\\\", "^\\a"), "^\\");
    assert_eq!(ok("", "abc"), "", "an empty list allows nothing");
    assert_eq!(ok("abc-", "a-c"), "a-c", "a trailing hyphen is itself");
    assert_eq!(ok("z-a", "amz"), "", "a backwards range holds nothing");
    assert_eq!(ok("é-ü😀", "é😀a"), "é😀");
    // Long and odd lists are read without trouble.
    assert!(!allowed(&"^".repeat(5000), 'a') || allowed(&"^".repeat(5000), 'a'));
    assert!(allowed(&"\\".repeat(1023), '\\'));
}


// Text that mixes directions (ADR 0130). Looks are written by hand, as the layout hands them
// over: every character 10 wide, capitals reading right to left.

fn cell(start: u32, x0: f32, rtl: bool) -> CaretBox {
    CaretBox { start, end: start + 1, x0, x1: x0 + 10.0, rtl }
}

fn look_of(lines: Vec<CaretLine>) -> FieldLook {
    let n = lines.len() as f32;
    FieldLook { outline: Arc::new(Path::new()), pieces: Arc::from(Vec::new()), lines: lines.into(), line_height: 10.0, width: 50.0, height: n * 10.0 }
}

/// "abCDE" in a left-to-right paragraph, displayed a b E D C.
fn word_in_latin() -> FieldLook {
    let boxes = vec![cell(0, 0.0, false), cell(1, 10.0, false), cell(4, 20.0, true), cell(3, 30.0, true), cell(2, 40.0, true)];
    look_of(vec![CaretLine { start: 0, end: 5, stops: vec![(0, 0.0), (1, 10.0), (2, 50.0), (3, 40.0), (4, 30.0), (5, 20.0)], rtl: false, boxes }])
}

/// "AB CD" in a right-to-left paragraph wrapped after the space: B A on the first line, D C on
/// the second, each starting at its right end.
fn wrapped_rtl() -> FieldLook {
    let first = CaretLine { start: 0, end: 3, stops: vec![(0, 20.0), (1, 10.0), (2, 0.0), (3, 0.0)], rtl: true, boxes: vec![cell(1, 0.0, true), cell(0, 10.0, true)] };
    let second = CaretLine { start: 3, end: 5, stops: vec![(3, 20.0), (4, 10.0), (5, 0.0)], rtl: true, boxes: vec![cell(4, 0.0, true), cell(3, 10.0, true)] };
    look_of(vec![first, second])
}

/// The carets a key visits from `from`, pressed `n` times.
fn walk(text: &str, look: &FieldLook, from: u32, key: Edit, n: usize) -> Vec<u32> {
    let field = TextField { lines: LineType::Multiline, ..TextField::new(TextKind::Input) };
    let b = block(text, field.clone());
    let mut st = FieldState { caret: from, anchor: from, ..FieldState::default() };
    (0..n)
        .map(|_| {
            Editor { block: &b, field: &field, st: &mut st }.key(key, false, Some(look));
            st.caret
        })
        .collect()
}

#[test]
fn arrows_move_the_caret_the_way_they_point_through_a_right_to_left_word() {
    let look = word_in_latin();
    // Right from the start: past a and b, then through the word from its left (its end) to
    // its right (its start), where the line ends.
    assert_eq!(walk("abCDE", &look, 0, Edit::Right, 6), vec![1, 5, 4, 3, 2, 2]);
    assert_eq!(walk("abCDE", &look, 2, Edit::Left, 6), vec![3, 4, 5, 1, 0, 0]);
    // Each press moves the caret further the way the key points.
    let xs: Vec<f32> = [0, 1, 5, 4, 3, 2].iter().map(|i| look.caret_at(*i).x).collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    // Shift keeps the anchor: the selection is still a range of characters.
    let field = TextField::new(TextKind::Input);
    let b = block("abCDE", field.clone());
    let mut st = FieldState { caret: 1, anchor: 1, ..FieldState::default() };
    Editor { block: &b, field: &field, st: &mut st }.key(Edit::Right, true, Some(&look));
    assert_eq!(st.selection(), (1, 5));
    // A line that reads left to right throughout moves by characters, as it always has.
    assert_eq!(mono_look("abc", 10.0, 10.0, None).step(1, true), None);
}

#[test]
fn arrows_cross_lines_in_the_paragraphs_direction() {
    let look = wrapped_rtl();
    // Left reads on in a right-to-left paragraph: to the line's left end, then the next line's
    // right end. The wrapped line's end is the next line's start, shown there.
    assert_eq!(walk("AB CD", &look, 0, Edit::Left, 6), vec![1, 2, 3, 4, 5, 5]);
    assert_eq!(walk("AB CD", &look, 5, Edit::Right, 6), vec![4, 3, 2, 1, 0, 0]);
    assert_eq!((look.caret_at(3), look.caret_at(2)), (Vec2::new(20.0, 10.0), Vec2::new(0.0, 0.0)));
    // Home is where the line starts and End where it ends, whichever side that is.
    assert_eq!(walk("AB CD", &look, 4, Edit::Home, 1), vec![3]);
    assert_eq!(look.lines[1].x_at(3), 20.0);
}

#[test]
fn a_selection_across_directions_is_highlighted_in_stretches() {
    let look = word_in_latin();
    let line = &look.lines[0];
    assert_eq!(line.spans(0, 2), vec![(0.0, 20.0)]);
    assert_eq!(line.spans(2, 5), vec![(20.0, 50.0)], "the whole word");
    // b and the word's first two letters (C and D, its rightmost): two stretches.
    assert_eq!(line.spans(1, 4), vec![(10.0, 20.0), (30.0, 50.0)]);
    assert_eq!(line.spans(0, 9), vec![(0.0, 50.0)]);
    assert_eq!(line.spans(3, 3), Vec::new());
    assert_eq!(line.spans(7, 9), Vec::new());
    // Without boxes (text in one direction): from one caret place to the other.
    let plain = mono_look("abcd", 10.0, 10.0, None);
    assert_eq!(plain.lines[0].spans(1, 3), vec![(10.0, 30.0)]);
    assert!(!plain.lines[0].mixed() && line.mixed() && wrapped_rtl().lines[0].mixed());
}
