//! Unions of flattened polygons (ADR 0110).
//!
//! The renderers work out a pixel's coverage by adding up the signed area of every edge that
//! crosses it, which is exact only when the loops they are given don't overlap: a pixel partly
//! inside two overlapping loops gets both parts. [`union`] turns loops that may overlap (or
//! fold over themselves) into the loops of the area they cover together, each point of it
//! inside exactly one of them, so that sum is the true coverage.
//!
//! The union is taken on an integer grid (the `i_overlay` crate): every point is snapped to
//! one of about 2³⁰ steps across the loops' bounds, far below anything a pixel shows, and
//! from there the work is exact, so edges that touch, cross at a shallow angle or lie on one
//! another come out the same on every platform.

use crate::Vec2;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::i_float::float::compatible::FloatPointCompatible;

impl FloatPointCompatible for Vec2 {
    type Scalar = f32;

    #[inline(always)]
    fn from_xy(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[inline(always)]
    fn x(&self) -> f32 {
        self.x
    }

    #[inline(always)]
    fn y(&self) -> f32 {
        self.y
    }
}

/// Coordinates past this are left alone (nothing drawn is that far out; the grid above would
/// be coarser than single precision itself).
const MAX_COORDINATE: f32 = 1e12;

/// Twice the signed area of a closed loop (the closing edge is implicit).
fn area2(pts: &[Vec2]) -> f64 {
    let Some(&last) = pts.last() else { return 0.0 };
    let mut prev = last;
    let mut a = 0.0f64;
    for &p in pts {
        a += f64::from(prev.x) * f64::from(p.y) - f64::from(prev.y) * f64::from(p.x);
        prev = p;
    }
    a
}

/// Whether a closed loop is convex and goes round once: every turn the same way (collinear
/// points allowed) and a whole turn in all. Such a loop can't overlap itself.
pub fn is_convex(pts: &[Vec2]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let at = |i: usize| pts.get(i % n).copied().unwrap_or(Vec2::ZERO);
    let mut sign = 0.0f32;
    // A loop that winds round twice also turns one way throughout: count the times its
    // direction passes from pointing down to pointing up.
    let mut wraps = 0u32;
    let mut prev = at(0) - at(n - 1);
    for i in 0..n {
        let d = at(i + 1) - at(i);
        if !d.is_finite() {
            return false;
        }
        if d.x == 0.0 && d.y == 0.0 {
            continue;
        }
        let c = prev.cross(d);
        if c != 0.0 {
            if sign != 0.0 && (c > 0.0) != (sign > 0.0) {
                return false;
            }
            sign = c;
        } else if prev.dot(d) < 0.0 {
            // Straight back along the way it came.
            return false;
        }
        if (prev.y < 0.0 || (prev.y == 0.0 && prev.x < 0.0)) != (d.y < 0.0 || (d.y == 0.0 && d.x < 0.0)) {
            wraps += 1;
        }
        prev = d;
    }
    sign != 0.0 && wraps <= 2
}

/// The area `loops` cover together (each closed, its closing edge implicit; a point is
/// covered where their winding numbers don't add up to zero), as loops that don't overlap:
/// outer boundaries one way round and holes the other, so the non-zero and even-odd rules
/// fill them alike and a renderer that adds up signed areas gets the true coverage.
///
/// Loops with fewer than three points or with a point that isn't finite are dropped. Loops
/// that all go the same way round never cancel one another.
pub fn union(loops: Vec<Vec<Vec2>>) -> Vec<Vec<Vec2>> {
    let mut loops = loops;
    loops.retain(|l| l.len() >= 3 && l.iter().all(|p| p.is_finite()));
    // One convex loop is its own union.
    if let [only] = loops.as_slice()
        && is_convex(only)
    {
        return loops;
    }
    if loops.is_empty() || loops.iter().flatten().any(|p| p.x.abs() > MAX_COORDINATE || p.y.abs() > MAX_COORDINATE) {
        return loops;
    }
    loops.simplify_shape(FillRule::NonZero).into_iter().flatten().filter(|l| l.len() >= 3).collect()
}

/// The area inside `loops` under the non-zero rule when they don't overlap (as [`union`]
/// returns them): the sum of their signed areas, holes counting against.
pub fn area(loops: &[Vec<Vec2>]) -> f64 {
    (loops.iter().map(|l| area2(l)).sum::<f64>() * 0.5).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Vec2> {
        vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn overlapping_loops_become_one_outline() {
        let out = union(vec![rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 5.0, 15.0, 15.0)]);
        assert_eq!(out.len(), 1);
        assert!(close(area(&out), 175.0, 1e-3), "{}", area(&out));
        // Loops apart stay apart; one inside another disappears.
        let out = union(vec![rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 0.0, 30.0, 10.0), rect(2.0, 2.0, 4.0, 4.0)]);
        assert_eq!(out.len(), 2);
        assert!(close(area(&out), 200.0, 1e-3));
    }

