//! Trace Bitmap (Modify › Bitmap, #463): a bitmap's pixels become filled shapes, with the four
//! settings of Animate's dialog.
//!
//! 1. **Colour threshold.** Pixels join a region, four-connected, while their colour differs from
//!    the region's mean by less than the threshold (the sum of the red, green and blue
//!    differences, 0..765). Pixels under half opacity are clear: they make regions that are not
//!    drawn.
//! 2. **Minimum area.** Regions smaller than this many pixels join the neighbour they share the
//!    longest border with, taking its colour.
//! 3. **Contours.** The borders between regions are walked once along the pixel edges and cut
//!    into chains where three or more regions meet, so neighbours share every chain and the
//!    shapes tile without gaps or overlaps.
//! 4. **Curve fit and corner threshold.** Each chain is smoothed (edge midpoints, keeping real
//!    corners), fitted with Béziers within the curve fit's tolerance, corners kept where the
//!    outline turns more than the corner threshold's angle (`anicraft_geom::fit_curve_with`), and
//!    written as quadratic curves and lines, as Animate's shapes are. **Pixels** follows the
//!    pixel edges exactly.
//!
//! Each region becomes one path, in the bitmap's pixel space, outer outlines and holes wound
//! opposite ways (non-zero).

use crate::{EngineError, Result};
use anicraft_doc::{Pixels, Rgba};
use anicraft_geom::{Path, PathCmd, Vec2};
use std::collections::HashMap;

/// Most pixels traced at once.
pub const MAX_TRACE_PIXELS: u64 = 4096 * 4096;
/// Most shapes one trace makes.
pub const MAX_REGIONS: usize = 20_000;
/// Most areas of colour before small ones join their neighbours. Every area costs memory while
/// they are merged (a few hundred bytes at worst), so an image that is all speckle at a low
/// colour threshold is refused here, in one sentence, instead of taking gigabytes.
pub const MAX_GROWN_REGIONS: usize = 2_000_000;

/// How closely outlines follow the pixels (Animate's Curve Fit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveFit {
    Pixels,
    VeryTight,
    Tight,
    Normal,
    Smooth,
    VerySmooth,
}

impl CurveFit {
    pub const ALL: [CurveFit; 6] = [CurveFit::Pixels, CurveFit::VeryTight, CurveFit::Tight, CurveFit::Normal, CurveFit::Smooth, CurveFit::VerySmooth];

    /// The name params use.
    pub fn key(self) -> &'static str {
        match self {
            CurveFit::Pixels => "pixels",
            CurveFit::VeryTight => "very_tight",
            CurveFit::Tight => "tight",
            CurveFit::Normal => "normal",
            CurveFit::Smooth => "smooth",
            CurveFit::VerySmooth => "very_smooth",
        }
    }

    pub fn from_key(k: &str) -> Option<CurveFit> {
        CurveFit::ALL.into_iter().find(|c| c.key() == k)
    }

    /// Largest distance between the curves and the smoothed outline (pixels).
    fn tolerance(self) -> f32 {
        match self {
            CurveFit::Pixels => 0.0,
            CurveFit::VeryTight => 0.35,
            CurveFit::Tight => 0.6,
            CurveFit::Normal => 1.0,
            CurveFit::Smooth => 1.6,
            CurveFit::VerySmooth => 2.5,
        }
    }
}

/// How readily outlines keep sharp corners (Animate's Corner Threshold).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    Many,
    Normal,
    Few,
}

impl Corners {
    pub const ALL: [Corners; 3] = [Corners::Many, Corners::Normal, Corners::Few];

    /// The name params use.
    pub fn key(self) -> &'static str {
        match self {
            Corners::Many => "many",
            Corners::Normal => "normal",
            Corners::Few => "few",
        }
    }

    pub fn from_key(k: &str) -> Option<Corners> {
        Corners::ALL.into_iter().find(|c| c.key() == k)
    }

    /// The turn (degrees) that makes a corner.
    fn angle(self) -> f32 {
        match self {
            Corners::Many => 40.0,
            Corners::Normal => 60.0,
            Corners::Few => 85.0,
        }
    }
}

/// The Trace Bitmap dialog's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceSettings {
    /// 1..=500: pixels whose colours differ by less are one colour.
    pub color_threshold: u32,
    /// 1..=1000 pixels: smaller regions join a neighbour.
    pub min_area: u32,
    pub curve_fit: CurveFit,
    pub corners: Corners,
}

