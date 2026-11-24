//! AniCraft text (layer 1): the fonts on this computer, and text laid out into vector outlines.
//!
//! `layout` shapes text with `harfrust` (a HarfBuzz port), breaks it into lines, and draws each
//! glyph's outline with `skrifa` into one [`Path`], in pixels, with the layout box's top-left
//! corner at the origin. The engine stores that path with the text element (ADR 0012), so
//! rendering never depends on which fonts are installed.
//!
//! [`Fonts::layout_glyphs`] also hands back each glyph on its own ([`PlacedGlyph`]), for text
//! placed glyph by glyph: along a path ([`on_path`], ADR 0070) or at SVG's per-character
//! positions.
//!
//! Text that mixes directions (Hebrew or Arabic with Latin words and numbers) follows the
//! Unicode bidi algorithm, and letters a run's face lacks come from a fallback face (ADR 0130).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use anicraft_geom::{Path, PathCmd, Vec2};
use std::sync::{Arc, OnceLock};

mod embed;
mod fallback;
mod lines;
mod rich;
#[cfg(test)]
mod testfont;
pub use embed::{EMBED_EM, Embedding, GlyphTable, MAX_EMBED_CHARS, TableGlyph};
pub use fallback::{FALLBACK_FAMILIES, MAX_FALLBACK_FACES, needs_glyph};
pub use rich::{MAX_FACES, MAX_RUNS, ParaStyle, RichText, Run, RunStyle, Shift, sanitize_para, sanitize_run};

/// Families tried, in order, when none is asked for or the one asked for isn't installed.
pub const DEFAULT_FAMILIES: &[&str] = &["Inter", "Arial", "Helvetica", "Helvetica Neue", "Segoe UI", "DejaVu Sans", "Liberation Sans", "Noto Sans", "Roboto", "Ubuntu"];

/// Installed families tried for the device font `_serif`, in order.
const SERIF_FAMILIES: &[&str] = &["Times New Roman", "Times", "Georgia", "DejaVu Serif", "Liberation Serif", "Noto Serif"];

/// Installed families tried for the device font `_typewriter`, in order.
const MONO_FAMILIES: &[&str] = &["Courier New", "Courier", "JetBrains Mono", "DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono", "Consolas", "Menlo"];

/// Largest text block laid out (characters).
pub const MAX_CHARS: usize = 100_000;

/// Horizontal alignment of each line in the block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// How to lay text out.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub family: String,
    /// Font size in pixels (the em square).
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub align: Align,
    /// Extra space between characters, in pixels.
    pub letter_spacing: f32,
    /// Extra space between lines, in pixels (added to the font's own line height).
    pub line_spacing: f32,
    /// Wrap lines to this width (pixels); `None` keeps each paragraph on one line.
    pub width: Option<f32>,
}

impl Default for Style {
    fn default() -> Self {
        Style { family: String::new(), size: 32.0, bold: false, italic: false, align: Align::Left, letter_spacing: 0.0, line_spacing: 0.0, width: None }
    }
}

/// Laid-out text.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Every glyph's outline, in pixels, top-left of the box at the origin (y down).
    pub outline: Path,
    /// The layout box: the given width (or the widest line) by the lines' height.
    pub width: f32,
    pub height: f32,
    /// The family actually used (the one asked for, or the fallback).
    pub family: String,
    /// Characters no face has a glyph for, neither the run's own nor a fallback (drawn as the
    /// run's face's "notdef" box).
    pub missing: usize,
    /// Lines laid out (at least one), and the distance between their baselines (pixels): a
    /// line that continued the block would start `lines × line_height` below its top.
    pub lines: usize,
    pub line_height: f32,
    /// From the top of the box to the first line's baseline (pixels), for placing text by its
    /// baseline, as SVG does.
    pub ascent: f32,
    /// Where a caret can stand, one entry per laid-out line, top first (ADR 0066: text fields
    /// that viewers type into).
    pub carets: Vec<CaretLine>,
    /// Each glyph on its own, in drawing order: only from [`Fonts::layout_glyphs`] (empty
    /// otherwise).
    pub glyphs: Vec<PlacedGlyph>,
    /// The outline cut by the runs' paints ([`RunStyle::paint`]), in the order paints first
    /// appear: `outline` is these pieces one after another (ADR 0085).
    pub pieces: Vec<(u32, Path)>,
    /// The family each run was laid out in (the one asked for, or the fallback).
    pub families: Vec<String>,
    /// The other families letters were drawn from, because their run's face lacks them, in
    /// the order they were first needed (ADR 0130).
    pub fallbacks: Vec<String>,
}

/// One laid-out glyph on its own.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedGlyph {
    /// Its outline, with its pen position on the baseline at the origin (pixels, y down).
    pub outline: Path,
    /// Its pen position in the block: from the box's left edge (alignment applied), and its
    /// line's baseline from the box's top.
    pub x: f32,
    pub baseline: f32,
    /// How far the pen moves past it (letter spacing included).
    pub advance: f32,
    /// The character it starts at (`char`s of the whole text, line breaks included).
    pub index: u32,
    /// Its run's paint ([`RunStyle::paint`]).
    pub paint: u32,
}

/// How text follows a path (ADR 0070).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPlacement {
    /// Where the text is anchored, as a distance along the path: left-aligned text starts
    /// there, centred text is centred on it, right-aligned text ends there.
    pub offset: f32,
    /// Along the path backwards: on its other side, upside down to the path's direction.
    pub flip: bool,
    pub align: Align,
}

/// Glyphs set along a path.
#[derive(Clone, Debug, PartialEq)]
pub struct PathText {
    /// Every glyph shown, in the path's space: `pieces` one after another.
    pub outline: Path,
    /// The glyphs by paint, in the order paints first appear.
    pub pieces: Vec<(u32, Path)>,
    /// The path's length.
    pub length: f32,
    /// Each shown glyph's character, the point of its line's baseline under its middle (on the
    /// path for the first line, on the curve parallel to it for further lines) and its angle
    /// (radians, the direction of travel).
    pub placed: Vec<(u32, Vec2, f32)>,
    /// Glyphs whose middle falls past an open path's ends (not drawn, as SVG leaves them).
    pub hidden: usize,
}

/// Most lines set along one path; glyphs on lines past it are left out (and counted as hidden).
pub const MAX_PATH_LINES: usize = 64;

/// Most samples of a path taken to measure a line parallel to it.
const PARALLEL_SAMPLES: usize = 2048;

