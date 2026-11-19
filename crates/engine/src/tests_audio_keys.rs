//! Convert Audio to Keyframes and auto lip-sync end to end (ADR 0114): a synthetic sound on
//! the timeline, the keys it writes on another layer, undo, and what is refused.

use super::*;
use crate::cmds_audio_keys::{self, KeyProperty, curve_keys, default_mouths, held_keys};
use anicraft_doc::{ElementKind, LayerId, LoopMode, Property};
use serde_json::json;
use std::f32::consts::TAU;

const RATE: u32 = 24_000;
const FPS: f32 = 24.0;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn fails(s: &mut Session, id: &str, p: Value) -> String {
    match s.execute(id, &p) {
        Ok(v) => panic!("{id} {p} should fail, gave {v}"),
        Err(e) => e.to_string(),
    }
}

/// Samples in `frames` movie frames: a thousand a frame, and a 480 Hz tone is twenty whole
/// cycles of one, so every frame of a steady tone is exactly as loud as the next.
fn samples(frames: u32) -> usize {
    (f64::from(frames) * f64::from(RATE) / f64::from(FPS)).round() as usize
}

fn silence(frames: u32) -> Vec<f32> {
    vec![0.0; samples(frames)]
}

/// `frames` movie frames of sines: (Hz, amplitude) summed.
fn tone(parts: &[(f32, f32)], frames: u32) -> Vec<f32> {
    (0..samples(frames)).map(|i| parts.iter().map(|(f, a)| a * (TAU * f * i as f32 / RATE as f32).sin()).sum()).collect()
}

/// Twelve silent frames, a six-frame burst, six silent frames.
fn burst() -> Vec<f32> {
    [silence(12), tone(&[(480.0, 0.5)], 6), silence(6)].concat()
}

/// Imports `sound` to the stage as a stream on a layer of its own; that layer's id.
fn add_sound(s: &mut Session, tag: &str, sound: &[f32]) -> u64 {
    let dir = std::env::temp_dir().join(format!("anicraft-audio-keys-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("voice.wav");
    std::fs::write(&wav, crate::cmds_sound::test_wav(RATE, sound)).unwrap();
    let v = run(s, "file.import", json!({"path": wav.display().to_string()}));
    let layer = v["layer"].as_u64().expect("the sound's layer");
    run(s, "sound.set", json!({"layer": layer, "frame": 0, "sync": "stream"}));
    let _ = std::fs::remove_dir_all(&dir);
    layer
}

/// A 24 fps document with a 100 px square (a drawing object) at 100, 100 on its first layer,
/// and `sound` as a stream on a layer above. `(session, art layer, square, sound layer)`.
fn stage(tag: &str, sound: &[f32]) -> (Session, u64, u64, u64) {
    let mut s = Session::default();
    run(&mut s, "doc.set_stage", json!({"fps": FPS}));
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": "#3366cc", "stroke": null, "object": true}));
    let art = s.layer.0;
    let square = s.selection.first().expect("the square is selected").1.0;
    let sound = add_sound(&mut s, tag, sound);
    (s, art, square, sound)
}

fn key_starts(s: &Session, layer: u64) -> Vec<u32> {
    s.timeline().layer(LayerId(layer)).expect("the layer").keyframes.iter().map(|k| k.start).collect()
}

/// The square's width at `frame`.
fn width_at(s: &mut Session, layer: u64, id: u64, frame: u32) -> f32 {
    s.set_frame(frame);
    s.element_bounds_on(LayerId(layer), anicraft_doc::ElementId(id)).width()
}

/// A sine burst gives keys exactly where it sounds: one where it starts, one where it stops.
#[test]
fn a_burst_keys_the_scale_exactly_where_it_sounds() {
    let (mut s, art, square, sound) = stage("burst", &burst());
    let before = s.doc.clone();
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "threshold": 0.1, "smoothing": 0}));
    assert_eq!((v["keys"].as_u64(), v["from"].as_u64(), v["to"].as_u64(), v["output"].as_str()), (Some(3), Some(0), Some(23), Some("keyframes")));
    assert_eq!((v["layer"].as_u64(), v["element"].as_u64(), v["property"].as_str()), (Some(art), Some(square), Some("scale")));
    assert_eq!(key_starts(&s, art), vec![0, 12, 18], "a key where the burst starts and where it ends");
    assert_eq!(key_starts(&s, sound), vec![0], "the sound's layer is untouched");
    for f in 0..24 {
        let want = if (12..18).contains(&f) { 200.0 } else { 100.0 };
        assert!((width_at(&mut s, art, square, f) - want).abs() < 0.01, "frame {f}");
    }
    // It grows about its middle.
    s.set_frame(14);
    let b = s.element_bounds_on(LayerId(art), anicraft_doc::ElementId(square));
    assert!((b.center().x - 150.0).abs() < 0.01 && (b.center().y - 150.0).abs() < 0.01, "{b:?}");
    // One undo step.
    assert_eq!(s.undo_label(), Some("Convert Audio to Keyframes…"));
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc == before, "undo brings the document back");
}