impl Default for TraceSettings {
    /// Animate's: threshold 100, minimum area 8 pixels, Normal and Normal.
    fn default() -> Self {
        TraceSettings { color_threshold: 100, min_area: 8, curve_fit: CurveFit::Normal, corners: Corners::Normal }
    }
}

/// One traced region: its colour and outline, in the bitmap's pixel space.
#[derive(Clone, Debug)]
pub struct Traced {
    pub color: Rgba,
    pub path: Path,
}

/// Traces `px` into one filled outline per visible region, in scan order of the regions' first
/// pixels.
pub fn trace(px: &Pixels, set: &TraceSettings) -> Result<Vec<Traced>> {
    let (w, h) = (px.width as usize, px.height as usize);
    if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_TRACE_PIXELS || px.rgba.len() != w * h * 4 {
        return Err(EngineError::State(format!("a bitmap of at most {MAX_TRACE_PIXELS} pixels can be traced")));
    }
    let mut map = grow(px, w, h, set.color_threshold.clamp(1, 500))?;
    absorb_small(&mut map, w, h, set.min_area.clamp(1, 1000));
    let shown = map.colors.iter().filter(|c| c.is_some()).count();
    if shown > MAX_REGIONS {
        return Err(EngineError::State(format!(
            "the trace would make {shown} shapes (at most {MAX_REGIONS}); raise the colour threshold or the minimum area"
        )));
    }
    let grid = Grid { w: w as i32, h: h as i32, lab: &map.lab };
    let chains = grid.chains();
    let fitted: Vec<Option<Fitted>> = chains.iter().map(|c| (map.shown(c.left) || map.shown(c.right)).then(|| fit_chain(c, set))).collect();
    let mut per: Vec<Vec<(usize, bool)>> = vec![Vec::new(); map.colors.len()];
    for (i, c) in chains.iter().enumerate() {
        if map.shown(c.left)
            && let Some(v) = per.get_mut(c.left as usize)
        {
            v.push((i, false));
        }
        if map.shown(c.right)
            && let Some(v) = per.get_mut(c.right as usize)
        {
            v.push((i, true));
        }
    }
    let mut out = Vec::with_capacity(shown);
    for (r, refs) in per.iter().enumerate() {
        let Some(Some(color)) = map.colors.get(r) else { continue };
        let path = region_path(&chains, &fitted, refs);
        if !path.is_empty() {
            out.push(Traced { color: *color, path });
        }
    }
    Ok(out)
}

// ----- regions --------------------------------------------------------------------------------

/// Pixels to regions: `lab` holds each pixel's region; `colors` each region's fill (`None` for
/// clear regions).
struct RegionMap {
    lab: Vec<u32>,
    colors: Vec<Option<Rgba>>,
    sizes: Vec<u32>,
}

impl RegionMap {
    fn shown(&self, r: u32) -> bool {
        self.colors.get(r as usize).is_some_and(Option::is_some)
    }
}

const NONE: u32 = u32::MAX;

fn pixel(px: &Pixels, i: usize) -> [u8; 4] {
    match px.rgba.get(i * 4..i * 4 + 4) {
        Some(&[r, g, b, a]) => [r, g, b, a],
        _ => [0; 4],
    }
}

fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
}

