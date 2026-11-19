//! Strokes as filled outlines, and fills grown or shrunk by a distance (ADR 0109).
//!
//! **The outline of a stroke** is built from the curves themselves, not from flattened lines,
//! so a straight line becomes four points and a curve stays a few curves:
//!
//! 1. Every segment is covered by *ribbons*: closed pieces between two cross-sections of the
//!    stroke, whose long sides are the segment's offset curves. An offset curve is not a
//!    Bézier, so each side is the cubic with the offset's ends and end derivatives (Hermite),
//!    and a ribbon is halved until both sides stay within [`TOLERANCE`] of the true offset and
//!    the segment turns less than 45° across it. Lines of one width are exact.
//! 2. Corners get the join (a pie slice, a miter or a bevel on the outside of the bend) and
//!    open ends the cap, with the rules the renderer's stroker uses, so the outline covers what
//!    is drawn. Dashes are cut by arc length and are open pieces of their own; a width function
//!    (a width profile, ADR 0018) gives each side its own distance along the way.
//! 3. The pieces overlap, and one may fold over itself where the stroke is wider than the curve
//!    is tight. Each is filled on its own (non-zero) and their **union** is taken with the
//!    planar map of merge drawing ([`crate::planar`]), which keeps exact sub-curves. Pieces that
//!    don't touch are united apart, and long runs in halves, so the map's size limit is about
//!    how tangled a stroke is, not how long.
//!
//! **Offsetting a fill** by `d` is the same machinery: the band within `|d|` of the fill's
//! boundary is the boundary stroked `2|d|` wide with round joins; the fill with the band added
//! is the fill expanded (corners rounded, as a disc rolled round it leaves them), and the fill
//! with the band taken away is the fill inset (corners kept). Both are exact but for the offset
//! curves' tolerance.
//!
//! The planar map joins points closer than [`planar::SNAP`]; everything is united scaled up, so
//! that distance is far below a pixel and thin strokes keep their shape.

use crate::planar::{self, Item};
use crate::{Affine, FillRule, Path, PathCmd, Rect, Vec2};

/// How far a ribbon's side may be from the true offset curve, in the path's units.
pub const TOLERANCE: f64 = 0.0025;
/// Most halvings of one segment's ribbon (so at most 256 ribbons per segment).
const MAX_DEPTH: u32 = 8;
/// Most pieces one outline is built from; past that it is declined.
pub const MAX_PIECES: usize = 60_000;
/// Most dashes one outline is cut into; past that it is declined.
const MAX_DASHES: usize = 20_000;
/// Widths below this draw nothing (as in the renderer).
const NO_WIDTH: f64 = 1e-4;
/// Samples of a curved segment's length table.
const LEN_SAMPLES: usize = 64;
/// Pieces united in one planar map; longer runs are united in halves.
const UNION_SEGMENTS: usize = 2500;

/// How a stroke's open ends are shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

/// How a stroke's corners are shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

/// A stroke to outline, in the units of the path it is given with.
#[derive(Clone, Copy)]
pub struct StrokeShape<'a> {
    /// Half the nominal width.
    pub half_width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    /// SVG's miter limit: a corner whose point would be longer than this many half-widths out
    /// is bevelled.
    pub miter_limit: f32,
    /// On/off lengths along each subpath (starting with a dash) and how far into them each
    /// subpath starts; `None`: solid. A zero-length dash is a dot: its caps alone.
    pub dash: Option<(&'a [f32], f32)>,
    /// Half-width factors `(left, right)` at a fraction of each subpath's length (a width
    /// profile); `None`: the nominal width everywhere. Left is the left of the direction of
    /// travel on a y-down screen.
    pub widths: Option<&'a dyn Fn(f32) -> (f32, f32)>,
    /// Maps the path to the space the width fractions are measured in (the shape's own space,
    /// when the path was transformed); `None`: the path's space.
    pub to_local: Option<Affine>,
    /// Swaps the sides of `widths` (for a mirroring transform).
    pub flip: bool,
}

impl StrokeShape<'_> {
    /// A solid stroke of one width with round caps and joins.
    pub fn round(half_width: f32) -> StrokeShape<'static> {
        StrokeShape { half_width, cap: LineCap::Round, join: LineJoin::Round, miter_limit: 4.0, dash: None, widths: None, to_local: None, flip: false }
    }
}

// ----- points -----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
struct D {
    x: f64,
    y: f64,
}