/// The curve parallel to a walked path at a distance (ADR 0107): distances along the path
/// against distances along the parallel curve, sampled, so a further line of text is spaced by
/// its own length (tighter inside a bend, wider outside it) and not by the path's.
struct Parallel {
    /// The path's length, and the parallel curve's length up to each of `n + 1` even samples.
    length: f32,
    along: Vec<f32>,
}

impl Parallel {
    fn new(length: f32, point: &dyn Fn(f32) -> Vec2) -> Parallel {
        let n = ((length * 0.5).ceil() as usize).clamp(16, PARALLEL_SAMPLES);
        let mut along = Vec::with_capacity(n + 1);
        let (mut sum, mut prev) = (0.0f32, point(0.0));
        along.push(0.0);
        for i in 1..=n {
            let q = point(length * i as f32 / n as f32);
            let d = (q - prev).length();
            if d.is_finite() {
                sum += d;
            }
            along.push(sum);
            prev = q;
        }
        Parallel { length, along }
    }

    fn total(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0)
    }

    /// The distance along the parallel curve under distance `s` along the path (past the
    /// path's ends it carries on at the path's own pace).
    fn along_at(&self, s: f32) -> f32 {
        let n = self.along.len().saturating_sub(1).max(1);
        if s <= 0.0 {
            return s;
        }
        if s >= self.length {
            return self.total() + (s - self.length);
        }
        let f = s / self.length * n as f32;
        let i = (f.floor() as usize).min(n - 1);
        let (a, b) = (self.along.get(i).copied().unwrap_or(0.0), self.along.get(i + 1).copied().unwrap_or(0.0));
        a + (b - a) * (f - i as f32)
    }

    /// The distance along the path over distance `t` along the parallel curve (in range).
    fn path_at(&self, t: f32) -> f32 {
        let n = self.along.len().saturating_sub(1).max(1);
        let i = self.along.partition_point(|l| *l <= t).clamp(1, n);
        let (a, b) = (self.along.get(i - 1).copied().unwrap_or(0.0), self.along.get(i).copied().unwrap_or(0.0));
        let f = if b > a { ((t - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        self.length * ((i - 1) as f32 + f) / n as f32
    }
}

/// The glyphs of a layout set along `path`'s first subpath (ADR 0070): each glyph's middle at
/// its distance along the path (measured on the curve itself), the glyph turned to the path's
/// direction there, its baseline on the path, standing on the path's left (above a path drawn
/// left to right; `flip` walks it backwards, onto the other side). On a closed path glyphs wrap
/// round; on an open one those past its ends are left out. `None` when the path has no length.
///
/// Several lines (ADR 0107): glyphs with another baseline are a further line. The first line
/// with glyphs sits on the path; each further line runs parallel to it, its baseline as far
/// from the path as it is from the first line's in the layout, on the side away from the
/// glyphs' tops (below the first line, as lines of text are), and is spaced along that parallel
/// curve. Every line is anchored on its own width at the same place on the path. Lay the text
/// out left-aligned: the placement's alignment anchors the lines. At most [`MAX_PATH_LINES`].
pub fn on_path(glyphs: &[PlacedGlyph], path: &Path, how: PathPlacement) -> Option<PathText> {
    let walk = anicraft_geom::walk::CurveWalk::new(path)?;
    let length = walk.length();
    if !(length.is_finite() && length > 0.0) {
        return None;
    }
    let closed = first_subpath_closed(path);
    let offset = if how.offset.is_finite() { how.offset.clamp(-1e6, 1e6) } else { 0.0 };
    // A point of the path as the text walks it, and its direction there.
    let at = |s: f32| -> (Vec2, Vec2) { if how.flip { (walk.at(length - s), walk.tangent(length - s) * -1.0) } else { (walk.at(s), walk.tangent(s)) } };
    // The lines: glyphs come line by line, each line's with one baseline.
    let mut lines: Vec<(f32, f32)> = Vec::new();
    let mut line_of: Vec<usize> = Vec::with_capacity(glyphs.len());
    for g in glyphs {
        if lines.last().is_none_or(|l| l.0.to_bits() != g.baseline.to_bits()) {
            lines.push((g.baseline, 0.0));
        }
        let w = g.x + g.advance;
        if let Some(l) = lines.last_mut()
            && w.is_finite()
        {
            l.1 = l.1.max(w);
        }
        line_of.push(lines.len() - 1);
    }
    let first = lines.first().map_or(0.0, |l| l.0);
    // Each further line's parallel curve (the first line is the path itself).
    let parallels: Vec<Option<(f32, Parallel)>> = lines
        .iter()
        .take(MAX_PATH_LINES)
        .map(|(baseline, _)| {
            let d = baseline - first;
            (d.is_finite() && d != 0.0).then(|| {
                (
                    d,
                    Parallel::new(length, &|s| {
                        let (p, t) = at(s);
                        p + Vec2::new(-t.y, t.x) * d
                    }),
                )
            })
        })
        .collect();
    let mut out = PathText { outline: Path::new(), pieces: Vec::new(), length, placed: Vec::with_capacity(glyphs.len()), hidden: 0 };
    for (g, &row) in glyphs.iter().zip(&line_of) {
        let (Some(line), Some(parallel)) = (lines.get(row), parallels.get(row)) else {
            out.hidden += 1;
            continue;
        };
        let back = match how.align {
            Align::Left => 0.0,
            Align::Center => line.1 * 0.5,
            Align::Right => line.1,
        };
        // Where the glyph's middle is along its line, and how long that line is.
        let (anchor, total) = match parallel {
            None => (offset, length),
            Some((_, par)) => (par.along_at(if closed { offset.rem_euclid(length) } else { offset }), par.total()),
        };
        let mid = anchor - back + g.x + g.advance * 0.5;
        // A parallel curve squeezed to (almost) nothing, as at a circle's centre, holds no text.
        if !mid.is_finite() || !(total.is_finite() && total > length * 0.01) {
            out.hidden += 1;
            continue;
        }
        let t = if closed {
            mid.rem_euclid(total)
        } else if (0.0..=total).contains(&mid) {
            mid
        } else {
            out.hidden += 1;
            continue;
        };
        let (p, dir) = match parallel {
            None => at(t),
            Some((d, par)) => {
                let (p, dir) = at(par.path_at(t));
                (p + Vec2::new(-dir.y, dir.x) * *d, dir)
            }
        };
        let angle = dir.y.atan2(dir.x);
        let m = anicraft_geom::Affine::translate(p.x, p.y).mul(&anicraft_geom::Affine::rotate(angle)).mul(&anicraft_geom::Affine::translate(-g.advance * 0.5, 0.0));
        let k = match out.pieces.iter().position(|(paint, _)| *paint == g.paint) {
            Some(k) => k,
            None => {
                out.pieces.push((g.paint, Path::new()));
                out.pieces.len() - 1
            }
        };
        if let Some((_, piece)) = out.pieces.get_mut(k) {
            piece.cmds.extend(g.outline.transformed(&m).cmds);
        }
        out.placed.push((g.index, p, angle));
    }
    for (_, piece) in &out.pieces {
        out.outline.cmds.extend_from_slice(&piece.cmds);
    }
    Some(out)
}

/// Text on a path in a box of its own, as text blocks keep it (ADR 0070): the glyphs set along
/// the path, then the glyphs and the path moved together so their bounds start at the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxedPathText {
    pub outline: Path,
    /// The glyphs by paint (see [`PathText::pieces`]).
    pub pieces: Vec<(u32, Path)>,
    pub path: Path,
    pub width: f32,
    pub height: f32,
    /// Where the box's origin is in the path's space: what to translate by to put it back.
    pub shift: Vec2,
    pub hidden: usize,
}

/// [`on_path`], boxed ([`BoxedPathText`]).
pub fn on_path_boxed(glyphs: &[PlacedGlyph], path: &Path, how: PathPlacement) -> Option<BoxedPathText> {
    let set = on_path(glyphs, path, how)?;
    let b = set.outline.bounds().union(&path.bounds());
    let shift = if b.min.x.is_finite() && b.min.y.is_finite() { b.min } else { Vec2::ZERO };
    let back = anicraft_geom::Affine::translate(-shift.x, -shift.y);
    let size = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let pieces = set.pieces.iter().map(|(k, p)| (*k, p.transformed(&back))).collect();
    Some(BoxedPathText { outline: set.outline.transformed(&back), pieces, path: path.transformed(&back), width: size(b.width()), height: size(b.height()), shift, hidden: set.hidden })
}

/// Whether a path's first subpath ends with a close.
fn first_subpath_closed(path: &Path) -> bool {
    for (i, c) in path.cmds.iter().enumerate() {
        match c {
            PathCmd::Close => return true,
            PathCmd::MoveTo(_) if i > 0 => return false,
            _ => {}
        }
    }
    false
}

/// Where one cluster (a letter with its marks, or a ligature) stands on its line (ADR 0130).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CaretBox {
    /// The characters it covers: from `start` up to `end`.
    pub start: u32,
    pub end: u32,
    /// Its left and right edges.
    pub x0: f32,
    pub x1: f32,
    /// It reads right to left: `start` is at its right edge.
    pub rtl: bool,
}