/// Four-connected regions of similar colour, each compared with the mean of what joined it.
fn grow(px: &Pixels, w: usize, h: usize, threshold: u32) -> Result<RegionMap> {
    let n = w * h;
    let mut lab = vec![NONE; n];
    let (mut colors, mut sizes) = (Vec::new(), Vec::new());
    let mut stack = Vec::new();
    for seed in 0..n {
        if lab.get(seed) != Some(&NONE) {
            continue;
        }
        if colors.len() >= MAX_GROWN_REGIONS {
            return Err(EngineError::State(format!(
                "the bitmap has more than {MAX_GROWN_REGIONS} separate areas of colour at this colour threshold; raise the colour threshold or trace a smaller bitmap"
            )));
        }
        let r = colors.len() as u32;
        let first = pixel(px, seed);
        let clear = first[3] < 128;
        let mut sum = [u64::from(first[0]), u64::from(first[1]), u64::from(first[2])];
        let (mut count, mut alpha) = (1u64, first[3]);
        if let Some(l) = lab.get_mut(seed) {
            *l = r;
        }
        stack.push(seed);
        while let Some(i) = stack.pop() {
            for j in neighbours(i, w, h) {
                if lab.get(j) != Some(&NONE) {
                    continue;
                }
                let q = pixel(px, j);
                let same = if clear {
                    q[3] < 128
                } else {
                    let d: u64 = (0..3).map(|c| (u64::from(q[c]) * count).abs_diff(sum[c])).sum();
                    q[3] >= 128 && d < u64::from(threshold) * count
                };
                if same {
                    if let Some(l) = lab.get_mut(j) {
                        *l = r;
                    }
                    for c in 0..3 {
                        sum[c] += u64::from(q[c]);
                    }
                    count += 1;
                    alpha = alpha.max(q[3]);
                    stack.push(j);
                }
            }
        }
        let mean = |c: usize| (sum[c] as f32 / count as f32) / 255.0;
        colors.push((!clear).then(|| Rgba::new(mean(0), mean(1), mean(2), f32::from(alpha) / 255.0)));
        sizes.push(count as u32);
    }
    Ok(RegionMap { lab, colors, sizes })
}

fn root(parent: &mut [u32], r: u32) -> u32 {
    let mut a = r;
    while let Some(&p) = parent.get(a as usize) {
        if p == a {
            break;
        }
        a = p;
    }
    let mut b = r;
    while let Some(p) = parent.get_mut(b as usize) {
        if *p == a {
            break;
        }
        let next = *p;
        *p = a;
        b = next;
    }
    a
}

/// Regions under `min_area` pixels join the neighbour they share the longest border with
/// (ties: the earlier region), smallest first; the regions are then numbered again.
fn absorb_small(map: &mut RegionMap, w: usize, h: usize, min_area: u32) {
    let k = map.sizes.len();
    if min_area <= 1 || k < 2 {
        return;
    }
    let small = |sizes: &[u32], r: u32| sizes.get(r as usize).is_some_and(|&s| s < min_area);
    let mut adj: Vec<HashMap<u32, u32>> = vec![HashMap::new(); k];
    let touch = |a: u32, b: u32, adj: &mut [HashMap<u32, u32>]| {
        if a != b && (small(&map.sizes, a) || small(&map.sizes, b)) {
            for (x, y) in [(a, b), (b, a)] {
                if let Some(m) = adj.get_mut(x as usize) {
                    *m.entry(y).or_insert(0) += 1;
                }
            }
        }
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let a = map.lab.get(i).copied().unwrap_or(NONE);
            if x + 1 < w {
                touch(a, map.lab.get(i + 1).copied().unwrap_or(NONE), adj.as_mut_slice());
            }
            if y + 1 < h {
                touch(a, map.lab.get(i + w).copied().unwrap_or(NONE), adj.as_mut_slice());
            }
        }
    }
    let mut order: Vec<u32> = (0..k as u32).filter(|&r| small(&map.sizes, r)).collect();
    order.sort_by_key(|&r| (map.sizes.get(r as usize).copied().unwrap_or(0), r));
    let mut parent: Vec<u32> = (0..k as u32).collect();
    for r in order {
        if root(&mut parent, r) != r || !small(&map.sizes, r) {
            continue;
        }
        let mine = std::mem::take(&mut adj[r as usize]);
        let Some((&into, _)) = mine.iter().filter(|(n, _)| **n != r).max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) else {
            adj[r as usize] = mine;
            continue;
        };
        if let Some(p) = parent.get_mut(r as usize) {
            *p = into;
        }
        let grown = map.sizes.get(r as usize).copied().unwrap_or(0);
        if let Some(s) = map.sizes.get_mut(into as usize) {
            *s += grown;
        }
        for (&m, &count) in &mine {
            if m == into {
                continue;
            }
            if let Some(theirs) = adj.get_mut(m as usize) {
                theirs.remove(&r);
                *theirs.entry(into).or_insert(0) += count;
            }
            if let Some(target) = adj.get_mut(into as usize) {
                *target.entry(m).or_insert(0) += count;
            }
        }
        if let Some(target) = adj.get_mut(into as usize) {
            target.remove(&r);
        }
    }
    let mut renumber = vec![NONE; k];
    let (mut colors, mut sizes) = (Vec::new(), Vec::new());
    for r in 0..k as u32 {
        let top = root(&mut parent, r);
        if top == r
            && let Some(slot) = renumber.get_mut(r as usize)
        {
            *slot = colors.len() as u32;
            colors.push(map.colors.get(r as usize).copied().flatten());
            sizes.push(map.sizes.get(r as usize).copied().unwrap_or(0));
        }
    }
    for l in &mut map.lab {
        let top = root(&mut parent, *l);
        *l = renumber.get(top as usize).copied().unwrap_or(NONE);
    }
    map.colors = colors;
    map.sizes = sizes;
}

