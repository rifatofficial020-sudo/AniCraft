//! Video through the user's own ffmpeg (ADR 0116).
//!
//! AniCraft holds no video codec. Every picture and sound of a video comes out of an ffmpeg the
//! user installed, run as a separate program with an argument list (never a shell), found as
//! Export Video finds it. ffmpeg writes uncompressed pictures (binary PPM, which says its own
//! size) to a pipe; this module reads them with every size checked, so a broken or hostile
//! program on the other end gives an error, never a crash or an unbounded allocation.
//!
//! * [`probe`] asks what a file holds; [`plan`] says, before anything is decoded, how many
//!   frames of what size an import takes and where it is cut short.
//! * [`prepare`] does the slow part of an import away from the document (it can run on another
//!   thread, with progress and Cancel); `file.import_video` puts the result in the document as
//!   one undo step.
//! * [`Cache`] shows a reference video on the stage: frames decoded on demand, a short run at
//!   a time, kept within a memory budget.

use crate::publish::Control;
use crate::{EngineError, Result};
use anicraft_doc::video::VideoRef;
use anicraft_doc::{Bitmap, BitmapId, DisplayList, Document, ElementId, LayerId, Paint, Pixels, Sound, SoundId};
use anicraft_geom::Affine;
use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// Longest edge of a decoded picture, whatever is asked for.
pub const MAX_DECODE_EDGE: u32 = 4096;
/// Longest edge frames are embedded at unless told otherwise.
pub const DEFAULT_EDGE: u32 = 1280;
/// Most frames one import embeds.
pub const MAX_EMBED_FRAMES: u32 = 2000;
/// Most decoded pixels one import embeds, in bytes (frames × width × height × 4).
pub const EMBED_BUDGET: u64 = 1 << 30;
/// Largest frame step: one picture every `step` frames of the document.
pub const MAX_STEP: u32 = 60;
/// How much of ffmpeg's description of a file is read.
const PROBE_BYTES: u64 = 256 * 1024;
/// How much of ffmpeg's messages is kept for an error.
const LOG_BYTES: u64 = 64 * 1024;
/// Largest sound track kept (44.1 kHz stereo 16-bit: about 25 minutes).
const MAX_AUDIO_BYTES: u64 = 256 * 1024 * 1024;

/// Frames a reference video is decoded in at a time.
pub const CHUNK: u32 = 16;
/// Longest edge a reference video is decoded at.
pub const REFERENCE_EDGE: u32 = 1280;
/// Memory the reference frames may take unless told otherwise.
pub const DEFAULT_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// The ffmpeg to run: `given`, else `ANICRAFT_FFMPEG`, else `ffmpeg` on the PATH.
pub fn program(given: Option<&str>) -> OsString {
    if let Some(x) = given.filter(|x| !x.trim().is_empty()) {
        return x.into();
    }
    std::env::var_os("ANICRAFT_FFMPEG").filter(|x| !x.is_empty()).unwrap_or_else(|| "ffmpeg".into())
}

/// ffmpeg isn't there (or can't run): what to do about it.
fn not_started(e: &std::io::Error) -> EngineError {
    EngineError::State(format!("AniCraft reads video with ffmpeg, which could not be started ({e}). Install it from ffmpeg.org, or point ANICRAFT_FFMPEG at it."))
}

/// Starts `cmd`, retrying a few times while the system says the program file is busy
/// (`ETXTBSY`, 26 on Linux and macOS): a program written just now can still be open for
/// writing in another process that forked meanwhile, which passes in milliseconds.
fn spawn(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(26) && cfg!(unix) && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            r => return r,
        }
    }
}

/// A web page can't start programs (ADR 0036).
fn no_programs() -> EngineError {
    EngineError::State("importing video runs ffmpeg, which can't run in a web browser: use the desktop app, or import the frames as PNG images".into())
}

/// `program` as a command that opens no console window of its own (Windows shows one for
/// every console program a windowed app starts).
fn command(program: &OsStr) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The last lines of ffmpeg's messages, for an error.
fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let from = lines.len().saturating_sub(3);
    let text = lines.get(from..).unwrap_or(&[]).join(" | ");
    if text.is_empty() { "no message".into() } else { text.chars().take(400).collect() }
}

// ----- what a file holds ---------------------------------------------------------------------

/// What ffmpeg says a file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    /// The picture's size as shown (turned as the file says).
    pub width: u32,
    pub height: u32,
    /// Seconds; `None` when the file doesn't say.
    pub duration: Option<f64>,
    /// The video's own frame rate, for information.
    pub fps: Option<f64>,
    pub audio: bool,
}

