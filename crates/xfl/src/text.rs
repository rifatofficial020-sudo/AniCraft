//! Text fields (`DOMStaticText`, `DOMDynamicText`, `DOMInputText`) → text blocks laid out by
//! `anicraft-text` (ADR 0014, ADR 0085).
//!
//! A field holds text runs (`<textRuns><DOMTextRun><characters>…</characters><textAttrs>
//! <DOMTextAttrs face="Arial-BoldMT" size="24" fillColor="#FF0000" alignment="center"
//! letterSpacing="1" lineSpacing="2"/>`). A field becomes one block with the same runs: each
//! `DOMTextRun`'s face, size, bold, italic, colour, letter spacing, character position
//! (superscript, subscript) and link are its characters' style, and each paragraph takes its
//! alignment, indent, margins and line spacing from the run it starts in, as Animate's
//! paragraph attributes do.
//!
//! Dynamic and input fields (ADR 0066) are one field each, with the same runs for their authored
//! text, their instance name, line type, border, selectability and maximum length; text a script
//! or a viewer changes is laid out in the field's first style.

use crate::import::Report;
use crate::xml::Element as X;
use anicraft_doc::{BaselineShift, CharStyle, LineType, ParagraphStyle, Rgba, TextAlign, TextBlock, TextField, TextKind, TextRun};
use anicraft_geom::{Affine, Path, Vec2};
use anicraft_text::{Align, FaceMatch, Fonts, TextError};
use std::collections::HashMap;
use std::sync::Arc;

/// The space between a text field's box and its text, on every side (pixels), as in Animate's
/// text fields.
pub const GUTTER: f32 = 2.0;

/// The line spacing of a run whose `DOMTextAttrs` leaves `lineSpacing` out (pixels). Animate
/// writes the attribute only when it differs from 2 px: it writes `lineSpacing="0"` for 0, a
/// multi-line field's saved height fits 2 px between its lines, and legacy binary `.fla` files
/// store 40 twips (2 px) for exactly the runs whose XFL leaves it out.
pub const LINE_SPACING: f32 = 2.0;

/// Most runs read from one field.
const MAX_RUNS: usize = anicraft_doc::MAX_TEXT_RUNS;

/// A laid-out block and where its box's top-left goes, in the field's coordinates.
pub struct Placed {
    pub block: TextBlock,
    pub offset: Vec2,
}

/// What a field used that AniCraft draws differently, reported once per field.
#[derive(Default)]
struct Flags {
    justified: bool,
}

/// One run's character and paragraph style, as `DOMTextAttrs` gives it (the face still a
/// PostScript name).
fn run_style(attrs: Option<craft_xfl::text::TextStyle<'_, X>>, url: &str, flags: &mut Flags) -> (CharStyle, ParagraphStyle) {
    let Some(a) = attrs else {
        return (CharStyle { size: 12.0, ..CharStyle::default() }, ParagraphStyle { line_spacing: LINE_SPACING, ..ParagraphStyle::default() });
    };
    let clamp = |number: Option<f32>, lo: f32, hi: f32| number.unwrap_or(0.0).clamp(lo, hi);
    let align = match a.alignment() {
        Some("center") => TextAlign::Center,
        Some("right") => TextAlign::Right,
        Some(j) if j.starts_with("justify") => {
            flags.justified = true;
            TextAlign::Left
        }
        _ => TextAlign::Left,
    };
    let shift = match a.character_position() {
        Some("superscript") => BaselineShift::Superscript,
        Some("subscript") => BaselineShift::Subscript,
        _ => BaselineShift::Normal,
    };
    let color = a.color();
    let c = CharStyle {
        font: a.face().unwrap_or("").chars().take(256).collect(),
        size: a.size().unwrap_or(12.0).clamp(1.0, 2000.0),
        bold: a.bold().unwrap_or(false),
        italic: a.italic().unwrap_or(false),
        color: Rgba::hex(color.rgb).with_alpha(color.alpha),
        letter_spacing: clamp(a.letter_spacing(), -1000.0, 1000.0),
        shift,
        underline: false,
        url: anicraft_doc::safe_url(url).unwrap_or_default(),
    };
    let p = ParagraphStyle {
        align,
        indent: clamp(a.indent(), -1000.0, 1000.0),
        left_margin: clamp(a.left_margin(), 0.0, 1000.0),
        right_margin: clamp(a.right_margin(), 0.0, 1000.0),
        line_spacing: a.line_spacing().unwrap_or(LINE_SPACING).clamp(-1000.0, 1000.0),
    };
    (c, p)
}

