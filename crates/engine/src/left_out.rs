//! Saying when artwork was left out of a frame (ADR 0128).
//!
//! A frame's evaluation has an allowance of work (`anicraft_doc::MAX_EVAL_ELEMENTS`), so that no
//! document, however its symbols nest, hangs AniCraft or runs it out of memory. A frame cut
//! short draws in part. A command that writes such a frame still writes it, and says so in its
//! result, so a partial picture is never passed off as the whole one.

use anicraft_doc::{DisplayList, Document};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// What a result says when artwork was left out.
pub const WARNING: &str = "the document holds more artwork than AniCraft draws in one frame, so some of it was left out (symbols nested many times over?)";

/// Whether any frame one command evaluated was cut short. Shared by the threads that render a
/// sequence's frames.
#[derive(Default)]
pub struct LeftOut(AtomicBool);

impl LeftOut {
    /// Movie frame `frame`, as `anicraft_doc::evaluate` gives it, noted when it was cut short.
    pub fn evaluate(&self, doc: &Document, frame: u32) -> DisplayList {
        let dl = anicraft_doc::evaluate(doc, frame);
        self.note(&dl);
        dl
    }

    /// Notes `dl` when it was cut short.
    pub fn note(&self, dl: &DisplayList) {
        if dl.left_out {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    pub fn any(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// `result` as it is when every frame was whole; with `left_out: true` and a `warning`
    /// when one was not.
    pub fn noted(&self, mut result: Value) -> Value {
        if self.any()
            && let Some(o) = result.as_object_mut()
        {
            o.insert("left_out".into(), json!(true));
            o.insert("warning".into(), json!(WARNING));
        }
        result
    }
}