/// `HH:MM:SS.xx` as seconds.
fn clock(text: &str) -> Option<f64> {
    let mut parts = text.trim().splitn(3, ':');
    let h: f64 = parts.next()?.trim().parse().ok()?;
    let m: f64 = parts.next()?.trim().parse().ok()?;
    let s: f64 = parts.next()?.trim().parse().ok()?;
    let t = h * 3600.0 + m * 60.0 + s;
    (h >= 0.0 && m >= 0.0 && s >= 0.0 && t.is_finite() && t > 0.0).then_some(t)
}

/// `1920x1080` at the start of one comma-separated part of a stream line.
fn size_token(part: &str) -> Option<(u32, u32)> {
    let token = part.split_whitespace().next()?;
    let (w, h) = token.split_once('x')?;
    let ok = |t: &str| !t.is_empty() && t.len() <= 5 && t.bytes().all(|b| b.is_ascii_digit());
    if !(ok(w) && ok(h)) {
        return None;
    }
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    (w >= 1 && h >= 1).then_some((w, h))
}

/// Reads ffmpeg's description of a file (what `ffmpeg -i file` prints): the first video stream
/// that isn't cover art, the length, and whether there is sound. `None` without a video stream.
/// The text is for people, so this takes only what it recognises and never trusts a number.
pub fn parse_probe(text: &str) -> Option<Probe> {
    let mut duration = None;
    let mut video: Option<(u32, u32)> = None;
    let mut fps = None;
    let mut audio = false;
    let mut in_video = false;
    for line in text.lines().take(4096) {
        let l = line.trim();
        if duration.is_none()
            && let Some(rest) = l.strip_prefix("Duration:")
        {
            duration = rest.split(',').next().and_then(clock);
        }
        if l.starts_with("Stream #") {
            in_video = false;
            if let Some(at) = l.find(": Video:") {
                let rest = l.get(at + 8..).unwrap_or("");
                if video.is_some() || rest.contains("(attached pic)") {
                    continue;
                }
                if let Some(size) = rest.split(',').find_map(size_token) {
                    video = Some(size);
                    in_video = true;
                    fps = rest.split(',').find_map(|p| p.trim().strip_suffix(" fps").and_then(|n| n.trim().parse::<f64>().ok())).filter(|f| f.is_finite() && *f > 0.0);
                }
            } else if l.contains(": Audio:") {
                audio = true;
            }
        } else if in_video
            && let Some(at) = l.find("rotation of ")
        {
            // A phone's upright film is stored on its side; ffmpeg turns it as it decodes.
            let turn = l.get(at + 12..).and_then(|r| r.split_whitespace().next()).and_then(|d| d.parse::<f64>().ok()).filter(|d| d.is_finite() && d.abs() <= 3600.0);
            if let (Some(deg), Some((w, h))) = (turn, video)
                && ((deg / 90.0).round() as i64) % 2 != 0
            {
                video = Some((h, w));
            }
            in_video = false;
        }
    }
    let (width, height) = video?;
    Some(Probe { width, height, duration: duration.map(|d| d.min(anicraft_doc::video::MAX_VIDEO_SECONDS)), fps, audio })
}

/// Asks `ffmpeg` what the file at `path` holds.
pub fn probe(ffmpeg: &OsStr, path: &Path) -> Result<Probe> {
    if crate::vfs::in_memory() {
        return Err(no_programs());
    }
    // ffmpeg opens the file, not `vfs`: the automation roots are asked here (ADR 0124).
    let path = &crate::access::for_program(path, crate::access::Access::Read)?;
    let mut child = spawn(command(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped())).map_err(|e| not_started(&e))?;
    let mut text = Vec::new();
    if let Some(e) = child.stderr.take() {
        let _ = e.take(PROBE_BYTES).read_to_end(&mut text);
    }
    let _ = child.kill();
    let _ = child.wait();
    let text = String::from_utf8_lossy(&text);
    parse_probe(&text).ok_or_else(|| EngineError::Format(format!("{} has no video ffmpeg can read ({})", path.display(), tail(&text))))
}

// ----- what an import takes ------------------------------------------------------------------

/// How a video is taken.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// One picture every `step` frames of the document (1: every frame).
    pub step: u32,
    /// Longest edge of the pictures, in pixels.
    pub max_edge: u32,
    /// Seconds into the video to start at.
    pub start: f64,
    /// Seconds of video to take; `None`: to its end.
    pub length: Option<f64>,
    /// Most frames to take (the timeline's room, or a smaller number asked for).
    pub max_frames: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { step: 1, max_edge: DEFAULT_EDGE, start: 0.0, length: None, max_frames: MAX_EMBED_FRAMES }
    }
}

/// What an import takes, worked out before anything is decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The box the pictures are decoded into (they keep their shape inside it).
    pub width: u32,
    pub height: u32,
    /// How much larger than decoded the pictures are shown (1 unless the size cap bit).
    pub upscale: f32,
    /// Seconds of video taken; `None` when the file doesn't say how long it is.
    pub seconds: Option<f64>,
    /// Pictures taken (after the caps); `None` when the length is unknown.
    pub frames: Option<u32>,
    /// Most pictures it takes: the frame cap, the memory cap and the room asked for.
    pub limit: u32,
    /// The video is longer than `limit` pictures: the rest is left out.
    pub truncated: bool,
    /// Decoded pixels of the pictures taken, in bytes (`limit` of them when the length is
    /// unknown).
    pub memory: u64,
}

