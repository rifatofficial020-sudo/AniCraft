//! SVG geometry that AniCraft's path model needs: elliptical arcs as cubic Béziers, using the
//! endpoint-to-centre conversion of the SVG implementation notes (W3C SVG 1.1 Appendix F.6,
//! Royalty-Free Patent Policy), at most a quarter turn per cubic.
//!
//! Reading path data (`d`) is craft-svg's job; `anicraft_svg::parse_path_data` maps what it
//! reads onto a [`Path`] with this. [`crate::Path::from_svg`] stays the strict parser for
//! AniCraft's own file format.

use crate::{Path, Vec2};

/// Most commands one path may hold.
pub use crate::path::MAX_PATH_CMDS;

#[derive(Clone, Copy)]
struct P(f64, f64);

impl P {
    fn v(self) -> Vec2 {
        Vec2::new(self.0 as f32, self.1 as f32)
    }
}

/// Appends the SVG elliptical arc from `from` to `to` as cubics (F.6.5 / F.6.6): radii, the
/// x-axis rotation in degrees and the two flags as path data writes them.
#[allow(clippy::too_many_arguments)]
pub fn arc_to(path: &mut Path, from: [f64; 2], rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, to: [f64; 2]) {
    let (p0, p1) = (P(from[0], from[1]), P(to[0], to[1]));
    if (p0.0 - p1.0).abs() < 1e-12 && (p0.1 - p1.1).abs() < 1e-12 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        path.line_to(p1.v());
        return;
    }
    let (sin, cos) = rot_deg.to_radians().sin_cos();
    let (dx, dy) = ((p0.0 - p1.0) * 0.5, (p0.1 - p1.1) * 0.5);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    // Radii too small to reach: scale them up (F.6.6).
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
    if large == sweep {
        coef = -coef;
    }
    let (cxp, cyp) = (coef * rx * y1 / ry, -coef * ry * x1 / rx);
    let (cx, cy) = (cos * cxp - sin * cyp + (p0.0 + p1.0) * 0.5, sin * cxp + cos * cyp + (p0.1 + p1.1) * 0.5);
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((x1 - cxp) / rx, (y1 - cyp) / ry);
    let (vx, vy) = ((-x1 - cxp) / rx, (-y1 - cyp) / ry);
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    let tau = std::f64::consts::TAU;
    if !sweep && delta > 0.0 {
        delta -= tau;
    } else if sweep && delta < 0.0 {
        delta += tau;
    }
    let n = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().clamp(1.0, 4.0) as usize;
    let step = delta / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    // A point / tangent on the unit circle, mapped onto the rotated ellipse.
    let map = |x: f64, y: f64| P(cx + rx * cos * x - ry * sin * y, cy + rx * sin * x + ry * cos * y);
    let mut t = theta1;
    for i in 0..n {
        let (s0, c0) = t.sin_cos();
        let t1 = t + step;
        let (s1, c1) = t1.sin_cos();
        let a = map(c0 - k * s0, s0 + k * c0);
        let b = map(c1 + k * s1, s1 - k * c1);
        // Land exactly on the requested end point.
        let e = if i + 1 == n { p1 } else { map(c1, s1) };
        path.cubic_to(a.v(), b.v(), e.v());
        t = t1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcs_land_on_the_ellipse() {
        // A half circle of radius 5 from (0,0) to (10,0), sweeping through (5,-5) or (5,5).
        for (sweep, mid_y) in [(true, -5.0), (false, 5.0)] {
            let mut p = Path::new();
            p.move_to(Vec2::ZERO);
            arc_to(&mut p, [0.0, 0.0], 5.0, 5.0, 0.0, false, sweep, [10.0, 0.0]);
            let lines = p.flatten(&crate::Affine::IDENTITY, 0.01);
            let pts = &lines[0].points;
            assert!(pts.iter().all(|q| (q.distance(Vec2::new(5.0, 0.0)) - 5.0).abs() < 0.05), "on the circle");
            assert!(pts.iter().any(|q| (q.y - mid_y).abs() < 0.05), "through the right side ({sweep})");
            assert_eq!(pts.last().copied(), Some(Vec2::new(10.0, 0.0)));
        }
    }

    #[test]
    fn degenerate_arcs() {
        // No distance: nothing. Zero radius: a straight line.
        let mut p = Path::new();
        p.move_to(Vec2::ZERO);
        arc_to(&mut p, [0.0, 0.0], 5.0, 5.0, 0.0, false, true, [0.0, 0.0]);
        assert_eq!(p.cmds.len(), 1);
        arc_to(&mut p, [0.0, 0.0], 0.0, 5.0, 0.0, false, true, [3.0, 4.0]);
        assert_eq!(p.cmds.len(), 2);
    }
}
