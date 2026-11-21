# ADR 0116: Importing video through the user's ffmpeg

* Status: accepted (2026-11-18)
* Relates to: ADR 0020 (sound), ADR 0036 (the web build), ADR 0050 (bitmaps), ADR 0060 (Publish:
  progress and Cancel), ADR 0086 (XFL video items), ADR 0111 (layer opacity), ADR 0004
  (destructive actions), `docs/legal/compliance.md` (video codecs), user issue #369, roadmap M5

## Context

Animators trace over film (rotoscoping) and use video as reference for timing. Issue #369 asks
to import MP4 files. Animate offers two things: embedding a video's frames in the timeline, and
(since 2019, in practice through guide layers) keeping footage under the artwork while drawing.

Three constraints shape the answer:

* **No codec may enter AniCraft.** H.264, HEVC and AAC are in patent pools (`compliance.md`),
  and a video decoder is a large attack surface. Export Video already solves the same problem
  by running the user's own ffmpeg (argument list, never a shell).
* **Time is a function.** `evaluate(document, frame)` must stay pure, and the CPU renderer's
  output for a document must not depend on what files happen to be on the computer.
* **Never crash, never grow without bound.** What comes back from another program is input
  like any file: it can be wrong, huge or hostile.

Behaviour was learned from Adobe's public help pages on importing video and on guide layers,
ffmpeg's public documentation, and the Netpbm PPM specification; nothing was copied.

## Decision

1. **All decoding happens out of process, in the user's ffmpeg**, found exactly as Export Video
   finds it (`ffmpeg` parameter, else `ANICRAFT_FFMPEG`, else the PATH). `engine::video` starts
   it with an argument list and reads its standard output. If it can't be started the error
   says so and how to install it (`ffmpeg.org`, `ANICRAFT_FFMPEG`); nothing panics. On Windows
   it is started without a console window. In a web browser every video command says a browser
   can't start programs (ADR 0036).

2. **Pictures cross the pipe as binary PPM** (`-c:v ppm -f image2pipe`): uncompressed RGB with
   a three-number text header, so each picture says its own size and the reader is some forty
   lines with nothing to exploit. `read_ppm` checks the magic, three numbers of at most six
   digits, the maximum value 255 and the size against the box that was asked for *before*
   allocating; a short stream is an error; every picture of one stream must have the first
   one's size. ffmpeg's messages are read on a thread (so a full pipe never stalls it) and
   only their first 64 KB are kept. Asking ffmpeg for the size beforehand is therefore never
   trusted: `ffmpeg -i`'s description (`parse_probe`: length, first video stream that isn't
   cover art, rotation, whether there is sound) only feeds the *estimate* shown to the user.
   Both readers are fuzzed (60,000 hostile inputs).

