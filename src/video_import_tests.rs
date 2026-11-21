//! File › Import › Import Video (ADR 0116), through the window: what it says before anything
//! starts, the params it sends, a missing ffmpeg, and (with a stand-in ffmpeg, a shell script,
//! on Unix) a whole import with progress, a cancel that changes nothing, and a reference video
//! reaching the stage.

use crate::app::App;
use crate::video_import::{self, Mode};
use anicraft_engine::video::Probe;
use egui::{Pos2, Rect, vec2};
use serde_json::json;
use std::path::PathBuf;

fn opened() -> (App, egui::Context) {
    let ctx = egui::Context::default();
    let session = anicraft_engine::Session::new(anicraft_engine::doc::sample::demo());
    let mut app = App::with_context(&ctx, session, Some(PathBuf::from("unused.png")));
    frames(&ctx, &mut app, 3);
    (app, ctx)
}

fn frames(ctx: &egui::Context, app: &mut App, n: usize) {
    for _ in 0..n {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 940.0))), ..Default::default() };
        ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
    }
}

/// Runs frames until `done` (at most a minute); whether it got there.
fn until(ctx: &egui::Context, app: &mut App, done: impl Fn(&App) -> bool) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done(app) && std::time::Instant::now() < end {
        frames(ctx, app, 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    done(app)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anicraft-video-ui-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn film(seconds: f64, audio: bool) -> Probe {
    Probe { width: 1920, height: 1080, duration: Some(seconds), fps: Some(30.0), audio }
}

#[test]
fn the_window_starts_on_what_fits_and_sends_what_it_shows() {
    let (mut app, ctx) = opened();
    // A short film: frames, as Animate embeds a video. Only what was asked is sent.
    video_import::open_with(&mut app, PathBuf::from("walk.mp4"), film(1.0, true));
    frames(&ctx, &mut app, 2);
    let d = app.video_import.as_mut().unwrap();
    assert_eq!((d.mode, d.step, d.audio), (Mode::Frames, 1, true));
    assert_eq!(video_import::params(d), json!({"path": "walk.mp4", "mode": "frames", "step": 1, "max_edge": 1280, "audio": true}));
    d.step = 2;
    d.max_edge = 640;
    d.audio = false;
    d.start = 0.25;
    d.length = 0.5;
    assert_eq!(video_import::params(d), json!({"path": "walk.mp4", "mode": "frames", "step": 2, "max_edge": 640, "audio": false, "start": 0.25, "length": 0.5}));
    d.mode = Mode::Reference;
    assert_eq!(video_import::params(d), json!({"path": "walk.mp4", "mode": "reference", "start": 0.25}));
    frames(&ctx, &mut app, 2);
    assert!(app.modal_open(), "it takes the keyboard, as a dialog does");
    video_import::cancel(&mut app);
    assert!(app.video_import.is_none());
    // A long film would be a large document: the window starts on a reference layer.
    video_import::open_with(&mut app, PathBuf::from("feature.mp4"), film(600.0, true));
    assert_eq!(app.video_import.as_ref().unwrap().mode, Mode::Reference);
    frames(&ctx, &mut app, 2);
    // One that doesn't say its length can only come in as frames; one without sound has none.
    video_import::open_with(&mut app, PathBuf::from("stream.ts"), Probe { duration: None, ..film(1.0, false) });
    let d = app.video_import.as_ref().unwrap();
    assert_eq!((d.mode, d.audio), (Mode::Frames, false));
    frames(&ctx, &mut app, 2);
    video_import::cancel(&mut app);
}

#[test]
fn without_ffmpeg_the_window_says_how_to_get_it() {
    let (mut app, ctx) = opened();
    let dir = temp_dir("missing");
    let clip = dir.join("clip.mp4");
    std::fs::write(&clip, b"not a video").unwrap();
    let layers = app.s.timeline().layers.len();
    // From the menu the command comes with only the file: the window opens, nothing runs.
    app.perform_here("file.import_video", json!({"path": clip.display().to_string()}));
    assert!(app.video_import.is_some());
    video_import::cancel(&mut app);
    video_import::open_using(&mut app, clip.clone(), Some(dir.join("no-such-ffmpeg").display().to_string()));
    assert!(until(&ctx, &mut app, |a| a.video_import.as_ref().is_some_and(|d| d.info.is_some())), "the probe answers");
    let why = app.video_import.as_ref().and_then(|d| d.info.clone()).unwrap().unwrap_err();
    assert!(why.contains("ffmpeg.org"), "{why}");
    frames(&ctx, &mut app, 2);
    video_import::cancel(&mut app);
    assert!(app.video_import.is_none());
    assert_eq!(app.s.timeline().layers.len(), layers);
    assert!(!app.s.can_undo());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
mod with_a_stand_in {
    use super::*;

    /// A stand-in ffmpeg: an 8 × 6, one-second, 12 fps video with sound; twelve pictures, each
    /// after `pause` seconds.
    fn ffmpeg(dir: &std::path::Path, pause: &str) -> String {
        let script = [
            "#!/bin/sh",
            r#"frames=1000000; ppm=0; wav=0; prev=""; last="""#,
            r#"for a in "$@"; do"#,
            r#"  case "$prev" in -frames:v) frames="$a";; esac"#,
            r#"  case "$a" in ppm) ppm=1;; wav) wav=1;; esac"#,
            r#"  prev="$a"; last="$a""#,
            "done",
            r#"if [ "$wav" = 1 ]; then"#,
            r#"  { printf 'RIFF\014\105\000\000WAVEfmt \020\000\000\000\001\000\002\000\104\254\000\000\020\261\002\000\004\000\020\000data\350\104\000\000'; head -c 17640 /dev/zero; } > "$last""#,
            "  exit 0",
            "fi",
            r#"if [ "$ppm" = 1 ]; then"#,
            &format!(r#"  n=0; while [ $n -lt 12 ] && [ $n -lt "$frames" ]; do printf 'P6\n8 6\n255\n'; head -c 144 /dev/zero | tr '\000' 'M'; sleep {pause}; n=$((n + 1)); done"#),
            "  exit 0",
            "fi",
            r#"echo "  Duration: 00:00:01.00, start: 0.000000, bitrate: 1 kb/s" >&2"#,
            r#"echo "  Stream #0:0: Video: fake, rgb24, 8x6, 12 fps" >&2"#,
            r#"echo "  Stream #0:1: Audio: pcm_s16le, 44100 Hz, stereo" >&2"#,
            "exit 1",
            "",
        ]
        .join("\n");
        let path = dir.join(format!("ffmpeg-{}", pause.replace('.', "_")));
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.display().to_string()
    }

    fn clip(dir: &std::path::Path) -> PathBuf {
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, b"a video, as far as the stand-in cares").unwrap();
        clip
    }

    #[test]
    fn frames_come_in_behind_a_progress_window_and_cancel_changes_nothing() {
        let (mut app, ctx) = opened();
        let dir = temp_dir("frames");
        let layers = app.s.timeline().layers.len();
        // Cancelled while a slow ffmpeg is still sending: the window closes, nothing changed.
        video_import::open_using(&mut app, clip(&dir), Some(ffmpeg(&dir, "0.2")));
        assert!(until(&ctx, &mut app, |a| a.video_import.as_ref().is_some_and(|d| matches!(d.info, Some(Ok(_))))));
        assert_eq!(app.video_import.as_ref().unwrap().mode, Mode::Frames);
        video_import::begin(&mut app);
        frames(&ctx, &mut app, 3);
        assert!(app.video_import.is_some(), "the progress window is up");
        video_import::cancel(&mut app);
        assert!(until(&ctx, &mut app, |a| a.video_import.is_none()));
        assert_eq!(app.s.timeline().layers.len(), layers);
        assert!(!app.s.can_undo());
        assert!(app.status.as_ref().is_some_and(|s| !s.error), "cancelling is not an error");
        // Left to finish: twelve pictures and the sound, one undo step, said in the status bar.
        video_import::open_using(&mut app, clip(&dir), Some(ffmpeg(&dir, "0")));
        assert!(until(&ctx, &mut app, |a| a.video_import.as_ref().is_some_and(|d| matches!(d.info, Some(Ok(_))))));
        video_import::begin(&mut app);
        assert!(until(&ctx, &mut app, |a| a.video_import.is_none()));
        assert_eq!(app.s.timeline().layers.len(), layers + 2, "{:?}", app.status.as_ref().map(|s| s.text.clone()));
        assert_eq!(app.s.doc.bitmaps.len(), 12);
        assert!(app.s.staged_video.is_none());
        let said = app.status.as_ref().map(|s| (s.error, s.text.clone())).unwrap();
        assert!(!said.0 && said.1.contains("12"), "{said:?}");
        app.perform_here("edit.undo", json!({}));
        assert_eq!((app.s.timeline().layers.len(), app.s.doc.bitmaps.len()), (layers, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reference_video_reaches_the_stage_and_layer_properties() {
        let (mut app, ctx) = opened();
        let dir = temp_dir("reference");
        let program = ffmpeg(&dir, "0");
        app.video = anicraft_engine::video::Cache::with_ffmpeg(1 << 20, Some(program.clone().into()));
        let on = app.s.active_layer_index().unwrap();
        app.perform_here("file.import_video", json!({"path": clip(&dir).display().to_string(), "mode": "reference", "ffmpeg": program}));
        let layer = app.s.timeline().layers[on + 1].clone();
        assert!(layer.video.is_some() && layer.locked, "{:?}", app.status.as_ref().map(|s| s.text.clone()));
        // The stage asks for the frame under the playhead; it arrives and is drawn.
        frames(&ctx, &mut app, 3);
        assert!(app.video.wait_idle(std::time::Duration::from_secs(60)));
        frames(&ctx, &mut app, 3);
        let doc = app.s.doc.clone();
        let shown = video_import::stage_frames(&mut app, &ctx, &doc, 0, true);
        assert!(shown.is_some_and(|f| f.len() == 1 && f[0].pixels.rgba[0] == b'M'));
        assert!(app.video_failed.is_none());
        // Layer Properties shows the file, with Relink and Remove.
        crate::layer_props::open(&mut app, Some(layer.id));
        frames(&ctx, &mut app, 3);
        assert!(app.layer_props.is_some());
        app.layer_props = None;
        // The file goes missing: said once, in the status bar.
        app.perform_here("layer.set_video", json!({"layer": layer.id.0, "start": 0}));
        std::fs::remove_file(dir.join("clip.mp4")).unwrap();
        app.video.forget(&layer.video.as_ref().unwrap().path);
        frames(&ctx, &mut app, 2);
        assert!(app.video.wait_idle(std::time::Duration::from_secs(60)));
        frames(&ctx, &mut app, 2);
        assert!(app.video_failed.as_deref().is_some_and(|w| w.contains("missing")), "{:?}", app.video_failed);
        app.perform_here("layer.set_video", json!({"layer": layer.id.0, "remove": true}));
        assert!(app.s.timeline().layers[on + 1].video.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