// ----- chains ---------------------------------------------------------------------------------

/// Unit steps along pixel edges: right, down, left, up (y down).
const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

/// A stretch of border between two regions, along pixel edges from one meeting point of three or
/// more regions to the next (or a whole loop). `left` is the region on its left as it runs (y
/// down), `right` the other.
struct Chain {
    pts: Vec<(i32, i32)>,
    left: u32,
    right: u32,
    closed: bool,
}

struct Grid<'a> {
    w: i32,
    h: i32,
    lab: &'a [u32],
}

impl Grid<'_> {
    fn label(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return NONE;
        }
        self.lab.get((y * self.w + x) as usize).copied().unwrap_or(NONE)
    }

    /// The regions left and right of the step from `(x, y)` in direction `d`.
    fn sides(&self, (x, y): (i32, i32), d: usize) -> (u32, u32) {
        let (dx, dy) = DIRS[d % 4];
        let at = |o: i32| if o > 0 { 0 } else { -1 };
        (self.label(x + at(dx + dy), y + at(dy - dx)), self.label(x + at(dx - dy), y + at(dy + dx)))
    }

    /// The edge from `(x, y)` in direction `d`, as an index into the visited table, when it
    /// separates two regions.
    fn edge(&self, (x, y): (i32, i32), d: usize) -> Option<usize> {
        let (w, h) = (self.w, self.h);
        let horizontal = (w * (h + 1)) as usize;
        let id = match d % 4 {
            0 if x < w && (0..=h).contains(&y) && x >= 0 => (y * w + x) as usize,
            2 if x >= 1 && x <= w && (0..=h).contains(&y) => (y * w + x - 1) as usize,
            1 if y < h && y >= 0 && (0..=w).contains(&x) => horizontal + (y * (w + 1) + x) as usize,
            3 if y >= 1 && y <= h && (0..=w).contains(&x) => horizontal + ((y - 1) * (w + 1) + x) as usize,
            _ => return None,
        };
        let (l, r) = self.sides((x, y), d);
        (l != r).then_some(id)
    }

    fn degree(&self, v: (i32, i32)) -> usize {
        (0..4).filter(|&d| self.edge(v, d).is_some()).count()
    }

    fn chains(&self) -> Vec<Chain> {
        let edges = (self.w * (self.h + 1) + (self.w + 1) * self.h) as usize;
        let mut seen = vec![false; edges];
        let mut out = Vec::new();
        for y in 0..=self.h {
            for x in 0..=self.w {
                if self.degree((x, y)) < 3 {
                    continue;
                }
                for d in 0..4 {
                    if self.edge((x, y), d).is_some_and(|e| !seen.get(e).copied().unwrap_or(true))
                        && let Some(c) = self.walk((x, y), d, &mut seen)
                    {
                        out.push(c);
                    }
                }
            }
        }
        for y in 0..=self.h {
            for x in 0..=self.w {
                for d in [0, 1] {
                    if self.edge((x, y), d).is_some_and(|e| !seen.get(e).copied().unwrap_or(true))
                        && let Some(c) = self.walk((x, y), d, &mut seen)
                    {
                        out.push(c);
                    }
                }
            }
        }
        out
    }

    /// Walks from `start` in direction `d` to the next meeting point, or round to `start`.
    fn walk(&self, start: (i32, i32), d: usize, seen: &mut [bool]) -> Option<Chain> {
        let (left, right) = self.sides(start, d);
        let mut pts = vec![start];
        let (mut cur, mut dir) = (start, d);
        let limit = seen.len() + 1;
        for _ in 0..limit {
            let e = self.edge(cur, dir)?;
            if let Some(s) = seen.get_mut(e) {
                *s = true;
            }
            let (dx, dy) = DIRS[dir];
            let next = (cur.0 + dx, cur.1 + dy);
            pts.push(next);
            if next == start {
                let closed = self.degree(start) < 3;
                return Some(Chain { pts, left, right, closed });
            }
            if self.degree(next) >= 3 {
                return Some(Chain { pts, left, right, closed: false });
            }
            let back = (dir + 2) % 4;
            dir = (0..4).find(|&n| n != back && self.edge(next, n).is_some())?;
            cur = next;
        }
        None
    }
}

