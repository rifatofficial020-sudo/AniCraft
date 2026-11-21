//! A reference video on a layer (ADR 0116): a film to trace over.
//!
//! The document keeps only *where the video is* (a file on this computer), where it sits on the
//! stage and which timeline frame shows which moment of it. Its pictures are never in the
//! document: the editor decodes them out of process, on demand, and shows them on the stage
//! alone. So [`crate::evaluate`] stays a pure function of the document: for a layer with a
//! video it lists one placeholder (a rectangle with no paint, marked as guide artwork), which
//! draws nothing in the movie or any export, and which the stage fills with the frame.

use crate::ElementId;
use anicraft_geom::Affine;
use serde::{Deserialize, Serialize};

/// Longest edge a reference video is shown at, in stage pixels.
pub const MAX_VIDEO_EDGE: u32 = 8192;
/// Longest video a layer refers to: a day.
pub const MAX_VIDEO_SECONDS: f64 = 86_400.0;
/// Most timeline frames a video covers, and the last frame it may start on.
pub const MAX_VIDEO_FRAMES: u32 = 100_000;
/// Longest file name kept.
pub const MAX_VIDEO_PATH: usize = 4096;

/// A layer's reference video.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoRef {
    /// Names the video's placeholder in a display list (from the document's id counter; it
    /// is no element, and nothing selects it).
    pub id: ElementId,
    /// The video file, as a path on this computer.
    pub path: String,
    /// The picture's size in the layer's space, before `transform`.
    pub width: u32,
    pub height: u32,
    /// Where the picture sits on the stage.
    #[serde(default)]
    pub transform: Affine,
    /// The timeline frame that shows the video's moment `offset`.
    #[serde(default)]
    pub start: u32,
    /// Seconds into the video shown at `start`.
    #[serde(default)]
    pub offset: f64,
    /// The video's length in seconds.
    pub duration: f64,
}

impl VideoRef {
    /// The moment of the video, in seconds, that timeline frame `frame` shows at `fps`; `None`
    /// before the video starts and after it ends. Frame `start + n` shows `offset + n / fps`
    /// exactly: the mapping is a function of the frame alone.
    pub fn time_at(&self, frame: u32, fps: f32) -> Option<f64> {
        let fps = f64::from(fps);
        if frame < self.start || !(fps.is_finite() && fps > 0.0) {
            return None;
        }
        let t = self.offset + f64::from(frame - self.start) / fps;
        (t.is_finite() && t >= 0.0 && t < self.duration).then_some(t)
    }

    /// How many timeline frames the video covers at `fps` (at least one).
    pub fn frames(&self, fps: f32) -> u32 {
        let left = ((self.duration - self.offset).max(0.0) * f64::from(fps.max(0.0))).ceil();
        if left.is_finite() { (left as u32).clamp(1, MAX_VIDEO_FRAMES) } else { 1 }
    }

    /// The reference as a loaded file may keep it; `None` when it can't be repaired.
    pub fn sanitized(&self) -> Option<VideoRef> {
        let size = |v: u32| (1..=MAX_VIDEO_EDGE).contains(&v);
        if self.path.is_empty() || self.path.len() > MAX_VIDEO_PATH || !size(self.width) || !size(self.height) {
            return None;
        }
        if !(self.duration.is_finite() && self.duration > 0.0) {
            return None;
        }
        let mut v = self.clone();
        v.duration = v.duration.min(MAX_VIDEO_SECONDS);
        if !(v.offset.is_finite() && v.offset >= 0.0 && v.offset < v.duration) {
            v.offset = 0.0;
        }
        if !v.transform.is_finite() {
            v.transform = Affine::IDENTITY;
        }
        v.start = v.start.min(MAX_VIDEO_FRAMES);
        Some(v)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn video() -> VideoRef {
        VideoRef { id: ElementId(9), path: "walk.mp4".into(), width: 320, height: 240, transform: Affine::IDENTITY, start: 4, offset: 0.5, duration: 2.0 }
    }

    #[test]
    fn frames_map_to_moments_exactly() {
        let v = video();
        assert_eq!(v.time_at(3, 24.0), None, "before it starts");
        assert_eq!(v.time_at(4, 24.0), Some(0.5));
        for n in 0..36u32 {
            assert_eq!(v.time_at(4 + n, 24.0), Some(0.5 + f64::from(n) / 24.0));
        }
        assert_eq!(v.time_at(4 + 36, 24.0), None, "1.5 s later the video has ended");
        assert_eq!(v.frames(24.0), 36);
        assert_eq!(v.frames(12.0), 18);
        assert_eq!(v.time_at(4, 0.0), None);
        assert_eq!(v.time_at(4, f32::NAN), None);
    }

    #[test]
    fn hostile_references_are_repaired_or_dropped() {
        assert_eq!(video().sanitized(), Some(video()));
        let bad = [
            VideoRef { path: String::new(), ..video() },
            VideoRef { path: "x".repeat(MAX_VIDEO_PATH + 1), ..video() },
            VideoRef { width: 0, ..video() },
            VideoRef { height: MAX_VIDEO_EDGE + 1, ..video() },
            VideoRef { duration: f64::NAN, ..video() },
            VideoRef { duration: -1.0, ..video() },
        ];
        assert!(bad.iter().all(|v| v.sanitized().is_none()));
        let fixed = VideoRef { offset: f64::INFINITY, duration: 1e30, start: u32::MAX, transform: Affine { m: [f32::NAN; 6] }, ..video() }.sanitized().expect("repairable");
        assert_eq!((fixed.offset, fixed.duration, fixed.transform), (0.0, MAX_VIDEO_SECONDS, Affine::IDENTITY));
        assert!(fixed.start <= MAX_VIDEO_FRAMES && fixed.frames(240.0) <= MAX_VIDEO_FRAMES);
    }
}