/// Works out what importing `probe` into a document at `fps` with a `stage`-sized stage takes.
/// The picture is shown at its own size, smaller when the stage is smaller, and decoded no
/// larger than `max_edge`.
pub fn plan(probe: &Probe, fps: f32, stage: (u32, u32), o: &Options) -> Plan {
    let (w, h) = (f64::from(probe.width.max(1)), f64::from(probe.height.max(1)));
    let fit = (f64::from(stage.0.max(1)) / w).min(f64::from(stage.1.max(1)) / h).min(1.0);
    let (sw, sh) = (w * fit, h * fit);
    let cap = (f64::from(o.max_edge.clamp(1, MAX_DECODE_EDGE)) / sw.max(sh)).min(1.0);
    let edge = |v: f64| (v * cap).round().clamp(1.0, f64::from(MAX_DECODE_EDGE)) as u32;
    let (width, height) = (edge(sw), edge(sh));
    let per_frame = u64::from(width) * u64::from(height) * 4;
    let limit = (EMBED_BUDGET / per_frame.max(1)).min(u64::from(o.max_frames.min(MAX_EMBED_FRAMES))).max(1) as u32;
    let seconds = probe.duration.map(|d| {
        let left = (d - o.start).max(0.0);
        o.length.map_or(left, |l| left.min(l.max(0.0)))
    });
    let wanted = seconds.map(|s| ((s * f64::from(fps) / f64::from(o.step.max(1))).round().clamp(1.0, 4e9)) as u32);
    let frames = wanted.map(|n| n.min(limit));
    Plan { width, height, upscale: (1.0 / cap) as f32, seconds, frames, limit, truncated: wanted.is_some_and(|n| n > limit), memory: u64::from(frames.unwrap_or(limit)) * per_frame }
}

// ----- pictures from a pipe ------------------------------------------------------------------

fn byte<R: Read>(r: &mut R) -> std::io::Result<Option<u8>> {
    let mut b = [0u8; 1];
    loop {
        return match r.read(&mut b) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(b[0])),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => Err(e),
        };
    }
}

/// One number of a PPM header: white space, then up to six digits, then one white space byte.
fn header_number<R: Read>(r: &mut R) -> Option<u32> {
    let mut n: Option<u32> = None;
    for _ in 0..24 {
        let b = byte(r).ok()??;
        match (b, n) {
            (b'0'..=b'9', _) => {
                let v = n.unwrap_or(0) * 10 + u32::from(b - b'0');
                if v > 999_999 {
                    return None;
                }
                n = Some(v);
            }
            (b' ' | b'\t' | b'\r' | b'\n', None) => {}
            (b' ' | b'\t' | b'\r' | b'\n', Some(v)) => return Some(v),
            _ => return None,
        }
    }
    None
}

/// The next picture of a stream of binary PPM images (`P6`, 8 bits a channel), as RGBA.
/// `Ok(None)` at a clean end. A picture larger than `max_w × max_h`, any other kind of image,
/// or a stream that stops mid-picture is an error; nothing is allocated before the size is
/// checked.
pub fn read_ppm<R: Read>(r: &mut R, max_w: u32, max_h: u32) -> std::result::Result<Option<Pixels>, String> {
    let bad = || "ffmpeg sent something that is not a picture".to_string();
    match byte(r).map_err(|e| e.to_string())? {
        None => return Ok(None),
        Some(b'P') => {}
        Some(_) => return Err(bad()),
    }
    if byte(r).map_err(|e| e.to_string())? != Some(b'6') {
        return Err(bad());
    }
    let (Some(width), Some(height), Some(max)) = (header_number(r), header_number(r), header_number(r)) else { return Err(bad()) };
    if max != 255 {
        return Err(bad());
    }
    if width == 0 || height == 0 || width > max_w || height > max_h {
        return Err(format!("ffmpeg sent a {width} × {height} picture, larger than the {max_w} × {max_h} asked for"));
    }
    let n = width as usize * height as usize;
    let mut rgb = vec![0u8; n * 3];
    r.read_exact(&mut rgb).map_err(|_| "ffmpeg stopped in the middle of a picture".to_string())?;
    let mut rgba = Vec::with_capacity(n * 4);
    for p in rgb.as_chunks::<3>().0 {
        rgba.extend_from_slice(p);
        rgba.push(255);
    }
    Ok(Some(Pixels { width, height, rgba }))
}