impl D {
    fn of(v: Vec2) -> D {
        D { x: v.x as f64, y: v.y as f64 }
    }
    fn vec(self) -> Vec2 {
        Vec2::new(self.x as f32, self.y as f32)
    }
    fn add(self, o: D) -> D {
        D { x: self.x + o.x, y: self.y + o.y }
    }
    fn sub(self, o: D) -> D {
        D { x: self.x - o.x, y: self.y - o.y }
    }
    fn mul(self, k: f64) -> D {
        D { x: self.x * k, y: self.y * k }
    }
    fn dot(self, o: D) -> f64 {
        self.x * o.x + self.y * o.y
    }
    fn cross(self, o: D) -> f64 {
        self.x * o.y - self.y * o.x
    }
    fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn unit(self) -> D {
        let l = self.len();
        if l > 1e-300 { self.mul(1.0 / l) } else { D { x: 0.0, y: 0.0 } }
    }
    /// The unit normal on the left of direction `self` on a y-down screen.
    fn left(self) -> D {
        D { x: self.y, y: -self.x }
    }
    fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

// ----- segments ---------------------------------------------------------------------------------

/// One segment as a cubic, with its length tables.
struct Seg {
    c: [D; 4],
    line: bool,
    /// Cumulative length at `j / (tab.len() - 1)`, in the path's space and in the width space.
    out_tab: Vec<f64>,
    loc_tab: Vec<f64>,
}

impl Seg {
    fn new(c: [D; 4], line: bool, to_local: Option<&Affine>) -> Seg {
        let n = if line { 1 } else { LEN_SAMPLES };
        let mut seg = Seg { c, line, out_tab: Vec::with_capacity(n + 1), loc_tab: Vec::with_capacity(n + 1) };
        // Simpson's rule on the speed: exact for lines, and to a millionth for curves.
        let local = |v: D| to_local.map_or(v, |m| D::of(m.apply_vector(v.vec())));
        let (mut out, mut loc) = (0.0, 0.0);
        seg.out_tab.push(0.0);
        seg.loc_tab.push(0.0);
        let h = 1.0 / n as f64;
        for j in 0..n {
            let t = j as f64 * h;
            let (a, m, b) = (seg.d1(t), seg.d1(t + h * 0.5), seg.d1(t + h));
            out += h / 6.0 * (a.len() + 4.0 * m.len() + b.len());
            loc += h / 6.0 * (local(a).len() + 4.0 * local(m).len() + local(b).len());
            seg.out_tab.push(out);
            seg.loc_tab.push(loc);
        }
        seg
    }

    fn point(&self, t: f64) -> D {
        let [a, b, c, d] = self.c;
        if self.line {
            return a.add(d.sub(a).mul(t));
        }
        let u = 1.0 - t;
        a.mul(u * u * u).add(b.mul(3.0 * u * u * t)).add(c.mul(3.0 * u * t * t)).add(d.mul(t * t * t))
    }

    fn d1(&self, t: f64) -> D {
        let [a, b, c, d] = self.c;
        if self.line {
            return d.sub(a);
        }
        let u = 1.0 - t;
        b.sub(a).mul(3.0 * u * u).add(c.sub(b).mul(6.0 * u * t)).add(d.sub(c).mul(3.0 * t * t))
    }

    fn d2(&self, t: f64) -> D {
        let [a, b, c, d] = self.c;
        if self.line {
            return D { x: 0.0, y: 0.0 };
        }
        let u = 1.0 - t;
        c.sub(b).sub(b.sub(a)).mul(6.0 * u).add(d.sub(c).sub(c.sub(b)).mul(6.0 * t))
    }

    /// The unit direction of travel at `t` (across a stretch where the derivative vanishes,
    /// such as an end whose handle has no length).
    fn tangent(&self, t: f64) -> D {
        let d = self.d1(t);
        if d.len() > 1e-9 * (1.0 + self.out_len()) {
            return d.unit();
        }
        for h in [1e-4, 1e-3, 1e-2, 0.5] {
            let v = self.point((t + h).min(1.0)).sub(self.point((t - h).max(0.0)));
            if v.len() > 1e-12 {
                return v.unit();
            }
        }
        D { x: 1.0, y: 0.0 }
    }

    fn out_len(&self) -> f64 {
        self.out_tab.last().copied().unwrap_or(0.0)
    }

    fn loc_len(&self) -> f64 {
        self.loc_tab.last().copied().unwrap_or(0.0)
    }

    /// Length from the segment's start to `t`, from a table.
    fn len_at(tab: &[f64], t: f64) -> f64 {
        let n = tab.len().saturating_sub(1);
        if n == 0 {
            return 0.0;
        }
        let x = t.clamp(0.0, 1.0) * n as f64;
        let j = (x.floor() as usize).min(n - 1);
        let (a, b) = (tab.get(j).copied().unwrap_or(0.0), tab.get(j + 1).copied().unwrap_or(0.0));
        a + (b - a) * (x - j as f64)
    }

