//! Styled text (ADR 0085): runs of characters with their own style, paragraphs with their own
//! layout, and the outline cut into one filled piece per colour and link.
//!
//! A [`TextBlock`] with no runs is one run in the block's own style, and with no paragraph
//! styles every paragraph has the block's alignment and line spacing: files from before styled
//! text open unchanged and save byte for byte the same. When a block has runs, its own style
//! fields mirror the first run (and its alignment and line spacing the first paragraph), so code
//! that reads "the block's style" sees the style the text starts with.
//!
//! The run edits here are pure functions on run lists, so typing, deleting, pasting and styling
//! a range split and merge runs the same way in the engine, the shell and the tests.

use crate::{Rgba, TextAlign, TextBlock};
use anicraft_geom::Path;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

/// Most runs one block keeps; past it, the rest of the text takes the last run's style.
pub const MAX_TEXT_RUNS: usize = 10_000;

/// Longest link a run keeps (characters).
pub const MAX_URL: usize = 2048;

/// Where characters sit against the line's baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BaselineShift {
    #[default]
    Normal,
    /// Smaller and raised (the font's superscript size and offset).
    Superscript,
    /// Smaller and lowered (the font's subscript size and offset).
    Subscript,
}

impl BaselineShift {
    pub const ALL: [BaselineShift; 3] = [BaselineShift::Normal, BaselineShift::Superscript, BaselineShift::Subscript];
    pub fn key(self) -> &'static str {
        match self {
            BaselineShift::Normal => "normal",
            BaselineShift::Superscript => "superscript",
            BaselineShift::Subscript => "subscript",
        }
    }
    pub fn from_key(s: &str) -> Option<BaselineShift> {
        let s = s.trim().to_ascii_lowercase();
        BaselineShift::ALL.into_iter().find(|k| k.key() == s)
    }
}

/// A link a run may keep: trimmed and cut to [`MAX_URL`]; `None` for schemes that run code
/// where the link is followed (`javascript:`, `vbscript:`, `data:`), which exports must never
/// write.
pub fn safe_url(url: &str) -> Option<String> {
    let url: String = url.trim().chars().filter(|c| !c.is_control()).take(MAX_URL).collect();
    let scheme: String = url.chars().filter(|c| !c.is_whitespace()).take_while(|c| *c != ':').collect::<String>().to_ascii_lowercase();
    let has_scheme = url.contains(':') && !scheme.contains(['/', '?', '#']);
    (!(has_scheme && ["javascript", "vbscript", "data"].contains(&scheme.as_str()))).then_some(url)
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}
fn is_normal(s: &BaselineShift) -> bool {
    *s == BaselineShift::Normal
}

/// One character style: what a run of characters is drawn in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharStyle {
    /// Font family the outlines were laid out with.
    pub font: String,
    /// Font size in pixels.
    pub size: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    pub color: Rgba,
    /// Extra space after each character, in pixels.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub letter_spacing: f32,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub shift: BaselineShift,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    /// A link the characters go to (empty: none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

impl Default for CharStyle {
    fn default() -> Self {
        CharStyle { font: String::new(), size: 32.0, bold: false, italic: false, color: Rgba::BLACK, letter_spacing: 0.0, shift: BaselineShift::Normal, underline: false, url: String::new() }
    }
}

impl CharStyle {
    /// The style clamped to what layout accepts (sizes 1 to 2000 px, spacing within ±1000 px,
    /// names and links cut to length).
    pub fn sanitized(&self) -> CharStyle {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        CharStyle {
            font: self.font.chars().take(256).collect(),
            size: fin(self.size, 32.0).clamp(1.0, 2000.0),
            letter_spacing: fin(self.letter_spacing, 0.0).clamp(-1000.0, 1000.0),
            color: Rgba::new(fin(self.color.r, 0.0).clamp(0.0, 1.0), fin(self.color.g, 0.0).clamp(0.0, 1.0), fin(self.color.b, 0.0).clamp(0.0, 1.0), fin(self.color.a, 1.0).clamp(0.0, 1.0)),
            url: safe_url(&self.url).unwrap_or_default(),
            ..self.clone()
        }
    }
}

