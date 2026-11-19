//! Styled text (ADR 0085): runs of characters, each in its own face, size, spacing, baseline
//! shift and underline, in paragraphs with their own alignment, indent, margins and line
//! spacing. Each run is shaped on its own with `harfrust`, lines break across runs, and the
//! glyphs come out grouped by the run's `paint` (the caller's colour and link), so a block with
//! several colours draws as one filled path per colour.
//!
//! One run in one paragraph lays out exactly as [`crate::Fonts::layout`] always has: the same
//! outline, box, lines and carets.

use crate::{Align, CaretLine, Fonts, Layout, MAX_CHARS, Pen, PlacedGlyph, TextError};
use anicraft_geom::{Path, PathCmd, Vec2};
use skrifa::MetadataProvider as _;
use skrifa::raw::TableProvider as _;
use std::collections::HashMap;
use std::sync::Arc;

/// Most runs laid out in one block; more are joined into the last.
pub const MAX_RUNS: usize = 10_000;

/// Most different faces (family, bold, italic) in one block; runs past it use the first face.
pub const MAX_FACES: usize = 64;

/// Where a run sits against the line's baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shift {
    #[default]
    Normal,
    /// Smaller and raised, by the font's own superscript size and offset.
    Superscript,
    /// Smaller and lowered, by the font's own subscript size and offset.
    Subscript,
}

/// One run's character style.
#[derive(Clone, Debug, PartialEq)]
pub struct RunStyle {
    pub family: String,
    /// The em size in pixels (before a superscript or subscript makes it smaller).
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    /// Extra space after each character, in pixels.
    pub letter_spacing: f32,
    pub shift: Shift,
    pub underline: bool,
    /// The caller's paint for these glyphs (a colour and link): glyphs come out grouped by it.
    pub paint: u32,
}

impl Default for RunStyle {
    fn default() -> Self {
        RunStyle { family: String::new(), size: 32.0, bold: false, italic: false, letter_spacing: 0.0, shift: Shift::Normal, underline: false, paint: 0 }
    }
}

/// A run: this many characters (`char`s, line breaks included) in this style.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub chars: usize,
    pub style: RunStyle,
}

/// One paragraph's layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParaStyle {
    pub align: Align,
    /// The first line starts this much further in (pixels; negative hangs it out).
    pub indent: f32,
    /// Space kept clear at the box's left and right (pixels).
    pub left_margin: f32,
    pub right_margin: f32,
    /// Extra space below each of its lines (pixels), added to the font's line height.
    pub line_spacing: f32,
}

/// Text with its runs and paragraphs. Runs cover the text from its start (a shorter list leaves
/// the rest in the last run's style; a longer one is cut at the text's end); paragraphs are the
/// text's lines (split at `\n`), in order (missing ones take the last one's style).
#[derive(Clone, Copy, Debug)]
pub struct RichText<'a> {
    pub text: &'a str,
    pub runs: &'a [Run],
    pub paragraphs: &'a [ParaStyle],
    /// Lines wrap to this width (pixels); `None` keeps each paragraph on one line.
    pub width: Option<f32>,
}