3. **Two ways to take a video, one command**: `file.import_video {path, mode}`
   (File › Import › Import Video…).

   * **`frames`**: the frames become bitmaps on a new layer from the playhead, **one keyframe
     per picture at the document's frame rate**. ffmpeg's `fps` filter resamples to the
     document's rate (divided by `step`), so picture `j` shows on frame `playhead + j × step`
     for `step` frames: the timing is exact by construction and holds at any frame rate. The
     pictures go into a Library folder named after the file. The first sound track comes
     along as a 44.1 kHz stereo 16-bit WAV written by ffmpeg and read by AniCraft's own WAV
     reader (ADR 0020), placed as a **stream** sound on a layer of its own under the pictures.
     Options: `step` (1–60), `max_edge` (default 1280; never enlarged), `start`, `length`,
     `max_frames`, `audio`.
   * **`reference`**: a **locked guide layer right under the active layer** (under its whole folder or
     mask group, at the top level, so a backdrop further down doesn't cover the video) with
     `Layer.video: VideoRef` (the file's path, the picture's place on the stage, the frame it
     starts on, an offset into the video, its length). The document holds no picture. The
     layer being drawn on stays active.

4. **A reference video is in no movie and no export, by construction.** `evaluate` lists, for a
   layer with a video inside its time range, one **placeholder**: the picture's rectangle with
   no fill and no stroke, marked as guide artwork, carrying the layer's opacity (ADR 0111) so
   a faded reference is one click in Layer Properties. No renderer or exporter draws it (guide
   items never render, and it has no paint). The **stage** alone fills it:
   `FrameJob::with_reference(frames)` gives the render job the decoded pictures, keyed into
   the job's cache key by a serial per picture, and `video::fill` turns the placeholder into a
   bitmap item just before rasterising. So the picture sits at its layer's exact place in the
   stack, through the camera and zoom, on the CPU oracle and the GPU stage alike, and
   `evaluate` never touches a file. Picking skips items with no paint, so nothing selects it.

5. **Reference frames are decoded on demand and kept within a budget.** `video::Cache` decodes
   runs of 16 frames (`-ss <moment> … -frames:v 16`, at most 1280 pixels on the long edge) on a
   thread of its own; a look-up never waits. The run under the playhead is decoded first
   (latest wins, so scrubbing leaves no backlog), the next run is fetched ahead once playback
   is half-way through one, and while a picture is on its way the stage shows the last one.
   Runs are evicted least recently shown first to stay under **256 MB**. A failure (ffmpeg
   missing, the file moved) is remembered and said once in the status bar; `layer.set_video
   {path}` relinks, `{start, offset}` retime, `{remove}` takes the video off.

6. **Large videos are bounded, and told up front.** `video::plan` works out, from the
   description alone, the pictures' size, how many there will be and the decoded memory. One
   import takes at most **2000 pictures** and **1 GB of decoded pixels**
   (pictures × width × height × 4); past that the rest is left out, and the dialog says so
   in advance, with what to change (frame step, size, length, or a reference layer). ffmpeg is
   told the same limit (`-frames:v`), and the reader stops there whatever arrives. A video
   over 256 MB decoded opens the dialog on *Reference layer*.

7. **The slow part never touches the document.** `video::prepare(request, control)` probes,
   decodes and compresses (PNG, in parallel batches of eight) away from the session; it counts
   frames into a `publish::Control` and stops when cancelled. The editor runs it on a thread
   behind a progress window with Cancel, then hands the result to the session
   (`Session::staged_video`) and runs `file.import_video` with the same parameters: the command
   takes a staged result only when its request is equal, otherwise it prepares one itself
   (headless, CLI, MCP). The document changes once, at the end, as one undo step; a cancel or
   a failure leaves it untouched (tested).

8. **Embedded pictures keep only their PNG until shown** (`Bitmap::from_rgba_stored`), so an
   import costs its PNG bytes, not every frame's pixels at once.

9. **Plug-ins can't call these commands**: they read files and start a program.

## Consequences

* A video needs ffmpeg installed. That is the price of holding no codec; the message says how
  to get it, and the guide and troubleshooting pages repeat it.
* A document with a reference layer depends on a file outside it. The path is stored as given
  (absolute); moved files show an outline where the video was and a one-line reason, and are
  relinked from Layer Properties. Collecting footage beside the document is future work.
* Embedded frames are PNGs inside the `.anicraft` JSON: a long clip makes a large document.
  The dialog shows the size before importing; a reference layer costs nothing.
* The reference shows the source frame at or just after each moment, the embedded frames the
  nearest one (ffmpeg's seek and `fps` filter); with a source at the document's frame rate the
  two agree exactly (tested), otherwise they can differ by one source frame.
* Bitmaps in the document stay decoded once shown (ADR 0050), so playing through a long
  embedded clip uses up to the decoded size the dialog showed; hence the 1 GB cap.

## Not in this slice

* Sound with a reference layer (lip-sync against the footage's audio), and a reference inside
  a symbol's timeline.
* Moving, scaling or trimming the reference on the stage with the tools (it is placed centred
  at its own size; `layer.set_video` retimes it).
* Alpha in video (frames are opaque), HDR and colour management of footage.
* Collecting or embedding the referenced file with the document; a relative path.
* Compressing the imported sound track (it is kept as WAV), and JPEG-compressed frames (the
  bitmap model is PNG).
* Video in the web build, and playback of XFL video items (ADR 0086 imports stills).
* A preference for the cache budget and the reference size.

## Alternatives considered

* **A pure-Rust decoder in-tree** (for royalty-free formats only): still a codec in the
  process, a tiny share of real footage (phones record H.264/HEVC), and a large surface.
* **Linking libav\***: LGPL/GPL and patent-pool codecs in our binaries. Never.
* **Raw `rgba` video on the pipe**: needs the size before decoding, which means trusting the
  probe (rotation and non-square pixels make it wrong); PPM says its size per picture.
* **PNG on the pipe**: self-describing too, but ffmpeg then compresses and we decompress every
  reference frame for nothing.
* **Drawing the reference in the shell, behind the stage picture**: the stage background and
  the layers below would cover it, and it could not sit at its layer's place in the stack.
* **Putting decoded pixels in the document** for the reference: breaks "the document holds no
  picture", bloats undo snapshots and files.
* **A new `LayerKind::Video`**: every `match` on the kind across the engine, shell, exporters
  and XFL would change for one optional attribute; a guide layer with a video is what Animate
  users already do, and guide layers are already excluded from every output.