#[test]
fn the_same_sound_gives_the_same_keys() {
    let voice = [tone(&[(730.0, 0.6), (1090.0, 0.3)], 7), silence(3), tone(&[(300.0, 0.5), (870.0, 0.15)], 9), tone(&[(480.0, 0.2)], 5)].concat();
    let once = |tag: &str| {
        let (mut s, art, _, _) = stage(tag, &voice);
        let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "y", "min": 0, "max": -60, "smoothing": 2}));
        let a = run(&mut s, "sound.analyse", json!({}));
        (s.doc.timeline(0).layer(LayerId(art)).cloned(), v, a)
    };
    let (a, b) = (once("same-a"), once("same-b"));
    assert!(a.0 == b.0 && a.1 == b.1 && a.2 == b.2, "same sound, same keys");
    assert!(a.1["keys"].as_u64().is_some_and(|n| n > 4));
}

#[test]
fn every_property_maps_silence_to_min_and_the_loudest_frame_to_max() {
    let cases: [(&str, f64, f64); 5] = [("scale_x", 100.0, 300.0), ("scale_y", 50.0, 100.0), ("x", 0.0, 40.0), ("y", -10.0, 30.0), ("rotation", 0.0, 90.0)];
    for (prop, min, max) in cases {
        let (mut s, art, square, _) = stage(&format!("prop-{prop}"), &burst());
        run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": prop, "min": min, "max": max, "threshold": 0.1, "smoothing": 0}));
        let at = |s: &mut Session, f: u32| {
            s.set_frame(f);
            s.element_bounds_on(LayerId(art), anicraft_doc::ElementId(square))
        };
        let (quiet, loud) = (at(&mut s, 3), at(&mut s, 14));
        let near = |a: f32, b: f64| (f64::from(a) - b).abs() < 0.01;
        match prop {
            "scale_x" => assert!(near(quiet.width(), 100.0) && near(loud.width(), 300.0) && near(loud.height(), 100.0), "{prop}: {loud:?}"),
            "scale_y" => assert!(near(quiet.height(), 50.0) && near(loud.height(), 100.0) && near(loud.width(), 100.0), "{prop}: {quiet:?}"),
            "x" => assert!(near(quiet.min.x, 100.0) && near(loud.min.x, 140.0) && near(loud.min.y, 100.0), "{prop}: {loud:?}"),
            "y" => assert!(near(quiet.min.y, 90.0) && near(loud.min.y, 130.0), "{prop}: {quiet:?} {loud:?}"),
            // A quarter turn about its middle: the square covers itself.
            _ => assert!(near(loud.min.x, 100.0) && near(loud.min.y, 100.0) && near(loud.width(), 100.0), "{prop}: {loud:?}"),
        }
    }
    // Alpha: the colour effect, 20 % in silence and 90 % on the burst.
    let (mut s, art, square, _) = stage("prop-alpha", &burst());
    run(&mut s, "sound.to_keyframes", json!({"layer": art, "element": square, "property": "opacity", "min": 20, "max": 90, "threshold": 0.1, "smoothing": 0}));
    let alpha = |s: &Session, f: u32| s.timeline().layer(LayerId(art)).and_then(|l| l.keyframe_at(f)).and_then(|k| k.elements.first().map(|e| e.color.alpha())).expect("the square");
    assert!((alpha(&s, 5) - 0.2).abs() < 1e-5 && (alpha(&s, 13) - 0.9).abs() < 1e-5 && (alpha(&s, 20) - 0.2).abs() < 1e-5);
}