fn fin(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

/// A run style clamped to sane ranges (sizes from 1 to 2000 px, spacing within ±1000 px).
pub fn sanitize_run(s: &RunStyle) -> RunStyle {
    RunStyle { size: fin(s.size, 32.0).clamp(1.0, 2000.0), letter_spacing: fin(s.letter_spacing, 0.0).clamp(-1000.0, 1000.0), ..s.clone() }
}

/// A paragraph style clamped to sane ranges (indent within ±10000 px, margins 0 to 10000 px,
/// line spacing within ±1000 px).
pub fn sanitize_para(p: &ParaStyle) -> ParaStyle {
    ParaStyle {
        align: p.align,
        indent: fin(p.indent, 0.0).clamp(-10_000.0, 10_000.0),
        left_margin: fin(p.left_margin, 0.0).clamp(0.0, 10_000.0),
        right_margin: fin(p.right_margin, 0.0).clamp(0.0, 10_000.0),
        line_spacing: fin(p.line_spacing, 0.0).clamp(-1000.0, 1000.0),
    }
}

/// A run's vertical metrics at its own size (pixels): what a line it is on must make room for.
#[derive(Clone, Copy, Debug, Default)]
struct RunMetrics {
    ascent: f32,
    /// Below the baseline, positive.
    descent: f32,
    /// The font's line height ((ascent − descent + leading), at least half the size).
    height: f32,
    /// The underline's top above the baseline (negative: below) and its thickness.
    underline: (f32, f32),
    /// How far a superscript or subscript moves the baseline (pixels, y down) and the size its
    /// glyphs are drawn at.
    shift_y: f32,
    glyph_size: f32,
}

/// One shaped glyph, ready to place.
struct Shaped {
    outline: Option<Arc<Path>>,
    /// Character it starts at (of the whole text).
    index: u32,
    run: u32,
    x_offset: f32,
    y_offset: f32,
    advance: f32,
    space: bool,
}

/// A stretch of one paragraph in one run: what is shaped on its own.
struct Segment {
    run: usize,
    /// Byte range in the text, and the character its start is.
    bytes: (usize, usize),
    first_char: u32,
}

/// A laid-out line: its paragraph, whether it is the paragraph's first, its width, its glyphs
/// with their pen x (from the line's start).
struct Line {
    para: usize,
    first: bool,
    width: f32,
    glyphs: Vec<(Shaped, f32)>,
}

/// One paragraph: its first character, its length in characters, and its byte range (without
/// a trailing `\r`).
struct Para {
    start: u32,
    len: u32,
    bytes: (usize, usize),
}

impl Fonts {
    /// Lays styled text out ([`RichText`]). `glyphs` also hands back each glyph on its own.
    pub fn layout_rich(&self, rich: &RichText, glyphs: bool) -> Result<Layout, TextError> {
        let text = rich.text;
        let n_chars = text.chars().count();
        if n_chars > MAX_CHARS {
            return Err(TextError::TooLong);
        }
        // Runs as character ranges over the whole text, sanitized.
        let mut runs: Vec<(u32, u32, RunStyle)> = Vec::new();
        let mut at = 0usize;
        for r in rich.runs.iter().take(MAX_RUNS) {
            if at >= n_chars && !runs.is_empty() {
                break;
            }
            let end = at.saturating_add(r.chars).min(n_chars);
            runs.push((at as u32, end as u32, sanitize_run(&r.style)));
            at = end;
        }
        if runs.is_empty() {
            runs.push((0, n_chars as u32, RunStyle::default()));
        }
        if let Some(last) = runs.last_mut() {
            last.1 = n_chars as u32;
        }
        // Faces: one per family, weight and slant.
        let mut faces: Vec<(fontdb::ID, String)> = Vec::new();
        let mut face_of_run: Vec<usize> = Vec::with_capacity(runs.len());
        let mut families: Vec<String> = Vec::with_capacity(runs.len());
        let mut keys: HashMap<(String, bool, bool), usize> = HashMap::new();
        for (_, _, st) in &runs {
            let family = self.resolve(&st.family).ok_or(TextError::NoFonts)?;
            let key = (family.clone(), st.bold, st.italic);
            let face = match keys.get(&key) {
                Some(&f) => f,
                None if faces.len() >= MAX_FACES => 0,
                None => {
                    let q = fontdb::Query {
                        families: &[fontdb::Family::Name(&family)],
                        weight: if st.bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
                        stretch: fontdb::Stretch::Normal,
                        style: if st.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
                    };
                    let id = self.db.query(&q).ok_or(TextError::NoFonts)?;
                    faces.push((id, family.clone()));
                    keys.insert(key, faces.len() - 1);
                    faces.len() - 1
                }
            };
            face_of_run.push(face);
            families.push(faces.get(face).map_or(family, |f| f.1.clone()));
        }
        let chars: Vec<char> = text.chars().collect();

        // Paragraphs at line breaks.
        let mut paras: Vec<Para> = Vec::new();
        let (mut byte, mut ch) = (0usize, 0u32);
        for raw in text.split('\n') {
            let body = raw.strip_suffix('\r').unwrap_or(raw);
            let len = body.chars().count() as u32;
            paras.push(Para { start: ch, len, bytes: (byte, byte + body.len()) });
            byte += raw.len() + 1;
            ch += raw.chars().count() as u32 + 1;
        }
        // Byte offset of every character (and of the end).
        let mut byte_of: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        byte_of.push(text.len());
        // Segments: each paragraph cut at run boundaries (both in order: one pass over each).
        let mut segs: Vec<(usize, Segment)> = Vec::new();
        let mut ri = 0usize;
        for (pi, p) in paras.iter().enumerate() {
            let (s, e) = (p.start, p.start + p.len);
            while runs.get(ri).is_some_and(|r| r.1 <= s) && ri + 1 < runs.len() {
                ri += 1;
            }
            let mut k = ri;
            while let Some((rs, re, _)) = runs.get(k) {
                if *rs >= e {
                    break;
                }
                let (a, b) = (s.max(*rs), e.min(*re));
                if a < b {
                    let bytes = (byte_of.get(a as usize).copied().unwrap_or(p.bytes.1), byte_of.get(b as usize).copied().unwrap_or(p.bytes.1));
                    segs.push((pi, Segment { run: k, bytes, first_char: a }));
                }
                k += 1;
            }
        }
        // The segments of each run.
        let mut segs_of_run: Vec<Vec<usize>> = vec![Vec::new(); runs.len()];
        for (si, (_, seg)) in segs.iter().enumerate() {
            if let Some(list) = segs_of_run.get_mut(seg.run) {
                list.push(si);
            }
        }
        // Shape every segment and read every run's metrics, face by face.
        let mut runs_of_face: Vec<Vec<usize>> = vec![Vec::new(); faces.len()];
        for (ri, f) in face_of_run.iter().enumerate() {
            if let Some(list) = runs_of_face.get_mut(*f) {
                list.push(ri);
            }
        }
        let mut shaped: Vec<Vec<Shaped>> = (0..segs.len()).map(|_| Vec::new()).collect();
        let mut metrics: Vec<RunMetrics> = vec![RunMetrics::default(); runs.len()];
        let mut missing = 0usize;
        for (fi, (id, _)) in faces.iter().enumerate() {
            let ok = self.db.with_face_data(*id, |data, index| {
                let font = skrifa::FontRef::from_index(data, index).ok()?;
                let shaper_data = harfrust::ShaperData::new(&font);
                let shaper = shaper_data.shaper(&font).build();
                let outlines = font.outline_glyphs();
                let charmap = font.charmap();
                let os2 = font.os2().ok();
                let mut cache: HashMap<(u32, u32), Option<Arc<Path>>> = HashMap::new();
                for &ri in runs_of_face.get(fi).map_or(&[][..], Vec::as_slice) {
                    let Some((rs, re, st)) = runs.get(ri) else { continue };
                    let m = run_metrics(&font, os2.as_ref(), st);
                    if let Some(slot) = metrics.get_mut(ri) {
                        *slot = m;
                    }
                    missing += chars.get(*rs as usize..*re as usize).unwrap_or(&[]).iter().filter(|c| !c.is_control() && !c.is_whitespace() && charmap.map(**c).is_none()).count();
                    let size = skrifa::instance::Size::new(m.glyph_size);
                    let upem = f32::from(skrifa::MetadataProvider::metrics(&font, size, skrifa::instance::LocationRef::default()).units_per_em.max(1));
                    let scale = m.glyph_size / upem;
                    for &si in segs_of_run.get(ri).map_or(&[][..], Vec::as_slice) {
                        let Some((_, seg)) = segs.get(si) else { continue };
                        let Some(piece) = text.get(seg.bytes.0..seg.bytes.1) else { continue };
                        let mut to_char = vec![0u32; piece.len() + 1];
                        let mut n = 0u32;
                        for (b, _) in piece.char_indices() {
                            if let Some(slot) = to_char.get_mut(b) {
                                *slot = n;
                            }
                            n += 1;
                        }
                        if let Some(last) = to_char.last_mut() {
                            *last = n;
                        }
                        let mut buffer = harfrust::UnicodeBuffer::new();
                        buffer.push_str(piece);
                        buffer.guess_segment_properties();
                        let out = shaper.shape(buffer, harfrust::ShapeOptions::new());
                        let mut list: Vec<Shaped> = Vec::with_capacity(out.glyph_infos().len());
                        for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
                            let local = to_char.get(info.cluster as usize).copied().unwrap_or(n).min(n);
                            let index = seg.first_char + local;
                            let outline = cache
                                .entry((info.glyph_id, m.glyph_size.to_bits()))
                                .or_insert_with(|| {
                                    let g = outlines.get(skrifa::GlyphId::new(info.glyph_id))?;
                                    let mut p = Path::new();
                                    let settings = skrifa::outline::DrawSettings::unhinted(size, skrifa::instance::LocationRef::default());
                                    let mut pen = Pen { cmds: &mut p.cmds, x: 0.0, y: 0.0 };
                                    let _ = g.draw(settings, &mut pen);
                                    Some(Arc::new(p))
                                })
                                .clone();
                            list.push(Shaped {
                                outline,
                                index,
                                run: ri as u32,
                                x_offset: pos.x_offset as f32 * scale,
                                y_offset: pos.y_offset as f32 * scale,
                                advance: pos.x_advance as f32 * scale + st.letter_spacing,
                                space: chars.get(index as usize).is_some_and(|c| c.is_whitespace()),
                            });
                        }
                        if let Some(slot) = shaped.get_mut(si) {
                            *slot = list;
                        }
                    }
                }
                Some(())
            });
            if ok.flatten().is_none() {
                return Err(TextError::NoFonts);
            }
        }

        // Paragraph styles, sanitized; missing ones take the last one's.
        let para_style = |pi: usize| -> ParaStyle { rich.paragraphs.get(pi).or(rich.paragraphs.last()).map(sanitize_para).unwrap_or_default() };
        let width = rich.width.filter(|w| w.is_finite() && *w > 0.0).map(|w| w.min(100_000.0));

        // Lines: each paragraph's glyphs, wrapped at spaces to its room if there is a width.
        let mut lines: Vec<Line> = Vec::new();
        let mut seg_iter = segs.into_iter().zip(shaped).peekable();
        for (pi, _) in paras.iter().enumerate() {
            let ps = para_style(pi);
            let mut run_glyphs: Vec<Shaped> = Vec::new();
            while let Some(((p, _), _)) = seg_iter.peek() {
                if *p != pi {
                    break;
                }
                if let Some((_, g)) = seg_iter.next() {
                    run_glyphs.extend(g);
                }
            }
            let room = |first: bool| width.map(|w| w - ps.left_margin - ps.right_margin - if first { ps.indent } else { 0.0 });
            let mut first = true;
            let mut line: Vec<(Shaped, f32)> = Vec::new();
            let mut x = 0.0f32;
            // Where the current line could break: after its last space.
            let mut break_at: Option<usize> = None;
            for g in run_glyphs {
                if let Some(w) = room(first)
                    && !g.space
                    && x + g.advance > w
                    && !line.is_empty()
                {
                    // Wrap: at the last space if there is one (dropping trailing spaces), else here.
                    let rest = match break_at {
                        Some(b) => line.split_off(b),
                        None => Vec::new(),
                    };
                    while line.last().is_some_and(|(g, _)| g.space) {
                        line.pop();
                    }
                    let lw = line.last().map_or(0.0, |(g, gx)| gx + g.advance);
                    lines.push(Line { para: pi, first, width: lw, glyphs: std::mem::take(&mut line) });
                    first = false;
                    let shift = rest.first().map_or(0.0, |(_, gx)| *gx);
                    line = rest.into_iter().map(|(g, gx)| (g, gx - shift)).collect();
                    x = line.last().map_or(0.0, |(g, gx)| gx + g.advance);
                    break_at = None;
                }
                line.push((g, x));
                x += line.last().map_or(0.0, |(g, _)| g.advance);
                if line.last().is_some_and(|(g, _)| g.space) {
                    break_at = Some(line.len());
                }
            }
            while line.last().is_some_and(|(g, _)| g.space) {
                line.pop();
            }
            let lw = line.last().map_or(0.0, |(g, gx)| gx + g.advance);
            lines.push(Line { para: pi, first, width: lw, glyphs: line });
        }

        // Each line's room above and below its baseline: the most any run on it needs (an
        // empty line: the run its paragraph starts in). `(ascent, descent, line height)`, the
        // height with the paragraph's line spacing.
        let run_at = |c: u32| runs.partition_point(|r| r.0 <= c).saturating_sub(1);
        let line_metrics = |l: &Line| -> (f32, f32, f32) {
            let mut list: Vec<usize> = l.glyphs.iter().map(|(g, _)| g.run as usize).collect();
            if list.is_empty() {
                let start = paras.get(l.para).map_or(0, |p| p.start);
                list.push(run_at(if start > 0 && start as usize >= n_chars { start - 1 } else { start }));
            }
            list.sort_unstable();
            list.dedup();
            let ls = para_style(l.para).line_spacing;
            if let [one] = list.as_slice() {
                let m = metrics.get(*one).copied().unwrap_or_default();
                return (m.ascent, m.descent, m.height + ls);
            }
            let (mut asc, mut below, mut desc) = (f32::MIN, f32::MIN, f32::MIN);
            for r in list {
                let m = metrics.get(r).copied().unwrap_or_default();
                asc = asc.max(m.ascent);
                below = below.max(m.height - m.ascent);
                desc = desc.max(m.descent);
            }
            (asc, desc, asc + below + ls)
        };
        let lm: Vec<(f32, f32, f32)> = lines.iter().map(line_metrics).collect();
        // Every line alike (one style): baselines a line height apart, as text always was.
        let uniform = lm.windows(2).all(|w| matches!(w, [a, b] if a == b));
        let mut tops: Vec<f32> = Vec::with_capacity(lines.len());
        let first = lm.first().copied().unwrap_or((0.0, 0.0, 0.0));
        let line_height = first.2;
        let height = if uniform {
            for row in 0..lines.len() {
                tops.push(row as f32 * line_height);
            }
            (lines.len().max(1) as f32 - 1.0) * line_height + (first.0 - -first.1)
        } else {
            let mut top = 0.0f32;
            for m in &lm {
                tops.push(top);
                top += m.2;
            }
            let last = lm.last().copied().unwrap_or(first);
            tops.last().copied().unwrap_or(0.0) + last.0 + last.1
        };

        let box_width = width.unwrap_or_else(|| {
            lines
                .iter()
                .map(|l| {
                    let ps = para_style(l.para);
                    ps.left_margin + if l.first { ps.indent } else { 0.0 } + l.width + ps.right_margin
                })
                .fold(0.0, f32::max)
        });
        let shift_of = |l: &Line| -> f32 {
            let ps = para_style(l.para);
            let start = ps.left_margin + if l.first { ps.indent } else { 0.0 };
            match ps.align {
                Align::Left => start,
                Align::Center => start + ((box_width - ps.right_margin - start) - l.width) * 0.5,
                Align::Right => box_width - ps.right_margin - l.width,
            }
        };

        // Draw: glyphs grouped by paint, in the order paints first appear.
        let mut pieces: Vec<(u32, Path)> = Vec::new();
        let mut piece_of: HashMap<u32, usize> = HashMap::new();
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        let paint_of = |r: u32| runs.get(r as usize).map_or(0, |x| x.2.paint);
        for (row, l) in lines.iter().enumerate() {
            let shift = shift_of(l);
            let top = tops.get(row).copied().unwrap_or(0.0);
            let base = if uniform { first.0 + top } else { top + lm.get(row).map_or(0.0, |m| m.0) };
            let mut underline: Option<(u32, f32, f32)> = None;
            let flush = |u: Option<(u32, f32, f32)>, pieces: &mut Vec<(u32, Path)>, piece_of: &mut HashMap<u32, usize>| {
                let Some((r, x0, x1)) = u else { return };
                let m = metrics.get(r as usize).copied().unwrap_or_default();
                let (top, thick) = m.underline;
                let rect = Path::rect(x0, base + m.shift_y - top, (x1 - x0).max(0.0), thick);
                let k = *piece_of.entry(paint_of(r)).or_insert_with(|| {
                    pieces.push((paint_of(r), Path::new()));
                    pieces.len() - 1
                });
                if let Some(p) = pieces.get_mut(k) {
                    p.1.cmds.extend(rect.cmds);
                }
            };
            for (g, gx) in &l.glyphs {
                let st = runs.get(g.run as usize).map(|r| &r.2);
                let m = metrics.get(g.run as usize).copied().unwrap_or_default();
                // Underlines: one bar per stretch of an underlined run on the line.
                let under = st.is_some_and(|s| s.underline);
                match underline {
                    Some((r, x0, _)) if under && r == g.run => underline = Some((r, x0, shift + gx + g.advance)),
                    _ => {
                        flush(underline.take(), &mut pieces, &mut piece_of);
                        if under {
                            underline = Some((g.run, shift + gx, shift + gx + g.advance));
                        }
                    }
                }
                let Some(outline) = &g.outline else { continue };
                let paint = paint_of(g.run);
                let k = *piece_of.entry(paint).or_insert_with(|| {
                    pieces.push((paint, Path::new()));
                    pieces.len() - 1
                });
                let (px, py) = (shift + gx + g.x_offset, base + m.shift_y - g.y_offset);
                if let Some(p) = pieces.get_mut(k) {
                    p.1.cmds.extend(outline.cmds.iter().map(|c| moved(c, px, py)));
                }
                if glyphs {
                    let (ox, oy) = (g.x_offset, m.shift_y - g.y_offset);
                    let own = Path { cmds: outline.cmds.iter().map(|c| moved(c, ox, oy)).collect() };
                    placed.push(PlacedGlyph { outline: own, x: shift + gx, baseline: base, advance: g.advance, index: g.index, paint });
                }
            }
            flush(underline.take(), &mut pieces, &mut piece_of);
        }
        let mut outline = Path::new();
        for (_, p) in &pieces {
            outline.cmds.extend_from_slice(&p.cmds);
        }
        let carets = caret_lines(&lines, &paras, &tops, &lm, uniform, first.0, &shift_of);
        Ok(Layout {
            outline,
            width: box_width.max(0.0),
            height: height.max(0.0),
            family: families.first().cloned().unwrap_or_default(),
            missing,
            lines: lines.len().max(1),
            line_height,
            ascent: first.0,
            carets,
            glyphs: placed,
            pieces,
            families,
        })
    }
}