/// A run: this many characters (`char`s, line breaks included) in one style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub chars: u32,
    #[serde(flatten)]
    pub style: CharStyle,
}

/// One paragraph's layout (a paragraph is a line of the text, up to a line break).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParagraphStyle {
    #[serde(default)]
    pub align: TextAlign,
    /// The first line starts this much further in (pixels; negative hangs it out).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub indent: f32,
    /// Space kept clear at the box's left and right (pixels).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub left_margin: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub right_margin: f32,
    /// Extra space below each of its lines (pixels).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub line_spacing: f32,
}

impl ParagraphStyle {
    /// Clamped to what layout accepts (indent within ±10000 px, margins 0 to 10000 px, line
    /// spacing within ±1000 px).
    pub fn sanitized(&self) -> ParagraphStyle {
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        ParagraphStyle {
            align: self.align,
            indent: fin(self.indent).clamp(-10_000.0, 10_000.0),
            left_margin: fin(self.left_margin).clamp(0.0, 10_000.0),
            right_margin: fin(self.right_margin).clamp(0.0, 10_000.0),
            line_spacing: fin(self.line_spacing).clamp(-1000.0, 1000.0),
        }
    }
}

/// One filled piece of a block's outline: the next `cmds` path commands, in one colour, with a
/// link if its characters have one. A block's fills cut its outline in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextFill {
    pub color: Rgba,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    pub cmds: u32,
}

/// The fills' outlines, cut once from the block's outline the first time they are drawn (never
/// saved, and empty again in a copy).
#[derive(Default)]
pub struct FillCache(OnceLock<(usize, Vec<u32>, Vec<Arc<Path>>)>);

impl Clone for FillCache {
    fn clone(&self) -> Self {
        FillCache::default()
    }
}
impl PartialEq for FillCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl std::fmt::Debug for FillCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FillCache")
    }
}

impl TextBlock {
    /// The style the block's own fields hold (its first run's).
    pub fn char_style(&self) -> CharStyle {
        CharStyle {
            font: self.font.clone(),
            size: self.size,
            bold: self.bold,
            italic: self.italic,
            color: self.color,
            letter_spacing: self.letter_spacing,
            shift: self.shift,
            underline: self.underline,
            url: self.url.clone(),
        }
    }

    /// The paragraph style the block's own fields hold (its first paragraph's).
    pub fn para_style(&self) -> ParagraphStyle {
        ParagraphStyle { align: self.align, indent: self.indent, left_margin: self.left_margin, right_margin: self.right_margin, line_spacing: self.line_spacing }
    }

    /// Its characters.
    pub fn char_count(&self) -> u32 {
        self.text.chars().count() as u32
    }

    /// Its runs, covering every character (one run in the block's style when it has none).
    pub fn styled_runs(&self) -> Vec<TextRun> {
        let n = self.char_count();
        if self.runs.is_empty() {
            return vec![TextRun { chars: n, style: self.char_style() }];
        }
        fit_runs(self.runs.clone(), n)
    }

    /// One style per paragraph (the block's when it has none; a missing one takes the last).
    pub fn paragraph_styles(&self) -> Vec<ParagraphStyle> {
        let n = paragraph_count(&self.text);
        let own = self.para_style();
        (0..n).map(|i| self.paragraphs.get(i).or(self.paragraphs.last()).copied().unwrap_or(own)).collect()
    }

    /// Whether characters differ in style (more than one run).
    pub fn has_runs(&self) -> bool {
        self.runs.len() > 1
    }