/// A field's text with its runs (faces resolved to installed families) and one style per
/// paragraph.
struct Styled {
    text: String,
    runs: Vec<TextRun>,
    paragraphs: Vec<ParagraphStyle>,
    /// Faces asked for that aren't installed (reported once the text is laid out).
    missing: Vec<String>,
}

/// Reads a field's runs. Fonts are looked up once per face name (`faces`); missing ones go into
/// the report.
fn read_runs(el: &X, native: &craft_xfl::text::TextField<'_, X>, fonts: &Fonts, faces: &mut HashMap<String, FaceMatch>, report: &mut Report, flags: &mut Flags) -> Styled {
    // Links aren't in the shared schema view: read them from the runs' attributes directly.
    let urls: Vec<String> = el.list("textRuns").iter().filter(|r| r.name == "DOMTextRun").map(|r| r.child("textAttrs").and_then(|t| t.child("DOMTextAttrs")).and_then(|a| a.attr("url")).unwrap_or("").to_string()).collect();
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut para_of_run: Vec<ParagraphStyle> = Vec::new();
    let mut chars = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut cut = native.run_entries_count() > MAX_RUNS;
    for (i, run) in native.runs().take(MAX_RUNS).enumerate() {
        let (mut c, p) = run_style(run.style(), urls.get(i).map_or("", String::as_str), flags);
        // The face: a PostScript name, a family or a device font; bold and italic faces count.
        let face = faces.entry(c.font.clone()).or_insert_with(|| fonts.find_face(&c.font)).clone();
        if face.family.is_none() && !face.wanted.is_empty() && !missing.contains(&face.wanted) {
            missing.push(face.wanted.clone());
        }
        c.font = face.family.clone().unwrap_or_default();
        c.bold |= face.bold;
        c.italic |= face.italic;
        // Animate ends lines with a carriage return; any line break starts a new line here.
        let piece = run.characters().replace("\r\n", "\n").replace(['\r', '\u{2028}', '\u{2029}'], "\n");
        let room = anicraft_text::MAX_CHARS.saturating_sub(chars);
        let n = piece.chars().count();
        if n > room {
            cut = true;
        }
        let piece: String = piece.chars().take(room).collect();
        let n = n.min(room);
        chars += n;
        text.push_str(&piece);
        runs.push(TextRun { chars: n as u32, style: c });
        para_of_run.push(p);
        if chars >= anicraft_text::MAX_CHARS {
            break;
        }
    }
    if cut {
        report.skip("text cut to 100,000 characters per field");
    }
    // Each paragraph takes the paragraph attributes of the run its first character is in.
    let mut paragraphs = Vec::new();
    let mut starts = vec![0usize];
    starts.extend(text.chars().enumerate().filter(|(_, c)| *c == '\n').map(|(i, _)| i + 1));
    let mut ends: Vec<usize> = Vec::with_capacity(runs.len());
    let mut at = 0usize;
    for r in &runs {
        at += r.chars as usize;
        ends.push(at);
    }
    for s in starts {
        let k = ends.partition_point(|e| *e <= s).min(para_of_run.len().saturating_sub(1));
        paragraphs.push(para_of_run.get(k).copied().unwrap_or_default());
    }
    Styled { text, runs, paragraphs, missing }
}