    /// The parameter at length `s` from the segment's start (path's space).
    fn t_at(&self, s: f64) -> f64 {
        let n = self.out_tab.len().saturating_sub(1);
        if n == 0 {
            return 0.0;
        }
        let j = self.out_tab.partition_point(|&c| c < s).clamp(1, n);
        let (a, b) = (self.out_tab.get(j - 1).copied().unwrap_or(0.0), self.out_tab.get(j).copied().unwrap_or(0.0));
        let k = if b > a { ((s - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        ((j - 1) as f64 + k) / n as f64
    }
}

/// One subpath: its segments of some length, in order.
struct Sub {
    segs: Vec<Seg>,
    closed: bool,
    /// Length before each segment, in the path's space and in the width space.
    out_cum: Vec<f64>,
    loc_cum: Vec<f64>,
    out_total: f64,
    loc_total: f64,
    /// Where a subpath with no length is (it may still draw a dot).
    at: D,
}

/// The subpaths of `path`; `None` when a point is not finite. `close_all` treats every subpath
/// as closed (a fill's boundary).
fn subpaths(path: &Path, to_local: Option<&Affine>, close_all: bool) -> Option<Vec<Sub>> {
    let mut out: Vec<Sub> = Vec::new();
    let mut segs: Vec<Seg> = Vec::new();
    let (mut cur, mut start, mut drawn) = (D { x: 0.0, y: 0.0 }, D { x: 0.0, y: 0.0 }, false);
    let mut finish = |segs: &mut Vec<Seg>, closed: bool, at: D, drawn: bool| {
        if !drawn {
            segs.clear();
            return;
        }
        let (mut out_cum, mut loc_cum, mut o, mut l) = (Vec::with_capacity(segs.len()), Vec::with_capacity(segs.len()), 0.0, 0.0);
        for s in segs.iter() {
            out_cum.push(o);
            loc_cum.push(l);
            o += s.out_len();
            l += s.loc_len();
        }
        out.push(Sub { segs: std::mem::take(segs), closed, out_cum, loc_cum, out_total: o, loc_total: l, at });
    };
    let push = |segs: &mut Vec<Seg>, c: [D; 4], line: bool| {
        // A segment whose points are all one point has no direction: it draws nothing itself.
        let span = c.iter().map(|p| p.sub(c[0]).len()).fold(0.0, f64::max);
        if span > 1e-9 {
            segs.push(Seg::new(c, line, to_local));
        }
    };
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                finish(&mut segs, close_all, start, drawn);
                (cur, start, drawn) = (D::of(p), D::of(p), false);
            }
            PathCmd::LineTo(p) => {
                let p = D::of(p);
                push(&mut segs, [cur, cur, p, p], true);
                (cur, drawn) = (p, true);
            }
            PathCmd::QuadTo(q, p) => {
                let (q, p) = (D::of(q), D::of(p));
                push(&mut segs, [cur, cur.add(q.sub(cur).mul(2.0 / 3.0)), p.add(q.sub(p).mul(2.0 / 3.0)), p], false);
                (cur, drawn) = (p, true);
            }
            PathCmd::CubicTo(a, b, p) => {
                let (a, b, p) = (D::of(a), D::of(b), D::of(p));
                push(&mut segs, [cur, a, b, p], false);
                (cur, drawn) = (p, true);
            }
            PathCmd::Close => {
                if cur.sub(start).len() > 1e-9 {
                    push(&mut segs, [cur, cur, start, start], true);
                }
                finish(&mut segs, true, start, true);
                (cur, drawn) = (start, false);
            }
        }
        if !cur.finite() {
            return None;
        }
    }
    if close_all && drawn && cur.sub(start).len() > 1e-9 {
        push(&mut segs, [cur, cur, start, start], true);
    }
    finish(&mut segs, close_all, start, drawn);
    out.iter().all(|s| s.segs.iter().all(|g| g.c.iter().all(|p| p.finite()) && g.out_len().is_finite())).then_some(out)
}

// ----- the pieces -------------------------------------------------------------------------------

/// A cross-section of the stroke: the two edge points, their derivatives along the segment's
/// parameter, and the direction of travel.
struct Cross {
    pl: D,
    dl: D,
    pr: D,
    dr: D,
    tan: D,
    wl: f64,
    wr: f64,
}

struct Builder<'a> {
    shape: &'a StrokeShape<'a>,
    hw: f64,
    pieces: Vec<Path>,
    /// Too many pieces: the outline is declined.
    over: bool,
}

