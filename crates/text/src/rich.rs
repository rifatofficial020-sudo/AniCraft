//! Styled text (ADR 0085): runs of characters, each in its own face, size, spacing, baseline
//! shift and underline, in paragraphs with their own alignment, indent, margins and line
//! spacing. Each run is shaped on its own with `harfrust`, lines break across runs, and the
//! glyphs come out grouped by the run's `paint` (the caller's colour and link), so a block with
//! several colours draws as one filled path per colour.
//!
//! Text that mixes directions (ADR 0130): a run is shaped in stretches of one direction and one
//! script, lines are found in reading order and each is then put in the order it is displayed
//! ([`crate::lines`]); a cluster its run's face has no glyph for is shaped again in a fallback
//! face ([`crate::fallback`]).
//!
//! One run in one paragraph lays out exactly as [`crate::Fonts::layout`] always has: the same
//! outline, box, lines and carets.

use crate::fallback::{Coverage, MAX_FALLBACK_FACES, MAX_WIDE_SEARCHES, needs_glyph, pick_face};
use crate::lines::{Cluster, Scripts, break_lines, place, stops};
use crate::{Align, CaretBox, CaretLine, Fonts, Layout, MAX_CHARS, Pen, PlacedGlyph, TextError};
use anicraft_geom::{Path, PathCmd, Vec2};
use craft_text::bidi;
use skrifa::MetadataProvider as _;
use skrifa::raw::TableProvider as _;
use std::collections::HashMap;
use std::ops::Range;
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
#[derive(Clone)]
struct Shaped {
    outline: Option<Arc<Path>>,
    /// The character its cluster starts at (of the whole text).
    index: u32,
    run: u32,
    x_offset: f32,
    y_offset: f32,
    advance: f32,
    /// Its face has no glyph for it: this is the face's "notdef" box.
    notdef: bool,
}

/// A stretch of one paragraph in one run, one direction and one script: what is shaped on its
/// own.
struct Segment {
    para: usize,
    run: usize,
    /// Its bytes and its characters in the text (from the first, up to the second).
    bytes: (usize, usize),
    chars: (u32, u32),
    /// Its embedding level: odd reads right to left.
    level: u8,
    script: Option<harfrust::Script>,
}

/// A laid-out line: its paragraph, whether it is the paragraph's first, its width, its glyphs
/// in the order they are displayed with their pen x (from the line's left end), and where its
/// clusters stand.
struct Line {
    para: usize,
    first: bool,
    width: f32,
    glyphs: Vec<(Shaped, f32)>,
    rtl: bool,
    boxes: Vec<CaretBox>,
}

/// One paragraph: its first character, its length in characters, its byte range (without a
/// trailing `\r`) and its embedding level (odd: it reads right to left).
struct Para {
    start: u32,
    len: u32,
    bytes: (usize, usize),
    level: u8,
}

/// A face ready to shape with.
struct Face<'a> {
    font: skrifa::FontRef<'a>,
    shaper: harfrust::Shaper<'a>,
    outlines: skrifa::outline::OutlineGlyphCollection<'a>,
    /// Outlines drawn so far, by glyph and size.
    cache: HashMap<(u32, u32), Option<Arc<Path>>>,
}

/// A stretch of text to shape in one face.
struct Job<'t> {
    text: &'t str,
    /// Its paragraph's bytes (the letters around the stretch are its context) and its own.
    para: (usize, usize),
    bytes: (usize, usize),
    first_char: u32,
    run: u32,
    rtl: bool,
    script: Option<harfrust::Script>,
    /// The size its glyphs are drawn at, and the extra space after each cluster (pixels).
    size: f32,
    letter_spacing: f32,
}

/// A cluster (or several in a row) its run's face has no glyph for, to shape in a fallback
/// face: its segment, the glyphs it replaces there, its characters and the fallback face.
struct Hole {
    seg: usize,
    glyphs: Range<usize>,
    chars: (u32, u32),
    face: usize,
}