/// The filters every decode shares: square pixels, then no larger than `w × h`, shape kept.
fn scale_filter(w: u32, h: u32) -> String {
    format!("scale='max(1,iw*sar)':ih,setsar=1,scale=w='min(iw,{w})':h='min(ih,{h})':force_original_aspect_ratio=decrease:flags=bicubic")
}

/// A running ffmpeg sending pictures. Dropped, it is stopped.
struct Stream {
    child: Child,
    out: Option<BufReader<std::process::ChildStdout>>,
    log: Option<std::thread::JoinHandle<String>>,
    max: (u32, u32),
    size: Option<(u32, u32)>,
}

impl Stream {
    /// Starts `ffmpeg` on `path` from `start` seconds: at most `frames` pictures at `rate`
    /// pictures a second, each inside `max`.
    fn open(ffmpeg: &OsStr, path: &Path, start: f64, length: Option<f64>, rate: f64, max: (u32, u32), frames: u32) -> Result<Stream> {
        if crate::vfs::in_memory() {
            return Err(no_programs());
        }
        let path = &crate::access::for_program(path, crate::access::Access::Read)?;
        let mut c = command(ffmpeg);
        c.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if start > 0.0 {
            c.args(["-ss", &format!("{start:.6}")]);
        }
        if let Some(l) = length {
            c.args(["-t", &format!("{:.6}", l.max(0.0))]);
        }
        c.arg("-i").arg(path);
        c.args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &format!("fps={rate},{}", scale_filter(max.0, max.1)), "-frames:v", &frames.max(1).to_string()]);
        c.args(["-c:v", "ppm", "-pix_fmt", "rgb24", "-f", "image2pipe", "-"]);
        let mut child = spawn(c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())).map_err(|e| not_started(&e))?;
        // ffmpeg's messages are read as they come, so a full pipe never stalls it.
        let log = child.stderr.take().and_then(|e| {
            std::thread::Builder::new()
                .name("anicraft-ffmpeg-log".into())
                .spawn(move || {
                    let mut text = Vec::new();
                    let mut e = e;
                    let _ = (&mut e).take(LOG_BYTES).read_to_end(&mut text);
                    // Past what is kept, the rest is read and dropped.
                    let _ = std::io::copy(&mut e, &mut std::io::sink());
                    String::from_utf8_lossy(&text).into_owned()
                })
                .ok()
        });
        let out = child.stdout.take().map(|o| BufReader::with_capacity(1 << 16, o));
        Ok(Stream { child, out, log, max, size: None })
    }

    /// The next picture; `None` at the end. Every picture has the first one's size.
    fn next(&mut self) -> Result<Option<Pixels>> {
        let Some(out) = self.out.as_mut() else { return Ok(None) };
        let px = read_ppm(out, self.max.0, self.max.1).map_err(EngineError::Format)?;
        if let Some(p) = &px {
            match self.size {
                None => self.size = Some((p.width, p.height)),
                Some(s) if s == (p.width, p.height) => {}
                Some(_) => return Err(EngineError::Format("ffmpeg sent pictures of different sizes".into())),
            }
        }
        Ok(px)
    }

    /// Stops ffmpeg and gives what it said.
    fn finish(mut self) -> String {
        self.stop()
    }

    fn stop(&mut self) -> String {
        let _ = self.child.kill();
        self.out = None;
        let _ = self.child.wait();
        self.log.take().and_then(|h| h.join().ok()).unwrap_or_default()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ----- importing -----------------------------------------------------------------------------

/// One import, as asked for. Equal requests give equal results, which is how the editor hands
/// `file.import_video` a result it prepared on another thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub path: PathBuf,
    pub ffmpeg: OsString,
    /// A reference layer (the video stays a file) rather than frames in the document.
    pub reference: bool,
    pub options: Options,
    /// Take the sound track too (frames only).
    pub audio: bool,
    /// The document's frame rate and stage size.
    pub fps: f32,
    pub stage: (u32, u32),
}

/// The import `file.import_video` would run for params `p` on `s`, checked; nothing is started.
/// The editor prepares exactly this on another thread and hands the result over.
pub fn request(s: &crate::Session, p: &serde_json::Value) -> Result<Request> {
    crate::cmds_video::request(s, p)
}

/// The slow part of an import, done: ready to go into a document.
pub struct Prepared {
    pub request: Request,
    pub probe: Probe,
    pub plan: Plan,
    /// The pictures' size as decoded.
    pub width: u32,
    pub height: u32,
    pub what: What,
}

pub enum What {
    /// The video's frames as bitmaps (ids not yet given), and its sound.
    Frames { bitmaps: Vec<Bitmap>, sound: Option<Sound>, truncated: bool, note: Option<String> },
    /// A reference layer: the video's length.
    Reference { duration: f64 },
}