impl Builder<'_> {
    fn emit(&mut self, p: Path) {
        if self.pieces.len() >= MAX_PIECES {
            self.over = true;
        } else if p.cmds.len() >= 4 && p.cmds.iter().all(cmd_finite) {
            self.pieces.push(p);
        }
    }

    /// Half-widths `(left, right)` at `t` of segment `i`.
    fn widths(&self, sub: &Sub, i: usize, t: f64) -> (f64, f64) {
        let Some(f) = self.shape.widths else { return (self.hw, self.hw) };
        let Some(seg) = sub.segs.get(i) else { return (0.0, 0.0) };
        let at = sub.loc_cum.get(i).copied().unwrap_or(0.0) + Seg::len_at(&seg.loc_tab, t);
        let frac = if sub.loc_total > 0.0 { (at / sub.loc_total).clamp(0.0, 1.0) } else { 0.0 };
        let (l, r) = f(frac as f32);
        let clean = |x: f32| if x.is_finite() { (x as f64).clamp(0.0, 1e6) * self.hw } else { 0.0 };
        if self.shape.flip { (clean(r), clean(l)) } else { (clean(l), clean(r)) }
    }

    fn cross(&self, sub: &Sub, i: usize, t: f64) -> Option<Cross> {
        let seg = sub.segs.get(i)?;
        let (p, d1, d2, tan) = (seg.point(t), seg.d1(t), seg.d2(t), seg.tangent(t));
        let n = tan.left();
        let (wl, wr) = self.widths(sub, i, t);
        // n' = left(T'), T' = (c'' − T (T·c'')) / |c'|.
        let speed = d1.len();
        let dn = if speed > 1e-9 { d2.sub(tan.mul(tan.dot(d2))).mul(1.0 / speed).left() } else { D { x: 0.0, y: 0.0 } };
        // How the widths change along the parameter (none for one width).
        let (dwl, dwr) = if self.shape.widths.is_some() {
            let h = 1e-3;
            let (t0, t1) = ((t - h).max(0.0), (t + h).min(1.0));
            let ((l0, r0), (l1, r1)) = (self.widths(sub, i, t0), self.widths(sub, i, t1));
            ((l1 - l0) / (t1 - t0), (r1 - r0) / (t1 - t0))
        } else {
            (0.0, 0.0)
        };
        Some(Cross { pl: p.add(n.mul(wl)), dl: d1.add(n.mul(dwl)).add(dn.mul(wl)), pr: p.sub(n.mul(wr)), dr: d1.sub(n.mul(dwr)).sub(dn.mul(wr)), tan, wl, wr })
    }

    /// The ribbons of segment `i` from `t0` to `t1`.
    fn ribbon(&mut self, sub: &Sub, i: usize, t0: f64, t1: f64, depth: u32) {
        if self.over || t1 - t0 <= 1e-12 {
            return;
        }
        let (Some(a), Some(b)) = (self.cross(sub, i, t0), self.cross(sub, i, t1)) else { return };
        let line = sub.segs.get(i).is_some_and(|s| s.line);
        if line && self.shape.widths.is_none() {
            if a.wl.max(a.wr) > NO_WIDTH {
                let mut p = Path::new();
                p.move_to(a.pl.vec()).line_to(b.pl.vec()).line_to(b.pr.vec()).line_to(a.pr.vec()).close();
                self.emit(p);
            }
            return;
        }
        let dt = t1 - t0;
        let k = dt / 3.0;
        let left = [a.pl, a.pl.add(a.dl.mul(k)), b.pl.sub(b.dl.mul(k)), b.pl];
        let right = [a.pr, a.pr.add(a.dr.mul(k)), b.pr.sub(b.dr.mul(k)), b.pr];
        let mut fits = a.tan.dot(b.tan) > 0.5;
        let mut widest = a.wl.max(a.wr).max(b.wl).max(b.wr);
        if depth < MAX_DEPTH {
            for u in [0.25, 0.5, 0.75] {
                let Some(m) = self.cross(sub, i, t0 + dt * u) else { return };
                widest = widest.max(m.wl).max(m.wr);
                fits = fits
                    && hermite(&left, u).sub(m.pl).len() <= TOLERANCE
                    && hermite(&right, u).sub(m.pr).len() <= TOLERANCE
                    && a.tan.dot(m.tan) > 0.7
                    && m.tan.dot(b.tan) > 0.7;
            }
            if !fits {
                let mid = t0 + dt * 0.5;
                self.ribbon(sub, i, t0, mid, depth + 1);
                self.ribbon(sub, i, mid, t1, depth + 1);
                return;
            }
        }
        if widest <= NO_WIDTH {
            return;
        }
        let mut p = Path::new();
        p.move_to(left[0].vec());
        p.cubic_to(left[1].vec(), left[2].vec(), left[3].vec());
        line_to_new(&mut p, right[3]);
        p.cubic_to(right[2].vec(), right[1].vec(), right[0].vec());
        p.close();
        self.emit(p);
    }

    /// What fills the outside of the bend at `p` from direction `u` to `v`.
    fn join(&mut self, p: D, u: D, v: D, (l, r): (f64, f64)) {
        if u.dot(v) > 0.0 && u.cross(v).abs() * l.max(r) < 0.005 {
            return;
        }
        // The gap opens on the outside of the bend, around the direction u − v.
        let mid = u.sub(v).unit();
        let (n0, n1) = (u.left(), v.left());
        let (s, radius) = if mid.dot(n0) >= 0.0 { (1.0, l) } else { (-1.0, r) };
        if radius <= NO_WIDTH {
            return;
        }
        let (o0, o1) = (n0.mul(s), n1.mul(s));
        let (a, b) = (p.add(o0.mul(radius)), p.add(o1.mul(radius)));
        let mut path = Path::new();
        path.move_to(p.vec()).line_to(a.vec());
        match self.shape.join {
            LineJoin::Round => {
                let (a0, a1, am) = (o0.angle(), o1.angle(), mid.angle());
                let mut sweep = wrap(a1 - a0);
                let to_mid = wrap(am - a0);
                // Where the path nearly turns back, go round the side that passes through the
                // gap's middle; on a gentler bend the short way is the outside.
                if u.dot(v) < -0.5 && (to_mid * sweep < 0.0 || to_mid.abs() > sweep.abs()) {
                    sweep -= std::f64::consts::TAU.copysign(sweep);
                }
                arc(&mut path, p, radius, a0, sweep);
            }
            LineJoin::Miter | LineJoin::Bevel => {
                // The outer edges meet 1 / cos(θ/2) half-widths out (θ the turn).
                let c = 1.0 + o0.dot(o1);
                let limit = (self.shape.miter_limit as f64).clamp(1.0, 100.0);
                if self.shape.join == LineJoin::Miter && c > 1e-6 && c * limit * limit >= 2.0 {
                    path.line_to(p.add(o0.add(o1).mul(radius / c)).vec());
                }
                path.line_to(b.vec());
            }
        }
        path.close();
        self.emit(path);
    }

    /// The cap at `at`, facing `out` (away from the piece), across the cross-section with left
    /// normal `nrm`. `end`: the piece's last point.
    fn cap(&mut self, at: D, out: D, nrm: D, (l, r): (f64, f64), end: bool) {
        let half = (l + r) * 0.5;
        if half <= NO_WIDTH {
            return;
        }
        let mut path = Path::new();
        match self.shape.cap {
            LineCap::Butt => return,
            LineCap::Round => {
                // Half a disc across the cross-section (so a one-sided width stays on its side).
                let c = at.add(nrm.mul((l - r) * 0.5));
                path.move_to(c.add(nrm.mul(half)).vec());
                arc(&mut path, c, half, nrm.angle(), if end { std::f64::consts::PI } else { -std::f64::consts::PI });
            }
            LineCap::Square => {
                let (pl, pr) = (at.add(nrm.mul(l)), at.sub(nrm.mul(r)));
                path.move_to(pl.vec()).line_to(pl.add(out.mul(half)).vec()).line_to(pr.add(out.mul(half)).vec()).line_to(pr.vec());
            }
        }
        path.close();
        self.emit(path);
    }

    /// One run of a subpath from `(i0, t0)` to `(i1, t1)`: ribbons, the joins between its
    /// segments (and round the start when `closed`), and caps when open.
    fn run(&mut self, sub: &Sub, (i0, t0): (usize, f64), (i1, t1): (usize, f64), closed: bool) {
        for i in i0..=i1 {
            let (a, b) = (if i == i0 { t0 } else { 0.0 }, if i == i1 { t1 } else { 1.0 });
            self.ribbon(sub, i, a, b, 0);
            if i > i0 {
                self.join_between(sub, i - 1, i);
            }
        }
        if closed {
            self.join_between(sub, i1, i0);
            return;
        }
        if let (Some(s0), Some(s1)) = (sub.segs.get(i0), sub.segs.get(i1)) {
            let (d0, d1) = (s0.tangent(t0), s1.tangent(t1));
            let (w0, w1) = (self.widths(sub, i0, t0), self.widths(sub, i1, t1));
            self.cap(s0.point(t0), d0.mul(-1.0), d0.left(), w0, false);
            self.cap(s1.point(t1), d1, d1.left(), w1, true);
        }
    }

    fn join_between(&mut self, sub: &Sub, before: usize, after: usize) {
        if let (Some(a), Some(b)) = (sub.segs.get(before), sub.segs.get(after)) {
            let w = self.widths(sub, after, 0.0);
            self.join(b.point(0.0), a.tangent(1.0), b.tangent(0.0), w);
        }
    }

    fn subpath(&mut self, sub: &Sub) {
        let n = sub.segs.len();
        if n == 0 {
            // No length: round caps still draw a dot, as the renderer does.
            if self.shape.cap == LineCap::Round && self.shape.join == LineJoin::Round && self.shape.dash.is_none() && self.shape.widths.is_none() {
                let (d, w) = (D { x: 1.0, y: 0.0 }, (self.hw, self.hw));
                self.cap(sub.at, d.mul(-1.0), d.left(), w, false);
                self.cap(sub.at, d, d.left(), w, true);
            }
            return;
        }
        let dash = self.shape.dash.filter(|(l, _)| l.len() >= 2 && l.iter().all(|x| x.is_finite() && *x >= 0.0) && l.iter().sum::<f32>() > 1e-6);
        let Some((pattern, offset)) = dash else {
            self.run(sub, (0, 0.0), (n - 1, 1.0), sub.closed);
            return;
        };
        let Some(intervals) = dash_intervals(pattern, offset as f64, sub.out_total) else {
            self.over = true;
            return;
        };
        for (a, b) in intervals {
            let (from, to) = (locate(sub, a), locate(sub, b));
            if b - a <= 1e-6 {
                // A dot: the caps of a piece with no length.
                if let Some(seg) = sub.segs.get(from.0) {
                    let (p, d, w) = (seg.point(from.1), seg.tangent(from.1), self.widths(sub, from.0, from.1));
                    self.cap(p, d.mul(-1.0), d.left(), w, false);
                    self.cap(p, d, d.left(), w, true);
                }
                continue;
            }
            self.run(sub, from, to, false);
            if self.over {
                return;
            }
        }
    }
}