#[test]
fn smoothing_threshold_step_and_range_shape_the_keys() {
    // Smoothing ramps into and out of the burst: more keys, the peak still on the burst.
    let (mut s, art, square, _) = stage("shape", &burst());
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "smoothing": 2}));
    assert!(v["keys"].as_u64().is_some_and(|n| n > 6), "{v}");
    assert!(width_at(&mut s, art, square, 9) == 100.0 && width_at(&mut s, art, square, 11) > 100.0 && (width_at(&mut s, art, square, 14) - 200.0).abs() < 0.01);
    // A key every four frames: the keys fall on frames 0, 4, 8…, held in between.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "smoothing": 0, "step": 4}));
    assert_eq!(key_starts(&s, art), vec![0, 12, 20]);
    // A range inside the sound: only its frames are keyed.
    run(&mut s, "edit.undo", json!({}));
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "smoothing": 0, "from": 14, "to": 20}));
    assert_eq!((v["from"].as_u64(), v["to"].as_u64()), (Some(14), Some(20)));
    assert_eq!(key_starts(&s, art), vec![0, 14, 18]);
    // A threshold above the burst's own level leaves nothing to key but the first frame.
    run(&mut s, "edit.undo", json!({}));
    let quiet = [silence(6), tone(&[(480.0, 0.05)], 6), tone(&[(480.0, 0.5)], 6), silence(6)].concat();
    let (mut s, art, _, _) = stage("gate", &quiet);
    run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "smoothing": 0, "threshold": 0.3}));
    assert_eq!(key_starts(&s, art), vec![0, 12, 18], "the quiet part is under the threshold");
}

/// Keyframes already on the layer keep their place and take the value; the frame after the
/// sound shows what it showed before.
#[test]
fn keyframes_already_there_take_the_value_and_the_layer_goes_on_as_it_was() {
    let loud_to_the_end = [silence(12), tone(&[(480.0, 0.5)], 12)].concat();
    let (mut s, art, square, _) = stage("existing", &loud_to_the_end);
    run(&mut s, "timeline.insert_frame", json!({"layer": art, "frame": 0, "count": 39}));
    run(&mut s, "timeline.insert_keyframe", json!({"layer": art, "frame": 14}));
    run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "threshold": 0.1, "smoothing": 0}));
    assert_eq!(key_starts(&s, art), vec![0, 12, 14, 24]);
    assert!((width_at(&mut s, art, square, 14) - 200.0).abs() < 0.01, "the keyframe that was there is loud too");
    assert!((width_at(&mut s, art, square, 23) - 200.0).abs() < 0.01);
    assert!((width_at(&mut s, art, square, 24) - 100.0).abs() < 0.01 && (width_at(&mut s, art, square, 39) - 100.0).abs() < 0.01, "after the sound, as it was");
}

