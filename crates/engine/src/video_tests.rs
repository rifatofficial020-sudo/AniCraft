//! Importing video (ADR 0116): what ffmpeg says and sends is read without trust; frames land
//! on exact timeline frames; a cancel or a failure leaves the document alone; a reference
//! video shows on the stage and in no export. The tests that need a real ffmpeg make their
//! own tiny video with it, and are skipped (with a line saying so) where there is none.

use super::*;
use crate::video::{self, Cache, Lookup, Options, Probe, parse_probe, plan, read_ppm};
use anicraft_doc::LayerKind;
use anicraft_doc::video::VideoRef;
use serde_json::json;
use std::path::{Path, PathBuf};

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anicraft-video-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The document as its file would hold it: equal text, equal documents.
fn snap(doc: &anicraft_doc::Document) -> String {
    serde_json::to_string(doc).unwrap()
}

/// A small deterministic generator (no dependency, the same on every run).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const MP4: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'walk.mp4':
  Metadata:
    major_brand     : isom
  Duration: 00:01:02.50, start: 0.000000, bitrate: 2106 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 1971 kb/s, 29.97 fps, 29.97 tbr, 30k tbn (default)
      Metadata:
        handler_name    : VideoHandler
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 128 kb/s (default)
At least one output file must be specified
";

#[test]
fn what_ffmpeg_says_about_a_file_is_read() {
    assert_eq!(parse_probe(MP4), Some(Probe { width: 1920, height: 1080, duration: Some(62.5), fps: Some(29.97), audio: true }));
    // A phone's upright film: stored on its side, shown turned.
    let phone = "  Duration: 00:00:03.00, start: 0.000000, bitrate: 900 kb/s
  Stream #0:0(und): Video: hevc (Main) (hvc1 / 0x31637668), yuv420p(tv), 1920x1080, 800 kb/s, 30 fps, 30 tbr, 600 tbn (default)
      Side data:
        displaymatrix: rotation of -90.00 degrees
";
    assert_eq!(parse_probe(phone), Some(Probe { width: 1080, height: 1920, duration: Some(3.0), fps: Some(30.0), audio: false }));
    // Cover art is not the video; a stream with no length says so.
    let art = "  Duration: N/A, start: 0.000000, bitrate: N/A
  Stream #0:0: Video: mjpeg (Baseline), yuvj420p(pc), 600x600, 90k tbr (attached pic)
  Stream #0:1: Video: vp9 (Profile 0), yuv420p(tv), 320x240, SAR 1:1 DAR 4:3, 25 fps, 25 tbr, 1k tbn
";
    assert_eq!(parse_probe(art), Some(Probe { width: 320, height: 240, duration: None, fps: Some(25.0), audio: false }));
    // No video at all, nonsense, and numbers that are not sizes.
    for text in ["", "walk.mp4: No such file or directory", "  Stream #0:0: Audio: mp3, 44100 Hz, stereo", "  Stream #0:0: Video: h264, 0x31637661, 0x0, 99999999x2, x, 12x, x12"] {
        assert_eq!(parse_probe(text), None, "{text}");
    }
    assert_eq!(parse_probe("Duration: 99:99:99.99,\nStream #0:0: Video: raw, 8x8").and_then(|p| p.duration), Some(anicraft_doc::video::MAX_VIDEO_SECONDS), "a length is capped");
    assert_eq!(parse_probe("Duration: -1:00:00,\nStream #0:0: Video: raw, 8x8").and_then(|p| p.duration), None);
}

#[test]
fn fuzz_hostile_descriptions_never_panic() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let junk = ["Stream #", ": Video:", ": Audio:", "Duration:", "rotation of ", "x", ",", " fps", "\n", "99999999999999999999", "-", ":", "NaN", "inf", "1e999", "\u{fffd}", "é", "(attached pic)", " "];
    let mut parsed = 0;
    for _ in 0..20_000 {
        let mut bytes = MP4.as_bytes().to_vec();
        for _ in 0..1 + rng.below(8) {
            match rng.below(4) {
                0 => {
                    let at = rng.below(bytes.len());
                    bytes.truncate(at);
                }
                1 => {
                    let at = rng.below(bytes.len() + 1);
                    let piece = junk[rng.below(junk.len())].as_bytes();
                    bytes.splice(at..at, piece.iter().copied());
                }
                2 if !bytes.is_empty() => {
                    let at = rng.below(bytes.len());
                    bytes[at] = rng.next() as u8;
                }
                _ if !bytes.is_empty() => {
                    let (a, b) = (rng.below(bytes.len()), rng.below(bytes.len()));
                    bytes.drain(a.min(b)..a.max(b));
                }
                _ => {}
            }
        }
        if let Some(p) = parse_probe(&String::from_utf8_lossy(&bytes)) {
            parsed += 1;
            assert!(p.width >= 1 && p.height >= 1 && p.width <= 99_999 && p.height <= 99_999);
            assert!(p.duration.is_none_or(|d| d > 0.0 && d <= anicraft_doc::video::MAX_VIDEO_SECONDS));
            assert!(p.fps.is_none_or(|f| f.is_finite() && f > 0.0));
            // Whatever it says, the plan stays inside the caps.
            let pl = plan(&p, 24.0, (1280, 720), &Options::default());
            assert!(pl.width <= video::MAX_DECODE_EDGE && pl.height <= video::MAX_DECODE_EDGE && pl.limit >= 1 && pl.limit <= video::MAX_EMBED_FRAMES);
            assert!(pl.memory <= video::EMBED_BUDGET.max(u64::from(pl.width) * u64::from(pl.height) * 4));
        }
    }
    assert!(parsed > 1000, "the fuzz still reaches the parser's accepting paths ({parsed})");
}