// ----- fitting --------------------------------------------------------------------------------

/// A chain's outline: anchors, and the control point of the quadratic before each anchor after
/// the first (`None`: a straight line).
struct Fitted {
    anchors: Vec<Vec2>,
    ctrls: Vec<Option<Vec2>>,
}

fn fit_chain(c: &Chain, set: &TraceSettings) -> Fitted {
    let v = |p: (i32, i32)| Vec2::new(p.0 as f32, p.1 as f32);
    let steps: Vec<(i32, i32)> = c.pts.windows(2).map(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1)).collect();
    if set.curve_fit == CurveFit::Pixels || steps.len() < 3 {
        let mut anchors = vec![c.pts.first().map_or(Vec2::ZERO, |&p| v(p))];
        for (i, p) in c.pts.iter().enumerate().skip(1) {
            let turn = steps.get(i).is_none_or(|s| Some(s) != steps.get(i - 1));
            if turn {
                anchors.push(v(*p));
            }
        }
        let ctrls = vec![None; anchors.len().saturating_sub(1)];
        return Fitted { anchors, ctrls };
    }
    let n = steps.len();
    let mut run = vec![1usize; n];
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && steps[j + 1] == steps[i] {
            j += 1;
        }
        for r in run.iter_mut().take(j + 1).skip(i) {
            *r = j + 1 - i;
        }
        i = j + 1;
    }
    if c.closed && n > 1 && steps.first() == steps.last() && run[0] < n {
        let joined = run[0] + run[n - 1];
        let (a, b) = (run[0], run[n - 1]);
        for r in run.iter_mut().take(a) {
            *r = joined;
        }
        for r in run.iter_mut().skip(n - b) {
            *r = joined;
        }
    }
    let mut pts = Vec::with_capacity(2 * n + 2);
    if !c.closed {
        pts.push(v(c.pts[0]));
    }
    for k in 0..n {
        let (a, b) = (v(c.pts[k]), v(c.pts[k + 1]));
        if k > 0 || c.closed {
            let prev = (k + n - 1) % n;
            if steps[prev] != steps[k] && run[prev] >= 2 && run[k] >= 2 {
                pts.push(a);
            }
        }
        pts.push((a + b) * 0.5);
    }
    if !c.closed {
        pts.push(v(c.pts[n]));
    }
    let tol = set.curve_fit.tolerance();
    let window = (3.0 * tol).clamp(1.5, 6.0);
    let path = anicraft_geom::fit_curve_with(&pts, tol, window, c.closed, set.corners.angle());
    let quad_tol = (tol * 0.1).max(0.02);
    let mut anchors = Vec::new();
    let mut ctrls = Vec::new();
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => anchors.push(p),
            PathCmd::LineTo(p) => {
                anchors.push(p);
                ctrls.push(None);
            }
            PathCmd::QuadTo(q, p) => {
                anchors.push(p);
                ctrls.push(Some(q));
            }
            PathCmd::CubicTo(c1, c2, p) => {
                let p0 = anchors.last().copied().unwrap_or(p);
                cubic_to_quads([p0, c1, c2, p], quad_tol, &mut anchors, &mut ctrls);
            }
            PathCmd::Close => {}
        }
    }
    if c.closed
        && let (Some(&first), Some(&last)) = (anchors.first(), anchors.last())
        && first != last
    {
        anchors.push(first);
        ctrls.push(None);
    }
    Fitted { anchors, ctrls }
}

/// The cubic's polar form at `(t1, t2, t3)` (de Casteljau with a different `t` at each level).
fn blossom(c: &[Vec2; 4], t: [f32; 3]) -> Vec2 {
    let l1 = [c[0].lerp(c[1], t[0]), c[1].lerp(c[2], t[0]), c[2].lerp(c[3], t[0])];
    let l2 = [l1[0].lerp(l1[1], t[1]), l1[1].lerp(l1[2], t[1])];
    l2[0].lerp(l2[1], t[2])
}