/// On a motion tween the keys go on the property's curve, and the span stays one span.
#[test]
fn a_motion_tween_takes_the_keys_on_its_curve() {
    let mut s = Session::default();
    run(&mut s, "doc.set_stage", json!({"fps": FPS}));
    run(&mut s, "draw.rect", json!({"x": 100, "y": 100, "w": 100, "h": 100, "fill": "#3366cc", "stroke": null, "object": true}));
    run(&mut s, "modify.convert_to_symbol", json!({"name": "Ball"}));
    let art = s.layer.0;
    run(&mut s, "timeline.insert_frame", json!({"frame": 0, "count": 29}));
    run(&mut s, "timeline.create_motion_tween", json!({"frame": 0}));
    add_sound(&mut s, "motion", &burst());
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "alpha", "min": 10, "max": 100, "threshold": 0.1, "smoothing": 0}));
    assert_eq!((v["output"].as_str(), v["keys"].as_u64(), v["to"].as_u64()), (Some("motion"), Some(6), Some(23)));
    assert_eq!(key_starts(&s, art), vec![0], "the span is not cut up");
    let layer = s.timeline().layer(LayerId(art)).cloned().expect("the layer");
    let track = |p: Property| layer.keyframes[0].motion.as_ref().and_then(|m| m.track(p)).map(|t| t.keys.iter().map(|k| (k.frame, k.value)).collect::<Vec<_>>());
    // Flat stretches keep their two ends, and the frame after the sound returns to the
    // object's own alpha.
    let near = |got: Option<Vec<(u32, f32)>>, want: &[(u32, f32)]| got.is_some_and(|g| g.len() == want.len() && g.iter().zip(want).all(|(a, b)| a.0 == b.0 && (a.1 - b.1).abs() < 0.01));
    assert!(near(track(Property::Alpha), &[(0, 10.0), (11, 10.0), (12, 100.0), (17, 100.0), (18, 10.0), (23, 10.0), (24, 100.0)]), "{:?}", track(Property::Alpha));
    let alpha = |f: u32| anicraft_doc::evaluate_layer_elements(&layer, f).and_then(|(_, e)| e.first().map(|e| e.color.alpha())).expect("the ball");
    assert!((alpha(5) - 0.1).abs() < 1e-4 && (alpha(14) - 1.0).abs() < 1e-4 && (alpha(20) - 0.1).abs() < 1e-4 && (alpha(28) - 1.0).abs() < 1e-4);
    // Scale writes both scale curves, as shares of the object's own size; a second run
    // replaces the keys of the first.
    for _ in 0..2 {
        run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 150, "threshold": 0.1, "smoothing": 0, "output": "motion"}));
    }
    let layer = s.timeline().layer(LayerId(art)).cloned().expect("the layer");
    let track = |p: Property| layer.keyframes[0].motion.as_ref().and_then(|m| m.track(p)).map(|t| t.keys.iter().map(|k| (k.frame, k.value)).collect::<Vec<_>>());
    let want = [(0, 100.0), (11, 100.0), (12, 150.0), (17, 150.0), (18, 100.0), (23, 100.0), (24, 100.0)];
    assert!(near(track(Property::ScaleX), &want), "{:?}", track(Property::ScaleX));
    assert!(near(track(Property::ScaleY), &want), "{:?}", track(Property::ScaleY));
    // Keyframes would cut the tween up: refused, saying what to do.
    let why = fails(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 150, "output": "keyframes"}));
    assert!(why.contains("motion tween"), "{why}");
    // Without a tween, asking for the curve makes one on a lone object.
    let (mut s, art, _, _) = stage("motion-new", &burst());
    run(&mut s, "timeline.insert_frame", json!({"layer": art, "frame": 0, "count": 23}));
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "rotation", "min": 0, "max": 45, "threshold": 0.1, "smoothing": 0, "output": "motion"}));
    assert_eq!(v["output"].as_str(), Some("motion"));
    assert!(s.timeline().layer(LayerId(art)).is_some_and(|l| l.keyframes.len() == 1 && l.keyframes[0].motion.as_ref().is_some_and(|m| m.track(Property::Rotation).is_some())));
}

/// A graphic symbol's first frame follows the loudness: louder, a later frame.
#[test]
fn the_first_frame_of_a_graphic_follows_the_loudness() {
    let mut s = Session::default();
    run(&mut s, "file.new_from_template", json!({"template": "lip-sync"}));
    let fps = s.doc.stage.fps;
    assert_eq!(fps, FPS, "the template's frame rate is the tests'");
    let mouth = s.timeline().layers.iter().find(|l| l.name == "Mouth").map(|l| l.id.0).expect("the Mouth layer");
    add_sound(&mut s, "first", &[silence(6), tone(&[(480.0, 0.25)], 6), tone(&[(480.0, 0.5)], 6), silence(6)].concat());
    run(&mut s, "sound.to_keyframes", json!({"layer": mouth, "property": "first_frame", "min": 0, "max": 4, "smoothing": 0}));
    let shown = |s: &Session, f: u32| match s.timeline().layer(LayerId(mouth)).and_then(|l| l.keyframe_at(f)).and_then(|k| k.elements.first().map(|e| e.kind.clone())) {
        Some(ElementKind::Symbol { first_frame, loop_mode, .. }) => (first_frame, loop_mode),
        other => panic!("{other:?}"),
    };
    assert_eq!(shown(&s, 2), (0, LoopMode::SingleFrame));
    assert_eq!(shown(&s, 8), (2, LoopMode::SingleFrame), "half as loud, half way");
    assert_eq!(shown(&s, 14), (4, LoopMode::SingleFrame));
    assert_eq!(shown(&s, 20), (0, LoopMode::SingleFrame));
    // A frame the symbol doesn't have, and an object that isn't a graphic symbol.
    let why = fails(&mut s, "sound.to_keyframes", json!({"layer": mouth, "property": "first_frame", "min": 0, "max": 10}));
    assert!(why.contains("10 frames"), "{why}");
    let (mut s, art, _, _) = stage("first-shape", &burst());
    let why = fails(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "first_frame", "min": 0, "max": 1}));
    assert!(why.contains("graphic symbol"), "{why}");
    let why = fails(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "first_frame", "min": 0, "max": 1, "output": "motion"}));
    assert!(why.contains("graphic symbol"), "{why}");
}