    #[test]
    fn a_ring_keeps_its_hole_and_touching_loops_join() {
        // Four bars round a square hole.
        let bars = vec![rect(0.0, 0.0, 30.0, 10.0), rect(20.0, 0.0, 30.0, 30.0), rect(0.0, 20.0, 30.0, 30.0), rect(0.0, 0.0, 10.0, 30.0)];
        let out = union(bars);
        assert_eq!(out.len(), 2, "an outer loop and the hole");
        assert!(close(area(&out), 800.0, 1e-3), "{}", area(&out));
        // Sharing an edge exactly.
        let out = union(vec![rect(0.0, 0.0, 10.0, 10.0), rect(10.0, 0.0, 20.0, 10.0)]);
        assert_eq!(out.len(), 1);
        assert!(close(area(&out), 200.0, 1e-3));
        // The same loop twice, and a loop that goes round twice, are covered once.
        let twice: Vec<Vec2> = rect(0.0, 0.0, 10.0, 10.0).into_iter().chain(rect(0.0, 0.0, 10.0, 10.0)).collect();
        for loops in [vec![rect(0.0, 0.0, 10.0, 10.0), rect(0.0, 0.0, 10.0, 10.0)], vec![twice]] {
            let out = union(loops);
            assert!(close(area(&out), 100.0, 1e-3), "{}", area(&out));
        }
    }

    #[test]
    fn a_loop_that_crosses_itself_is_untangled() {
        // A bow tie: two triangles that meet at a point, wound opposite ways.
        let bow = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 10.0)];
        let out = union(vec![bow]);
        assert!(close(area(&out), 50.0, 1e-3), "{}", area(&out));
    }

    #[test]
    fn convex_loops_are_told_from_the_rest() {
        assert!(is_convex(&rect(0.0, 0.0, 4.0, 3.0)));
        let mut back = rect(0.0, 0.0, 4.0, 3.0);
        back.reverse();
        assert!(is_convex(&back));
        // Collinear and repeated points are fine.
        assert!(is_convex(&[Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0)]));
        // A dent, a bow tie, a spike, two turns round, a line and nothing are not.
        assert!(!is_convex(&[Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 4.0)]));
        assert!(!is_convex(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 10.0)]));
        assert!(!is_convex(&[Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 3.0)]));
        let star: Vec<Vec2> = (0..5).map(|i| (i as f32 * 2.0 * std::f32::consts::TAU / 5.0).sin_cos()).map(|(s, c)| Vec2::new(c, s)).collect();
        assert!(!is_convex(&star), "a pentagram turns one way but goes round twice");
        assert!(!is_convex(&[Vec2::ZERO, Vec2::new(1.0, 0.0)]));
        assert!(!is_convex(&[Vec2::ZERO; 5]));
        assert!(!is_convex(&[]));
    }

    #[test]
    fn odd_input_is_safe() {
        assert!(union(Vec::new()).is_empty());
        assert!(union(vec![vec![Vec2::ZERO, Vec2::new(1.0, 0.0)]]).is_empty());
        assert!(union(vec![vec![Vec2::ZERO, Vec2::new(f32::NAN, 0.0), Vec2::new(1.0, 1.0)]]).is_empty());
        assert!(union(vec![vec![Vec2::ZERO, Vec2::new(f32::INFINITY, 0.0), Vec2::new(1.0, 1.0)]]).is_empty());
        // Nothing but a point or a line: no area, no loops, no panic.
        assert!(area(&union(vec![vec![Vec2::new(3.0, 3.0); 4], rect(0.0, 0.0, 0.0, 9.0)])) < 1e-6);
        // Far out and tiny: left as they are or united, never a panic.
        let far = union(vec![rect(1e20, 0.0, 2e20, 1e20), rect(0.0, 0.0, 1.0, 1.0), rect(0.5, 0.5, 2.0, 2.0)]);
        assert!(far.iter().flatten().all(|p| p.is_finite()));
        let tiny = union(vec![rect(0.0, 0.0, 1e-20, 1e-20), rect(0.0, 0.0, 1e-20, 2e-20)]);
        assert!(tiny.iter().flatten().all(|p| p.is_finite()));
        let wide = union(vec![rect(-1e9, -1e9, 1e9, 1e9), rect(0.0, 0.0, 2e9, 2e9)]);
        assert!(close(area(&wide) / 7e18, 1.0, 1e-3));
    }
}