/// Lays a block out from its text, runs, paragraphs and width (the engine's text layout does
/// the same for edits; the importer sits below it, so this is its own copy of the few lines
/// that hand runs to the text engine).
fn lay_out(fonts: &Fonts, block: &mut TextBlock, shown: &str, wrap: Option<f32>) -> Result<anicraft_text::Layout, TextError> {
    let mut paints: Vec<(Rgba, String)> = Vec::new();
    let align = |a: TextAlign| match a {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    };
    let runs: Vec<anicraft_text::Run> = block
        .styled_runs()
        .into_iter()
        .map(|r| {
            let key = (r.style.color, r.style.url.clone());
            let paint = paints.iter().position(|k| *k == key).unwrap_or_else(|| {
                paints.push(key);
                paints.len() - 1
            }) as u32;
            let shift = match r.style.shift {
                BaselineShift::Normal => anicraft_text::Shift::Normal,
                BaselineShift::Superscript => anicraft_text::Shift::Superscript,
                BaselineShift::Subscript => anicraft_text::Shift::Subscript,
            };
            anicraft_text::Run { chars: r.chars as usize, style: anicraft_text::RunStyle { family: r.style.font, size: r.style.size, bold: r.style.bold, italic: r.style.italic, letter_spacing: r.style.letter_spacing, shift, underline: r.style.underline, paint } }
        })
        .collect();
    let paras: Vec<anicraft_text::ParaStyle> = block.paragraph_styles().into_iter().map(|p| anicraft_text::ParaStyle { align: align(p.align), indent: p.indent, left_margin: p.left_margin, right_margin: p.right_margin, line_spacing: p.line_spacing }).collect();
    let l = fonts.layout_rich(&anicraft_text::RichText { text: shown, runs: &runs, paragraphs: &paras, width: wrap }, false)?;
    // The runs record the families actually used.
    let mut styled = block.styled_runs();
    for (r, used) in styled.iter_mut().zip(&l.families) {
        r.style.font.clone_from(used);
    }
    let p = block.paragraph_styles();
    block.set_styles(styled, p);
    let pieces = l.pieces.iter().map(|(k, p)| paints.get(*k as usize).map_or((block.color, String::new(), p.clone()), |(c, u)| (*c, u.clone(), p.clone()))).collect();
    block.set_outline(pieces);
    Ok(l)
}

/// Lays out one text field: one block with the field's runs.
pub fn field(el: &X, fonts: &Fonts, faces: &mut HashMap<String, FaceMatch>, report: &mut Report) -> Vec<Placed> {
    let Some(native) = craft_xfl::text::TextField::new(el) else { return Vec::new() };
    if native.orientation().is_some_and(|o| o != "horizontal") {
        report.skip("vertical text (drawn horizontal)");
    }
    // Filters on text come across: the importer sets them on each block (ADR 0099).
    let mut flags = Flags::default();
    let styled = read_runs(el, &native, fonts, faces, report, &mut flags);
    if flags.justified {
        report.skip("justified text (drawn left-aligned)");
    }
    if native.kind() != craft_xfl::text::TextFieldKind::Static {
        return text_field(native, fonts, report, styled).into_iter().collect();
    }
    // Trailing empty lines draw nothing.
    let keep = styled.text.trim_end().chars().count();
    if keep == 0 || styled.runs.is_empty() {
        return Vec::new();
    }
    let text: String = styled.text.chars().take(keep).collect();
    let auto = native.auto_expand().unwrap_or(false);
    // A fixed-width field wraps inside its margins; an auto-size one never wraps.
    let width = if auto { None } else { native.width().filter(|w| *w > 0.0).map(|w| w.min(100_000.0)) };
    let mut block = TextBlock { text: text.clone(), width, ..TextBlock::default() };
    block.set_styles(styled.runs, styled.paragraphs);
    match lay_out(fonts, &mut block, &text, width) {
        Ok(l) => {
            // A family that isn't installed falls back to the default, as the text engine does
            // (ADR 0012), and the report names it.
            report.missing_fonts.extend(styled.missing);
            if !report.missing_fonts.is_empty() {
                report.fallback_font.get_or_insert_with(|| l.family.clone());
            }
            block.box_width = l.width;
            block.box_height = l.height;
        }
        Err(TextError::NoFonts) => {
            report.skip("text fields (no fonts on this computer)");
            return Vec::new();
        }
        Err(TextError::TooLong) => {
            report.skip("text cut to 100,000 characters per field");
            return Vec::new();
        }
    }
    let origin = Vec2::new(native.left().unwrap_or(0.0) + GUTTER, native.top().unwrap_or(0.0) + GUTTER);
    vec![Placed { block, offset: origin }]
}

