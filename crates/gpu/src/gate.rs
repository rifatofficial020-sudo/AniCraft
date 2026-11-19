//! When work may be submitted on a device shared with a window (ADR 0105).
//!
//! A window reconfigures its surface when it opens and whenever it is resized. wgpu first waits
//! for the device's queue to empty, and fails the reconfiguration when another thread submits
//! work meanwhile ("Failed to wait for GPU to come idle before reconfiguring the Surface": its
//! own comment says it "can only be non-empty if another thread is submitting at the same
//! time"). The stage renderer draws on the window's device from the render service's threads,
//! so that is what it did: the error behind AniCraft closing at start-up and on resizing
//! (#264, #309) before ADR 0092 kept the window up, and a skipped frame and a false "graphics
//! driver problem" since.
//!
//! The window's thread owns a [`SubmitGate`]: open while it runs the app's frame (it can't be
//! reconfiguring a surface then), closed the rest of the time. A renderer given the gate
//! ([`crate::GpuRenderer::with_gate`]) submits through [`SubmitGate::run`], which waits for the
//! gate to open and asks the owner for a frame. Nothing waits for ever: a submission goes ahead
//! after [`SUBMIT_WAIT`] (a window that draws no frames, minimised say), and closing waits at
//! most [`CLOSE_WAIT`] for submissions under way; then the old race is possible again, and
//! survived as before.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// Longest a submission waits for the gate to open.
pub const SUBMIT_WAIT: Duration = Duration::from_millis(250);
/// Longest closing the gate waits for submissions under way (each takes well under a
/// millisecond).
pub const CLOSE_WAIT: Duration = Duration::from_millis(100);

#[derive(Default)]
struct State {
    open: bool,
    /// Submissions under way.
    running: u32,
}

/// See the module.
pub struct SubmitGate {
    state: Mutex<State>,
    changed: Condvar,
    /// The thread that opens and closes the gate, and reconfigures the surface: its own
    /// submissions never wait (it can't be doing both).
    owner: ThreadId,
    /// Asks the owner for a frame (a frame opens the gate).
    wake: Box<dyn Fn() + Send + Sync>,
    waits: AtomicU64,
    timeouts: AtomicU64,
}

/// One submission under way: counted out when it ends, however it ends.
struct Running<'a>(&'a SubmitGate);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let mut s = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.running = s.running.saturating_sub(1);
        drop(s);
        self.0.changed.notify_all();
    }
}

