//! Shape hints as Animate saves them (ADR 0023).
//!
//! Animate writes a shape tween's hints as a `<morphHintsList>` on the keyframe that starts the
//! tween, one `<MorphHint startPoint="x, y" endPoint="x, y" key="…"/>` per hint in letter order
//! (`a` first), each record carrying its point on both keyframes. Learned by observing files
//! Animate saved; no Animate code involved.
//!
//! Each coordinate is decimal or `#` hex fixed point (signed 24.8, as in `edges`), in units of
//! 256 twips: `#FFFFFB.FB, #0.84` is (-1029, 132) twips, a vertex of the start shape. `key` is
//! 0 in every file seen and is not read; nor is the `<morphHintsList>` inside a frame's
//! `<MorphShape>`, which was empty in every file seen.
//!
//! craft-xfl's `shape_hints` reads only the `<shapeHints>` spelling; this belongs there.

use crate::xml::Element as X;
use anicraft_geom::Vec2;

/// One native unit is 256 twips.
const TWIPS_PER_UNIT: f32 = 256.0;
const TWIPS_PER_PIXEL: f32 = 20.0;

/// One `<MorphHint>`: its points in pixels on the start and end keyframes, each `None` when
/// missing or malformed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MorphHint {
    pub start: Option<Vec2>,
    pub end: Option<Vec2>,
}

/// The hints a frame lists in `<morphHintsList>`, in file order (any child counts as a hint).
pub(crate) fn list(frame: &X) -> impl Iterator<Item = MorphHint> + '_ {
    frame.list("morphHintsList").iter().map(|h| MorphHint { start: point(h, "startPoint"), end: point(h, "endPoint") })
}

/// A `"x, y"` attribute in pixels.
fn point(hint: &X, name: &str) -> Option<Vec2> {
    let mut parts = hint.attr(name)?.split([',', ' ']).filter(|p| !p.trim().is_empty());
    let (x, y) = (number(parts.next()?)?, number(parts.next()?)?);
    parts.next().is_none().then(|| Vec2::new(x * TWIPS_PER_UNIT / TWIPS_PER_PIXEL, y * TWIPS_PER_UNIT / TWIPS_PER_PIXEL))
}

/// A decimal number, or `#` hex fixed point: up to eight whole digits (two's complement once
/// shifted into 24.8) and up to two fraction digits (a single one is the high nibble).
fn number(s: &str) -> Option<f32> {
    let s = s.trim();
    let value = match s.strip_prefix('#') {
        Some(hex) => {
            let (whole, frac) = hex.split_once('.').unwrap_or((hex, ""));
            let digits = |d: &str| d.bytes().all(|b| b.is_ascii_hexdigit());
            if !digits(whole) || !digits(frac) || whole.len() > 8 || frac.len() > 2 || (whole.is_empty() && frac.is_empty()) {
                return None;
            }
            let whole = if whole.is_empty() { 0 } else { u32::from_str_radix(whole, 16).ok()? };
            let frac = if frac.is_empty() { 0 } else { u32::from_str_radix(frac, 16).ok()? << if frac.len() == 1 { 4 } else { 0 } };
            (whole.wrapping_shl(8) | frac) as i32 as f32 / 256.0
        }
        None => s.parse::<f32>().ok()?,
    };
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(xml: &str) -> X {
        crate::xml::parse(xml.as_bytes()).unwrap()
    }

    #[test]
    fn reads_hex_and_decimal_points_in_units_of_256_twips() {
        let f = frame(
            r##"<DOMFrame><morphHintsList>
                <MorphHint startPoint="#FFFFFB.FB, #0.84" endPoint="#FFFFFC.3B, -0.0078125" key="0"/>
                <MorphHint startPoint="#29.D4, 4.76953125" endPoint="#28.A, #5" key="0"/>
            </morphHintsList></DOMFrame>"##,
        );
        let hints: Vec<_> = list(&f).collect();
        assert_eq!(hints.len(), 2);
        // (-1029, 132) twips; (-965, -2) twips.
        assert_eq!(hints[0].start, Some(Vec2::new(-1029.0 / 20.0, 132.0 / 20.0)));
        assert_eq!(hints[0].end, Some(Vec2::new(-965.0 / 20.0, -2.0 / 20.0)));
        // (10708, 1221) twips; a single fraction digit is the high nibble: #28.A is 0x28A0.
        assert_eq!(hints[1].start, Some(Vec2::new(10708.0 / 20.0, 1221.0 / 20.0)));
        assert_eq!(hints[1].end, Some(Vec2::new(10400.0 / 20.0, 1280.0 / 20.0)));
    }

    #[test]
    fn malformed_points_are_none_never_a_panic() {
        let bad = [
            "", ",", "1", "1,", "1, 2, 3", "#", "#.", "#G, 0", "#1.234, 0", "#123456789, 0", "nan, 0", "inf, 1", "1e40, 0", "#-1, 0",
            "#+1, 0", "🙂, 1", "#FFFFFFFF.FF, #80000000",
        ];
        for s in bad {
            let f = frame(&format!(r#"<DOMFrame><morphHintsList><MorphHint startPoint="{s}"/></morphHintsList></DOMFrame>"#));
            let hint = list(&f).next().unwrap();
            assert_eq!(hint.end, None, "{s:?}");
            // The two largest hex values still decode, to finite numbers.
            if s.starts_with("#FFFFFFFF") {
                assert!(hint.start.is_some_and(|p| p.x.is_finite() && p.y.is_finite()));
            } else {
                assert_eq!(hint.start, None, "{s:?}");
            }
        }
        assert_eq!(list(&frame("<DOMFrame/>")).count(), 0);
        assert_eq!(list(&frame("<DOMFrame><morphHintsList/></DOMFrame>")).count(), 0);
    }
}