/// One laid-out line's characters and caret positions. Indices count `char`s of the whole text
/// (line breaks included); `x` is from the box's left edge, alignment applied.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaretLine {
    /// The first character on the line and the boundary after its last (a wrapped line's
    /// trailing spaces belong to it; a line break belongs to neither line).
    pub start: u32,
    pub end: u32,
    /// `(index, x)` of the boundaries clusters start at and of the line's end, ascending by
    /// index. A cluster's boundary is at the edge it is read from, so in text that mixes
    /// directions `x` does not rise with the index.
    pub stops: Vec<(u32, f32)>,
    /// The paragraph reads right to left: the line starts at its right end.
    pub rtl: bool,
    /// The line's clusters as they are displayed, left to right (trailing spaces left out):
    /// what a selection highlights, and the order a caret moves in.
    pub boxes: Vec<CaretBox>,
    /// The line's top and height (to the next line's top, its line spacing included) and its
    /// baseline, from the box's top (pixels).
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
}

impl CaretLine {
    /// The caret's x at character boundary `i` on this line: at the glyph starting there, or
    /// after the line's last glyph, or (inside a ligature) at the glyph it is part of.
    pub fn x_at(&self, i: u32) -> f32 {
        let mut x = self.stops.first().map_or(0.0, |s| s.1);
        for &(c, sx) in &self.stops {
            if c > i {
                break;
            }
            x = sx;
        }
        x
    }

    /// The boundary nearest to `x` on this line.
    pub fn index_at(&self, x: f32) -> u32 {
        let mut best = (self.start, f32::INFINITY);
        for &(c, sx) in &self.stops {
            let d = (sx - x).abs();
            if d < best.1 {
                best = (c, d);
            }
        }
        best.0.clamp(self.start, self.end)
    }
}

/// What a font name written by another program means on this computer: a PostScript name
/// (`Arial-BoldMT`, as Animate documents store faces), a family name, or a device font
/// (`_sans`, `_serif`, `_typewriter`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceMatch {
    /// The installed family, or `None` when it isn't installed (layout then falls back to the
    /// default family).
    pub family: Option<String>,
    /// The family the name stands for, installed or not ("Times New Roman"), for reports.
    pub wanted: String,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TextError {
    /// No usable font on this computer.
    NoFonts,
    /// More than [`MAX_CHARS`] characters.
    TooLong,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextError::NoFonts => f.write_str("no fonts were found on this computer"),
            TextError::TooLong => write!(f, "text is limited to {MAX_CHARS} characters per block"),
        }
    }
}

impl std::error::Error for TextError {}

/// The fonts AniCraft can use: the system's, those in `CRAFT_FONTS_DIR`, and faces the app
/// registered (its embedded interface font).
pub struct Fonts {
    db: fontdb::Database,
    families: Vec<String>,
    /// Which face, if any, has each character the fallback list didn't (`Fonts::any_face_with`).
    wide: Remembered<char, Option<fontdb::ID>>,
    /// The fallback families' faces by weight and slant (`Fonts::fallback_list`), found once.
    lists: Remembered<(bool, bool), Arc<[fontdb::ID]>>,
    /// Whether a face has a character, for the letters fallback asked about (`Fonts::covers`).
    covered: Remembered<(fontdb::ID, char), bool>,
}

/// Answers about the fonts that are worked out once and kept (ADR 0130).
type Remembered<K, V> = std::sync::Mutex<std::collections::HashMap<K, V>>;

static FONTS: OnceLock<Fonts> = OnceLock::new();
static EXTRA: OnceLock<Vec<Arc<[u8]>>> = OnceLock::new();