/// The mouth chart template lip-synced to "ah", a stop, "ee", "oo": each mouth's labelled
/// frame on the frame its sound starts.
#[test]
fn the_mouth_chart_is_lip_synced_to_synthetic_speech() {
    let mut s = Session::default();
    run(&mut s, "file.new_from_template", json!({"template": "lip-sync"}));
    let mouth = s.timeline().layers.iter().find(|l| l.name == "Mouth").map(|l| l.id.0).expect("the Mouth layer");
    let speech = [silence(12), tone(&[(730.0, 0.6), (1090.0, 0.3)], 6), silence(2), tone(&[(270.0, 0.6), (2290.0, 0.15)], 6), tone(&[(300.0, 0.6), (870.0, 0.2)], 6), silence(12)].concat();
    let sound = add_sound(&mut s, "lips", &speech);
    let before = s.doc.clone();
    // The sound's layer is active: the mouth's layer is named, and its one object found.
    let v = run(&mut s, "sound.lip_sync", json!({"layer": mouth}));
    assert_eq!((v["keys"].as_u64(), v["from"].as_u64(), v["to"].as_u64()), (Some(6), Some(0), Some(43)), "{v}");
    // Rest, AI, E, O and MBP are the chart's frames 0, 1, 2, 3 and 8.
    assert_eq!(v["mouths"], json!({"rest": 0, "open": 1, "wide": 2, "round": 3, "closed": 8}));
    assert_eq!(v["counts"], json!({"rest": 2, "open": 1, "wide": 1, "round": 1, "closed": 1}));
    let layer = s.timeline().layer(LayerId(mouth)).cloned().expect("the layer");
    let keys: Vec<(u32, u32)> = layer
        .keyframes
        .iter()
        .map(|k| match k.elements.first().map(|e| &e.kind) {
            Some(ElementKind::Symbol { first_frame, loop_mode: LoopMode::SingleFrame, .. }) => (k.start, *first_frame),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(keys, vec![(0, 0), (12, 1), (18, 8), (20, 2), (26, 3), (32, 0)]);
    assert_eq!(key_starts(&s, sound), vec![0]);
    assert_eq!(s.undo_label(), Some("Auto Lip-Sync…"));
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc == before, "one undo step");
    // Mouths by label or by frame, over the defaults; a longer hold drops the short ones.
    let v = run(&mut s, "sound.lip_sync", json!({"layer": mouth, "mouths": {"open": "fv", "round": 9, "closed": "L"}, "hold": 7}));
    assert_eq!(v["mouths"], json!({"rest": 0, "open": 6, "wide": 2, "round": 9, "closed": 7}));
    assert_eq!(v["counts"], json!({"rest": 2, "open": 0, "wide": 0, "round": 1, "closed": 1}), "six-frame mouths join their neighbour; the stop stays");
    // What is refused, and why.
    for (p, part) in [
        (json!({"layer": mouth, "mouths": {"open": "nope"}}), "no frame labelled"),
        (json!({"layer": mouth, "mouths": {"open": 10}}), "has 10 frames"),
        (json!({"layer": mouth, "mouths": {"grin": 1}}), "unknown mouth"),
        (json!({"layer": mouth, "mouths": [1, 2]}), "must be an object"),
        (json!({"layer": mouth, "hold": 0}), "'hold'"),
        (json!({"layer": mouth, "threshold": 1}), "'threshold'"),
        (json!({"layer": mouth, "from": 9, "to": 3}), "'to'"),
        (json!({"layer": sound}), "nothing at frame"),
        (json!({"layer": mouth, "element": 999_999}), "no object with id"),
        (json!({"layer": mouth, "sound_layer": mouth}), "has no sound"),
    ] {
        let why = fails(&mut s, "sound.lip_sync", p.clone());
        assert!(why.contains(part), "{p}: {why}");
    }
}

#[test]
fn mouths_are_found_by_label_and_fall_back_sensibly() {
    let chart = anicraft_doc::templates::TEMPLATES.iter().find(|t| t.id == "lip-sync").map(|t| (t.build)()).expect("the template");
    let sym = chart.library.iter().find(|x| x.name == "Mouth").expect("the Mouth symbol");
    assert_eq!(default_mouths(sym), [0, 1, 2, 3, 8]);
    assert_eq!(cmds_audio_keys::labelled_frame(sym, " c-d-g-k-n-r-s-th-y-z "), Some(5));
    assert_eq!(cmds_audio_keys::labelled_frame(sym, ""), None);
    // No labels: the first five frames, in order; fewer frames, the last one again.
    let mut bare = (**sym).clone();
    for l in &mut bare.timeline.layers {
        for k in &mut Arc::make_mut(l).keyframes {
            Arc::make_mut(k).label.clear();
        }
    }
    assert_eq!(default_mouths(&bare), [0, 1, 2, 3, 4]);
    for l in &mut bare.timeline.layers {
        Arc::make_mut(l).keyframes.truncate(3);
    }
    assert_eq!(default_mouths(&bare), [0, 1, 2, 2, 2]);
    // Only some labelled: the others borrow the nearest mouth.
    let mut some = (**sym).clone();
    for l in &mut some.timeline.layers {
        for k in &mut Arc::make_mut(l).keyframes {
            let k = Arc::make_mut(k);
            if !matches!(k.label.as_str(), "AI" | "MBP") {
                k.label.clear();
            }
        }
    }
    assert_eq!(default_mouths(&some), [8, 1, 1, 1, 8], "rest borrows lips together; wide and round borrow open");
}

#[test]
fn analyse_reports_levels_and_mouths_without_changing_anything() {
    let (mut s, _, _, sound) = stage("analyse", &burst());
    let before = s.doc.clone();
    let v = run(&mut s, "sound.analyse", json!({"smoothing": 0, "threshold": 0.1}));
    assert_eq!((v["layer"].as_u64(), v["keyframe"].as_u64(), v["from"].as_u64(), v["to"].as_u64(), v["name"].as_str()), (Some(sound), Some(0), Some(0), Some(23), Some("voice.wav")));
    let levels: Vec<f64> = v["levels"].as_array().expect("levels").iter().filter_map(Value::as_f64).collect();
    assert_eq!(levels.len(), 24);
    assert!(levels.iter().enumerate().all(|(i, l)| if (12..18).contains(&i) { *l > 0.98 } else { *l == 0.0 }), "{levels:?}");
    assert_eq!(v["values"], v["levels"], "nothing under the threshold, no smoothing");
    let v = run(&mut s, "sound.analyse", json!({"what": "visemes", "from": 10, "to": 13}));
    assert_eq!(v["visemes"], json!(["rest", "rest", "round", "round"]), "a lone low tone reads as a round mouth");
    assert!(s.doc == before && !s.can_undo() || s.undo_label() != Some("Analyse Sound"));
    assert!(fails(&mut s, "sound.analyse", json!({"what": "words"})).contains("levels or visemes"));
    // A timeline without a sound says so.
    let mut empty = Session::default();
    let why = fails(&mut empty, "sound.to_keyframes", json!({"property": "scale", "min": 100, "max": 150}));
    assert!(why.contains("no sound"), "{why}");
}

#[test]
fn what_cannot_be_keyed_is_refused_with_a_reason() {
    let (mut s, art, _, sound) = stage("refuse", &burst());
    let before = s.doc.clone();
    for (p, part) in [
        (json!({"layer": art}), "'property' is required"),
        (json!({"layer": art, "property": "skew", "min": 0, "max": 1}), "unknown property"),
        (json!({"layer": art, "property": "scale", "min": 0, "max": 150}), "'min'"),
        (json!({"layer": art, "property": "alpha", "min": 0, "max": 101}), "'max'"),
        (json!({"layer": art, "property": "scale", "max": 150}), "min"),
        (json!({"layer": art, "property": "scale", "min": 100, "max": 150, "step": 0}), "'step'"),
        (json!({"layer": art, "property": "scale", "min": 100, "max": 150, "smoothing": 25}), "'smoothing'"),
        (json!({"layer": art, "property": "scale", "min": 100, "max": 150, "channel": "centre"}), "'channel'"),
        (json!({"layer": art, "property": "scale", "min": 100, "max": 150, "output": "film"}), "unknown output"),
        (json!({"layer": sound, "property": "scale", "min": 100, "max": 150}), "nothing at frame"),
        (json!({"layer": 999_999, "property": "scale", "min": 100, "max": 150}), "no layer"),
        (json!({"layer": art, "property": "scale", "min": 100, "max": 150, "to": 200_000}), "'to'"),
    ] {
        let why = fails(&mut s, "sound.to_keyframes", p.clone());
        assert!(why.contains(part), "{p}: {why}");
    }
    assert!(s.doc == before, "a refused command changes nothing");
    // Two objects on the keyframe: the selected one is keyed; with none selected it asks.
    run(&mut s, "layer.select", json!({"layer": art}));
    run(&mut s, "draw.rect", json!({"x": 400, "y": 100, "w": 50, "h": 50, "fill": "#cc3366", "stroke": null, "object": true}));
    let second = s.selection.first().expect("the second square").1.0;
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200, "threshold": 0.1, "smoothing": 0}));
    assert_eq!(v["element"].as_u64(), Some(second));
    assert!((width_at(&mut s, art, second, 14) - 100.0).abs() < 0.01);
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.deselect", json!({}));
    let why = fails(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 200}));
    assert!(why.contains("2 objects"), "{why}");
    // Plug-ins may call them (they edit the document, like the other sound commands).
    for id in ["sound.to_keyframes", "sound.lip_sync", "sound.analyse"] {
        assert!(crate::cmds_plugin::plugin_may_call(id).is_ok(), "{id}");
    }
}

