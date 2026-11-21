# ADR 0122: A moved raw selection merges when it is deselected

* Status: accepted (2026-11-20)
* Amends: ADR 0031 (merge drawing), Decision 5, first bullet
* Relates to: ADR 0041 (a marquee's provisional cut), ADR 0044 (selection across layers), issues
  #487 and #391

## Context

ADR 0031 said that the Selection tool merges a moved raw shape into the drawing when the drag
ends (`draw.drop`, in the same undo step as the move), and after each arrow-key nudge. That was
learned wrong. In Animate a raw fill or stroke that is moved onto other artwork cuts nothing
while it stays selected: the artist can let go of the mouse, look, move it on or back, nudge it
into place, and only when the selection lets go of it does it cut and join what it lies on
(issue #487, with a recording of both programs side by side).

The difference matters in daily work. With the merge on release, every drag that passes over
another fill and stops there for a moment damages that fill, and placing a shape in two moves
(drag near, then nudge) cuts twice. Animate artists rely on a selection being safe to move.

AniCraft already had the right mechanism for one case. Since #391 a paste in the editor leaves
its raw shapes *floating* (`Session::floating`): whole, on top, selected, and merged by
`Session::settle_floating` once none of them is selected any more. The merge is part of the
undo step around it, and undoing a command that deselected them brings them back floating.

## Decision

1. **A move floats, a deselect merges.** When a drag of the selection ends, and after an
   arrow-key nudge, the editor runs the new command `draw.float` instead of `draw.drop`. The
   selected raw shapes become floating, exactly as pasted ones are: they keep their own outline
   and paint, are drawn over what they lie on, and go into the drawing when the selection lets
   go of all of them (a click elsewhere, Escape, another tool's edit, a frame change).

2. **`draw.float` raises them.** The floating shapes move to the top of their keyframe's
   stacking order, where the merge will put them. Nothing changes on screen when they settle
   except the cut itself.

3. **Undo.** The move is one step, as before. The merge is not a step of its own: it belongs to
   the step around it, as for a paste. Undoing after the deselect takes back the move and the
   cut together; redo brings both back.

4. **`draw.drop` stays** for scripts, saved commands and assistants, with its meaning unchanged
   (merge the selection now). Headless callers that move raw shapes and want them merged at
   once keep calling it; nothing floats unless `draw.float` or `edit.paste {float: true}` is
   asked for. Edit Multiple Frames (ADR 0051) keeps merging on each keyframe when a move ends:
   floating is one frame's state.

5. **Shapes floating on one frame.** `Floating` holds one frame. A `draw.float` on the same
   frame adds to what floats there; on another frame it replaces it (what floated on the frame
   left behind was deselected by the frame change and has settled).

## Consequences

* A selected raw shape can be parked over other artwork and moved again without harm, as in
  Animate. The cost is that a document saved while a shape floats holds two overlapping raw
  shapes; they merge the next time one of them is moved and deselected, which is also what a
  floating paste does.
* The Selection tool's Alt-drag copy and the envelope (ADR 0080) are unchanged: the copy floats
  like any moved selection, and an envelope still merges when it closes.
* *Rejected:* merging on release but taking the cut back when the selection moves again (the
  provisional cut of ADR 0041). The artwork under the selection would show as cut while the
  shape sits on it, which is not what Animate shows, and every further move would re-run the
  merge both ways.
