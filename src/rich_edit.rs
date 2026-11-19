//! Styled text while typing on the stage (ADR 0085): the runs and paragraph styles of the text
//! being typed, kept as the text changes (what is typed continues the style before it), shown
//! in the editor with their colours, sizes, slant, underline and baseline shift; Properties
//! styles the selected characters (or what is typed next, at a caret); copying and pasting
//! inside AniCraft keeps styles, text from other apps comes in plain.
//!
//! Every style change on an existing block is one `text.edit` with the text, runs and
//! paragraphs, so it shows on the stage the same frame and undoes as one step.

use crate::app::{App, TextEditing};
use anicraft_engine::cmds_text::{CharPatch, ParaPatch};
use anicraft_engine::doc::rich_text as rt;
use anicraft_engine::doc::{BaselineShift, CharStyle, ElementId, ParagraphStyle, Rgba, TextBlock, TextRun};
use egui::text::{LayoutJob, TextFormat};
use serde_json::{Value, json};

/// Styled text copied from the stage editor: pasted back as the same text, it keeps its runs.
#[derive(Clone, Debug, Default)]
pub struct StyledClip {
    pub text: String,
    pub runs: Vec<TextRun>,
}

impl TextEditing {
    /// Editing `block` (or new text in `style`, when there is none) with its top-left at `at`.
    pub fn new(at: egui::Pos2, element: Option<ElementId>, block: Option<&TextBlock>, style: &anicraft_engine::cmds_text::TextStyle) -> TextEditing {
        let (text, runs, paras) = match block {
            Some(b) => (b.text.clone(), b.styled_runs(), b.paragraph_styles()),
            None => {
                let c = CharStyle { font: style.font.clone(), size: style.size, bold: style.bold, italic: style.italic, color: style.color, letter_spacing: style.letter_spacing, ..CharStyle::default() };
                (String::new(), vec![TextRun { chars: 0, style: c }], vec![ParagraphStyle { align: style.align, line_spacing: style.line_spacing, ..ParagraphStyle::default() }])
            }
        };
        let n = text.chars().count() as u32;
        TextEditing { at, element, original: text.clone(), last: text.clone(), text, runs, paras, sel: (n, n), pending: None, paste: None, focus: true }
    }

    /// The selected characters, first to last.
    pub fn range(&self) -> (u32, u32) {
        (self.sel.0.min(self.sel.1), self.sel.0.max(self.sel.1))
    }

    /// Follows a change of the text: the runs and paragraphs move with it, and a paste of
    /// styled text copied here takes its runs back.
    pub fn sync(&mut self) {
        if self.text == self.last {
            return;
        }
        let (start, old_end, new_end) = rt::diff_chars(&self.last, &self.text);
        let inserted: String = self.text.chars().skip(start as usize).take((new_end - start) as usize).collect();
        // A caret style picked before typing (Properties at a caret) styles what is typed.
        let typed = match (self.pending.take(), self.paste.take()) {
            (_, Some(clip)) if clip.text == inserted => clip.runs,
            (Some(style), _) => vec![TextRun { chars: new_end - start, style }],
            _ => vec![TextRun { chars: new_end - start, style: rt::style_at(&self.runs, start) }],
        };
        self.runs = rt::replace_runs(&self.runs, start, old_end, typed);
        self.paras = rt::edit_paragraphs(&self.paras, &self.last, start, old_end, &inserted);
        self.last = self.text.clone();
    }

    /// What the selection shares (`None` attributes are mixed); at a caret, what typing gets.
    pub fn summary(&self) -> rt::StyleSummary {
        let (a, b) = self.range();
        match (&self.pending, a == b) {
            (Some(p), true) => rt::StyleSummary::of(p),
            _ => rt::summarize(&self.runs, a, b),
        }
    }

    /// Restyles the selection (at a caret: what is typed next there).
    pub fn restyle(&mut self, patch: &CharPatch) {
        let (a, b) = self.range();
        if a == b {
            let mut s = self.pending.clone().unwrap_or_else(|| rt::style_at(&self.runs, a));
            patch.apply(&mut s);
            self.pending = Some(s);
            return;
        }
        self.runs = rt::restyle_runs(&self.runs, a, b, |s| patch.apply(s));
    }