fn ppm(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let mut v = format!("P6\n{w} {h}\n255\n").into_bytes();
    v.extend((0..w * h * 3).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
    v
}

#[test]
fn pictures_are_read_from_a_pipe_with_every_size_checked() {
    let mut stream = ppm(3, 2, 0);
    stream.extend(ppm(3, 2, 9));
    let mut r = &stream[..];
    let a = read_ppm(&mut r, 8, 8).unwrap().unwrap();
    assert_eq!((a.width, a.height, a.rgba.len()), (3, 2, 24));
    assert_eq!(&a.rgba[..8], &[0, 31, 62, 255, 93, 124, 155, 255], "RGB becomes opaque RGBA");
    let b = read_ppm(&mut r, 8, 8).unwrap().unwrap();
    assert_eq!(b.rgba[0], 9);
    assert_eq!(read_ppm(&mut r, 8, 8), Ok(None), "a clean end");
    // Cut anywhere, it is an error (or a clean end before the first byte), never a picture.
    let one = ppm(3, 2, 0);
    for cut in 0..one.len() {
        let got = read_ppm(&mut &one[..cut], 8, 8);
        assert!(if cut == 0 { got == Ok(None) } else { got.is_err() }, "cut at {cut}: {got:?}");
    }
    // Larger than asked for, other kinds of image, and headers that lie.
    assert!(read_ppm(&mut &ppm(9, 2, 0)[..], 8, 8).unwrap_err().contains("larger"));
    assert!(read_ppm(&mut &ppm(2, 9, 0)[..], 8, 8).is_err());
    let hostile: [&[u8]; 9] = [&b"P5\n1 1\n255\n\0"[..], b"P6\n0 1\n255\n", b"P6\n1 1\n65535\n\0\0\0\0\0\0", b"P6\n1 -1\n255\n\0\0\0", b"P6 # note\n1 1\n255\n\0\0\0", b"P6\n9999999999 1\n255\n", b"P6\n1 1\n255", b"\x89PNG\r\n\x1a\n", b"P6\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n1 1\n255\n\0\0\0"];
    for bad in hostile {
        assert!(read_ppm(&mut &*bad, 4096, 4096).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
    // A header promising the largest picture with nothing behind it fails without a crash.
    assert!(read_ppm(&mut &b"P6\n4096 4096\n255\n"[..], 4096, 4096).is_err());
}

#[test]
fn fuzz_hostile_pictures_never_panic() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let good = ppm(5, 4, 3);
    let (mut ok, mut bad) = (0, 0);
    for round in 0..40_000 {
        let mut bytes = if round % 4 == 0 { (0..rng.below(64)).map(|_| rng.next() as u8).collect() } else { good.clone() };
        if round % 4 != 0 {
            for _ in 0..rng.below(4) {
                let at = rng.below(bytes.len().max(1));
                match rng.below(3) {
                    0 => bytes.truncate(at),
                    1 if at < bytes.len() => bytes[at] = b"0123456789 \nP6\xff"[rng.below(15)],
                    _ => bytes.insert(at.min(bytes.len()), b"0123456789 \n"[rng.below(12)]),
                }
            }
        }
        let mut r = &bytes[..];
        // Several pictures in a row, as a stream has: bounded, and every one inside the box.
        for _ in 0..4 {
            match read_ppm(&mut r, 16, 16) {
                Ok(Some(p)) => {
                    ok += 1;
                    assert!(p.width >= 1 && p.height >= 1 && p.width <= 16 && p.height <= 16);
                    assert_eq!(p.rgba.len(), (p.width * p.height * 4) as usize);
                }
                Ok(None) => break,
                Err(_) => {
                    bad += 1;
                    break;
                }
            }
        }
    }
    assert!(ok > 1000 && bad > 1000, "both paths are exercised ({ok} read, {bad} refused)");
}

#[test]
fn the_plan_tells_what_an_import_takes_before_it_starts() {
    let film = Probe { width: 1920, height: 1080, duration: Some(10.0), fps: Some(30.0), audio: true };
    // On a 1280 × 720 stage at 24 fps: shown at stage size, 240 frames.
    let p = plan(&film, 24.0, (1280, 720), &Options::default());
    assert_eq!((p.width, p.height, p.upscale, p.frames, p.truncated), (1280, 720, 1.0, Some(240), false));
    assert_eq!(p.memory, 240 * 1280 * 720 * 4);
    // Every third frame, at most 320 pixels: fewer, smaller pictures, shown four times larger.
    let small = plan(&film, 24.0, (1280, 720), &Options { step: 3, max_edge: 320, ..Options::default() });
    assert_eq!((small.width, small.height, small.upscale, small.frames), (320, 180, 4.0, Some(80)));
    // From 4 s for 2 s.
    let part = plan(&film, 24.0, (1280, 720), &Options { start: 4.0, length: Some(2.0), ..Options::default() });
    assert_eq!((part.seconds, part.frames), (Some(2.0), Some(48)));
    // A small video is never enlarged.
    let tiny = plan(&Probe { width: 64, height: 48, ..film.clone() }, 24.0, (1280, 720), &Options::default());
    assert_eq!((tiny.width, tiny.height, tiny.upscale), (64, 48, 1.0));
    // A long video stops at the frame cap, and says so.
    let long = plan(&Probe { duration: Some(3600.0), ..film.clone() }, 24.0, (1280, 720), &Options { max_edge: 320, ..Options::default() });
    assert_eq!((long.frames, long.limit, long.truncated), (Some(video::MAX_EMBED_FRAMES), video::MAX_EMBED_FRAMES, true));
    // Large pictures stop at the memory cap first: 1 GB of 1280 × 720 pictures is 291.
    let big = plan(&Probe { duration: Some(3600.0), ..film.clone() }, 24.0, (1280, 720), &Options::default());
    assert_eq!((big.limit, big.truncated), (291, true));
    assert!(big.memory <= video::EMBED_BUDGET);
    // A file that doesn't say its length: the caps still hold.
    let unknown = plan(&Probe { duration: None, ..film }, 24.0, (1280, 720), &Options::default());
    assert_eq!((unknown.frames, unknown.truncated, unknown.limit), (None, false, 291));
}

/// An empty 1280 × 720 document with one tracing stroke on its layer (nothing covers the
/// stage: the demo's backdrop would hide a video under it).
fn blank() -> anicraft_doc::Document {
    let mut s = Session::new(anicraft_doc::Document::default());
    run(&mut s, "doc.set_stage", json!({"width": 1280, "height": 720}));
    run(&mut s, "draw.freehand", json!({"points": [[600, 300], [640, 340], [700, 320]], "stroke_width": 4}));
    s.doc.clone()
}

/// `doc` with its layers' videos taken off (the layers stay): what the movie should equal.
fn without_video(doc: &anicraft_doc::Document) -> anicraft_doc::Document {
    let mut plain = doc.clone();
    for scene in &mut plain.scenes {
        for layer in &mut Arc::make_mut(scene).timeline.layers {
            if layer.video.is_some() {
                Arc::make_mut(layer).video = None;
            }
        }
    }
    plain
}

/// A document with a reference video on a guide layer at the bottom, from frame 2.
fn doc_with_reference() -> (anicraft_doc::Document, VideoRef) {
    let mut doc = blank();
    let v = VideoRef { id: anicraft_doc::ElementId(doc.alloc_id()), path: "nowhere/walk.mp4".into(), width: 320, height: 240, transform: anicraft_geom::Affine::translate(40.0, 30.0), start: 2, offset: 0.0, duration: 1.0 };
    let mut layer = anicraft_doc::Layer::new(anicraft_doc::LayerId(doc.alloc_id()), "walk");
    layer.kind = LayerKind::Guide;
    layer.keyframes = vec![Arc::new(anicraft_doc::Keyframe::new(0, 30))];
    layer.video = Some(Arc::new(v.clone()));
    Arc::make_mut(&mut doc.scenes[0]).timeline.layers.push(Arc::new(layer));
    (doc, v)
}

fn picture(w: u32, h: u32, rgb: [u8; 3]) -> Arc<anicraft_doc::Pixels> {
    Arc::new(anicraft_doc::Pixels { width: w, height: h, rgba: (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect() })
}

#[test]
fn a_reference_video_is_on_the_stage_and_in_no_export() {
    let (doc, v) = doc_with_reference();
    let plain = without_video(&doc);
    // The movie: every frame renders exactly as without the video layer.
    for frame in [0, 2, 10, 25] {
        let with = crate::cmds_file::render_png(&doc, frame, 0.25).unwrap();
        let without = crate::cmds_file::render_png(&plain, frame, 0.25).unwrap();
        assert_eq!(with, without, "frame {frame}");
    }
    assert_eq!(crate::cmds_file::frame_svg(&doc, 10), crate::cmds_file::frame_svg(&plain, 10));
    // Evaluation lists a placeholder with no paint while the video runs, under the artwork.
    let dl = anicraft_doc::evaluate(&doc, 10);
    let holders: Vec<_> = dl.items.iter().enumerate().filter(|(_, i)| i.element == v.id).collect();
    assert_eq!(holders.len(), 1);
    let (at, holder) = holders[0];
    assert_eq!(at, 0, "the bottom layer draws first");
    assert!(holder.guide && holder.fill.is_none() && holder.stroke.is_none() && holder.image.is_none());
    assert_eq!(holder.transform, v.transform);
    assert!(anicraft_doc::evaluate(&doc, 1).items.iter().all(|i| i.element != v.id), "not before it starts");
    assert!(anicraft_doc::evaluate(&doc, 2 + 24).items.iter().all(|i| i.element != v.id), "not after it ends (1 s at 24 fps)");
    // The stage's job fills it: the picture shows where no artwork covers it.
    let view = anicraft_render::View::stage(&doc, 0.5);
    let job = crate::render_service::make_job(1, &doc, 10, view, None);
    let filled = job.clone().with_reference(Some(Arc::new(vec![video::RefFrame { element: v.id, pixels: picture(4, 3, [255, 0, 255]), serial: 7 }])));
    assert_ne!(filled.key, job.key, "keyed apart from the frame without the picture");
    let other = job.clone().with_reference(Some(Arc::new(vec![video::RefFrame { element: v.id, pixels: picture(4, 3, [0, 255, 0]), serial: 8 }])));
    assert_ne!(filled.key, other.key, "and from another picture");
    assert_eq!(job.clone().with_reference(None).key, job.key);
    let dl = filled.display_list();
    let item = dl.items.iter().find(|i| i.element == v.id).unwrap();
    assert!(!item.guide && item.image.is_some() && matches!(item.fill, Some(anicraft_doc::Paint::Bitmap { .. })));
    let (a, b) = (crate::render_service::render_job(&job), crate::render_service::render_job(&filled));
    assert!(a.error.is_none() && b.error.is_none());
    assert_ne!(a.rgba_premul, b.rgba_premul);
    // The video's rectangle is (40, 30)–(360, 270) on the stage, (20, 15)–(180, 135) at half
    // scale: magenta inside it, the stage outside.
    let px = |img: &crate::render_service::FrameImage, x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba_premul[i], img.rgba_premul[i + 1], img.rgba_premul[i + 2]]
    };
    let magenta = (21..180).flat_map(|x| (16..135).map(move |y| (x, y))).filter(|&(x, y)| matches!(px(&b, x, y), [250..=255, 0..=5, 250..=255])).count();
    assert!(magenta > 18_000, "the picture fills its rectangle ({magenta} pixels)");
    assert_ne!(px(&b, 10, 10), [255, 0, 255], "and only inside its rectangle");
}

#[test]
fn the_reference_survives_saving_and_older_files_open() {
    let (doc, v) = doc_with_reference();
    let dir = temp_dir("format");
    let path = dir.join("ref.anicraft");
    crate::format::save(&doc, &path).unwrap();
    let back = crate::format::load(&path).unwrap();
    let layer = back.scenes[0].timeline.layers.last().unwrap();
    assert_eq!(layer.video.as_deref(), Some(&v));
    assert_eq!(layer.kind, LayerKind::Guide);
    // A file from before reference videos: no `video` key anywhere, and none after loading.
    let old = dir.join("old.anicraft");
    crate::format::save(&anicraft_doc::sample::demo(), &old).unwrap();
    assert!(!std::fs::read_to_string(&old).unwrap().contains("\"video\""));
    assert!(crate::format::load(&old).unwrap().scenes[0].timeline.layers.iter().all(|l| l.video.is_none()));
    // A file with a video no one could mean loses it and still opens.
    let text = std::fs::read_to_string(&path).unwrap().replace("\"width\":320", "\"width\":0").replace("\"width\": 320", "\"width\": 0");
    std::fs::write(&path, text).unwrap();
    let hostile = crate::format::load(&path).unwrap();
    assert!(hostile.scenes[0].timeline.layers.last().unwrap().video.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_picks_the_placeholder_and_set_video_changes_it() {
    let (doc, v) = doc_with_reference();
    let mut s = Session::new(doc);
    let id = s.timeline().layers.last().unwrap().id;
    run(&mut s, "timeline.goto", json!({"frame": 10}));
    // On the rectangle's edge and inside it: nothing of the video is hit.
    for p in [Vec2::new(40.0, 100.0), Vec2::new(41.0, 31.0)] {
        assert!(s.hit_test(p, 3.0).is_none_or(|(_, e)| e != v.id));
    }
    let props = run(&mut s, "layer.properties", json!({"layer": id.0}));
    assert_eq!(props["video"]["path"], "nowhere/walk.mp4");
    assert_eq!(props["video"]["missing"], true);
    let out = run(&mut s, "layer.set_video", json!({"layer": id.0, "start": 5, "offset": 0.25}));
    assert_eq!((out["video"]["start"].as_u64(), out["video"]["offset"].as_f64()), (Some(5), Some(0.25)));
    assert!(s.execute("layer.set_video", &json!({"layer": id.0, "offset": 1.0})).is_err(), "past the end");
    assert!(s.execute("layer.set_video", &json!({"layer": id.0, "path": "nowhere/else.mp4"})).is_err(), "not a file");
    assert!(s.execute("layer.set_video", &json!({"start": 1})).is_err(), "the active layer has no video");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.timeline().layer(id).unwrap().video.as_deref(), Some(&v));
    run(&mut s, "layer.set_video", json!({"layer": id.0, "remove": true}));
    assert!(s.timeline().layer(id).unwrap().video.is_none());
    assert!(crate::cmds_plugin::plugin_may_call("layer.set_video").is_err() && crate::cmds_plugin::plugin_may_call("file.import_video").is_err());
}

#[test]
fn without_ffmpeg_the_message_says_how_to_get_it() {
    let dir = temp_dir("missing");
    let clip = dir.join("clip.mp4");
    std::fs::write(&clip, b"not a video").unwrap();
    let mut s = Session::new(anicraft_doc::sample::demo());
    let before = s.doc.clone();
    let ghost = dir.join("no-such-ffmpeg").display().to_string();
    for (cmd, mode) in [("file.import_video", "frames"), ("file.import_video", "reference"), ("file.video_info", "frames")] {
        let e = s.execute(cmd, &json!({"path": clip.display().to_string(), "mode": mode, "ffmpeg": ghost})).unwrap_err().to_string();
        assert!(e.contains("ffmpeg.org") && e.contains("ANICRAFT_FFMPEG"), "{e}");
    }
    assert!(s.execute("file.import_video", &json!({"path": dir.join("nothing.mp4").display().to_string(), "ffmpeg": ghost})).unwrap_err().to_string().contains("not a file"));
    for bad in [json!({"mode": "film"}), json!({"step": 0}), json!({"step": 61}), json!({"max_edge": 8}), json!({"start": -1}), json!({"length": 0}), json!({"max_frames": 0}), json!({"max_frames": 5000})] {
        let mut p = bad.clone();
        p["path"] = json!(clip.display().to_string());
        p["ffmpeg"] = json!(ghost);
        assert!(matches!(s.execute("file.import_video", &p), Err(EngineError::Param(_))), "{bad}");
    }
    assert_eq!(snap(&s.doc), snap(&before), "nothing changed");
    // A cache with no ffmpeg and no file fails once, with a reason, and never waits.
    let cache = Cache::new(1 << 20);
    let v = VideoRef { id: anicraft_doc::ElementId(1), path: dir.join("gone.mp4").display().to_string(), width: 64, height: 48, transform: anicraft_geom::Affine::IDENTITY, start: 0, offset: 0.0, duration: 2.0 };
    assert!(matches!(cache.lookup(&v, 0, 12.0, true), Lookup::Pending(None)));
    assert!(cache.wait_idle(std::time::Duration::from_secs(20)));
    let Lookup::Failed(why) = cache.lookup(&v, 0, 12.0, true) else { panic!("a missing file fails") };
    assert!(why.contains("missing"), "{why}");
    assert!(matches!(cache.lookup(&v, 99, 12.0, true), Lookup::Outside));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ffmpeg to test with, when there is one here.
fn ffmpeg() -> Option<std::ffi::OsString> {
    let program = video::program(None);
    let ok = std::process::Command::new(&program).arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("no ffmpeg here: importing a real video was not tested");
    }
    ok.then_some(program)
}

/// A 2-second 64 × 48 test film at 25 fps with a 440 Hz tone, made by `ffmpeg` (its own test
/// pattern, MPEG-4 Part 2 video and PCM sound in Matroska: encoders every ffmpeg build has).
fn make_clip(ffmpeg: &std::ffi::OsStr, dir: &Path, sound: bool) -> PathBuf {
    let clip = dir.join(if sound { "clip.mkv" } else { "silent.mkv" });
    let mut c = std::process::Command::new(ffmpeg);
    c.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=2:size=64x48:rate=25"]);
    if sound {
        c.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-c:a", "pcm_s16le"]);
    }
    c.args(["-c:v", "mpeg4", "-q:v", "2"]).arg(&clip);
    let out = c.output().unwrap();
    assert!(out.status.success(), "ffmpeg could not make the test film: {}", String::from_utf8_lossy(&out.stderr));
    clip
}

#[test]
fn frames_land_on_exact_timeline_frames_with_the_sound() {
    let Some(ffmpeg) = ffmpeg() else { return };
    let dir = temp_dir("frames");
    let clip = make_clip(&ffmpeg, &dir, true);
    let path = clip.display().to_string();
    let mut s = Session::new(anicraft_doc::sample::demo());
    run(&mut s, "doc.set_stage", json!({"fps": 12}));
    run(&mut s, "timeline.goto", json!({"frame": 3}));
    let before = s.doc.clone();
    let layers = s.timeline().layers.len();

    let info = run(&mut s, "file.video_info", json!({"path": path}));
    assert_eq!((info["width"].as_u64(), info["height"].as_u64(), info["audio"].as_bool()), (Some(64), Some(48), Some(true)));
    assert_eq!(info["plan"]["frames"], 24, "2 s at the document's 12 fps");
    assert_eq!(snap(&s.doc), snap(&before), "asking changes nothing");

    let out = run(&mut s, "file.import_video", json!({"path": path}));
    assert_eq!((out["mode"].as_str(), out["frames"].as_u64(), out["width"].as_u64(), out["height"].as_u64(), out["truncated"].as_bool()), (Some("frames"), Some(24), Some(64), Some(48), Some(false)));
    assert_eq!(s.timeline().layers.len(), layers + 2, "the pictures' layer and the sound's");
    let layer = s.active_layer().unwrap().clone();
    assert_eq!((layer.name.as_str(), layer.id.0), ("clip", out["layer"].as_u64().unwrap()));
    // A blank keyframe to the playhead, then one keyframe per picture: picture j on frame 3 + j.
    let keys: Vec<(u32, u32, usize)> = layer.keyframes.iter().map(|k| (k.start, k.duration, k.elements.len())).collect();
    assert_eq!(keys[0], (0, 3, 0));
    assert_eq!(keys.len(), 25);
    for (j, k) in keys[1..].iter().enumerate() {
        assert_eq!(*k, (3 + j as u32, 1, 1), "picture {j}");
    }
    // Each keyframe shows its own bitmap, 64 × 48, in the middle of the stage, in a Library
    // folder named after the file; the pictures differ (the test pattern counts).
    let stage = s.doc.stage;
    let mut seen = std::collections::BTreeSet::new();
    for k in &layer.keyframes[1..] {
        let el = &k.elements[0];
        let anicraft_doc::ElementKind::Shape(shape) = &el.kind else { panic!("a shape") };
        let Some(anicraft_doc::Paint::Bitmap { bitmap, .. }) = shape.fill else { panic!("a bitmap fill") };
        let b = s.doc.bitmap(bitmap).unwrap();
        assert_eq!((b.width, b.height), (64, 48));
        assert!(seen.insert(b.png.clone()), "every picture is its own");
        assert_eq!(el.transform, anicraft_geom::Affine::translate((stage.width as f32 - 64.0) * 0.5, (stage.height as f32 - 48.0) * 0.5));
        assert!(s.doc.parent_of(anicraft_doc::library::LibraryEntry::Item(anicraft_doc::library::LibraryItem::Bitmap(bitmap))).is_some());
    }
    assert_eq!(s.doc.bitmaps.len(), before.bitmaps.len() + 24);
    // The sound: 2 s, a stream sound from the playhead on its own layer.
    let sid = anicraft_doc::SoundId(out["sound"].as_u64().unwrap());
    let snd = s.doc.sound(sid).unwrap();
    assert!((snd.seconds() - 2.0).abs() < 0.05, "{} s", snd.seconds());
    let on = s.timeline().layers.iter().find_map(|l| l.keyframes.iter().find(|k| k.sound.as_ref().is_some_and(|f| f.sound == sid))).unwrap();
    assert_eq!((on.start, on.sound.as_ref().unwrap().sync), (3, anicraft_doc::SoundSync::Stream));
    // It renders, saves and opens again; one undo takes all of it away.
    assert!(crate::cmds_file::render_png(&s.doc, 5, 0.25).is_ok());
    let file = dir.join("with-video.anicraft");
    run(&mut s, "file.save", json!({"path": file.display().to_string()}));
    assert_eq!(crate::format::load(&file).unwrap().bitmaps.len(), s.doc.bitmaps.len());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(snap(&s.doc), snap(&before), "one undo step");

    // Every second frame, half the size, from 0.5 s for 1 s, without sound: 6 pictures of
    // 32 × 24 held two frames each, shown at the video's own size.
    let out = run(&mut s, "file.import_video", json!({"path": path, "step": 2, "max_edge": 32, "start": 0.5, "length": 1.0, "audio": false}));
    assert_eq!((out["frames"].as_u64(), out["width"].as_u64(), out["height"].as_u64(), out["sound"].is_null()), (Some(6), Some(32), Some(24), true));
    let layer = s.active_layer().unwrap().clone();
    let keys: Vec<(u32, u32)> = layer.keyframes.iter().skip(1).map(|k| (k.start, k.duration)).collect();
    assert_eq!(keys, (0..6).map(|j| (3 + 2 * j, 2)).collect::<Vec<_>>());
    assert_eq!(layer.keyframes[1].elements[0].transform.mean_scale(), 2.0);
    assert_eq!(s.timeline().layers.len(), layers + 1);
    run(&mut s, "edit.undo", json!({}));

    // A cap that bites is reported, and the document gets exactly that many.
    let out = run(&mut s, "file.import_video", json!({"path": path, "max_frames": 5, "audio": false}));
    assert_eq!((out["frames"].as_u64(), out["truncated"].as_bool()), (Some(5), Some(true)));
    run(&mut s, "edit.undo", json!({}));

    // A film with no sound track imports without one; a file that is no video is refused.
    let silent = make_clip(&ffmpeg, &dir, false);
    let out = run(&mut s, "file.import_video", json!({"path": silent.display().to_string()}));
    assert!(out["sound"].is_null() && out["note"].is_null());
    run(&mut s, "edit.undo", json!({}));
    let text = dir.join("notes.mp4");
    std::fs::write(&text, b"these are not the frames you are looking for").unwrap();
    let e = s.execute("file.import_video", &json!({"path": text.display().to_string()})).unwrap_err().to_string();
    assert!(e.contains("no video"), "{e}");
    assert_eq!(snap(&s.doc), snap(&before));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_leaves_the_document_unchanged_and_a_prepared_import_is_taken() {
    let Some(ffmpeg) = ffmpeg() else { return };
    let dir = temp_dir("cancel");
    let clip = make_clip(&ffmpeg, &dir, true);
    let params = json!({"path": clip.display().to_string()});
    let mut s = Session::new(anicraft_doc::sample::demo());
    let before = s.doc.clone();
    // Cancelled while decoding: the command fails as cancelled, with nothing changed and
    // nothing to undo.
    let control = Arc::new(crate::publish::Control::default());
    control.cancel();
    s.job = Some(control.clone());
    assert!(matches!(s.execute("file.import_video", &params), Err(EngineError::Cancelled)));
    assert_eq!(snap(&s.doc), snap(&before));
    assert!(s.doc.bitmaps.len() == before.bitmaps.len() && s.doc.sounds.len() == before.sounds.len());
    s.job = None;
    // Prepared on another thread (as the editor does, with progress), then handed over: the
    // command takes it as it is. Here the prepared result is cut to 3 pictures to prove it.
    let stage = s.doc.stage;
    let req = video::Request { path: clip.clone(), ffmpeg: video::program(None), reference: false, options: Options { max_frames: video::MAX_EMBED_FRAMES, ..Options::default() }, audio: true, fps: stage.fps, stage: (stage.width, stage.height) };
    let progress = Arc::new(crate::publish::Control::default());
    let worker = {
        let (req, progress) = (req.clone(), progress.clone());
        std::thread::spawn(move || video::prepare(&req, Some(&progress)))
    };
    let mut prepared = worker.join().unwrap().unwrap();
    assert_eq!(progress.progress(), (48, 48), "2 s at 24 fps, counted as they were decoded");
    let video::What::Frames { bitmaps, .. } = &mut prepared.what else { panic!("frames") };
    assert_eq!(bitmaps.len(), 48);
    bitmaps.truncate(3);
    s.staged_video = Some(prepared);
    let out = run(&mut s, "file.import_video", params.clone());
    assert_eq!(out["frames"], 3);
    assert!(s.staged_video.is_none());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(snap(&s.doc), snap(&before));
    // A prepared import for something else is not taken.
    s.staged_video = Some(video::prepare(&req, None).unwrap());
    let out = run(&mut s, "file.import_video", json!({"path": clip.display().to_string(), "step": 2, "audio": false}));
    assert_eq!(out["frames"], 24);
    assert!(s.staged_video.is_none(), "and it doesn't linger");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reference_layer_shows_the_video_from_a_bounded_cache() {
    let Some(ffmpeg) = ffmpeg() else { return };
    let dir = temp_dir("reference");
    let clip = make_clip(&ffmpeg, &dir, true);
    let mut s = Session::new(blank());
    run(&mut s, "doc.set_stage", json!({"fps": 25}));
    run(&mut s, "timeline.goto", json!({"frame": 4}));
    let (active, layers, bitmaps) = (s.layer, s.timeline().layers.len(), s.doc.bitmaps.len());
    let out = run(&mut s, "file.import_video", json!({"path": clip.display().to_string(), "mode": "reference"}));
    assert_eq!((out["mode"].as_str(), out["frames"].as_u64(), out["width"].as_u64(), out["height"].as_u64()), (Some("reference"), Some(50), Some(64), Some(48)));
    // A locked guide layer at the bottom, spanning the video; nothing else changed.
    assert_eq!((s.timeline().layers.len(), s.doc.bitmaps.len(), s.doc.sounds.len(), s.layer), (layers + 1, bitmaps, 0, active));
    let layer = s.timeline().layers.last().unwrap().clone();
    assert_eq!((layer.kind, layer.locked, layer.name.as_str(), layer.end_frame()), (LayerKind::Guide, true, "clip", 54));
    let v = layer.video.as_deref().unwrap().clone();
    assert_eq!((v.start, v.offset, v.width, v.height), (4, 0.0, 64, 48));
    assert!((v.duration - 2.0).abs() < 0.05 && Path::new(&v.path).is_absolute());
    // In a document with layers below (the demo's backdrop), it goes right under the layer
    // being drawn on, which stays active.
    let mut d = Session::new(anicraft_doc::sample::demo());
    let on = d.active_layer_index().unwrap();
    let made = run(&mut d, "file.import_video", json!({"path": clip.display().to_string(), "mode": "reference"}));
    assert_eq!(d.timeline().layers[on + 1].id.0, made["layer"].as_u64().unwrap());
    assert_eq!(d.active_layer_index(), Some(on));
    assert!(d.timeline().layers.len() > on + 2, "with the demo's other layers under it");
    // The cache decodes on demand, a run at a time, and never blocks a look.
    let cache = Cache::new(1 << 20);
    assert!(matches!(cache.lookup(&v, 3, 25.0, true), Lookup::Outside), "before the video starts");
    assert!(matches!(cache.lookup(&v, 4, 25.0, false), Lookup::Pending(None)), "a look that doesn't ask starts nothing");
    assert!(!cache.pending());
    assert!(matches!(cache.lookup(&v, 4, 25.0, true), Lookup::Pending(None)));
    assert!(cache.wait_idle(std::time::Duration::from_secs(60)));
    let Lookup::Ready(first) = cache.lookup(&v, 4, 25.0, true) else { panic!("decoded") };
    assert_eq!((first.pixels.width, first.pixels.height, first.element), (64, 48, v.id));
    let Lookup::Ready(second) = cache.lookup(&v, 5, 25.0, true) else { panic!("the same run") };
    assert_ne!(first.pixels.rgba, second.pixels.rgba, "the test pattern moves");
    assert_ne!(first.serial, second.serial);
    // The frames the reference shows are the frames an import would embed: the same moments.
    let stage = s.doc.stage;
    let req = video::Request { path: clip.clone(), ffmpeg: ffmpeg.clone(), reference: false, options: Options::default(), audio: false, fps: 25.0, stage: (stage.width, stage.height) };
    let prepared = video::prepare(&req, None).unwrap();
    let video::What::Frames { bitmaps: embedded, .. } = &prepared.what else { panic!("frames") };
    assert_eq!(embedded.len(), 50);
    for n in [0u32, 1, 5, 15] {
        let Lookup::Ready(f) = cache.lookup(&v, 4 + n, 25.0, false) else { panic!("frame {n}") };
        assert_eq!(embedded[n as usize].pixels().unwrap().rgba, f.pixels.rgba, "frame {n}");
    }
    // A later run: asked for and decoded.
    assert!(matches!(cache.lookup(&v, 4 + 40, 25.0, true), Lookup::Pending(Some(_))), "meanwhile, the last picture shown");
    assert!(cache.wait_idle(std::time::Duration::from_secs(60)));
    let Lookup::Ready(late) = cache.lookup(&v, 4 + 40, 25.0, true) else { panic!("decoded") };
    assert_eq!(embedded[40].pixels().unwrap().rgba, late.pixels.rgba, "frame 40 is the 41st moment, in the third run");
    assert!(matches!(cache.lookup(&v, 4 + 50, 25.0, true), Lookup::Outside), "after it ends");
    // With a budget under two runs, the run shown and the one fetched ahead are all that is
    // ever held, however far the playhead goes, forwards and back.
    let tight = Cache::new(64 * 48 * 4 * 20);
    for frame in (4..54).chain((4..54).rev()) {
        let _ = tight.lookup(&v, frame, 25.0, true);
        assert!(tight.wait_idle(std::time::Duration::from_secs(60)));
        assert!(matches!(tight.lookup(&v, frame, 25.0, false), Lookup::Ready(_)), "frame {frame}");
        let (bytes, runs) = tight.usage();
        assert!(bytes <= 64 * 48 * 4 * 32 && runs <= 2, "{bytes} bytes in {runs} runs");
    }
    // The stage's view of the frame carries the picture; the movie doesn't.
    let shown = video::shown(&cache, &s.doc, 4, true);
    assert!(shown.frames.as_ref().is_some_and(|f| f.len() == 1) && !shown.pending && shown.failed.is_none());
    let view = anicraft_render::View::stage(&s.doc, 0.25);
    let job = crate::render_service::make_job(1, &s.doc, 4, view, None);
    let (plain, with) = (crate::render_service::render_job(&job), crate::render_service::render_job(&job.clone().with_reference(shown.frames)));
    assert_ne!(plain.rgba_premul, with.rgba_premul);
    assert_eq!(crate::cmds_file::render_png(&s.doc, 4, 0.25).unwrap(), crate::cmds_file::render_png(&without_video(&s.doc), 4, 0.25).unwrap());
    // Hidden, it isn't asked for; relinked to a film that is gone, it fails with a reason.
    run(&mut s, "layer.toggle_visible", json!({"layer": layer.id.0}));
    assert!(video::shown(&cache, &s.doc, 4, true).frames.is_none());
    run(&mut s, "layer.toggle_visible", json!({"layer": layer.id.0}));
    let moved = dir.join("moved.mkv");
    std::fs::copy(&clip, &moved).unwrap();
    let out = run(&mut s, "layer.set_video", json!({"layer": layer.id.0, "path": moved.display().to_string()}));
    assert_eq!(out["video"]["missing"], false);
    std::fs::remove_file(&moved).unwrap();
    let gone = video::shown(&cache, &s.doc, 4, true);
    assert!(gone.pending);
    assert!(cache.wait_idle(std::time::Duration::from_secs(60)));
    assert!(video::shown(&cache, &s.doc, 4, true).failed.is_some_and(|(l, why)| l == layer.id && why.contains("missing")));
    for _ in 0..6 {
        if s.timeline().layers.len() > layers {
            run(&mut s, "edit.undo", json!({}));
        }
    }
    assert_eq!(s.timeline().layers.len(), layers, "undo takes the layer away again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for ffmpeg (a shell script, so Unix only): the whole path from the command to the
/// document runs wherever the tests do, and ffmpeg's side of the pipe can be made hostile.
#[cfg(unix)]
mod fake {
    use super::*;

    /// Writes the stand-in to `dir` and gives its path. Asked about a file it describes an
    /// 8 × 6, one-second, 12 fps video with sound; asked for the sound it writes a tenth of a
    /// second of silence as a WAV; asked for pictures it sends what `kind` says:
    /// `good` (twelve 8 × 6 pictures filled with the bytes `A`, `B`, …, or as many as were asked
    /// for), `slow` (the same, a fifth of a second apart), `endless`, `noisy` (good, after
    /// 200 KB of messages), `big`, `junk`, `sizes`, `short` or `none`.
    fn ffmpeg(dir: &Path, kind: &str) -> String {
        let frame = r#"frame() { printf 'P6\n8 6\n255\n'; head -c 144 /dev/zero | tr '\000' "$(printf "\\$(printf %03o $((65 + $1)))")"; }"#;
        let good = r#"n=0; while [ $n -lt 12 ] && [ $n -lt "$frames" ]; do frame $n; n=$((n + 1)); done"#;
        let pictures = match kind {
            "good" => good.to_string(),
            "slow" => r#"n=0; while [ $n -lt 12 ] && [ $n -lt "$frames" ]; do frame $n; sleep 0.2; n=$((n + 1)); done"#.to_string(),
            "endless" => "n=0; while :; do frame $((n % 20)); n=$((n + 1)); done".to_string(),
            "noisy" => format!("head -c 200000 /dev/zero | tr '\\000' 'x' >&2; {good}"),
            "big" => r#"printf 'P6\n9000 9000\n255\n'; head -c 1000 /dev/zero"#.to_string(),
            "junk" => "printf 'GIF89a: not what was asked for'".to_string(),
            "sizes" => r#"frame 0; printf 'P6\n4 4\n255\n'; head -c 48 /dev/zero"#.to_string(),
            "short" => r#"frame 0; printf 'P6\n8 6\n255\n'; head -c 70 /dev/zero"#.to_string(),
            "none" => "echo 'Conversion failed!' >&2".to_string(),
            other => panic!("no stand-in called {other}"),
        };
        let wav = r#"{ printf 'RIFF\014\105\000\000WAVEfmt \020\000\000\000\001\000\002\000\104\254\000\000\020\261\002\000\004\000\020\000data\350\104\000\000'; head -c 17640 /dev/zero; } > "$last""#;
        let script = [
            "#!/bin/sh",
            r#"frames=1000000; ppm=0; wav=0; prev=""; last="""#,
            r#"for a in "$@"; do"#,
            r#"  case "$prev" in -frames:v) frames="$a";; esac"#,
            r#"  case "$a" in ppm) ppm=1;; wav) wav=1;; esac"#,
            r#"  prev="$a"; last="$a""#,
            "done",
            frame,
            r#"if [ "$wav" = 1 ]; then"#,
            wav,
            "  exit 0",
            "fi",
            r#"if [ "$ppm" = 1 ]; then"#,
            &pictures,
            "  exit 0",
            "fi",
            r#"echo "  Duration: 00:00:01.00, start: 0.000000, bitrate: 1 kb/s" >&2"#,
            r#"echo "  Stream #0:0: Video: fake, rgb24, 8x6, 12 fps" >&2"#,
            r#"echo "  Stream #0:1: Audio: pcm_s16le, 44100 Hz, stereo" >&2"#,
            "exit 1",
            "",
        ]
        .join("\n");
        let path = dir.join(format!("ffmpeg-{kind}"));
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.display().to_string()
    }

    /// A file to import (the stand-in never opens it) and a session at 12 fps on frame 2.
    fn setup(tag: &str) -> (PathBuf, String, Session) {
        let dir = temp_dir(tag);
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, b"a video, as far as the stand-in cares").unwrap();
        let mut s = Session::new(blank());
        run(&mut s, "doc.set_stage", json!({"fps": 12}));
        run(&mut s, "timeline.goto", json!({"frame": 2}));
        (dir, clip.display().to_string(), s)
    }

    #[test]
    fn a_stand_in_ffmpeg_imports_frames_sound_and_a_reference() {
        let (dir, clip, mut s) = setup("fake");
        let good = ffmpeg(&dir, "good");
        let before = snap(&s.doc);
        let layers = s.timeline().layers.len();
        let info = run(&mut s, "file.video_info", json!({"path": clip, "ffmpeg": good}));
        assert_eq!((info["width"].as_u64(), info["height"].as_u64(), info["seconds"].as_f64(), info["audio"].as_bool(), info["plan"]["frames"].as_u64()), (Some(8), Some(6), Some(1.0), Some(true), Some(12)));
        // Frames: twelve pictures on frames 2 to 13, each its own, with the sound under them.
        let out = run(&mut s, "file.import_video", json!({"path": clip, "ffmpeg": good}));
        assert_eq!((out["frames"].as_u64(), out["width"].as_u64(), out["height"].as_u64(), out["from"].as_u64(), out["truncated"].as_bool()), (Some(12), Some(8), Some(6), Some(2), Some(false)));
        let layer = s.active_layer().unwrap().clone();
        let keys: Vec<(u32, u32)> = layer.keyframes.iter().skip(1).map(|k| (k.start, k.duration)).collect();
        assert_eq!(keys, (0..12).map(|j| (2 + j, 1)).collect::<Vec<_>>());
        for (j, k) in layer.keyframes.iter().skip(1).enumerate() {
            let anicraft_doc::ElementKind::Shape(shape) = &k.elements[0].kind else { panic!("a shape") };
            let Some(anicraft_doc::Paint::Bitmap { bitmap, .. }) = shape.fill else { panic!("a bitmap fill") };
            let px = s.doc.bitmap(bitmap).unwrap().pixels().unwrap();
            assert_eq!(&px.rgba[..4], &[65 + j as u8, 65 + j as u8, 65 + j as u8, 255], "picture {j} is the video's picture {j}");
        }
        let sid = anicraft_doc::SoundId(out["sound"].as_u64().unwrap());
        assert!((s.doc.sound(sid).unwrap().seconds() - 0.1).abs() < 1e-6);
        let on = s.timeline().layers.iter().find_map(|l| l.keyframes.iter().find(|k| k.sound.as_ref().is_some_and(|f| f.sound == sid))).unwrap();
        assert_eq!((on.start, on.sound.as_ref().unwrap().sync), (2, anicraft_doc::SoundSync::Stream), "the sound starts with the pictures");
        assert_eq!(s.timeline().layers.len(), layers + 2);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(snap(&s.doc), before, "one undo step");
        // On twos, without sound, capped: three pictures held two frames each, and told.
        let out = run(&mut s, "file.import_video", json!({"path": clip, "ffmpeg": good, "step": 2, "max_frames": 3, "audio": false}));
        assert_eq!((out["frames"].as_u64(), out["truncated"].as_bool(), out["sound"].is_null()), (Some(3), Some(true), true));
        let keys: Vec<(u32, u32)> = s.active_layer().unwrap().keyframes.iter().skip(1).map(|k| (k.start, k.duration)).collect();
        assert_eq!(keys, vec![(2, 2), (4, 2), (6, 2)]);
        run(&mut s, "edit.undo", json!({}));
        // A reference layer: under the active layer, twelve frames from frame 2, and the cache
        // shows the video's own pictures on the timeline's frames.
        let out = run(&mut s, "file.import_video", json!({"path": clip, "ffmpeg": good, "mode": "reference"}));
        assert_eq!((out["frames"].as_u64(), out["width"].as_u64(), out["height"].as_u64()), (Some(12), Some(8), Some(6)));
        let layer = s.timeline().layers.last().unwrap().clone();
        assert_eq!((layer.kind, layer.locked, layer.end_frame()), (LayerKind::Guide, true, 14));
        let v = layer.video.as_deref().unwrap().clone();
        let cache = Cache::with_ffmpeg(1 << 20, Some(good.clone().into()));
        assert!(matches!(cache.lookup(&v, 2, 12.0, true), Lookup::Pending(None)));
        assert!(cache.wait_idle(std::time::Duration::from_secs(60)));
        let generation = cache.generation();
        assert!(generation > 0 && !cache.pending());
        for n in 0..12u32 {
            let Lookup::Ready(f) = cache.lookup(&v, 2 + n, 12.0, true) else { panic!("frame {n}") };
            assert_eq!((f.pixels.width, f.pixels.height, f.pixels.rgba[0]), (8, 6, 65 + n as u8));
        }
        assert!(matches!(cache.lookup(&v, 14, 12.0, true), Lookup::Outside));
        assert_eq!(cache.usage(), (8 * 6 * 4 * 12, 1));
        let shown = video::shown(&cache, &s.doc, 2, true);
        assert!(shown.frames.is_some_and(|f| f.len() == 1 && f[0].element == v.id));
        cache.forget(&v.path);
        assert_eq!(cache.usage(), (0, 0));
        assert!(cache.generation() > generation);
        // Relinked to another file, retimed, then taken off.
        let other = dir.join("other.mp4");
        std::fs::write(&other, b"another").unwrap();
        let out = run(&mut s, "layer.set_video", json!({"layer": layer.id.0, "path": other.display().to_string(), "ffmpeg": good, "start": 4, "offset": 0.5}));
        assert_eq!((out["video"]["start"].as_u64(), out["video"]["offset"].as_f64(), out["video"]["missing"].as_bool()), (Some(4), Some(0.5), Some(false)));
        assert!(out["video"]["path"].as_str().unwrap().ends_with("other.mp4"));
        run(&mut s, "layer.set_video", json!({"layer": layer.id.0, "remove": true}));
        // A reference can't start past the video's end.
        assert!(s.execute("file.import_video", &json!({"path": clip, "ffmpeg": good, "mode": "reference", "start": 5})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostile_ffmpeg_output_is_refused_and_changes_nothing() {
        let (dir, clip, mut s) = setup("hostile");
        let before = snap(&s.doc);
        // A picture larger than asked for, something that is no picture, pictures that change
        // size, a stream cut mid-picture, and nothing at all: each is an error with a reason,
        // and the document is as it was.
        for (kind, says) in [("big", "larger"), ("junk", "not a picture"), ("sizes", "different sizes"), ("short", "middle of a picture"), ("none", "Conversion failed")] {
            let bad = ffmpeg(&dir, kind);
            for mode in ["frames", "reference"] {
                // (A reference takes one picture, so it never sees the second, wrong one.)
                if mode == "reference" && matches!(kind, "sizes" | "short") {
                    continue;
                }
                let e = s.execute("file.import_video", &json!({"path": clip, "ffmpeg": bad, "mode": mode})).unwrap_err().to_string();
                assert!(e.contains(says), "{kind} as {mode}: {e}");
                assert_eq!(snap(&s.doc), before, "{kind} as {mode}");
            }
            // The stage's cache fails the same way, with a reason, and tries again when told.
            let v = VideoRef { id: anicraft_doc::ElementId(1), path: clip.clone(), width: 8, height: 6, transform: anicraft_geom::Affine::IDENTITY, start: 0, offset: 0.0, duration: 1.0 };
            let cache = Cache::with_ffmpeg(1 << 20, Some(bad.into()));
            let _ = cache.lookup(&v, 0, 12.0, true);
            assert!(cache.wait_idle(std::time::Duration::from_secs(60)));
            let found = cache.lookup(&v, 0, 12.0, true);
            assert!(matches!(found, Lookup::Failed(_)), "{kind}: {found:?}");
            cache.retry();
            assert!(matches!(cache.lookup(&v, 0, 12.0, false), Lookup::Pending(_)), "a retry forgets the failure");
        }
        // A program that never stops sending is stopped at the cap.
        let endless = ffmpeg(&dir, "endless");
        let out = run(&mut s, "file.import_video", json!({"path": clip, "ffmpeg": endless, "max_frames": 7, "audio": false}));
        assert_eq!((out["frames"].as_u64(), out["truncated"].as_bool()), (Some(7), Some(true)));
        run(&mut s, "edit.undo", json!({}));
        // One that floods its messages doesn't stall the pictures.
        let noisy = ffmpeg(&dir, "noisy");
        let out = run(&mut s, "file.import_video", json!({"path": clip, "ffmpeg": noisy, "audio": false}));
        assert_eq!(out["frames"], 12);
        run(&mut s, "edit.undo", json!({}));
        // Cancelled part-way through a slow decode: stopped, and nothing changed.
        let slow = ffmpeg(&dir, "slow");
        let control = Arc::new(crate::publish::Control::default());
        s.job = Some(control.clone());
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            control.cancel();
        });
        assert!(matches!(s.execute("file.import_video", &json!({"path": clip, "ffmpeg": slow})), Err(EngineError::Cancelled)));
        stopper.join().unwrap();
        s.job = None;
        assert_eq!(snap(&s.doc), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