/// Shapes a stretch of text. Glyphs come back in the order they are displayed (left to right),
/// as the shaper gives them, each with the character its cluster starts at.
fn shape(face: &mut Face<'_>, job: &Job<'_>) -> Vec<Shaped> {
    let Some(piece) = job.text.get(job.bytes.0..job.bytes.1) else { return Vec::new() };
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
    let size = skrifa::instance::Size::new(job.size);
    let upem = f32::from(skrifa::MetadataProvider::metrics(&face.font, size, skrifa::instance::LocationRef::default()).units_per_em.max(1));
    let scale = job.size / upem;
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(piece);
    // The letters around it in its paragraph: Arabic letters join across a change of style or
    // face as they do inside one.
    if let Some(before) = job.text.get(job.para.0..job.bytes.0) {
        buffer.set_pre_context(before);
    }
    if let Some(after) = job.text.get(job.bytes.1..job.para.1) {
        buffer.set_post_context(after);
    }
    buffer.set_direction(if job.rtl { harfrust::Direction::RightToLeft } else { harfrust::Direction::LeftToRight });
    if let Some(script) = job.script {
        buffer.set_script(script);
    }
    buffer.guess_segment_properties();
    let out = face.shaper.shape(buffer, harfrust::ShapeOptions::new());
    let mut list: Vec<Shaped> = Vec::with_capacity(out.glyph_infos().len());
    for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
        let local = to_char.get(info.cluster as usize).copied().unwrap_or(n).min(n);
        let outlines = &face.outlines;
        let outline = face
            .cache
            .entry((info.glyph_id, job.size.to_bits()))
            .or_insert_with(|| {
                let g = outlines.get(skrifa::GlyphId::new(info.glyph_id))?;
                let mut p = Path::new();
                let settings = skrifa::outline::DrawSettings::unhinted(size, skrifa::instance::LocationRef::default());
                let mut pen = Pen { cmds: &mut p.cmds, x: 0.0, y: 0.0 };
                let _ = g.draw(settings, &mut pen);
                Some(Arc::new(p))
            })
            .clone();
        list.push(Shaped { outline, index: job.first_char + local, run: job.run, x_offset: pos.x_offset as f32 * scale, y_offset: pos.y_offset as f32 * scale, advance: pos.x_advance as f32 * scale, notdef: info.glyph_id == 0 });
    }
    // Letter spacing follows each cluster, not each glyph: a letter's marks stay on it.
    let mut i = 0;
    while let Some(index) = list.get(i).map(|g| g.index) {
        if list.get(i + 1).is_none_or(|next| next.index != index)
            && let Some(g) = list.get_mut(i)
        {
            g.advance += job.letter_spacing;
        }
        i += 1;
    }
    list
}

/// The clusters of one segment's glyphs, in reading order: each one's glyphs (a range of
/// `glyphs`) and its characters, from its first up to the next cluster's (`end` for the last).
fn clusters_of(glyphs: &[Shaped], end: u32) -> Vec<(Range<usize>, u32, u32)> {
    let mut out: Vec<(Range<usize>, u32, u32)> = Vec::new();
    let mut i = 0;
    while let Some(g) = glyphs.get(i) {
        let mut j = i + 1;
        while glyphs.get(j).is_some_and(|next| next.index == g.index) {
            j += 1;
        }
        out.push((i..j, g.index, end));
        i = j;
    }
    // Right-to-left glyphs come last character first.
    out.sort_by_key(|c| c.1);
    for k in 1..out.len() {
        let next = out.get(k).map(|c| c.1);
        if let (Some(c), Some(next)) = (out.get_mut(k - 1), next) {
            c.2 = next;
        }
    }
    out
}