/// A cubic as quadratics within `tol`: equal pieces, each approximated by the quadratic through
/// its ends whose control point is `(3(c1 + c2) − p0 − p3) / 4`; the error of that falls with
/// the cube of the number of pieces. Pieces whose control point is on their chord are lines.
fn cubic_to_quads(c: [Vec2; 4], tol: f32, anchors: &mut Vec<Vec2>, ctrls: &mut Vec<Option<Vec2>>) {
    let d3 = c[3] - c[2] * 3.0 + c[1] * 3.0 - c[0];
    let err = 3f32.sqrt() / 36.0 * d3.length();
    let pieces = if err.is_finite() && err > tol { (err / tol).cbrt().ceil().clamp(1.0, 16.0) as u32 } else { 1 };
    for i in 0..pieces {
        let (a, b) = (i as f32 / pieces as f32, (i + 1) as f32 / pieces as f32);
        let p0 = if i == 0 { c[0] } else { blossom(&c, [a, a, a]) };
        let c1 = blossom(&c, [a, a, b]);
        let c2 = blossom(&c, [a, b, b]);
        let p3 = if i + 1 == pieces { c[3] } else { blossom(&c, [b, b, b]) };
        let q = (c1 + c2) * 0.75 - (p0 + p3) * 0.25;
        anchors.push(p3);
        ctrls.push((anicraft_geom::segment_distance(q, p0, p3) > 0.01).then_some(q));
    }
}

// ----- regions' outlines ----------------------------------------------------------------------

/// A region's outline from the chains around it (`(chain, reversed)`, each with the region on
/// its left once reversed as marked), joined end to start into closed loops.
fn region_path(chains: &[Chain], fitted: &[Option<Fitted>], refs: &[(usize, bool)]) -> Path {
    let ends = |&(i, rev): &(usize, bool)| -> Option<((i32, i32), (i32, i32))> {
        let pts = &chains.get(i)?.pts;
        let (a, b) = (*pts.first()?, *pts.last()?);
        Some(if rev { (b, a) } else { (a, b) })
    };
    let mut path = Path::new();
    let mut from: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (k, r) in refs.iter().enumerate() {
        match chains.get(r.0) {
            Some(c) if c.closed => {
                if let Some(Some(f)) = fitted.get(r.0) {
                    path.move_to(start_of(f, r.1));
                    emit(&mut path, f, r.1);
                    path.close();
                }
            }
            Some(_) => {
                if let Some((a, _)) = ends(r) {
                    from.entry(a).or_default().push(k);
                }
            }
            None => {}
        }
    }
    let mut used = vec![false; refs.len()];
    for k in 0..refs.len() {
        if used[k] || chains.get(refs[k].0).is_none_or(|c| c.closed) {
            continue;
        }
        let Some((start, mut at)) = ends(&refs[k]) else { continue };
        used[k] = true;
        let mut lp = vec![k];
        for _ in 0..refs.len() {
            if at == start {
                break;
            }
            let Some(next) = from.get(&at).and_then(|v| v.iter().copied().find(|&j| !used[j])) else { break };
            used[next] = true;
            lp.push(next);
            at = ends(&refs[next]).map_or(start, |e| e.1);
        }
        let mut first = true;
        for j in lp {
            let (i, rev) = refs[j];
            let Some(Some(f)) = fitted.get(i) else { continue };
            if first {
                path.move_to(start_of(f, rev));
                first = false;
            }
            emit(&mut path, f, rev);
        }
        if !first {
            path.close();
        }
    }
    path
}

fn start_of(f: &Fitted, rev: bool) -> Vec2 {
    let p = if rev { f.anchors.last() } else { f.anchors.first() };
    p.copied().unwrap_or(Vec2::ZERO)
}

fn emit(path: &mut Path, f: &Fitted, rev: bool) {
    let n = f.ctrls.len().min(f.anchors.len().saturating_sub(1));
    for s in 0..n {
        let k = if rev { n - 1 - s } else { s };
        let to = if rev { f.anchors[k] } else { f.anchors[k + 1] };
        match f.ctrls[k] {
            Some(q) => path.quad_to(q, to),
            None => path.line_to(to),
        };
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
