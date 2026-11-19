# ADR 0114: Audio to keyframes and auto lip-sync

* Status: accepted (2026-11-18)
* Relates to: ADR 0020 (sound), ADR 0039 (envelopes, layer volume), ADR 0095 (audio output),
  ADR 0016 (motion tweens), ADR 0065 (templates: the Lip-sync mouth chart), ADR 0017 (plug-ins),
  ADR 0092 (surviving GPU failures), issue #332, roadmap M5

## Context

Issue #332 asks for After Effects' Convert Audio to Keyframes: a sound's amplitude, frame by
frame, driving scale, position, rotation or opacity, with threshold and smoothing, for
music-reactive motion and simple lip-sync. Animate users also expect its Lip Syncing: a graphic
symbol of mouth poses whose frames a voice track picks.

AniCraft already has sounds on keyframes with stream sync, decoded PCM, waveforms, motion tweens
with a curve per property, graphic instances with a first frame, and a template with a labelled
mouth chart. What was missing is the analysis, and commands that write keys from it.

## Decision

1. **The analysis is pure and lives in the audio crate** (`anicraft_audio::analysis`, L0, no
   new dependency). `levels` is the RMS of the samples each movie frame plays (one channel or
   both; repeats and loops followed; frame boundaries as the mixer has them). `normalised`
   divides by the loudest frame, so settings mean the same for a quiet recording and a loud
   one. `shape` gates by the threshold and rescales what is above it (so the value rises from
   zero at the gate instead of jumping), then smooths with a centred moving average of *n*
   frames either side. Centred, not a trailing follower: a beat stays on its frame, which is
   what an animator lines up against the waveform. Same input, same output, always.

2. **The work is bounded.** At most 100 000 frames (the timeline's limit) per analysis;
   `levels` reads at most 96 M samples in all (longer frames are sampled evenly); each
   spectrum reads 512 analysis samples of at most 4 source frames. A command writes at most
   10 000 keys (the motion tween limit) and says how to get under it. Measured (release-like
   test profile, 10 minutes at 24 kHz, 14 400 frames): levels 19 ms, mouth shapes 135 ms,
   7 199 keyframes written in 79 ms.

3. **Lip-sync is clean-room signal processing from public phonetics, not speech
   recognition.** Vowels are told apart by their first two formants (Peterson and Barney,
   1952). Each sounding frame gets a 512-sample Hann-windowed FFT (our own radix-2,
   Cooley and Tukey 1965) at about 8 kHz, and three numbers: the share of pre-emphasised
   energy in 1.6–3.6 kHz (a front vowel's second formant, or hiss), and the centre of the
   energy in 200–1400 Hz (how high the first formant is). They sort the frame into five mouth
   shapes: **rest** (silence), **open** (ah), **wide** (ee, e, s), **round** (oh, oo) and
   **closed** (a weak low hum, and silences inside speech no longer than 0.12 s: the stops of
   b, p, m). Mouths shorter than a hold (default 2 frames) join their neighbour; closures are
   kept. No model, no training data, no network. We chose five shapes because that is what
   these measurements can honestly tell apart: f/v, l and the other consonant mouths of a
   ten-mouth chart need recognition we don't have, and the dialog and the guide say it is a
   first pass.

4. **Three commands, in the `sound` namespace** (which plug-ins may already call: they edit
   the document like the other sound commands, so no new plug-in policy):
   * `sound.to_keyframes` maps the shaped level onto `scale`, `scale_x`, `scale_y`, `alpha`,
     `x`, `y`, `rotation` or `first_frame` between `min` (silence) and `max` (the loudest
     frame). Scale is a percentage of the object's own size and position and rotation are
     offsets from where it is, so the same settings work on any object.
   * `sound.lip_sync` writes a keyframe wherever the mouth changes, setting the instance's
     first frame and Single Frame. `mouths` maps each shape to a frame (a number or a frame
     label); by default by the symbol's labels, the mouth chart's first (Rest, AI, E, O, MBP).
   * `sound.analyse` (read-only) returns the levels or the mouth shapes, so scripts and
     assistants can see what the others would work from.

   The sound is the one at the playhead (a stream first), or named by `sound_layer` and
   `sound_frame`; the range defaults to the frames it is heard on. Any sync but Stop is
   accepted: an artist who dropped music in as an Event sound shouldn't be sent away.

5. **Two outputs.** *Keyframes*: a keyframe at each key, made as Insert Keyframe makes it
   (so it holds what the frame showed), then every keyframe in the range takes the value held
   there, including keyframes that were already on the layer. Keys are written only where the
   value changes by more than 0.2 % of the range. The frame after the range keeps what it
   showed, and a layer shorter than the sound grows to it. *Motion*: property keys on the
   span's motion tween (made for a lone object), replacing that property's keys in the range;
   flat stretches keep only their two ends. `auto` picks motion when the span has a motion
   tween. Keyframes on a span with a motion tween are refused, because they would cut the
   tween up; a symbol's first frame has no curve, so it is keyframes only.

6. **The dialog previews by running the command.** One dialog serves both commands
   (`src/audio_keys.rs`): the sound, the layer and object, the settings, and a graph of the
   loudness per frame with the threshold, the value the keys follow and a tick per key (for
   lip-sync, a row per mouth). The stage and timeline preview is the real command run inside
   a gesture that each change reverts and re-runs, as the plug-in dialog does (ADR 0017): what
   you see is exactly what OK keeps, in one undo step, and Cancel leaves nothing. It is
   reached from Modify › Timeline, from the Sound rows of Properties, and from a graphic
   instance's Looping rows, where Animate has Lip Syncing.

7. **Nothing in the file format changes.** The keys are ordinary keyframes and property
   keys, so older builds open the result.

8. **The flaky `gpu_watch::errors_and_the_loss_come_to_the_app_once` test was a race on
   process-wide state.** The window's graphics health is one static, and every app frame
   reads it (`gpu_watch::tick` → `take_news`), which consumes the "said once" flags. A test
   running an app on another thread could take the news between two lines of that test (and,
   the other way, be told by that test's simulated device loss to close its window). The
   logic now lives in methods of `Health`; the static is only the process's instance, and the
   test works on a `Health` of its own. The assertions are unchanged.

## Consequences

* Music-reactive motion and a first-pass lip-sync work in the app, headless and over MCP.
* Lip-sync quality is that of a formant heuristic. It finds open, wide and round vowels and
  closures; it will mislabel some consonants and voices far from the averages. A recognition
  based pass could replace `analysis::visemes` later without changing the commands.
* The sound's own effect (a fade) and its layer's volume are not counted in the loudness.

## Not in this slice

Driving the camera, armature layers or filter values; classic tweens between the keys; more
mouth shapes; a script or phoneme track as input; several sounds at once; reading the keys
back into the dialog to edit them.

## Question left for the lead

Any sync mode but Stop is accepted, although the task said "from a stream sound". An Event
sound keeps time with the frames only while the movie plays straight through, so the keys can
drift from what is heard after a jump. The dialog doesn't warn about it yet.

Answered in the follow-up (with ADR 0117's PR): both dialogs now show one line under the frames
when the sound's sync is not Stream, saying the keys can drift after a jump on the timeline and
that Stream keeps them locked. They still accept the sound.