    /// Sets the runs and paragraph styles (fitted to the text, merged, capped and sanitized),
    /// and the block's own style fields to the first of each. A block whose characters all share
    /// one style, and paragraphs one layout, keeps none, as text always was.
    pub fn set_styles(&mut self, runs: Vec<TextRun>, paragraphs: Vec<ParagraphStyle>) {
        let n = self.char_count();
        let runs = normalize_runs(runs.into_iter().map(|r| TextRun { chars: r.chars, style: r.style.sanitized() }).collect(), n);
        let first = runs.first().map_or_else(|| self.char_style(), |r| r.style.clone());
        self.font = first.font.clone();
        self.size = first.size;
        self.bold = first.bold;
        self.italic = first.italic;
        self.color = first.color;
        self.letter_spacing = first.letter_spacing;
        self.shift = first.shift;
        self.underline = first.underline;
        self.url = first.url.clone();
        self.runs = if runs.len() > 1 { runs } else { Vec::new() };
        let count = paragraph_count(&self.text);
        let mut paras: Vec<ParagraphStyle> = paragraphs.iter().take(count).map(ParagraphStyle::sanitized).collect();
        let fill = paras.last().copied().unwrap_or_else(|| self.para_style());
        paras.resize(count, fill);
        if let Some(p) = paras.first() {
            self.align = p.align;
            self.indent = p.indent;
            self.left_margin = p.left_margin;
            self.right_margin = p.right_margin;
            self.line_spacing = p.line_spacing;
        }
        self.paragraphs = if paras.windows(2).all(|w| matches!(w, [a, b] if a == b)) { Vec::new() } else { paras };
    }

    /// Each fill's outline with its colour and link: the block's outline cut by its fills, or
    /// the whole outline in the block's colour when it has none.
    pub fn fill_paths(&self) -> Vec<(Rgba, &str, Arc<Path>)> {
        if self.fills.is_empty() {
            return vec![(self.color, self.url.as_str(), self.outline.clone())];
        }
        let key = Arc::as_ptr(&self.outline) as usize;
        let counts: Vec<u32> = self.fills.iter().map(|f| f.cmds).collect();
        let cut = || split_outline(&self.outline, &counts);
        let paths = match self.fill_cache.0.get() {
            Some((k, c, p)) if *k == key && *c == counts => p.clone(),
            Some(_) => cut(),
            None => self.fill_cache.0.get_or_init(|| (key, counts.clone(), cut())).2.clone(),
        };
        self.fills.iter().zip(paths).map(|(f, p)| (f.color, f.url.as_str(), p)).collect()
    }

    /// Sets the outline from pieces in their colours and links (one after another), keeping
    /// no fills when there is one piece in the block's colour without a link.
    pub fn set_outline(&mut self, pieces: Vec<(Rgba, String, Path)>) {
        let single = matches!(pieces.as_slice(), [(c, u, _)] if *c == self.color && u.is_empty()) || pieces.is_empty();
        let mut outline = Path::new();
        let mut fills = Vec::with_capacity(pieces.len());
        for (color, url, p) in pieces {
            fills.push(TextFill { color, url, cmds: p.cmds.len() as u32 });
            outline.cmds.extend(p.cmds);
        }
        self.outline = Arc::new(outline);
        self.fills = if single { Vec::new() } else { fills };
        self.fill_cache = FillCache::default();
    }
}

/// `outline` cut into pieces of `counts` commands (the last takes what is left; extra counts
/// past the end are empty).
fn split_outline(outline: &Path, counts: &[u32]) -> Vec<Arc<Path>> {
    let mut at = 0usize;
    let n = counts.len();
    counts
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let end = if i + 1 == n { outline.cmds.len() } else { at.saturating_add(*c as usize).min(outline.cmds.len()) };
            let p = Path { cmds: outline.cmds.get(at..end).map(<[_]>::to_vec).unwrap_or_default() };
            at = end;
            Arc::new(p)
        })
        .collect()
}

/// The paragraphs in `text` (one more than its line breaks).
pub fn paragraph_count(text: &str) -> usize {
    text.bytes().filter(|b| *b == b'\n').count() + 1
}