impl SubmitGate {
    /// A closed gate owned by the calling thread; `wake` asks that thread for a frame.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> SubmitGate {
        SubmitGate { state: Mutex::new(State::default()), changed: Condvar::new(), owner: std::thread::current().id(), wake: Box::new(wake), waits: AtomicU64::new(0), timeouts: AtomicU64::new(0) }
    }

    /// The owner's frame begins: submissions may run.
    pub fn open(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).open = true;
        self.changed.notify_all();
    }

    /// The owner's frame ends: no submission starts from now on, and those under way are waited
    /// for (at most [`CLOSE_WAIT`]). Whether the queue is now left alone.
    pub fn close(&self) -> bool {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.open = false;
        let until = Instant::now() + CLOSE_WAIT;
        while s.running > 0 {
            let Some(left) = until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { return false };
            s = self.changed.wait_timeout(s, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }

    /// Runs `submit` while the gate is open, waiting for it to open if need be (at most
    /// [`SUBMIT_WAIT`]; the owner is asked for a frame). On the owner's thread it runs at once.
    pub fn run<R>(&self, submit: impl FnOnce() -> R) -> R {
        let _running = self.enter();
        submit()
    }

    fn enter(&self) -> Running<'_> {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !s.open && std::thread::current().id() != self.owner {
            self.waits.fetch_add(1, Ordering::Relaxed);
            let until = Instant::now() + SUBMIT_WAIT;
            while !s.open {
                // Not under the lock: the wake belongs to the window system.
                drop(s);
                (self.wake)();
                s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if s.open {
                    break;
                }
                let Some(left) = until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                    self.timeouts.fetch_add(1, Ordering::Relaxed);
                    break;
                };
                // Woken when the gate opens; asks again now and then in case a wake was lost.
                s = self.changed.wait_timeout(s, left.min(Duration::from_millis(50))).unwrap_or_else(PoisonError::into_inner).0;
            }
        }
        s.running = s.running.saturating_add(1);
        Running(self)
    }

    /// Submissions that had to wait for the gate, and those that gave up waiting and went
    /// ahead (for the log).
    pub fn waited(&self) -> (u64, u64) {
        (self.waits.load(Ordering::Relaxed), self.timeouts.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// The race itself, without a GPU: an owner that "reconfigures" between frames (it checks
    /// that nothing is submitting, as wgpu does) while four threads submit as fast as they can.
    /// Through the gate no submission ever overlaps a reconfiguration; and the test's own
    /// detector does see overlaps without it.
    #[test]
    fn nothing_is_submitted_while_the_gate_is_closed() {
        fn run(gated: bool) -> (u64, u64) {
            // The run without the gate only shows that the detector works: there the threads
            // are submitting nearly all the time, so it can't miss them on a busy machine.
            let work = if gated { Duration::from_micros(20) } else { Duration::from_millis(2) };
            let frames = Arc::new(AtomicU64::new(0));
            let gate = Arc::new(SubmitGate::new(|| {}));
            let submitting = Arc::new(AtomicU64::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicU64::new(0));
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    let (gate, submitting, stop, done) = (gate.clone(), submitting.clone(), stop.clone(), done.clone());
                    std::thread::spawn(move || {
                        while !stop.load(Ordering::Relaxed) {
                            let submit = || {
                                submitting.fetch_add(1, Ordering::SeqCst);
                                // A moment's work, without the sleep's coarse clock.
                                let t = Instant::now();
                                while t.elapsed() < work {
                                    std::hint::spin_loop();
                                }
                                submitting.fetch_sub(1, Ordering::SeqCst);
                            };
                            if gated { gate.run(submit) } else { submit() }
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                    })
                })
                .collect();
            let mut overlaps = 0;
            let until = Instant::now() + Duration::from_millis(300);
            // The run without the gate goes on until its detector has seen an overlap (it must:
            // the threads submit nearly all the time), however long a busy machine takes to
            // let them run at all. On CI's Linux runner 300 ms could pass before any did.
            let patience = Instant::now() + Duration::from_secs(30);
            while Instant::now() < until || (!gated && overlaps == 0 && Instant::now() < patience) {
                // A frame: the gate is open for a moment.
                gate.open();
                std::thread::sleep(Duration::from_micros(200));
                // Closing waits for the submissions under way; on a machine too busy to let
                // them end in that time it says so, and that frame proves nothing either way.
                if !gate.close() {
                    continue;
                }
                // Between frames: the surface is reconfigured.
                for _ in 0..200 {
                    overlaps += submitting.load(Ordering::SeqCst);
                }
                frames.fetch_add(1, Ordering::Relaxed);
            }
            stop.store(true, Ordering::Relaxed);
            gate.open();
            for w in workers {
                let _ = w.join();
            }
            (overlaps, done.load(Ordering::Relaxed))
        }
        let (overlaps, done) = run(true);
        assert_eq!(overlaps, 0, "a submission ran while the gate was closed");
        assert!(done > 0, "work still gets through");
        // The control: without the gate the detector catches the race. On a runner with few
        // cores the workers and the detector may not overlap in one short run, so it gets a few
        // tries; the gated run above must hold in every run, this one in any.
        let seen = (0..8).any(|_| run(false).0 > 0);
        assert!(seen, "the detector sees the race without the gate");
    }

    /// A closed gate asks its owner for a frame, and the submission runs when that frame opens it.
    #[test]
    fn a_waiting_submission_asks_for_a_frame_and_runs_when_it_comes() {
        let asked = Arc::new(AtomicU64::new(0));
        let a = asked.clone();
        let gate = Arc::new(SubmitGate::new(move || {
            a.fetch_add(1, Ordering::SeqCst);
        }));
        let g = gate.clone();
        let worker = std::thread::spawn(move || g.run(|| 7));
        while asked.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        gate.open();
        assert_eq!(worker.join().ok(), Some(7));
        // It waited once and the frame let it through (not the timeout), unless this machine
        // stalled the test for the whole wait: then it is counted, and still ran.
        let (waits, timeouts) = gate.waited();
        assert_eq!(waits, 1);
        assert!(timeouts <= 1);
    }

    /// Nothing waits for ever: a window that draws no frames lets work through after
    /// [`SUBMIT_WAIT`]; the owner's own submissions never wait; and a submission that panics
    /// doesn't hold the gate.
    #[test]
    fn nothing_waits_for_ever() {
        let gate = Arc::new(SubmitGate::new(|| {}));
        // The owner (this thread), gate closed: at once, without waiting.
        assert_eq!(gate.run(|| 1), 1);
        assert_eq!(gate.waited(), (0, 0));
        // Another thread, and no frame comes.
        let g = gate.clone();
        let t = Instant::now();
        assert_eq!(std::thread::spawn(move || g.run(|| 2)).join().ok(), Some(2));
        assert!(t.elapsed() >= SUBMIT_WAIT);
        assert_eq!(gate.waited(), (1, 1));
        // A panic inside a submission: counted out, so closing doesn't wait for it.
        gate.open();
        let g = gate.clone();
        assert!(std::thread::spawn(move || g.run(|| panic!("the driver said no"))).join().is_err());
        assert!(gate.close(), "nothing is under way");
        // A submission that doesn't end doesn't hold the owner's frame up: closing gives up
        // after its wait and says the queue isn't left alone.
        gate.open();
        let g = gate.clone();
        let (inside, is_inside) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let slow = std::thread::spawn(move || {
            g.run(|| {
                let _ = inside.send(());
                let _ = released.recv();
            });
        });
        assert!(is_inside.recv().is_ok(), "the submission is under way");
        let t = Instant::now();
        assert!(!gate.close(), "it says the queue isn't left alone");
        assert!(t.elapsed() >= CLOSE_WAIT);
        let _ = release.send(());
        let _ = slow.join();
        assert!(gate.close(), "and it is, once the submission ends");
    }
}