/// Does the slow part of an import: asks ffmpeg what the file is and, for frames, decodes them
/// (and the sound). `job` counts frames and cancels. The document is not involved.
pub fn prepare(req: &Request, job: Option<&Control>) -> Result<Prepared> {
    let probe = probe(&req.ffmpeg, &req.path)?;
    let plan = plan(&probe, req.fps, req.stage, &req.options);
    let o = &req.options;
    if req.reference {
        let duration = probe.duration.ok_or_else(|| EngineError::Format(format!("{} doesn't say how long it is, so it can't be a reference layer: import it as frames", req.path.display())))?;
        if o.start >= duration {
            return Err(EngineError::Param(format!("'start' is past the end of the video ({duration:.2} s)")));
        }
        // One picture, so a file ffmpeg can't decode is refused now, and for its true size.
        let mut stream = Stream::open(&req.ffmpeg, &req.path, o.start, None, f64::from(req.fps), (plan.width, plan.height), 1)?;
        let first = stream.next();
        let log = stream.finish();
        let px = first?.ok_or_else(|| EngineError::Format(format!("ffmpeg could not read a picture from {} ({})", req.path.display(), tail(&log))))?;
        return Ok(Prepared { request: req.clone(), probe, plan, width: px.width, height: px.height, what: What::Reference { duration } });
    }
    let limit = plan.limit;
    if let Some(j) = job {
        j.begin(u64::from(plan.frames.unwrap_or(limit)));
    }
    let rate = f64::from(req.fps) / f64::from(o.step.max(1));
    let mut stream = Stream::open(&req.ffmpeg, &req.path, o.start, o.length, rate, (plan.width, plan.height), limit)?;
    let mut bitmaps: Vec<Bitmap> = Vec::new();
    let mut done = false;
    while !done && (bitmaps.len() as u32) < limit {
        // A few pictures at a time: they compress in parallel, and memory stays bounded.
        let mut batch = Vec::with_capacity(8);
        while batch.len() < 8 && ((bitmaps.len() + batch.len()) as u32) < limit {
            match stream.next()? {
                Some(px) => batch.push(px),
                None => {
                    done = true;
                    break;
                }
            }
        }
        let made: Vec<Option<Bitmap>> = batch.par_iter().map(|px| Bitmap::from_rgba_stored(BitmapId(0), "", px.width, px.height, &px.rgba)).collect();
        let n = made.len() as u64;
        for b in made {
            bitmaps.push(b.ok_or_else(|| EngineError::Format("a video frame could not be kept as a picture".into()))?);
        }
        crate::publish::tick(job, n)?;
    }
    let size = stream.size;
    let log = stream.finish();
    let Some((width, height)) = size.filter(|_| !bitmaps.is_empty()) else {
        return Err(EngineError::Format(format!("ffmpeg could not read a picture from {} ({})", req.path.display(), tail(&log))));
    };
    // The video went on past the cap when it stopped exactly at it and was known to be longer.
    let truncated = plan.truncated || (plan.frames.is_none() && bitmaps.len() as u32 >= limit);
    let (sound, note) = if req.audio && probe.audio {
        let seconds = bitmaps.len() as f64 / rate;
        match audio(&req.ffmpeg, &req.path, o.start, seconds) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(format!("the sound track was left out: {e}"))),
        }
    } else {
        (None, None)
    };
    crate::publish::tick(job, 0)?;
    Ok(Prepared { request: req.clone(), probe, plan, width, height, what: What::Frames { bitmaps, sound, truncated, note } })
}

/// A file in the temp folder, removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `seconds` of the video's first sound track from `start`, as a sound (id not yet given):
/// ffmpeg writes it as a 44.1 kHz stereo 16-bit WAV, which AniCraft's own reader takes.
fn audio(ffmpeg: &OsStr, path: &Path, start: f64, seconds: f64) -> std::result::Result<Sound, String> {
    let path = &crate::access::for_program(path, crate::access::Access::Read).map_err(|e| e.to_string())?;
    let tmp = TempFile(std::env::temp_dir().join(format!("anicraft-video-{}-{}.wav", std::process::id(), crate::vfs::random_suffix())));
    let mut c = command(ffmpeg);
    c.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
    if start > 0.0 {
        c.args(["-ss", &format!("{start:.6}")]);
    }
    c.args(["-t", &format!("{:.6}", seconds.max(0.0))]).arg("-i").arg(path);
    c.args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "2", "-ar", "44100", "-c:a", "pcm_s16le", "-f", "wav"]).arg(&tmp.0);
    let out = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(tail(&String::from_utf8_lossy(out.stderr.get(..out.stderr.len().min(LOG_BYTES as usize)).unwrap_or(&[]))));
    }
    let len = std::fs::metadata(&tmp.0).map_err(|e| e.to_string())?.len();
    if len > MAX_AUDIO_BYTES {
        return Err("it is too long to keep".into());
    }
    let bytes = std::fs::read(&tmp.0).map_err(|e| e.to_string())?;
    Sound::from_data(SoundId(0), "", bytes).map_err(|e| e.to_string())
}