impl Fonts {
    /// Faces to add to the library (call before the first [`Fonts::get`]; later calls are
    /// ignored). The app registers its embedded Inter so text has a known default everywhere.
    pub fn register(faces: Vec<Arc<[u8]>>) {
        let _ = EXTRA.set(faces);
    }

    /// The font library, scanned once on first use.
    pub fn get() -> &'static Fonts {
        FONTS.get_or_init(|| {
            let mut db = fontdb::Database::new();
            for face in EXTRA.get().into_iter().flatten() {
                db.load_font_data(face.to_vec());
            }
            // A web page can't see the computer's fonts: there, text uses the faces the app
            // registered (ADR 0036).
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR") {
                    db.load_fonts_dir(dir);
                }
                db.load_system_fonts();
            }
            Fonts::from_db(db)
        })
    }

    fn from_db(db: fontdb::Database) -> Fonts {
        let mut families: Vec<String> = db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).filter(|n| !n.starts_with('.')).collect();
        families.sort_by_key(|f| f.to_lowercase());
        families.dedup();
        Fonts { db, families, wide: Remembered::default(), lists: Remembered::default(), covered: Remembered::default() }
    }

    /// A library of just these faces (tests, and documents carrying their own fonts).
    pub fn from_faces(faces: impl IntoIterator<Item = Vec<u8>>) -> Fonts {
        let mut db = fontdb::Database::new();
        for f in faces {
            db.load_font_data(f);
        }
        Fonts::from_db(db)
    }

    /// Installed family names, sorted.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// The installed family named `name`, ignoring case and spaces ("SegoeUI" finds "Segoe UI").
    fn installed(&self, name: &str) -> Option<String> {
        let key = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
        let want = key(name);
        if want.is_empty() {
            return None;
        }
        self.families.iter().find(|f| key(f) == want).cloned()
    }

    /// What `name` (a PostScript name, a family or a device font) is on this computer: the
    /// face with that PostScript name, else the family, else the family the name suggests
    /// ([`guess_family`]).
    pub fn find_face(&self, name: &str) -> FaceMatch {
        let name = name.trim();
        if name.is_empty() {
            return FaceMatch { family: None, wanted: String::new(), bold: false, italic: false };
        }
        if let Some(f) = self.db.faces().find(|f| f.post_script_name.eq_ignore_ascii_case(name))
            && let Some((family, _)) = f.families.first()
        {
            return FaceMatch { family: Some(family.clone()), wanted: family.clone(), bold: f.weight.0 >= 600, italic: f.style != fontdb::Style::Normal };
        }
        let device: Option<&[&str]> = match name {
            "_sans" => Some(DEFAULT_FAMILIES),
            "_serif" => Some(SERIF_FAMILIES),
            "_typewriter" => Some(MONO_FAMILIES),
            _ => None,
        };
        if let Some(list) = device {
            let family = list.iter().find_map(|f| self.installed(f));
            return FaceMatch { wanted: family.clone().unwrap_or_else(|| name.to_string()), family, bold: false, italic: false };
        }
        if let Some(family) = self.installed(name) {
            return FaceMatch { wanted: family.clone(), family: Some(family), bold: false, italic: false };
        }
        let (wanted, bold, italic) = guess_family(name);
        FaceMatch { family: self.installed(&wanted), wanted, bold, italic }
    }

    /// A face on this computer that can draw `c`, for showing text in a script the interface's
    /// own fonts lack (#482): the regular style of the first family in `prefer` that has the
    /// character, else of any other installed family. Files over `max_bytes` are passed over
    /// (the caller keeps the bytes in memory), and only faces that parse are returned.
    pub fn face_covering(&self, c: char, prefer: &[&str], max_bytes: usize) -> Option<CoveringFace> {
        let regular = |f: &&fontdb::FaceInfo| f.style == fontdb::Style::Normal && (300..=500).contains(&f.weight.0);
        let named = |name: &str| self.db.faces().filter(regular).filter(|f| f.families.first().is_some_and(|(n, _)| n.eq_ignore_ascii_case(name))).min_by_key(|f| f.weight.0.abs_diff(400)).map(|f| f.id);
        let preferred = prefer.iter().filter_map(|name| named(name));
        // Any other family, in name order so the answer is the same from run to run.
        let mut others: Vec<(&str, fontdb::ID)> = self.db.faces().filter(regular).filter_map(|f| f.families.first().map(|(n, _)| (n.as_str(), f.id))).filter(|(n, _)| !n.starts_with('.')).collect();
        others.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let ids = preferred.chain(others.into_iter().map(|(_, id)| id)).take(MAX_FALLBACK_SEARCH);
        for id in ids {
            let found = self.db.with_face_data(id, |data, index| (data.len() <= max_bytes && face_has(data, index, c)).then(|| (data.to_vec(), index)));
            if let Some(Some((data, index))) = found {
                let family = self.db.face(id).and_then(|f| f.families.first().map(|(n, _)| n.clone())).unwrap_or_default();
                return Some(CoveringFace { family, data, index });
            }
        }
        None
    }

    /// The family used for `asked`: itself when installed (any case), else the default.
    pub fn resolve(&self, asked: &str) -> Option<String> {
        let find = |name: &str| self.families.iter().find(|f| f.eq_ignore_ascii_case(name)).cloned();
        if !asked.trim().is_empty()
            && let Some(f) = find(asked.trim())
        {
            return Some(f);
        }
        DEFAULT_FAMILIES.iter().find_map(|f| find(f)).or_else(|| self.families.first().cloned())
    }

    /// Lays `text` out in `style`.
    pub fn layout(&self, text: &str, style: &Style) -> Result<Layout, TextError> {
        self.lay_out_with(text, style, false)
    }

    /// Lays `text` out in `style`, with each glyph on its own too ([`Layout::glyphs`]).
    pub fn layout_glyphs(&self, text: &str, style: &Style) -> Result<Layout, TextError> {
        self.lay_out_with(text, style, true)
    }

    fn lay_out_with(&self, text: &str, style: &Style, glyphs: bool) -> Result<Layout, TextError> {
        let style = sanitize(style);
        let run = Run { chars: usize::MAX, style: RunStyle { family: style.family.clone(), size: style.size, bold: style.bold, italic: style.italic, letter_spacing: style.letter_spacing, ..RunStyle::default() } };
        let para = ParaStyle { align: style.align, line_spacing: style.line_spacing, ..ParaStyle::default() };
        self.layout_rich(&RichText { text, runs: &[run], paragraphs: &[para], width: style.width }, glyphs)
    }
}