fn cmd_finite(c: &PathCmd) -> bool {
    match c {
        PathCmd::MoveTo(p) | PathCmd::LineTo(p) => p.is_finite(),
        PathCmd::QuadTo(a, p) => a.is_finite() && p.is_finite(),
        PathCmd::CubicTo(a, b, p) => a.is_finite() && b.is_finite() && p.is_finite(),
        PathCmd::Close => true,
    }
}

fn hermite(c: &[D; 4], t: f64) -> D {
    let u = 1.0 - t;
    c[0].mul(u * u * u).add(c[1].mul(3.0 * u * u * t)).add(c[2].mul(3.0 * u * t * t)).add(c[3].mul(t * t * t))
}

/// A line to `p` unless the path is already there.
fn line_to_new(path: &mut Path, p: D) {
    let at = match path.cmds.last() {
        Some(PathCmd::MoveTo(q) | PathCmd::LineTo(q) | PathCmd::QuadTo(_, q) | PathCmd::CubicTo(_, _, q)) => Some(*q),
        _ => None,
    };
    if at != Some(p.vec()) {
        path.line_to(p.vec());
    }
}

fn wrap(x: f64) -> f64 {
    (x + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

/// An arc round `c` from angle `a0` through `sweep` radians, as cubics of at most a quarter
/// turn (each within 0.03 % of the radius), from where the path is.
fn arc(path: &mut Path, c: D, r: f64, a0: f64, sweep: f64) {
    if !(sweep.is_finite() && a0.is_finite() && r.is_finite()) {
        return;
    }
    let n = ((sweep.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 4);
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |a: f64| D { x: c.x + r * a.cos(), y: c.y + r * a.sin() };
    let tangent = |a: f64| D { x: -r * a.sin(), y: r * a.cos() };
    for j in 0..n {
        let (a, b) = (a0 + step * j as f64, a0 + step * (j + 1) as f64);
        path.cubic_to(at(a).add(tangent(a).mul(k)).vec(), at(b).sub(tangent(b).mul(k)).vec(), at(b).vec());
    }
}

/// Segment and parameter at length `s` along a subpath.
fn locate(sub: &Sub, s: f64) -> (usize, f64) {
    let n = sub.segs.len();
    let i = sub.out_cum.partition_point(|&c| c <= s).saturating_sub(1).min(n.saturating_sub(1));
    let t = sub.segs.get(i).map_or(0.0, |seg| seg.t_at(s - sub.out_cum.get(i).copied().unwrap_or(0.0)));
    (i, t)
}

/// The on stretches `[a, b]` of a dash pattern along a subpath `total` long, in order (a dot
/// is a stretch with `a == b`); `None` past [`MAX_DASHES`]. The renderer's rule: the subpath
/// starts `offset` into the pattern, and positions are computed, not summed.
fn dash_intervals(pattern: &[f32], offset: f64, total: f64) -> Option<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    let period: f64 = pattern.iter().map(|x| *x as f64).sum();
    if !(total.is_finite() && total > 0.0 && period.is_finite() && period > 0.0) {
        return Some(out);
    }
    let base = -(if offset.is_finite() { offset.rem_euclid(period) } else { 0.0 });
    let mut starts = Vec::with_capacity(pattern.len());
    let mut acc = 0.0f64;
    for &len in pattern {
        starts.push(acc);
        acc += len as f64;
    }
    let periods = ((total - base) / period).ceil();
    if !(periods.is_finite() && periods * pattern.len() as f64 <= 4.0 * MAX_DASHES as f64) {
        return None;
    }
    for k in 0..periods.max(0.0) as u64 + 1 {
        let p0 = base + k as f64 * period;
        for (i, (&len, &st)) in pattern.iter().zip(&starts).enumerate() {
            let (s, len) = (p0 + st, len as f64);
            if s > total {
                return Some(out);
            }
            let e = s + len;
            if i % 2 == 0 && (if len > 0.0 { e > 0.0 && s < total } else { s >= 0.0 }) {
                if out.len() >= MAX_DASHES {
                    return None;
                }
                out.push((s.max(0.0), e.min(total)));
            }
        }
    }
    Some(out)
}

/// The closed pieces that together cover the stroke of `path`: fill each non-zero and unite
/// them ([`unite`]). `None` when the path has a non-finite point or the stroke would take more
/// than [`MAX_PIECES`] pieces. Empty when it draws nothing.
pub fn stroke_pieces(path: &Path, shape: &StrokeShape) -> Option<Vec<Path>> {
    pieces_of(path, shape, false)
}

fn pieces_of(path: &Path, shape: &StrokeShape, close_all: bool) -> Option<Vec<Path>> {
    let hw = shape.half_width as f64;
    if !(hw.is_finite() && hw > 0.0) {
        return Some(Vec::new());
    }
    let subs = subpaths(path, shape.to_local.as_ref(), close_all)?;
    let mut b = Builder { shape, hw, pieces: Vec::new(), over: false };
    for sub in &subs {
        b.subpath(sub);
        if b.over {
            return None;
        }
    }
    Some(b.pieces)
}

/// The stroke of `path` as one filled outline of loops that don't overlap (even-odd and
/// non-zero fill it alike). Empty when the stroke draws nothing; `None` when it is too much to
/// outline at once.
pub fn stroke_outline(path: &Path, shape: &StrokeShape) -> Option<Path> {
    let pieces = stroke_pieces(path, shape)?;
    unite(&pieces, scale_for(&pieces, shape.half_width))
}

// ----- unions -----------------------------------------------------------------------------------

/// The scale to unite at, so the planar map's snap distance is small beside `feature` (a half-
/// width or an offset) and coordinates still fit single precision.
fn scale_for(paths: &[Path], feature: f32) -> f32 {
    let mut reach = 1.0f32;
    for p in paths {
        let b = p.bounds();
        if !b.is_empty() {
            reach = reach.max(b.min.x.abs()).max(b.min.y.abs()).max(b.max.x.abs()).max(b.max.y.abs());
        }
    }
    let wanted = if feature.is_finite() && feature > 0.0 { (8.0 / feature).clamp(16.0, 256.0) } else { 16.0 };
    // Keep scaled coordinates below 2^21: single precision still resolves 1/4 of a unit there.
    wanted.min(2_097_152.0 / reach).max(1.0)
}

fn segments(p: &Path) -> usize {
    p.cmds.iter().filter(|c| !matches!(c, PathCmd::MoveTo(_))).count()
}

/// The area `pieces` cover together (each filled non-zero), as loops that don't overlap.
/// United at `scale` (see the module notes); pieces that don't touch are united apart. `None`
/// when one tangle of pieces is too much for the planar map.
pub fn unite(pieces: &[Path], scale: f32) -> Option<Path> {
    let scale = if scale.is_finite() && scale >= 1.0 { scale } else { 1.0 };
    let up = Affine::scale(scale, scale);
    let scaled: Vec<Path> = pieces.iter().filter(|p| !p.is_empty()).map(|p| p.transformed(&up)).collect();
    let mut out = Path::new();
    for group in clusters(&scaled) {
        let members: Vec<&Path> = group.iter().filter_map(|&i| scaled.get(i)).collect();
        out.cmds.extend(unite_run(&members)?.cmds);
    }
    Some(out.transformed(&Affine::scale(1.0 / scale, 1.0 / scale)))
}

/// Pieces grouped by overlapping bounds (transitively), each group in its pieces' order.
fn clusters(pieces: &[Path]) -> Vec<Vec<usize>> {
    let pad = planar::SNAP * 2.0;
    let bounds: Vec<Rect> = pieces.iter().map(|p| p.bounds().inflate(pad)).collect();
    let mut parent: Vec<usize> = (0..pieces.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while let Some(&p) = parent.get(i) {
            if p == i {
                break;
            }
            let g = parent.get(p).copied().unwrap_or(p);
            if let Some(slot) = parent.get_mut(i) {
                *slot = g;
            }
            i = g;
        }
        i
    }
    // Sweep along x: a piece is compared with those whose bounds are still open.
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_by(|&a, &b| {
        let (xa, xb) = (bounds.get(a).map_or(0.0, |r| r.min.x), bounds.get(b).map_or(0.0, |r| r.min.x));
        xa.total_cmp(&xb)
    });
    let mut open: Vec<usize> = Vec::new();
    for &i in &order {
        let Some(bi) = bounds.get(i) else { continue };
        open.retain(|&j| bounds.get(j).is_some_and(|bj| bj.max.x >= bi.min.x));
        for &j in &open {
            if bounds.get(j).is_some_and(|bj| bj.min.y <= bi.max.y && bi.min.y <= bj.max.y) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b
                    && let Some(slot) = parent.get_mut(a.max(b))
                {
                    *slot = a.min(b);
                }
            }
        }
        open.push(i);
    }
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); pieces.len()];
    for i in 0..pieces.len() {
        let root = find(&mut parent, i);
        if let Some(g) = groups.get_mut(root) {
            g.push(i);
        }
    }
    groups.retain(|g| !g.is_empty());
    groups
}