// ----- a reference video on the stage --------------------------------------------------------

/// A reference video's picture for one frame of the stage.
#[derive(Clone, Debug)]
pub struct RefFrame {
    /// The placeholder it fills ([`VideoRef::id`]).
    pub element: ElementId,
    pub pixels: Arc<Pixels>,
    /// Names the picture: equal serials, equal pixels (for caching what is drawn with it).
    pub serial: u64,
}

/// Fills the reference videos' placeholders in `dl` with their pictures: the stage's view of a
/// frame, never an export's. A placeholder without a picture stays as it is (nothing drawn).
pub fn fill(dl: &mut DisplayList, frames: &[RefFrame]) {
    for f in frames {
        // The frame's own placeholder is the last one (onion frames, if any, lie under it).
        let Some(item) = dl.items.iter_mut().rev().find(|i| i.element == f.element && i.guide && i.fill.is_none() && i.stroke.is_none()) else { continue };
        let b = item.path.bounds();
        let (pw, ph) = (f.pixels.width.max(1) as f32, f.pixels.height.max(1) as f32);
        let matrix = Affine::translate(b.min.x, b.min.y).mul(&Affine::scale(b.width() / pw, b.height() / ph));
        if !matrix.is_finite() {
            continue;
        }
        item.fill = Some(Paint::Bitmap { bitmap: BitmapId(0), repeat: false, matrix });
        item.image = Some(f.pixels.clone());
        item.guide = false;
    }
}

/// A run of [`CHUNK`] frames of one video at one size and timing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    width: u32,
    height: u32,
    fps: u32,
    offset: u64,
    chunk: u32,
}

impl Key {
    /// The run holding timeline frame `frame` of `v`, and the frame's place in it.
    fn of(v: &VideoRef, frame: u32, fps: f32) -> Option<(Key, usize)> {
        v.time_at(frame, fps)?;
        let n = frame.checked_sub(v.start)?;
        let cap = (f64::from(REFERENCE_EDGE) / f64::from(v.width.max(v.height).max(1))).min(1.0);
        let edge = |e: u32| ((f64::from(e) * cap).round() as u32).clamp(1, MAX_DECODE_EDGE);
        Some((Key { path: v.path.clone(), width: edge(v.width), height: edge(v.height), fps: fps.to_bits(), offset: v.offset.to_bits(), chunk: n / CHUNK }, (n % CHUNK) as usize))
    }

    fn next(&self) -> Key {
        Key { chunk: self.chunk.saturating_add(1), ..self.clone() }
    }

    /// Seconds into the video where the run starts.
    fn start(&self) -> f64 {
        f64::from_bits(self.offset) + f64::from(self.chunk) * f64::from(CHUNK) / f64::from(f32::from_bits(self.fps))
    }
}

enum Chunk {
    Ready { frames: Vec<(u64, Arc<Pixels>)>, bytes: u64, used: u64 },
    Failed(String),
}

#[derive(Default)]
struct State {
    chunks: HashMap<Key, Chunk>,
    /// Runs to decode, the most wanted first (the one under the playhead, then the next).
    wanted: VecDeque<Key>,
    decoding: Option<Key>,
    /// The picture each video showed last: shown while the one asked for is on its way.
    last: HashMap<ElementId, RefFrame>,
    bytes: u64,
    clock: u64,
    serial: u64,
    worker: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    idle: Condvar,
    generation: AtomicU64,
    closed: AtomicBool,
    budget: u64,
    /// The ffmpeg to decode with; `None`: found as [`program`] finds it, each time.
    ffmpeg: Option<OsString>,
}

/// What the cache has for a frame.
#[derive(Clone, Debug)]
pub enum Lookup {
    Ready(RefFrame),
    /// On its way; meanwhile, the picture the video showed last (if any).
    Pending(Option<RefFrame>),
    /// It can't be shown, and why (ffmpeg missing, the file gone, …).
    Failed(String),
    /// The frame is before the video's start or after its end.
    Outside,
}

/// Reference videos' pictures, decoded on demand on a thread of its own and kept within a
/// memory budget (least recently shown runs go first). The run shown last and the one just
/// decoded always stay, so a budget smaller than two runs still plays.
pub struct Cache {
    shared: Arc<Shared>,
}

impl Default for Cache {
    fn default() -> Self {
        Cache::new(DEFAULT_CACHE_BYTES)
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }
}

impl Cache {
    pub fn new(budget: u64) -> Cache {
        Cache::with_ffmpeg(budget, None)
    }

    /// A cache that decodes with the ffmpeg at `ffmpeg` (`None`: the one [`program`] finds).
    pub fn with_ffmpeg(budget: u64, ffmpeg: Option<OsString>) -> Cache {
        Cache { shared: Arc::new(Shared { state: Mutex::new(State::default()), wake: Condvar::new(), idle: Condvar::new(), generation: AtomicU64::new(0), closed: AtomicBool::new(false), budget, ffmpeg }) }
    }