/// A dynamic or input field (ADR 0066): one block with the field's runs, its box from the
/// field's `width` and `height` (or the text's size, for an auto-size field), its text laid out
/// inside the gutter. Empty fields are kept (an empty input field is the usual kind).
fn text_field(native: craft_xfl::text::TextField<'_, X>, fonts: &Fonts, report: &mut Report, styled: Styled) -> Option<Placed> {
    let kind = if native.kind() == craft_xfl::text::TextFieldKind::Input { TextKind::Input } else { TextKind::Dynamic };
    report.run(if kind == TextKind::Input { "input text fields" } else { "dynamic text fields" });
    let keep = styled.text.trim_end_matches('\n').chars().count();
    let text: String = styled.text.chars().take(keep).collect();
    let line_type = match native.line_type() {
        Some("multiline") => LineType::Multiline,
        Some("multiline no wrap") => LineType::MultilineNoWrap,
        Some("password") if kind == TextKind::Input => LineType::Password,
        _ => LineType::Single,
    };
    let border = native.border().unwrap_or(false);
    let variable: String = native.variable_name().unwrap_or("").trim().chars().take(anicraft_doc::MAX_FIELD_NAME).collect();
    if !variable.is_empty() {
        report.keep("text field variables (ActionScript 1.0 and 2.0)");
    }
    if native.render_as_html().unwrap_or(false) {
        report.skip("text fields rendered as HTML (drawn as plain text)");
    }
    let max_chars = native.max_characters().filter(|n| *n > 0.0).map_or(0, |n| n.min(anicraft_doc::MAX_FIELD_CHARS as f32) as u32);
    // Animate's "show border" draws a black line round a white box.
    let field = TextField { lines: line_type, selectable: native.is_selectable().unwrap_or(true), border, background: border, max_chars, variable, ..TextField::new(kind) };
    let auto = native.auto_expand().unwrap_or(false);
    let width = native.width().filter(|w| *w > 0.0).map(|w| w.min(100_000.0));
    let mut block = TextBlock { text: text.clone(), ..TextBlock::default() };
    block.set_styles(styled.runs, styled.paragraphs);
    let missing = styled.missing;
    let wrap = width.and_then(|w| field.wrap_width(w));
    // A fixed box that doesn't wrap still aligns its lines in the box.
    let lay_width = wrap.or(if auto { None } else { width.map(|w| (w - 2.0 * GUTTER).max(1.0)).filter(|_| block.align != TextAlign::Left) });
    let shown = field.shown(&text);
    let (tw, th) = match lay_out(fonts, &mut block, &shown, lay_width) {
        Ok(l) => {
            report.missing_fonts.extend(missing);
            if !report.missing_fonts.is_empty() {
                report.fallback_font.get_or_insert_with(|| l.family.clone());
            }
            let to = Affine::translate(GUTTER, GUTTER);
            let pieces: Vec<(Rgba, String, Path)> = block.fill_paths().into_iter().map(|(c, u, p)| (c, u.to_string(), p.transformed(&to))).collect();
            block.set_outline(pieces);
            (l.width, l.height)
        }
        Err(e) => {
            report.skip(if e == TextError::NoFonts { "text fields (no fonts on this computer)" } else { "text cut to 100,000 characters per field" });
            block.outline = Arc::new(Path::new());
            (0.0, block.size * 1.2)
        }
    };
    block.box_width = match (width, auto) {
        (Some(w), false) => w,
        (Some(w), true) => w.max(tw + 2.0 * GUTTER),
        (None, _) => tw + 2.0 * GUTTER,
    };
    block.box_height = native.height().filter(|h| *h > 0.0).map_or(th + 2.0 * GUTTER, |h| h.min(100_000.0));
    block.width = wrap;
    block.field = Some(field);
    Some(Placed { block, offset: Vec2::new(native.left().unwrap_or(0.0), native.top().unwrap_or(0.0)) })
}