/// Most faces [`Fonts::face_covering`] opens for one character.
pub const MAX_FALLBACK_SEARCH: usize = 4096;

/// A face that can draw a character the interface's fonts lack ([`Fonts::face_covering`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoveringFace {
    pub family: String,
    /// The font file (a collection holds several faces: `index` says which).
    pub data: Vec<u8>,
    pub index: u32,
}

/// Whether face `index` of the font file `data` parses and has a glyph for `c`.
pub fn face_has(data: &[u8], index: u32, c: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(data, index).is_ok_and(|f| f.charmap().map(c).is_some())
}

/// The family, boldness and slant a PostScript name suggests, for fonts that aren't installed:
/// `TimesNewRomanPS-BoldItalicMT` is Times New Roman, bold and italic.
pub fn guess_family(postscript: &str) -> (String, bool, bool) {
    let name: String = postscript.trim().chars().take(256).collect();
    let (base, style) = name.split_once(['-', ',']).unwrap_or((&name, ""));
    let style = style.to_ascii_lowercase();
    let bold = ["bold", "black", "heavy"].iter().any(|w| style.contains(w));
    let italic = ["italic", "oblique"].iter().any(|w| style.contains(w));
    // The suffixes PostScript names give Monotype and PostScript versions of a face.
    let mut base = base;
    for suffix in ["MT", "PS"] {
        base = base.strip_suffix(suffix).filter(|b| !b.is_empty()).unwrap_or(base);
    }
    // CamelCase to words: "HelveticaNeue" → "Helvetica Neue"; capital runs stay ("ComicSansMS").
    let mut family = String::with_capacity(base.len() + 4);
    let mut prev: Option<char> = None;
    for c in base.chars() {
        if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) {
            family.push(' ');
        }
        family.push(c);
        prev = Some(c);
    }
    (family.trim().to_string(), bold, italic)
}

/// Clamps a style to sane ranges (sizes from 1 to 2000 px, spacing within ±1000 px).
pub fn sanitize(style: &Style) -> Style {
    let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
    Style {
        size: fin(style.size, 32.0).clamp(1.0, 2000.0),
        letter_spacing: fin(style.letter_spacing, 0.0).clamp(-1000.0, 1000.0),
        line_spacing: fin(style.line_spacing, 0.0).clamp(-1000.0, 1000.0),
        width: style.width.filter(|w| w.is_finite() && *w > 0.0).map(|w| w.min(100_000.0)),
        ..style.clone()
    }
}

/// Draws glyph outlines (font space, y up) into a path (pixels, y down) at a pen position.
pub(crate) struct Pen<'a> {
    cmds: &'a mut Vec<PathCmd>,
    x: f32,
    y: f32,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(self.x + x, self.y - y)
    }
}