/// The union of pieces that follow one another: all at once when few, else the union of the
/// unions of each half (what is inside a half's outline no longer takes part).
fn unite_run(pieces: &[&Path]) -> Option<Path> {
    let total: usize = pieces.iter().map(|p| segments(p)).sum();
    if pieces.len() <= 1 || total <= UNION_SEGMENTS {
        let items: Vec<(&Path, FillRule)> = pieces.iter().map(|p| (*p, FillRule::NonZero)).collect();
        return planar::union(&items);
    }
    let (a, b) = pieces.split_at(pieces.len() / 2);
    let (a, b) = (unite_run(a)?, unite_run(b)?);
    planar::union(&[(&a, FillRule::EvenOdd), (&b, FillRule::EvenOdd)])
}

/// `a` without what `b` covers (both loops that don't overlap, as the unions here are).
fn subtract(a: &Path, b: &Path) -> Option<Path> {
    if a.is_empty() || b.is_empty() {
        return Some(a.clone());
    }
    let items = [Item { path: a, fill: Some(0), fill_rule: FillRule::EvenOdd, stroke: None }, Item { path: b, fill: Some(1), fill_rule: FillRule::EvenOdd, stroke: None }];
    let merged = planar::merge(&items)?;
    let mut out = Path::new();
    for piece in merged.fills.into_iter().filter(|p| p.style == 0) {
        out.cmds.extend(piece.path.cmds);
    }
    Some(out)
}