    /// Sets the layout of the paragraphs the selection touches.
    pub fn repara(&mut self, patch: &ParaPatch) {
        let (a, b) = self.range();
        let range = rt::paragraphs_in(&self.text, a, b);
        let n = rt::paragraph_count(&self.text);
        let fill = self.paras.last().copied().unwrap_or_default();
        self.paras.resize(n, fill);
        for (i, p) in self.paras.iter_mut().enumerate() {
            if range.contains(&i) {
                patch.apply(p);
            }
        }
    }

    /// The `text.edit` params that set the block to what is being typed.
    pub fn edit_params(&self) -> Value {
        let runs: Vec<Value> = self.runs.iter().filter(|r| r.chars > 0).map(run_json).collect();
        let paras: Vec<Value> = self.paras.iter().map(para_json).collect();
        json!({"text": self.text, "runs": runs, "paragraphs": paras})
    }

    /// The styled text of the selection (for Copy and Cut).
    pub fn clip(&self) -> Option<StyledClip> {
        let (a, b) = self.range();
        (a < b).then(|| StyledClip { text: self.text.chars().skip(a as usize).take((b - a) as usize).collect(), runs: rt::slice_runs(&self.runs, a, b) })
    }
}

fn hex(c: Rgba) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

fn run_json(r: &TextRun) -> Value {
    let s = &r.style;
    json!({"chars": r.chars, "font": s.font, "size": s.size, "bold": s.bold, "italic": s.italic, "color": hex(s.color), "letter_spacing": s.letter_spacing, "shift": s.shift.key(), "underline": s.underline, "url": s.url})
}

fn para_json(p: &ParagraphStyle) -> Value {
    let align = match p.align {
        anicraft_engine::doc::TextAlign::Left => "left",
        anicraft_engine::doc::TextAlign::Center => "center",
        anicraft_engine::doc::TextAlign::Right => "right",
    };
    json!({"align": align, "indent": p.indent, "left_margin": p.left_margin, "right_margin": p.right_margin, "line_spacing": p.line_spacing})
}

/// How the editor shows the text being typed: each run in its colour, size (at the stage's
/// zoom, within what the editor can show), slant, underline and baseline shift. The type is
/// the interface's (the stage itself shows the real faces).
pub fn layout_job(runs: &[TextRun], last: &str, text: &str, zoom: f32, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob { wrap: egui::text::TextWrapping { max_width: wrap, ..Default::default() }, ..LayoutJob::default() };
    // Typed this frame (before the runs follow it): laid out as typing will style it.
    let runs = if last == text {
        runs.to_vec()
    } else {
        let (start, old_end, new_end) = rt::diff_chars(last, text);
        rt::type_into_runs(runs, start, old_end, new_end - start)
    };
    let mut bytes = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len()));
    let mut at_byte = bytes.next().unwrap_or(0);
    let total = text.chars().count() as u32;
    let mut covered = 0u32;
    for (i, r) in runs.iter().enumerate() {
        let n = if i + 1 == runs.len() { total.saturating_sub(covered) } else { r.chars.min(total.saturating_sub(covered)) };
        if n == 0 {
            continue;
        }
        let mut end_byte = at_byte;
        for _ in 0..n {
            end_byte = bytes.next().unwrap_or(text.len());
        }
        let s = &r.style;
        let small = s.shift != BaselineShift::Normal;
        let size = s.size * zoom * if small { 0.6 } else { 1.0 };
        let mut f = TextFormat::simple(crate::theme::stage_text_run_font(size), crate::theme::stage_text_colour(s.color));
        f.italics = s.italic;
        if s.underline || !s.url.is_empty() {
            f.underline = egui::Stroke::new((size / 14.0).max(1.0), f.color);
        }
        f.valign = if s.shift == BaselineShift::Superscript { egui::Align::TOP } else { egui::Align::BOTTOM };
        if let Some(seg) = text.get(at_byte..end_byte) {
            job.append(seg, 0.0, f);
        }
        at_byte = end_byte;
        covered += n;
    }
    job
}