/// Runs fitted to `n` characters: the last runs cut or the last one lengthened.
fn fit_runs(mut runs: Vec<TextRun>, n: u32) -> Vec<TextRun> {
    let mut at = 0u32;
    let mut keep = 0;
    for r in &mut runs {
        if at >= n && keep > 0 {
            break;
        }
        r.chars = r.chars.min(n - at);
        at += r.chars;
        keep += 1;
    }
    runs.truncate(keep.max(1));
    if let Some(last) = runs.last_mut() {
        last.chars += n - at.min(n);
    }
    runs
}

/// Runs fitted to `n` characters, empty ones dropped, neighbours in one style joined, at most
/// [`MAX_TEXT_RUNS`] (the rest of the text takes the last kept run's style). Always at least one.
pub fn normalize_runs(runs: Vec<TextRun>, n: u32) -> Vec<TextRun> {
    let fallback = runs.first().map(|r| r.style.clone()).unwrap_or_default();
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len().min(MAX_TEXT_RUNS));
    for r in fit_runs(runs, n) {
        if r.chars == 0 {
            continue;
        }
        let full = out.len() >= MAX_TEXT_RUNS;
        match out.last_mut() {
            Some(last) if last.style == r.style || full => last.chars += r.chars,
            _ => out.push(r),
        }
    }
    if out.is_empty() {
        out.push(TextRun { chars: n, style: fallback });
    }
    out
}

/// The style typing at character boundary `at` continues: the character before it, or the
/// first character's at the start.
pub fn style_at(runs: &[TextRun], at: u32) -> CharStyle {
    let mut end = 0u32;
    for r in runs {
        end = end.saturating_add(r.chars);
        if at <= end && (at > end - r.chars || at == 0) {
            return r.style.clone();
        }
    }
    runs.last().map(|r| r.style.clone()).unwrap_or_default()
}

/// The runs of characters `start..end`.
pub fn slice_runs(runs: &[TextRun], start: u32, end: u32) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut at = 0u32;
    for r in runs {
        let (a, b) = (at.max(start), at.saturating_add(r.chars).min(end));
        if a < b {
            out.push(TextRun { chars: b - a, style: r.style.clone() });
        }
        at = at.saturating_add(r.chars);
        if at >= end {
            break;
        }
    }
    out
}

/// Characters `start..end` replaced by `inserted` (runs of the new characters): the runs on
/// either side are cut, the new ones go between, neighbours in one style join.
pub fn replace_runs(runs: &[TextRun], start: u32, end: u32, inserted: Vec<TextRun>) -> Vec<TextRun> {
    let total: u32 = runs.iter().map(|r| r.chars).fold(0, u32::saturating_add);
    let (start, end) = (start.min(total), end.clamp(start.min(total), total));
    let mut out = slice_runs(runs, 0, start);
    out.extend(inserted);
    out.extend(slice_runs(runs, end, total));
    let n = out.iter().map(|r| r.chars).fold(0, u32::saturating_add);
    let style = runs.first().map(|r| r.style.clone()).unwrap_or_default();
    if out.is_empty() {
        return vec![TextRun { chars: 0, style }];
    }
    normalize_runs(out, n)
}

/// Characters `start..end` replaced by `inserted` characters that continue the style before
/// `start` (typing).
pub fn type_into_runs(runs: &[TextRun], start: u32, end: u32, inserted: u32) -> Vec<TextRun> {
    let style = style_at(runs, start);
    replace_runs(runs, start, end, vec![TextRun { chars: inserted, style }])
}

/// Characters `start..end` restyled by `f`, the runs split at both ends and joined again.
pub fn restyle_runs(runs: &[TextRun], start: u32, end: u32, f: impl Fn(&mut CharStyle)) -> Vec<TextRun> {
    let total: u32 = runs.iter().map(|r| r.chars).fold(0, u32::saturating_add);
    let (start, end) = (start.min(total), end.clamp(start.min(total), total));
    let mut mid = slice_runs(runs, start, end);
    for r in &mut mid {
        f(&mut r.style);
    }
    replace_runs(runs, start, end, mid)
}