// ----- offsets ----------------------------------------------------------------------------------

/// The fill of `path` grown by `distance` (shrunk when negative), as loops that don't overlap.
/// Growing rounds corners; shrinking keeps them. Empty when nothing is left; `None` when it is
/// too much to work out at once.
pub fn offset(path: &Path, rule: FillRule, distance: f32) -> Option<Path> {
    if !distance.is_finite() {
        return None;
    }
    let scale = scale_for(std::slice::from_ref(path), distance.abs());
    Offsetter::new(path, rule, scale)?.at(distance)
}

/// A fill ready to be offset several times: its outline as loops that don't overlap.
struct Offsetter {
    clean: Path,
    scale: f32,
}

impl Offsetter {
    fn new(path: &Path, rule: FillRule, scale: f32) -> Option<Offsetter> {
        let up = Affine::scale(scale, scale);
        let clean = planar::union(&[(&path.transformed(&up), rule)])?.transformed(&Affine::scale(1.0 / scale, 1.0 / scale));
        Some(Offsetter { clean, scale })
    }

    fn at(&self, distance: f32) -> Option<Path> {
        if distance == 0.0 || self.clean.is_empty() {
            return Some(self.clean.clone());
        }
        // The band within the distance of the boundary: the boundary stroked, round.
        let pieces = pieces_of(&self.clean, &StrokeShape::round(distance.abs()), true)?;
        if distance > 0.0 {
            let mut all = Vec::with_capacity(pieces.len() + 1);
            all.push(self.clean.clone());
            all.extend(pieces);
            return unite(&all, self.scale);
        }
        let band = unite(&pieces, self.scale)?;
        let (up, down) = (Affine::scale(self.scale, self.scale), Affine::scale(1.0 / self.scale, 1.0 / self.scale));
        Some(subtract(&self.clean.transformed(&up), &band.transformed(&up))?.transformed(&down))
    }
}