impl skrifa::outline::OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.cmds.push(PathCmd::MoveTo(p));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.cmds.push(PathCmd::LineTo(p));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (c, p) = (self.at(cx, cy), self.at(x, y));
        self.cmds.push(PathCmd::QuadTo(c, p));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (a, b, p) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        self.cmds.push(PathCmd::CubicTo(a, b, p));
    }
    fn close(&mut self) {
        self.cmds.push(PathCmd::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font from this computer, if it has any (CI images and desktops do; a bare container
    /// may not, and then these tests say so and pass).
    fn fonts() -> Option<&'static Fonts> {
        let f = Fonts::get();
        if f.families().is_empty() {
            eprintln!("no fonts on this computer: text layout tests skipped");
            return None;
        }
        Some(f)
    }

    fn style(size: f32) -> Style {
        Style { size, ..Style::default() }
    }

    #[test]
    fn text_becomes_outlines_in_a_box() {
        let Some(fonts) = fonts() else { return };
        let l = fonts.layout("Hello", &style(40.0)).unwrap();
        assert!(!l.family.is_empty());
        assert!(l.outline.cmds.len() > 20, "five glyphs of outlines");
        let b = l.outline.bounds();
        assert!(b.min.x >= -2.0 && b.max.x <= l.width + 2.0, "{b:?} in {}", l.width);
        assert!(b.min.y >= -2.0 && b.max.y <= l.height + 2.0, "{b:?} in {}", l.height);
        assert!(l.width > 40.0 && l.height > 30.0 && l.height < 80.0, "{} × {}", l.width, l.height);
        // Bigger type, bigger box; letter spacing widens it; empty text is an empty box.
        let big = fonts.layout("Hello", &style(80.0)).unwrap();
        assert!((big.width / l.width - 2.0).abs() < 0.05);
        let spaced = fonts.layout("Hello", &Style { letter_spacing: 10.0, ..style(40.0) }).unwrap();
        assert!((spaced.width - l.width - 50.0).abs() < 0.5, "{} vs {}", spaced.width, l.width);
        assert!(fonts.layout("", &style(40.0)).unwrap().outline.cmds.is_empty());
    }

    #[test]
    fn lines_break_at_newlines_and_wrap_at_spaces() {
        let Some(fonts) = fonts() else { return };
        let one = fonts.layout("one", &style(20.0)).unwrap();
        let two = fonts.layout("one\ntwo", &style(20.0)).unwrap();
        assert!(two.height > one.height * 1.8, "{} vs {}", two.height, one.height);
        let spaced = fonts.layout("one\ntwo", &Style { line_spacing: 10.0, ..style(20.0) }).unwrap();
        assert!((spaced.height - two.height - 10.0).abs() < 0.01);
        // A width wraps at spaces, never wider than the width when the words fit.
        let wrapped = fonts.layout("one two three four five six", &Style { width: Some(80.0), ..style(20.0) }).unwrap();
        assert_eq!(wrapped.width, 80.0);
        assert!(wrapped.height > one.height * 2.5, "several lines: {}", wrapped.height);
        assert!(wrapped.outline.bounds().max.x <= 82.0);
        // Alignment moves lines within the box.
        let boxed = |align| fonts.layout("a", &Style { align, width: Some(300.0), ..style(20.0) }).unwrap().outline.bounds();
        assert!(boxed(Align::Left).max.x < 20.0);
        assert!((140.0..160.0).contains(&boxed(Align::Center).center().x), "{:?}", boxed(Align::Center));
        assert!(boxed(Align::Right).min.x > 280.0);
    }

    #[test]
    fn families_resolve_and_styles_are_clamped() {
        let Some(fonts) = fonts() else { return };
        let first = fonts.families()[0].clone();
        assert_eq!(fonts.resolve(&first.to_uppercase()), Some(first));
        let fallback = fonts.resolve("No Such Family 12345").unwrap();
        assert!(fonts.families().contains(&fallback));
        let s = sanitize(&Style { size: f32::NAN, letter_spacing: 1e9, width: Some(-5.0), ..Style::default() });
        assert_eq!((s.size, s.letter_spacing, s.width), (32.0, 1000.0, None));
        assert_eq!(fonts.layout(&"x".repeat(MAX_CHARS + 1), &style(10.0)), Err(TextError::TooLong));
        assert_eq!(Fonts::from_faces(Vec::new()).layout("x", &style(10.0)), Err(TextError::NoFonts));
    }

    #[test]
    fn postscript_names_suggest_families() {
        let g = |n: &str| guess_family(n);
        assert_eq!(g("ArialMT"), ("Arial".into(), false, false));
        assert_eq!(g("Arial-BoldMT"), ("Arial".into(), true, false));
        assert_eq!(g("TimesNewRomanPS-BoldItalicMT"), ("Times New Roman".into(), true, true));
        assert_eq!(g("TimesNewRomanPSMT"), ("Times New Roman".into(), false, false));
        assert_eq!(g("HelveticaNeue-LightOblique"), ("Helvetica Neue".into(), false, true));
        assert_eq!(g("ComicSansMS"), ("Comic Sans MS".into(), false, false));
        assert_eq!(g("Myriad Pro"), ("Myriad Pro".into(), false, false));
        assert_eq!(g("Gotham-Black"), ("Gotham".into(), true, false));
        assert_eq!(g(""), (String::new(), false, false));
        assert_eq!(g("MT"), ("MT".into(), false, false), "a suffix alone is the name");
    }

    #[test]
    fn faces_are_found_by_postscript_name_family_or_device_font() {
        let none = Fonts::from_faces(Vec::new());
        let m = none.find_face("Futura-Bold");
        assert_eq!((m.family, m.wanted.as_str(), m.bold, m.italic), (None, "Futura", true, false));
        assert_eq!(none.find_face("  ").wanted, "");
        let Some(fonts) = fonts() else { return };
        let face = fonts.db.faces().find(|f| !f.post_script_name.is_empty() && f.families.first().is_some_and(|(n, _)| !n.starts_with('.')));
        if let Some(face) = face {
            let m = fonts.find_face(&face.post_script_name);
            assert!(m.family.is_some(), "{}", face.post_script_name);
        }
        let first = fonts.families()[0].clone();
        assert_eq!(fonts.find_face(&first.to_uppercase()).family, Some(first.clone()));
        assert_eq!(fonts.find_face(&first.replace(' ', "")).family, Some(first), "spaces don't matter");
        assert!(fonts.find_face("_sans").family.is_some(), "the default families include an installed one");
        let missing = fonts.find_face("NoSuchFamily-Italic");
        assert_eq!((missing.family, missing.wanted.as_str(), missing.italic), (None, "No Such Family", true));
    }

    /// #482: a face for a character is found by coverage, preferred families first; files over
    /// the size limit and bytes that are not a font are passed over.
    #[test]
    fn a_face_covering_a_character_is_found_by_what_it_can_draw() {
        assert!(!face_has(b"not a font", 0, 'a'));
        assert!(!face_has(&[], 0, 'a'));
        let none = Fonts::from_faces(Vec::new());
        assert_eq!(none.face_covering('a', &["Arial"], usize::MAX), None);
        let Some(fonts) = fonts() else { return };
        let Some(face) = fonts.face_covering('a', &[], usize::MAX) else { return };
        assert!(face_has(&face.data, face.index, 'a') && !face.family.is_empty());
        // The family asked for first wins when it has the character.
        let again = fonts.face_covering('a', &[face.family.as_str()], usize::MAX).unwrap();
        assert_eq!(again.family, face.family);
        let named = fonts.face_covering('a', &["No Such Family 12345", &face.family.to_uppercase()], usize::MAX).unwrap();
        assert_eq!(named.family, face.family, "names match in any case; a missing family is skipped");
        // Nothing fits in no bytes, and a private-use character no face has gives nothing.
        assert_eq!(fonts.face_covering('a', &[], 0), None);
        if let Some(odd) = fonts.face_covering('\u{10FFFD}', &[], usize::MAX) {
            assert!(face_has(&odd.data, odd.index, '\u{10FFFD}'));
        }
    }

    #[test]
    fn layouts_report_their_lines() {
        let Some(fonts) = fonts() else { return };
        let one = fonts.layout("one", &style(20.0)).unwrap();
        let three = fonts.layout("one\ntwo\nthree", &style(20.0)).unwrap();
        assert_eq!((one.lines, three.lines), (1, 3));
        assert!((one.line_height - three.line_height).abs() < 1e-3 && one.line_height >= 10.0);
        assert!((three.height - one.height - 2.0 * one.line_height).abs() < 0.01);
        assert_eq!(fonts.layout("", &style(20.0)).unwrap().lines, 1);
        // The first baseline is inside the box, below its top by most of a line.
        assert!(one.ascent > 10.0 && one.ascent < one.height, "{} in {}", one.ascent, one.height);
        let b = one.outline.bounds();
        assert!(b.max.y > one.ascent - 1.0 && b.min.y < one.ascent, "glyphs sit on the baseline: {b:?}");
    }

    #[test]
    fn glyphs_on_their_own_add_up_to_the_outline() {
        let Some(fonts) = fonts() else { return };
        let l = fonts.layout_glyphs("AVA b", &style(30.0)).unwrap();
        assert_eq!(l.outline, fonts.layout("AVA b", &style(30.0)).unwrap().outline, "the same outline");
        assert_eq!(fonts.layout("AVA b", &style(30.0)).unwrap().glyphs, Vec::new(), "only when asked");
        assert_eq!(l.glyphs.iter().map(|g| g.index).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
        let mut joined = Path::new();
        for g in &l.glyphs {
            joined.cmds.extend(g.outline.transformed(&anicraft_geom::Affine::translate(g.x, g.baseline)).cmds);
            assert_eq!(g.baseline, l.ascent);
        }
        let (a, b) = (joined.bounds(), l.outline.bounds());
        assert!((a.min - b.min).length() < 1e-3 && (a.max - b.max).length() < 1e-3, "{a:?} vs {b:?}");
        let last = l.glyphs.last().unwrap();
        assert!((last.x + last.advance - l.width).abs() < 1e-3);
    }

    /// Synthetic glyphs: a 10 × 20 box standing on the baseline, `n` of them 10 apart.
    fn boxes(n: u32) -> Vec<PlacedGlyph> {
        (0..n).map(|i| PlacedGlyph { outline: Path::rect(0.0, -20.0, 10.0, 20.0), x: i as f32 * 10.0, baseline: 20.0, advance: 10.0, index: i, paint: 0 }).collect()
    }

    #[test]
    fn glyphs_follow_a_path_by_arc_length() {
        let left = PathPlacement { offset: 0.0, flip: false, align: Align::Left };
        // A straight line: glyph k's middle at 10 k + 5, upright, standing on the line (above it).
        let mut line = Path::new();
        line.cmds = vec![PathCmd::MoveTo(Vec2::new(0.0, 100.0)), PathCmd::LineTo(Vec2::new(200.0, 100.0))];
        let t = on_path(&boxes(3), &line, left).unwrap();
        assert_eq!(t.length, 200.0);
        for (k, (i, p, a)) in t.placed.iter().enumerate() {
            assert_eq!(*i, k as u32);
            assert!((p.x - (10.0 * k as f32 + 5.0)).abs() < 1e-3 && (p.y - 100.0).abs() < 1e-3 && a.abs() < 1e-4, "{p:?} {a}");
        }
        let b = t.outline.bounds();
        assert!((b.min.x).abs() < 1e-3 && (b.max.x - 30.0).abs() < 1e-3 && (b.min.y - 80.0).abs() < 1e-3 && (b.max.y - 100.0).abs() < 1e-3, "{b:?}");
        // Offset, centring and right alignment move the run along the path.
        let at = |offset, align| on_path(&boxes(3), &line, PathPlacement { offset, flip: false, align }).unwrap().placed[0].1.x;
        assert!((at(50.0, Align::Left) - 55.0).abs() < 1e-3);
        assert!((at(100.0, Align::Center) - 90.0).abs() < 1e-3);
        assert!((at(200.0, Align::Right) - 175.0).abs() < 1e-3);
        // Past an open path's end, glyphs are left out.
        let off = on_path(&boxes(3), &line, PathPlacement { offset: 180.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!((off.placed.len(), off.hidden), (2, 1));
        // Flipped: walked from the other end, upside down, below the line.
        let f = on_path(&boxes(1), &line, PathPlacement { offset: 0.0, flip: true, align: Align::Left }).unwrap();
        let (_, p, a) = f.placed[0];
        assert!((p.x - 195.0).abs() < 1e-3 && (a.abs() - std::f32::consts::PI).abs() < 1e-3, "{p:?} {a}");
        assert!(f.outline.bounds().min.y >= 100.0 - 1e-3, "{:?}", f.outline.bounds());
    }

    #[test]
    fn glyphs_turn_with_a_circle_and_wrap_round_it() {
        // A circle of radius 100 (clockwise on screen, from the right): a glyph at distance s
        // sits at angle s / r, turned a quarter more.
        let circle = Path::ellipse(0.0, 0.0, 100.0, 100.0);
        let walk = anicraft_geom::walk::CurveWalk::new(&circle).unwrap();
        let len = walk.length();
        assert!((len - 2.0 * std::f32::consts::PI * 100.0).abs() < 0.5, "{len}");
        let t = on_path(&boxes(4), &circle, PathPlacement { offset: len - 15.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!((t.placed.len(), t.hidden), (4, 0), "wrapped past the start of a closed path");
        for (_, p, a) in &t.placed {
            assert!((p.length() - 100.0).abs() < 0.05, "on the circle: {p:?}");
            let tangent = Vec2::new(a.cos(), a.sin());
            assert!(tangent.dot(*p).abs() < 0.5, "turned along it: {p:?} {a}");
        }
        // The second glyph's middle is at distance 0: the path's start.
        let (_, p, _) = t.placed[1];
        assert!((p - walk.at(0.0)).length() < 0.05, "{p:?}");
        // No length, no text.
        let mut dot = Path::new();
        dot.cmds = vec![PathCmd::MoveTo(Vec2::ZERO), PathCmd::LineTo(Vec2::ZERO)];
        assert!(on_path(&boxes(1), &dot, PathPlacement { offset: 0.0, flip: false, align: Align::Left }).is_none());
    }

    /// Two lines of synthetic glyphs: `n` on the first line and `m` on the second, 30 below.
    fn two_lines(n: u32, m: u32) -> Vec<PlacedGlyph> {
        let mut g = boxes(n);
        g.extend((0..m).map(|i| PlacedGlyph { outline: Path::rect(0.0, -20.0, 10.0, 20.0), x: i as f32 * 10.0, baseline: 50.0, advance: 10.0, index: n + 1 + i, paint: 0 }));
        g
    }

    #[test]
    fn further_lines_run_parallel_to_the_path() {
        // A straight line: the second line is 30 below the path, spaced as the first.
        let mut line = Path::new();
        line.cmds = vec![PathCmd::MoveTo(Vec2::new(0.0, 100.0)), PathCmd::LineTo(Vec2::new(200.0, 100.0))];
        let t = on_path(&two_lines(3, 2), &line, PathPlacement { offset: 20.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!((t.placed.len(), t.hidden), (5, 0));
        let want = [(25.0, 100.0), (35.0, 100.0), (45.0, 100.0), (25.0, 130.0), (35.0, 130.0)];
        for ((i, p, a), (x, y)) in t.placed.iter().zip(want) {
            assert!((p.x - x).abs() < 1e-2 && (p.y - y).abs() < 1e-2 && a.abs() < 1e-4, "glyph {i}: {p:?}");
        }
        assert_eq!(t.placed.iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, 1, 2, 4, 5], "glyph order kept");
        // Each line is anchored on its own width: centred, the shorter line is centred too.
        let c = on_path(&two_lines(3, 1), &line, PathPlacement { offset: 100.0, flip: false, align: Align::Center }).unwrap();
        assert!((c.placed[0].1.x - 90.0).abs() < 1e-2 && (c.placed[3].1.x - 100.0).abs() < 1e-2, "{:?}", c.placed);
        let r = on_path(&two_lines(3, 1), &line, PathPlacement { offset: 200.0, flip: false, align: Align::Right }).unwrap();
        assert!((r.placed[2].1.x - 195.0).abs() < 1e-2 && (r.placed[3].1.x - 195.0).abs() < 1e-2, "{:?}", r.placed);
        // Flipped, the text hangs below the path and its second line is above it.
        let f = on_path(&two_lines(1, 1), &line, PathPlacement { offset: 0.0, flip: true, align: Align::Left }).unwrap();
        assert!((f.placed[0].1 - Vec2::new(195.0, 100.0)).length() < 1e-2 && (f.placed[1].1 - Vec2::new(195.0, 70.0)).length() < 1e-2, "{:?}", f.placed);
    }

    #[test]
    fn a_line_inside_a_circle_is_spaced_on_its_own_circle() {
        // Radius 100, drawn clockwise on screen: glyphs stand outside, further lines go inside.
        // The second line's baseline is the circle of radius 70, and its glyphs' middles are an
        // advance (10) apart along that circle: 10 / 70 of a radian, not 10 / 100.
        let circle = Path::ellipse(0.0, 0.0, 100.0, 100.0);
        let t = on_path(&two_lines(2, 3), &circle, PathPlacement { offset: 50.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!((t.placed.len(), t.hidden), (5, 0));
        let ang = |p: Vec2| p.y.atan2(p.x);
        for (_, p, _) in &t.placed[..2] {
            assert!((p.length() - 100.0).abs() < 0.05, "{p:?}");
        }
        for (_, p, a) in &t.placed[2..] {
            assert!((p.length() - 70.0).abs() < 0.1, "on the inner circle: {p:?}");
            assert!(Vec2::new(a.cos(), a.sin()).dot(*p).abs() < 0.5, "turned along it");
        }
        let step = ang(t.placed[3].1) - ang(t.placed[2].1);
        assert!((step - 10.0 / 70.0).abs() < 2e-3, "{step}");
        assert!(((ang(t.placed[1].1) - ang(t.placed[0].1)) - 0.1).abs() < 1e-3);
        // Both lines start at the same place round the circle: the anchor's radius.
        let first_start = ang(t.placed[0].1) - 0.05;
        let second_start = ang(t.placed[2].1) - 5.0 / 70.0;
        assert!((first_start - second_start).abs() < 2e-3, "{first_start} vs {second_start}");
        // It wraps round its own length.
        let w = on_path(&two_lines(1, 60), &circle, PathPlacement { offset: 0.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!(w.hidden, 0, "60 glyphs of 10 wrap round 440");
        // A line whose parallel curve has no length (through the centre) is left out.
        let mut g = boxes(1);
        g.push(PlacedGlyph { outline: Path::rect(0.0, -20.0, 10.0, 20.0), x: 0.0, baseline: 120.0, advance: 10.0, index: 2, paint: 0 });
        let z = on_path(&g, &circle, PathPlacement { offset: 0.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!((z.placed.len(), z.hidden), (1, 1));
    }

    #[test]
    fn lines_past_the_limit_and_hostile_baselines_are_left_out() {
        let mut line = Path::new();
        line.cmds = vec![PathCmd::MoveTo(Vec2::new(0.0, 0.0)), PathCmd::LineTo(Vec2::new(500.0, 0.0))];
        let many: Vec<PlacedGlyph> = (0..MAX_PATH_LINES as u32 + 5).map(|i| PlacedGlyph { outline: Path::rect(0.0, -2.0, 1.0, 2.0), x: 0.0, baseline: i as f32 * 3.0, advance: 1.0, index: i, paint: 0 }).collect();
        let t = on_path(&many, &line, PathPlacement { offset: 0.0, flip: false, align: Align::Left }).unwrap();
        assert_eq!((t.placed.len(), t.hidden), (MAX_PATH_LINES, 5));
        let bad = vec![
            PlacedGlyph { outline: Path::new(), x: 0.0, baseline: 0.0, advance: 1.0, index: 0, paint: 0 },
            PlacedGlyph { outline: Path::new(), x: 0.0, baseline: f32::NAN, advance: 1.0, index: 1, paint: 0 },
            PlacedGlyph { outline: Path::new(), x: f32::INFINITY, baseline: 1e30, advance: 1.0, index: 2, paint: 0 },
        ];
        let t = on_path(&bad, &line, PathPlacement { offset: f32::NAN, flip: true, align: Align::Right }).unwrap();
        assert!(t.outline.cmds.iter().all(|c| match c {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => p.is_finite(),
            _ => true,
        }));
    }

    #[test]
    fn carets_stand_at_every_boundary_line_by_line() {
        let Some(fonts) = fonts() else { return };
        let l = fonts.layout("ab
cd", &style(20.0)).unwrap();
        assert_eq!(l.carets.len(), 2);
        let (a, b) = (&l.carets[0], &l.carets[1]);
        assert_eq!((a.start, a.end, b.start, b.end), (0, 2, 3, 5), "the line break belongs to neither line");
        assert_eq!(a.x_at(0), 0.0);
        assert!(a.x_at(1) > 0.0 && a.x_at(2) > a.x_at(1));
        assert!((a.x_at(2) - fonts.layout("ab", &style(20.0)).unwrap().width).abs() < 0.01, "after the last glyph");
        assert_eq!(a.index_at(-5.0), 0);
        assert_eq!(a.index_at(1000.0), 2);
        assert_eq!(b.index_at(a.x_at(1)), 4, "the same place on the next line");
        // Wrapped lines: the space stays on the first line, the next word starts the second.
        let w = fonts.layout("aaa bbb", &Style { width: Some(fonts.layout("aaa b", &style(20.0)).unwrap().width), ..style(20.0) }).unwrap();
        assert_eq!(w.carets.len(), 2);
        assert_eq!((w.carets[0].start, w.carets[0].end, w.carets[1].start, w.carets[1].end), (0, 4, 4, 7));
        // Empty text has one line with one place for the caret; centred text starts mid-box.
        let e = fonts.layout("", &style(20.0)).unwrap();
        assert_eq!((e.carets.len(), e.carets[0].start, e.carets[0].end, e.carets[0].stops.clone()), (1, 0, 0, vec![(0, 0.0)]));
        let c = fonts.layout("", &Style { width: Some(100.0), align: Align::Center, ..style(20.0) }).unwrap();
        assert_eq!(c.carets[0].x_at(0), 50.0);
    }
}