    /// Changes whenever a picture arrives (or fails): time to draw the stage again.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }

    /// Whether pictures are on their way.
    pub fn pending(&self) -> bool {
        let st = lock(&self.shared.state);
        st.decoding.is_some() || !st.wanted.is_empty()
    }

    /// Bytes of pictures held, and in how many runs.
    pub fn usage(&self) -> (u64, usize) {
        let st = lock(&self.shared.state);
        (st.bytes, st.chunks.values().filter(|c| matches!(c, Chunk::Ready { .. })).count())
    }

    /// Forgets what is known about the video at `path`, failures included (Relink, or trying
    /// again after installing ffmpeg).
    pub fn forget(&self, path: &str) {
        let mut st = lock(&self.shared.state);
        let st = &mut *st;
        st.chunks.retain(|k, _| k.path != path);
        st.wanted.retain(|k| k.path != path);
        st.bytes = st.chunks.values().map(|c| if let Chunk::Ready { bytes, .. } = c { *bytes } else { 0 }).sum();
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Forgets every failure, so the next look tries again.
    pub fn retry(&self) {
        lock(&self.shared.state).chunks.retain(|_, c| !matches!(c, Chunk::Failed(_)));
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Waits until nothing is on its way, at most `timeout`; whether it got there.
    pub fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let st = lock(&self.shared.state);
        let (st, _) = self.shared.idle.wait_timeout_while(st, timeout, |s| s.decoding.is_some() || !s.wanted.is_empty()).unwrap_or_else(PoisonError::into_inner);
        st.decoding.is_none() && st.wanted.is_empty()
    }

    /// The picture `v` shows at timeline frame `frame`. With `ask`, a picture that isn't here
    /// is decoded (this never waits for it), and the run after the one shown is fetched ahead.
    pub fn lookup(&self, v: &VideoRef, frame: u32, fps: f32, ask: bool) -> Lookup {
        let Some((key, at)) = Key::of(v, frame, fps) else { return Lookup::Outside };
        if crate::vfs::in_memory() {
            return Lookup::Failed("a web browser can't run ffmpeg, so reference videos show in the desktop app only".into());
        }
        // A video a document links to outside the automation roots is not shown to an agent
        // (ADR 0124); the decoding thread has no policy of its own, so it is asked here.
        if let Err(e) = crate::access::check(Path::new(&v.path), crate::access::Access::Read) {
            return Lookup::Failed(e.to_string());
        }
        let mut guard = lock(&self.shared.state);
        let st = &mut *guard;
        st.clock += 1;
        let clock = st.clock;
        let found = match st.chunks.get_mut(&key) {
            Some(Chunk::Ready { frames, used, .. }) => {
                *used = clock;
                Some(match frames.get(at) {
                    Some((serial, px)) => Lookup::Ready(RefFrame { element: v.id, pixels: px.clone(), serial: *serial }),
                    // The video ended inside this run.
                    None => Lookup::Outside,
                })
            }
            Some(Chunk::Failed(why)) => Some(Lookup::Failed(why.clone())),
            None => None,
        };
        if let Some(found) = found {
            if let Lookup::Ready(f) = &found {
                st.last.insert(v.id, f.clone());
                // Playing on: the next run is fetched before the playhead gets there.
                let next = key.next();
                let ends = v.time_at(frame.saturating_add(CHUNK - at as u32), fps).is_none();
                if ask && at as u32 >= CHUNK / 2 && !ends && !st.chunks.contains_key(&next) && st.decoding.as_ref() != Some(&next) && !st.wanted.contains(&next) {
                    st.wanted.push_back(next);
                    self.start(st);
                }
            }
            return found;
        }
        if ask && st.decoding.as_ref() != Some(&key) && st.wanted.front() != Some(&key) {
            // The latest wins: a scrub leaves no backlog.
            st.wanted.clear();
            st.wanted.push_back(key);
            if !self.start(st) {
                return Lookup::Failed("the video decoder could not be started".into());
            }
        }
        Lookup::Pending(st.last.get(&v.id).cloned())
    }

    /// Wakes the worker, starting it the first time. `false` when no thread can be started.
    fn start(&self, st: &mut State) -> bool {
        if !st.worker {
            let shared = self.shared.clone();
            if std::thread::Builder::new().name("anicraft-video".into()).spawn(move || work(&shared)).is_err() {
                st.wanted.clear();
                return false;
            }
            st.worker = true;
        }
        self.shared.wake.notify_all();
        true
    }
}

/// The decoding thread: one run at a time, until the cache is dropped.
fn work(shared: &Shared) {
    loop {
        let key = {
            let mut st = lock(&shared.state);
            loop {
                if shared.closed.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(k) = st.wanted.pop_front() {
                    st.decoding = Some(k.clone());
                    break k;
                }
                shared.idle.notify_all();
                st = shared.wake.wait_timeout(st, std::time::Duration::from_millis(500)).unwrap_or_else(PoisonError::into_inner).0;
            }
        };
        let result = decode_chunk(&shared.ffmpeg.clone().unwrap_or_else(|| program(None)), &key);
        let mut guard = lock(&shared.state);
        let st = &mut *guard;
        st.decoding = None;
        let chunk = match result {
            Ok(frames) => {
                let bytes = frames.iter().map(|p| p.rgba.len() as u64).sum();
                st.bytes += bytes;
                st.clock += 1;
                let frames = frames
                    .into_iter()
                    .map(|p| {
                        st.serial += 1;
                        (st.serial, Arc::new(p))
                    })
                    .collect();
                Chunk::Ready { frames, bytes, used: st.clock }
            }
            Err(why) => Chunk::Failed(why),
        };
        if let Some(Chunk::Ready { bytes, .. }) = st.chunks.insert(key.clone(), chunk) {
            st.bytes = st.bytes.saturating_sub(bytes);
        }
        // Over budget, the runs shown longest ago go: never the one that just arrived, nor the
        // one shown last (the run under the playhead while the next one is fetched ahead).
        let shown = st.chunks.iter().filter(|(k, _)| **k != key).filter_map(|(k, c)| if let Chunk::Ready { used, .. } = c { Some((*used, k.clone())) } else { None }).max_by_key(|(used, _)| *used).map(|(_, k)| k);
        while st.bytes > shared.budget {
            let oldest = st.chunks.iter().filter(|(k, _)| **k != key && Some(*k) != shown.as_ref()).filter_map(|(k, c)| if let Chunk::Ready { used, .. } = c { Some((*used, k.clone())) } else { None }).min_by_key(|(used, _)| *used);
            let Some((_, k)) = oldest else { break };
            if let Some(Chunk::Ready { bytes, .. }) = st.chunks.remove(&k) {
                st.bytes = st.bytes.saturating_sub(bytes);
            }
        }
        shared.generation.fetch_add(1, Ordering::Relaxed);
        if st.wanted.is_empty() {
            shared.idle.notify_all();
        }
    }
}

/// Decodes one run. A run past the video's end is empty, which is not an error.
fn decode_chunk(ffmpeg: &OsStr, key: &Key) -> std::result::Result<Vec<Pixels>, String> {
    let path = Path::new(&key.path);
    if !path.is_file() {
        return Err(format!("the video file is missing: {}", key.path));
    }
    let fps = f64::from(f32::from_bits(key.fps));
    let mut stream = Stream::open(ffmpeg, path, key.start(), None, fps, (key.width, key.height), CHUNK).map_err(|e| e.to_string())?;
    let mut frames = Vec::with_capacity(CHUNK as usize);
    let mut failure = None;
    while frames.len() < CHUNK as usize {
        match stream.next() {
            Ok(Some(px)) => frames.push(px),
            Ok(None) => break,
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }
    let log = stream.finish();
    match failure {
        Some(e) => Err(e),
        None if frames.is_empty() && !log.trim().is_empty() => Err(format!("ffmpeg could not read {} ({})", key.path, tail(&log))),
        None => Ok(frames),
    }
}

/// What the stage shows of a document's reference videos at one frame.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    /// The pictures, for [`crate::render_service::FrameJob::with_reference`].
    pub frames: Option<Arc<Vec<RefFrame>>>,
    /// Pictures are on their way: look again soon.
    pub pending: bool,
    /// A video that can't be shown: its layer and why.
    pub failed: Option<(LayerId, String)>,
}

/// The pictures of the reference videos on movie frame `frame` of `doc` (the visible layers
/// of the scene it falls in). With `ask`, missing ones are decoded for a later look.
pub fn shown(cache: &Cache, doc: &Document, frame: u32, ask: bool) -> Shown {
    let (scene, local) = doc.locate(frame);
    let Some(tl) = doc.scenes.get(scene).map(|s| &s.timeline) else { return Shown::default() };
    let mut out = Shown::default();
    let mut frames = Vec::new();
    for (index, layer) in tl.layers.iter().enumerate() {
        let Some(v) = layer.video.as_deref().filter(|_| layer.visible && !tl.hidden_by_folder(index)) else { continue };
        match cache.lookup(v, local, doc.stage.fps, ask) {
            Lookup::Ready(f) => frames.push(f),
            Lookup::Pending(last) => {
                out.pending = true;
                frames.extend(last);
            }
            Lookup::Failed(why) => out.failed = out.failed.or(Some((layer.id, why))),
            Lookup::Outside => {}
        }
    }
    if !frames.is_empty() {
        out.frames = Some(Arc::new(frames));
    }
    out
}