/// Applies the editor's runs to the block on the stage (one undo step) when it exists, so the
/// change shows there now.
fn push(app: &mut App, label: &'static str) {
    let Some(te) = app.text_edit.as_mut() else { return };
    te.focus = true;
    let Some(id) = te.element else { return };
    if te.text.trim().is_empty() {
        return;
    }
    let p = te.edit_params();
    te.original = te.text.clone();
    app.run("element.select", json!({"ids": [id.0]}));
    crate::panels::live(app, label, "text.edit", p);
}

/// Properties while typing on the stage: the selected characters' style (mixed values marked)
/// and the layout of the paragraphs they touch.
pub fn selection_rows(app: &mut App, ui: &mut egui::Ui) {
    let Some(te) = app.text_edit.as_ref() else { return };
    let m = te.summary();
    let (a, b) = te.range();
    let para = {
        let range = rt::paragraphs_in(&te.text, a, b);
        let list: Vec<ParagraphStyle> = te.paras.iter().enumerate().filter(|(i, _)| range.contains(i)).map(|(_, p)| *p).collect();
        let first = list.first().copied().or(te.paras.last().copied()).unwrap_or_default();
        (first, list.iter().any(|p| p.align != first.align), list.iter().any(|p| p.indent != first.indent || p.left_margin != first.left_margin || p.right_margin != first.right_margin), list.iter().any(|p| p.line_spacing != first.line_spacing))
    };
    let first = rt::style_at(&te.runs, a);
    let mut st = anicraft_engine::cmds_text::TextStyle {
        font: m.font.clone().unwrap_or(first.font.clone()),
        size: m.size.unwrap_or(first.size),
        bold: m.bold.unwrap_or(false),
        italic: m.italic.unwrap_or(false),
        color: m.color.unwrap_or(first.color),
        align: para.0.align,
        letter_spacing: m.letter_spacing.unwrap_or(first.letter_spacing),
        line_spacing: para.0.line_spacing,
    };
    let mixed = crate::panels::TextMixed { font: m.font.is_none(), size: m.size.is_none(), color: m.color.is_none(), bold: m.bold.is_none(), italic: m.italic.is_none(), letter_spacing: m.letter_spacing.is_none(), align: para.1, line_spacing: para.3 };
    let mut extra = crate::panels::TextExtra {
        shift: m.shift,
        underline: m.underline,
        url: m.url.clone(),
        indent: para.0.indent,
        left_margin: para.0.left_margin,
        right_margin: para.0.right_margin,
        layout_mixed: para.2,
    };
    // Its own ids: the block's rows may show in the same panel.
    let r = ui.push_id("text_selection_rows", |ui| {
        let edit = crate::panels::text_style_rows(app, ui, &mut st, None, Some(&mixed));
        let more = crate::panels::text_more_rows(ui, &mut extra);
        edit.or(more)
    });
    app.text_props_rect = Some(r.response.rect);
    let Some((label, p)) = r.inner else { return };
    let (cp, pp) = (CharPatch::parse(&p).unwrap_or_default(), ParaPatch::parse(&p).unwrap_or_default());
    if let Some(te) = app.text_edit.as_mut() {
        te.restyle(&cp);
        te.repara(&pp);
    }
    push(app, label);
}

/// Before the editor sees this frame's input: Copy and Cut keep the selection's styles, and a
/// paste of exactly that text takes them back.
pub fn watch_clipboard(app: &mut App, ctx: &egui::Context, focused: bool) {
    if !focused {
        return;
    }
    let events = ctx.input(|i| i.events.clone());
    for e in events {
        match e {
            egui::Event::Copy | egui::Event::Cut => {
                if let Some(clip) = app.text_edit.as_ref().and_then(TextEditing::clip) {
                    app.text_clip = Some(clip);
                }
            }
            egui::Event::Paste(s) => {
                let clip = app.text_clip.clone().filter(|c| c.text == s.replace("\r\n", "\n"));
                if let Some(te) = app.text_edit.as_mut() {
                    te.paste = clip;
                }
            }
            _ => {}
        }
    }
}