impl Fonts {
    /// Opens face `id` to shape with it.
    fn with_face<R>(&self, id: fontdb::ID, f: impl FnOnce(&mut Face<'_>) -> R) -> Option<R> {
        self.db
            .with_face_data(id, |data, index| {
                let font = skrifa::FontRef::from_index(data, index).ok()?;
                let shaper_data = harfrust::ShaperData::new(&font);
                let mut face = Face { shaper: shaper_data.shaper(&font).build(), outlines: font.outline_glyphs(), font, cache: HashMap::new() };
                Some(f(&mut face))
            })
            .flatten()
    }

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
        // Embedding levels (UAX #9), when anything in the text reads right to left.
        let levels = Some(text).filter(|t| !t.is_ascii()).map(|t| bidi::Paragraph::new(t, bidi::Direction::Auto)).filter(bidi::Paragraph::needs_reordering);

        // Paragraphs at line breaks.
        let mut paras: Vec<Para> = Vec::new();
        let (mut byte, mut ch) = (0usize, 0u32);
        for raw in text.split('\n') {
            let body = raw.strip_suffix('\r').unwrap_or(raw);
            let len = body.chars().count() as u32;
            let level = levels.as_ref().map_or(0, |l| l.paragraph_level_at(byte));
            paras.push(Para { start: ch, len, bytes: (byte, byte + body.len()), level });
            byte += raw.len() + 1;
            ch += raw.chars().count() as u32 + 1;
        }
        // Byte offset of every character (and of the end).
        let mut byte_of: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        byte_of.push(text.len());
        let byte_at = |c: u32| byte_of.get(c as usize).copied().unwrap_or(text.len());
        // Segments: each paragraph cut at run boundaries (both in order: one pass over each),
        // and each of those where the direction or the script changes.
        let mut scripts = Scripts::new();
        let mut segs: Vec<Segment> = Vec::new();
        let mut ri = 0usize;
        for (pi, p) in paras.iter().enumerate() {
            let (s, e) = (p.start, p.start + p.len);
            let plain = text.get(p.bytes.0..p.bytes.1).is_none_or(str::is_ascii);
            let script_of = if plain { Vec::new() } else { scripts.of(chars.get(s as usize..e as usize).unwrap_or(&[])) };
            let kind = |c: u32| (levels.as_ref().map_or(0, |l| l.level_at(byte_at(c))), script_of.get((c - s) as usize).copied().flatten());
            while runs.get(ri).is_some_and(|r| r.1 <= s) && ri + 1 < runs.len() {
                ri += 1;
            }
            let mut k = ri;
            while let Some((rs, re, _)) = runs.get(k) {
                if *rs >= e {
                    break;
                }
                let (a, b) = (s.max(*rs), e.min(*re));
                let mut from = a;
                while from < b {
                    let (level, script) = kind(from);
                    let mut to = from + 1;
                    while to < b && kind(to) == (level, script) {
                        to += 1;
                    }
                    segs.push(Segment { para: pi, run: k, bytes: (byte_at(from), byte_at(to)), chars: (from, to), level, script });
                    from = to;
                }
                k += 1;
            }
        }
        // The segments of each run.
        let mut segs_of_run: Vec<Vec<usize>> = vec![Vec::new(); runs.len()];
        for (si, seg) in segs.iter().enumerate() {
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
        let job_for = |seg: &Segment, bytes: (usize, usize), first_char: u32, m: &RunMetrics| -> Job<'_> {
            let para = paras.get(seg.para).map_or(bytes, |p| p.bytes);
            let letter_spacing = runs.get(seg.run).map_or(0.0, |r| r.2.letter_spacing);
            Job { text, para, bytes, first_char, run: seg.run as u32, rtl: seg.level % 2 == 1, script: seg.script, size: m.glyph_size, letter_spacing }
        };
        let mut shaped: Vec<Vec<Shaped>> = (0..segs.len()).map(|_| Vec::new()).collect();
        let mut metrics: Vec<RunMetrics> = vec![RunMetrics::default(); runs.len()];
        for (fi, (id, _)) in faces.iter().enumerate() {
            let ok = self.with_face(*id, |face| {
                let os2 = face.font.os2().ok();
                for &ri in runs_of_face.get(fi).map_or(&[][..], Vec::as_slice) {
                    let Some((_, _, st)) = runs.get(ri) else { continue };
                    let m = run_metrics(&face.font, os2.as_ref(), st);
                    if let Some(slot) = metrics.get_mut(ri) {
                        *slot = m;
                    }
                    for &si in segs_of_run.get(ri).map_or(&[][..], Vec::as_slice) {
                        let Some(seg) = segs.get(si) else { continue };
                        let list = shape(face, &job_for(seg, seg.bytes, seg.chars.0, &m));
                        if let Some(slot) = shaped.get_mut(si) {
                            *slot = list;
                        }
                    }
                }
            });
            if ok.is_none() {
                return Err(TextError::NoFonts);
            }
        }

        // Fallback faces (ADR 0130): a cluster its run's face has no glyph for goes, whole, to
        // the first fallback face that has all of it, and is shaped again there.
        let mut missing = 0usize;
        let mut fallback_ids: Vec<fontdb::ID> = Vec::new();
        let mut holes: Vec<Hole> = Vec::new();
        if shaped.iter().flatten().any(|g| g.notdef) {
            let mut cover = Coverage::new(self);
            let mut lists: HashMap<(bool, bool), Vec<fontdb::ID>> = HashMap::new();
            let mut searches = 0usize;
            for (si, seg) in segs.iter().enumerate() {
                let Some(list) = shaped.get(si).filter(|l| l.iter().any(|g| g.notdef)) else { continue };
                let Some((_, _, st)) = runs.get(seg.run) else { continue };
                let Some(own) = face_of_run.get(seg.run).and_then(|f| faces.get(*f)).map(|f| f.0) else { continue };
                let mut open: Option<Hole> = None;
                for (g, c0, c1) in clusters_of(list, seg.chars.1) {
                    let lacking = list.get(g.clone()).is_some_and(|gs| gs.iter().any(|x| x.notdef));
                    let drawn: Vec<char> = if lacking { chars.get(c0 as usize..c1 as usize).unwrap_or(&[]).iter().copied().filter(|c| needs_glyph(*c)).collect() } else { Vec::new() };
                    let had = drawn.iter().filter(|c| cover.has(own, **c)).count();
                    let mut chosen: Option<usize> = None;
                    if had < drawn.len() {
                        let candidates = lists.entry((st.bold, st.italic)).or_insert_with(|| self.fallback_list(st.bold, st.italic).iter().copied().filter(|id| *id != own).collect());
                        let mut pick = pick_face(&drawn, had, candidates.len(), &mut |f, c| candidates.get(f).is_some_and(|id| cover.has(*id, c)));
                        let lacks = |id: Option<fontdb::ID>, cover: &mut Coverage| drawn.iter().copied().find(|c| !cover.has(id.unwrap_or(own), *c));
                        // No listed face has all of it: any installed face with the letter.
                        if let Some(c) = lacks(pick.and_then(|f| candidates.get(f).copied()), &mut cover)
                            && searches < MAX_WIDE_SEARCHES
                        {
                            searches += 1;
                            if let Some(id) = self.any_face_with(c).filter(|id| *id != own && !candidates.contains(id)) {
                                candidates.push(id);
                                pick = pick_face(&drawn, had, candidates.len(), &mut |f, c| candidates.get(f).is_some_and(|id| cover.has(*id, c)));
                            }
                        }
                        chosen = pick.and_then(|f| candidates.get(f).copied()).and_then(|id| match fallback_ids.iter().position(|known| *known == id) {
                            Some(k) => Some(k),
                            None if fallback_ids.len() < MAX_FALLBACK_FACES => {
                                fallback_ids.push(id);
                                Some(fallback_ids.len() - 1)
                            }
                            None => None,
                        });
                        let face = chosen.and_then(|k| fallback_ids.get(k).copied()).unwrap_or(own);
                        missing += drawn.iter().filter(|c| !cover.has(face, **c)).count();
                    }
                    match (&mut open, chosen) {
                        (Some(h), Some(f)) if h.face == f && h.chars.1 == c0 => {
                            h.chars.1 = c1;
                            h.glyphs = h.glyphs.start.min(g.start)..h.glyphs.end.max(g.end);
                        }
                        _ => {
                            holes.extend(open.take());
                            open = chosen.map(|face| Hole { seg: si, glyphs: g, chars: (c0, c1), face });
                        }
                    }
                }
                holes.extend(open);
            }
        }
        let mut patches: Vec<(usize, Range<usize>, Vec<Shaped>)> = Vec::new();
        for (k, id) in fallback_ids.iter().enumerate() {
            let done = self.with_face(*id, |face| {
                let mut out = Vec::new();
                for h in holes.iter().filter(|h| h.face == k) {
                    let (Some(seg), Some(m)) = (segs.get(h.seg), segs.get(h.seg).and_then(|s| metrics.get(s.run))) else { continue };
                    out.push((h.seg, h.glyphs.clone(), shape(face, &job_for(seg, (byte_at(h.chars.0), byte_at(h.chars.1)), h.chars.0, m))));
                }
                out
            });
            // A face that can't be read after all leaves its clusters as boxes.
            patches.extend(done.unwrap_or_default());
        }
        // Last glyphs first, so the ranges before them stay where they are.
        patches.sort_by_key(|p| std::cmp::Reverse((p.0, p.1.start)));
        for (si, range, list) in patches {
            if let Some(slot) = shaped.get_mut(si)
                && range.start <= range.end
                && range.end <= slot.len()
            {
                slot.splice(range, list);
            }
        }
        let fallbacks: Vec<String> = fallback_ids.iter().map(|id| self.family_of(*id)).collect();

        // Paragraph styles, sanitized; missing ones take the last one's.
        let para_style = |pi: usize| -> ParaStyle { rich.paragraphs.get(pi).or(rich.paragraphs.last()).map(sanitize_para).unwrap_or_default() };
        let width = rich.width.filter(|w| w.is_finite() && *w > 0.0).map(|w| w.min(100_000.0));

        // Lines: each paragraph's clusters in reading order, wrapped at spaces to its room if
        // there is a width; then each line in the order it is displayed.
        let mut lines: Vec<Line> = Vec::new();
        let mut seg_iter = segs.iter().zip(shaped).peekable();
        for (pi, p) in paras.iter().enumerate() {
            let ps = para_style(pi);
            let mut para_glyphs: Vec<Shaped> = Vec::new();
            let mut clusters: Vec<Cluster> = Vec::new();
            while seg_iter.peek().is_some_and(|(seg, _)| seg.para == pi) {
                let Some((seg, list)) = seg_iter.next() else { break };
                let base = para_glyphs.len();
                for (g, start, end) in clusters_of(&list, seg.chars.1) {
                    let advance = list.get(g.clone()).map_or(0.0, |gs| gs.iter().map(|x| x.advance).sum());
                    let first = chars.get(start as usize).copied().unwrap_or(' ');
                    clusters.push(Cluster { glyphs: base + g.start..base + g.end, start, end, first, space: first.is_whitespace(), advance, level: seg.level });
                }
                para_glyphs.extend(list);
            }
            let room = |first: bool| width.map(|w| w - ps.left_margin - ps.right_margin - if first { ps.indent } else { 0.0 });
            for (n, range) in break_lines(&clusters, &room).into_iter().enumerate() {
                let mut line = Line { para: pi, first: n == 0, width: 0.0, glyphs: Vec::new(), rtl: p.level % 2 == 1, boxes: Vec::new() };
                for (k, at) in place(&clusters, range, p.level) {
                    let mut x = at.x0;
                    for g in clusters.get(k).and_then(|c| para_glyphs.get(c.glyphs.clone())).unwrap_or(&[]) {
                        line.glyphs.push((g.clone(), x));
                        x += g.advance;
                    }
                    line.width = at.x1;
                    line.boxes.push(at);
                }
                lines.push(line);
            }
        }

        // Each line's room above and below its baseline: the most any run on it needs (an
        // empty line: the run its paragraph starts in). `(ascent, descent, line height)`, the
        // height with the paragraph's line spacing. A fallback face's letters sit on the line
        // their run's face makes.
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
            fallbacks,
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
        let boxes: Vec<CaretBox> = l.boxes.iter().map(|b| CaretBox { start: ch(b.start), end: ch(b.end), x0: shift + b.x0, x1: shift + b.x1, rtl: b.rtl }).collect();
        let start = if l.first { pstart } else { boxes.iter().map(|b| b.start).min().unwrap_or(pstart) };
        // Where the line starts: its left end, or its right end in a right-to-left paragraph.
        let edge = shift + if l.rtl { l.width } else { 0.0 };
        let mut stops = stops(&boxes, (start, edge));
        if l.first && stops.first().is_none_or(|s| s.0 > start) {
            stops.insert(0, (start, edge));
        }
        let m = lm.get(row).copied().unwrap_or_default();
        let top = tops.get(row).copied().unwrap_or(0.0);
        let baseline = if uniform { first_ascent + top } else { top + m.0 };
        out.push(CaretLine { start, end: start, stops, top, height: m.2, baseline, rtl: l.rtl, boxes });
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