#[test]
fn keys_are_thinned_without_changing_what_is_seen() {
    let v = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.5, 0.0, 0.0];
    assert_eq!(held_keys(&v, 1, 0.001), vec![(0, 0.0), (3, 1.0), (6, 0.5), (7, 0.0)]);
    assert_eq!(held_keys(&v, 3, 0.001), vec![(0, 0.0), (3, 1.0), (6, 0.5)]);
    assert_eq!(curve_keys(&v, 1, 0.001), vec![(0, 0.0), (2, 0.0), (3, 1.0), (5, 1.0), (6, 0.5), (7, 0.0), (8, 0.0)]);
    assert_eq!(curve_keys(&[0.4; 6], 1, 0.001), vec![(0, 0.4), (5, 0.4)], "a flat sound: its two ends");
    assert!(held_keys(&[], 1, 0.1).is_empty() && curve_keys(&[], 0, 0.1).is_empty());
    assert_eq!(held_keys(&[1.0, 2.0], 0, 0.0).len(), 2, "a step of zero is one");
    for p in KeyProperty::ALL {
        assert_eq!(KeyProperty::from_key(p.key()), Some(p));
        let (min, max) = p.defaults();
        assert!(p.range().contains(&min) && p.range().contains(&max), "{}", p.key());
    }
}

