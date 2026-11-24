# ADR 0129: A long Brush stroke never holds the window

* Status: accepted (2026-11-21)
* Relates to: ADR 0033 (the Brush), ADR 0031 (merge drawing), ADR 0122 (a moved selection
  merges when it is deselected), ADR 0116 (work prepared on another thread, then one command),
  issue #519, backlog item P-003

## Context

Issue #519: a Brush stroke drawn for 40 to 50 seconds without lifting, then moved and
deselected, left the window "Not Responding", again and again, and the saved file did the same
when it was opened. The reporter's file holds one raw fill of 11 370 curves in 1 107 loops.

Measured on that file and on generated strokes (`anicraft-cli bench --stroke <samples>` and
`--stroke-file <file>`; a 32-thread desktop, so a laptop is several times slower):

| What | Before | Why |
|---|---|---|
| `draw.brush`, 6 000 / 20 000 / 60 000 samples of fast scribble | 306 / 807 / 1 563 ms | The distance field (ADR 0033) is sampled piece by piece, each piece over its own bounds. A stroke that goes over itself visits the same cells again and again: 190 million cell visits for 60 000 samples. The cost grew with the stroke's length without limit. |
| A frame with part of the stroke selected over the rest (after a marquee) | 3.2 s **a frame** (debug build) | The stipple of a selected raw fill is cut by the artwork above it. Every span of the selection was compared with every span of every occluder: tens of thousands by tens of thousands. |
| The merge when a moved stroke is deselected | 75 to 195 ms | `planar::merge`, already bounded (`MAX_SEGMENTS`, `MAX_PAIRS`). |
| Drawing the stage with the stroke on it | 3 to 5 ms CPU, 4 to 20 ms GPU | Not a problem. |

So there were two unbounded costs, one in the engine (the trace) and one in the shell (the
selection overlay, paid on every frame, which is the "loop" the reporter saw), and one bounded
cost that is still longer than a frame.

## Decision

1. **The trace is bounded.** `geom::brush` counts the cells a stroke's pieces would visit and,
   past `MAX_STAMP_WORK` (2²⁴), samples on a coarser grid, as it already does past `MAX_CELLS`.
   A cell that is already deep inside paint is skipped with one comparison, since nothing can
   raise it. The result for a stroke under the budget is unchanged. Past it the grid is at
   worst the size the brush's own tip asks for at full pressure (a sixth of its radius), so
   thin parts of a very long stroke are a little less fine; the outline is still traced from a
   true distance, well under a cell.

2. **The editor traces a long stroke on another thread.** The trace depends on nothing but the
   stroke, so it is a pure function of a `StrokeRequest`. When the pointer lifts on a stroke of
   400 samples or more, the shell starts the trace on a thread (`src/ink.rs`) and keeps drawing
   the stroke as it was drawn. When the outline is ready, the shell runs the same `draw.brush`
   as ever with the outline staged (`Session::staged_stroke`): the command takes a staged
   outline only when it is for exactly the stroke it was asked to paint, and traces otherwise.
   This is the pattern of ADR 0116 (`staged_video`). The command, its parameters, its undo step
   and what a script or an assistant sees are unchanged; headless callers trace in the command.

3. **Nothing comes between a stroke and its paint.** Strokes are painted in the order they
   were drawn. While strokes wait, only looking around and painting more go ahead: a frame
   whose input could edit (any key, text, a press that is not the Brush on the stage, a dropped
   file, playback, a dialog) first finishes the waiting strokes on the window's thread, as does
   any command (`App::run`, so an assistant's commands too) and leaving the document. The wait
   is bounded by decision 1. At most eight strokes wait; a ninth waits for the first.

4. **The merge stays on the window's thread**, bounded as before. A `Session` is not `Send`
   and cannot be cloned (plug-ins, caches, the undo history), and a merge needs the drawing it
   merges into, so moving it would mean running the command on a copy and adopting the result,
   which is a second way for an edit to enter the document. It is not worth that for 75 to
   220 ms on the largest strokes the app accepts. If it turns out to be, the copy-and-adopt
   route is the one to take, for every slow command at once, in its own ADR.

5. **The selection overlay looks up occluders by row.** `subtract_spans` sorts the occluders by
   their top edge and compares a span only with those in its own rows, in their original order,
   so the pieces it leaves are the ones it always left.

## Consequences

* `draw.brush` on the three generated strokes: 218 / 461 / 383 ms, of which the trace is at
  most about 300 ms whatever the stroke, and in the editor none of the trace is on the
  window's thread. What is left there is the merge.
* The marquee case is 200 ms once (the cut) instead of seconds on every frame.
* A stroke can be on the stage for a moment before it is in the document. Undo, save and every
  other edit see it, because they wait for it; an update is not installed while one waits.
* The web build has one thread: it traces in the command, bounded by decision 1.
* *Not done here:* the Subselection tool's click on a shape of 13 000 points (3 s once in a
  debug build), the Paint Bucket in a gap of such a shape (0.8 s), and the shape tools' snapping
  near it (about 100 ms a frame in a debug build). They are bounded and one-off or mild, and
  are listed under P-003 in the roadmap.
* *Rejected:* a progress window ("processing strokes", as the reference app shows). With the
  trace bounded and off the thread there is nothing long enough to report.
* *Rejected:* simplifying the outline to fewer curves. It changes what the artist drew, and
  the curve count was not what cost the time.