/// Paragraph styles after characters `start..end` of `old` are replaced by `inserted`: a
/// paragraph a line break splits gives both halves its style; paragraphs joined keep the
/// first's.
pub fn edit_paragraphs(paras: &[ParagraphStyle], old: &str, start: u32, end: u32, inserted: &str) -> Vec<ParagraphStyle> {
    let count = paragraph_count(old);
    let fill = paras.last().copied().unwrap_or_default();
    let full: Vec<ParagraphStyle> = (0..count).map(|i| paras.get(i).copied().unwrap_or(fill)).collect();
    let before = old.chars().take(start as usize).filter(|c| *c == '\n').count();
    let removed = old.chars().skip(start as usize).take(end.saturating_sub(start) as usize).filter(|c| *c == '\n').count();
    let added = inserted.chars().filter(|c| *c == '\n').count();
    let here = full.get(before).copied().unwrap_or(fill);
    let mut out: Vec<ParagraphStyle> = full.iter().take(before + 1).copied().collect();
    out.extend(std::iter::repeat_n(here, added));
    out.extend(full.iter().skip(before + 1 + removed).copied());
    out
}

/// The one edit that turns `old` into `new`: the characters `start..old_end` of `old` became
/// `start..new_end` of `new` (the longest common start and end kept).
pub fn diff_chars(old: &str, new: &str) -> (u32, u32, u32) {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let room = a.len().min(b.len()) - prefix;
    let suffix = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
    (prefix as u32, (a.len() - suffix) as u32, (b.len() - suffix) as u32)
}

/// What characters `start..end` share, attribute by attribute (`None`: mixed). An empty range
/// is the style typing there would use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleSummary {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<Rgba>,
    pub letter_spacing: Option<f32>,
    pub shift: Option<BaselineShift>,
    pub underline: Option<bool>,
    pub url: Option<String>,
}

impl StyleSummary {
    /// The summary of one style (nothing mixed).
    pub fn of(s: &CharStyle) -> StyleSummary {
        StyleSummary {
            font: Some(s.font.clone()),
            size: Some(s.size),
            bold: Some(s.bold),
            italic: Some(s.italic),
            color: Some(s.color),
            letter_spacing: Some(s.letter_spacing),
            shift: Some(s.shift),
            underline: Some(s.underline),
            url: Some(s.url.clone()),
        }
    }
}

/// What characters `start..end` of `runs` share ([`StyleSummary`]).
pub fn summarize(runs: &[TextRun], start: u32, end: u32) -> StyleSummary {
    if start >= end {
        return StyleSummary::of(&style_at(runs, start));
    }
    let part = slice_runs(runs, start, end);
    let Some(first) = part.first() else { return StyleSummary::of(&style_at(runs, start)) };
    let mut s = StyleSummary::of(&first.style);
    for r in part.iter().skip(1) {
        let st = &r.style;
        fn keep<T: PartialEq>(v: &mut Option<T>, x: &T) {
            if v.as_ref().is_some_and(|v| v != x) {
                *v = None;
            }
        }
        keep(&mut s.font, &st.font);
        keep(&mut s.size, &st.size);
        keep(&mut s.bold, &st.bold);
        keep(&mut s.italic, &st.italic);
        keep(&mut s.color, &st.color);
        keep(&mut s.letter_spacing, &st.letter_spacing);
        keep(&mut s.shift, &st.shift);
        keep(&mut s.underline, &st.underline);
        keep(&mut s.url, &st.url);
    }
    s
}

/// The paragraphs characters `start..end` of `text` touch (indices, first to last).
pub fn paragraphs_in(text: &str, start: u32, end: u32) -> std::ops::RangeInclusive<usize> {
    let first = text.chars().take(start as usize).filter(|c| *c == '\n').count();
    let last = text.chars().take(end.max(start) as usize).filter(|c| *c == '\n').count();
    first..=last
}

#[cfg(test)]
#[path = "rich_text_tests.rs"]
mod tests;