/// How long the analysis and the keys take (run with `--ignored --nocapture`).
#[test]
#[ignore = "a timing, not a check"]
fn timing_of_a_long_sound() {
    // Ten minutes of a voice-like sound at 24 kHz, 24 fps: 14 400 frames.
    let n = RATE as usize * 600;
    let voice: Vec<f32> = (0..n).map(|i| {
        let t = i as f32 / RATE as f32;
        let syllable = ((t * 4.0).fract() < 0.7) as u8 as f32;
        syllable * (0.5 * (TAU * 730.0 * t).sin() + 0.2 * (TAU * (1090.0 + 600.0 * (t * 0.5).sin()) * t).sin())
    }).collect();
    let (mut s, art, _, _) = stage("timing", &voice);
    let t = std::time::Instant::now();
    let v = run(&mut s, "sound.analyse", json!({}));
    eprintln!("levels of {} frames: {:?}", v["levels"].as_array().map_or(0, Vec::len), t.elapsed());
    let t = std::time::Instant::now();
    let v = run(&mut s, "sound.analyse", json!({"what": "visemes"}));
    eprintln!("visemes of {} frames: {:?}", v["visemes"].as_array().map_or(0, Vec::len), t.elapsed());
    let t = std::time::Instant::now();
    let v = run(&mut s, "sound.to_keyframes", json!({"layer": art, "property": "scale", "min": 100, "max": 150, "step": 2}));
    eprintln!("{} keyframes written: {:?}", v["keys"], t.elapsed());
}