/// A path command moved by `(dx, dy)`, added the way a pen at that position draws it.
fn moved(c: &PathCmd, dx: f32, dy: f32) -> PathCmd {
    let m = |p: Vec2| Vec2::new(dx + p.x, dy + p.y);
    match *c {
        PathCmd::MoveTo(p) => PathCmd::MoveTo(m(p)),
        PathCmd::LineTo(p) => PathCmd::LineTo(m(p)),
        PathCmd::QuadTo(a, p) => PathCmd::QuadTo(m(a), m(p)),
        PathCmd::CubicTo(a, b, p) => PathCmd::CubicTo(m(a), m(b), m(p)),
        PathCmd::Close => PathCmd::Close,
    }
}

/// A run's metrics in its face at its size.
fn run_metrics(font: &skrifa::FontRef, os2: Option<&skrifa::raw::tables::os2::Os2>, st: &RunStyle) -> RunMetrics {
    let size = skrifa::instance::Size::new(st.size);
    let m = skrifa::MetadataProvider::metrics(font, size, skrifa::instance::LocationRef::default());
    let em = f32::from(m.units_per_em.max(1));
    // The font's superscript and subscript size and offset (OS/2), else the usual ones.
    let (scale, raise) = match st.shift {
        Shift::Normal => (1.0, 0.0),
        Shift::Superscript => {
            let s = os2.map_or(0.0, |o| f32::from(o.y_superscript_y_size()) / em);
            let o = os2.map_or(0.0, |o| f32::from(o.y_superscript_y_offset()) / em);
            (if (0.3..=1.0).contains(&s) { s } else { 0.65 }, if (0.05..=1.0).contains(&o) { o } else { 0.35 })
        }
        Shift::Subscript => {
            let s = os2.map_or(0.0, |o| f32::from(o.y_subscript_y_size()) / em);
            let o = os2.map_or(0.0, |o| f32::from(o.y_subscript_y_offset()) / em);
            (if (0.3..=1.0).contains(&s) { s } else { 0.65 }, -(if (0.0..=1.0).contains(&o) && o > 0.0 { o } else { 0.15 }))
        }
    };
    let glyph_size = (st.size * scale).max(0.5);
    let underline = m.underline.filter(|d| d.thickness.is_finite() && d.thickness > 0.0 && d.offset.is_finite()).map_or((-0.1 * st.size, (0.05 * st.size).max(1.0)), |d| (d.offset, d.thickness));
    // A smaller underline for smaller glyphs.
    let underline = if scale < 1.0 { (underline.0 * scale, underline.1 * scale) } else { underline };
    RunMetrics {
        ascent: m.ascent,
        descent: -m.descent,
        height: (m.ascent - m.descent + m.leading).max(st.size * 0.5),
        underline,
        shift_y: -raise * st.size,
        glyph_size,
    }
}