/// Most steps [`soften`] makes.
pub const MAX_SOFTEN_STEPS: u32 = 50;

/// A fill with a soft edge `distance` wide in `steps` bands: the core first, then each band
/// from the core outwards (the caller steps their alpha down). With `inset` the edge is taken
/// from the fill (the core is the fill shrunk by `distance`); otherwise it is added round it
/// (the core is the fill). Bands and core don't overlap; empty ones are kept in place so the
/// index tells the band. `None` when it is too much to work out, or the params make no sense.
pub fn soften(path: &Path, rule: FillRule, distance: f32, steps: u32, inset: bool) -> Option<Vec<Path>> {
    if !(distance.is_finite() && distance > 0.0 && (1..=MAX_SOFTEN_STEPS).contains(&steps)) {
        return None;
    }
    let step = distance / steps as f32;
    let scale = scale_for(std::slice::from_ref(path), step);
    let fill = Offsetter::new(path, rule, scale)?;
    let base = if inset { -distance } else { 0.0 };
    let (up, down) = (Affine::scale(scale, scale), Affine::scale(1.0 / scale, 1.0 / scale));
    let mut out = Vec::with_capacity(steps as usize + 1);
    // The last band of an inset ends on the fill itself, exactly.
    let outline = |i: u32| if inset && i == steps { Some(fill.clean.clone()) } else { fill.at(base + step * i as f32) };
    let mut inner = outline(0)?;
    out.push(inner.clone());
    for i in 1..=steps {
        let outer = outline(i)?;
        out.push(subtract(&outer.transformed(&up), &inner.transformed(&up))?.transformed(&down));
        inner = outer;
    }
    Some(out)
}

#[cfg(test)]
#[path = "outline_tests.rs"]
mod tests;