/// A short title for the Properties section while typing.
pub fn title(te: &TextEditing) -> String {
    let (a, b) = te.range();
    if a == b { tr!("Text: at the caret").to_string() } else { trn!("Text: {n} character selected", "Text: {n} characters selected", (b - a) as usize).to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editing(text: &str) -> TextEditing {
        let mut te = TextEditing::new(egui::Pos2::ZERO, None, None, &anicraft_engine::cmds_text::TextStyle::default());
        te.text = text.into();
        te.sync();
        te
    }

    #[test]
    fn typing_after_a_styled_word_continues_its_style() {
        let mut te = editing("Hello world");
        te.sel = (0, 5);
        te.restyle(&CharPatch { bold: Some(true), ..CharPatch::default() });
        te.text = "Hello! world".into();
        te.sync();
        assert_eq!(te.runs.iter().map(|r| (r.chars, r.style.bold)).collect::<Vec<_>>(), vec![(6, true), (6, false)]);
        // At a caret, a style picked first styles what is typed next.
        te.sel = (12, 12);
        te.restyle(&CharPatch { italic: Some(true), ..CharPatch::default() });
        assert_eq!(te.summary().italic, Some(true));
        te.text = "Hello! world?".into();
        te.sync();
        assert_eq!(te.runs.last().map(|r| (r.chars, r.style.italic)), Some((1, true)));
    }

    #[test]
    fn copy_and_paste_inside_keeps_styles_and_outside_text_is_plain() {
        let mut te = editing("red blue");
        te.sel = (0, 3);
        te.restyle(&CharPatch { color: Some(Rgba::hex(0xff0000)), ..CharPatch::default() });
        let clip = te.clip().unwrap();
        assert_eq!(clip.text, "red");
        // Pasted at the end: red again.
        te.paste = Some(clip.clone());
        te.text = "red bluered".into();
        te.sync();
        assert_eq!(te.runs.last().map(|r| (r.chars, r.style.color)), Some((3, Rgba::hex(0xff0000))));
        // Other text (from another app): the style before it, plain.
        te.paste = Some(clip);
        te.text = "red bluered!!".into();
        te.sync();
        assert_eq!(te.runs.last().map(|r| (r.chars, r.style.color)), Some((5, Rgba::hex(0xff0000))), "continues the red before it");
        let mut te = editing("a b");
        te.paste = Some(StyledClip { text: "zz".into(), runs: vec![TextRun { chars: 2, style: CharStyle { bold: true, ..CharStyle::default() } }] });
        te.text = "a xyb".into();
        te.sync();
        assert!(te.runs.iter().all(|r| !r.style.bold), "a different text is plain");
    }

    #[test]
    fn paragraphs_follow_line_breaks_and_selection_rows_mark_mixed() {
        let mut te = editing("one\ntwo");
        te.sel = (0, 0);
        te.repara(&ParaPatch { align: Some(anicraft_engine::doc::TextAlign::Center), ..ParaPatch::default() });
        te.text = "one\ntwo\nthree".into();
        te.sync();
        assert_eq!(te.paras.len(), 3);
        assert_eq!(te.paras[0].align, anicraft_engine::doc::TextAlign::Center);
        let p = te.edit_params();
        assert_eq!(p["paragraphs"].as_array().map(Vec::len), Some(3));
        te.sel = (0, 5);
        te.restyle(&CharPatch { size: Some(50.0), ..CharPatch::default() });
        te.sel = (3, 9);
        assert_eq!(te.summary().size, None, "mixed sizes");
        // The editor shows each run in its own format.
        let job = layout_job(&te.runs, &te.last, &te.text, 1.0, 300.0);
        assert!(job.sections.len() >= 2);
    }
}