/// The caret positions of laid-out lines (see [`CaretLine`]).
fn caret_lines(lines: &[Line], paras: &[Para], tops: &[f32], lm: &[(f32, f32, f32)], uniform: bool, first_ascent: f32, shift_of: &dyn Fn(&Line) -> f32) -> Vec<CaretLine> {
    let mut out: Vec<CaretLine> = Vec::with_capacity(lines.len());
    for (row, l) in lines.iter().enumerate() {
        let shift = shift_of(l);
        let (pstart, plen) = paras.get(l.para).map_or((0, 0), |p| (p.start, p.len));
        let ch = |i: u32| i.clamp(pstart, pstart + plen);
        let start = if l.first { pstart } else { l.glyphs.first().map_or(pstart, |(g, _)| ch(g.index)) };
        let mut stops: Vec<(u32, f32)> = l.glyphs.iter().map(|(g, gx)| (ch(g.index), shift + gx)).collect();
        stops.sort_by_key(|s| s.0);
        stops.dedup_by_key(|s| s.0);
        if l.first && stops.first().is_none_or(|s| s.0 > start) {
            stops.insert(0, (start, shift));
        }
        // The boundary after the last glyph (its trailing spaces, if any, follow it).
        if let Some(&(c, _)) = stops.last() {
            stops.push((c + 1, shift + l.width));
        }
        let m = lm.get(row).copied().unwrap_or_default();
        let top = tops.get(row).copied().unwrap_or(0.0);
        let baseline = if uniform { first_ascent + top } else { top + m.0 };
        out.push(CaretLine { start, end: start, stops, top, height: m.2, baseline });
        // The previous line of the same paragraph ends where this one starts.
        if !l.first && let Some(prev) = out.len().checked_sub(2).and_then(|i| out.get_mut(i)) {
            prev.end = start;
        }
    }
    // The last line of each paragraph ends with it.
    for (row, l) in lines.iter().enumerate() {
        let last_of_para = lines.get(row + 1).is_none_or(|n| n.para != l.para);
        if last_of_para && let (Some(c), Some(p)) = (out.get_mut(row), paras.get(l.para)) {
            c.end = p.start + p.len;
        }
    }
    for l in &mut out {
        l.end = l.end.max(l.start);
        let end = l.end;
        l.stops.retain(|s| s.0 <= end);
        if l.stops.last().is_none_or(|s| s.0 < end) {
            let x = l.stops.last().map_or(0.0, |s| s.1);
            l.stops.push((end, x));
        }
    }
    out
}

#[cfg(test)]
#[path = "rich_tests.rs"]
mod tests;
