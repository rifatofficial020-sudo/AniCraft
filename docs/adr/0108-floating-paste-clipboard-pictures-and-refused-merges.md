# ADR 0108: Floating paste, pictures from the clipboard, and merges that are refused

* Status: accepted (2026-11-17)
* Relates to: ADR 0031 (merge drawing), ADR 0042 (merging everywhere), ADR 0006 (platforms),
  ADR 0041 (the marquee's provisional cut)
* From the open-issue batch for drawing, selection, transform and the timeline (#391, #263,
  #308, #291, #292, #393).

## Context

Three reports needed a decision that is hard to take back, because scripts, saved commands and
the undo history depend on it.

1. **Paste merged at once** (#391). ADR 0042 made pasted raw shapes cut and join the drawing
   they land on in the paste itself. A copy pasted over its original therefore joined it before
   anyone could drag it away. In Animate pasted artwork stays apart, selected, until it is
   deselected.
2. **Paste only knew AniCraft's own clipboard** (#263). A picture copied in a paint program gave
   "the clipboard is empty", and Ctrl+V did nothing: egui-winit turns that key into a paste
   event only when the clipboard holds text, and sends nothing otherwise.
3. **A merge could drop the drawing** (#308). After snapping, two pieces that leave a crossing
   almost along each other can cross again with no vertex there. The planar map's faces are then
   not the drawing's regions; a fill's inside reads as outside and is dropped. In 400 seeded
   scenarios of seven crossing brush strokes, four lost most or all of the drawing.

## Decision

1. **Paste can float.** `edit.paste` and `edit.paste_in_place` take `float: true`: raw shapes go
   on top whole and selected, and the session remembers them (`Session::floating`: their layers,
   ids and frame). After any command, when none of them is selected any more, they merge where
   they lie (`Session::settle_floating`). A move merges them on drop as it always did.
   * The editor always pastes with `float`. **Without it the paste merges at once, as before**:
     scripts, saved commands and the tests of ADR 0042 keep their meaning, and a headless paste
     never leaves shapes waiting for a deselect that will not come.
   * **Undo.** The merge records no step of its own; it belongs to the step around it. Undo
     after a paste and a deselect goes back to before the paste; redo brings back the merged
     drawing. The undo snapshot carries the floating set, so undoing a command that deselected
     the shapes brings them back floating and selected.
   * *Considered:* a provisional state like the marquee's cut (ADR 0041), healed when nothing
     used it. A paste is a real edit, not a view of the drawing, so it must stay; only the
     moment of merging moves. *Considered:* merging in the shell on deselect. Then a paste over
     the control port or from a macro would behave differently from the person's own.
2. **Paste takes the system clipboard's picture when it is the newest thing copied.**
   AniCraft's Copy already leaves a text note on the system clipboard (so the next Ctrl+V comes
   through, #197). While that note is there, AniCraft's artwork is the newest; otherwise a
   picture on the clipboard is pasted as a bitmap in the middle of the view (through
   `file.drop`, so it is in the Library and one undo step).
   * Reading the clipboard is platform code (`platform::clipboard_image`, ADR 0006) and uses
     `arboard`, which eframe already uses for text. Desktop only: a web page cannot read the
     clipboard unasked.
   * **The swallowed key** is recognised by its release: a release of V with no press and no
     paste event before it, with Ctrl/⌘ held then or within half a second
     (`platform::SwallowedPaste`). This depends on egui-winit's behaviour and is covered by a
     test; if egui starts sending the key, the paste event path takes over and this never fires.
   * Tests and screenshot runs never read the computer's clipboard.
   * Not yet: vectors copied in other apps (#62). They arrive in app-specific formats.
3. **A merge whose map is not planar is refused.** `planar::merge` checks Euler's formula
   (vertices − edges + cycles = 2) on each connected part of its map and returns `None` when it
   fails. The engine then does what it does for a merge that is too large: the new shapes stay
   whole on top, as drawing objects, and the drawing is untouched. Nothing drawn is lost; the
   cost is that one stroke in a few hundred stays an object until it is broken apart or merged
   by a later edit.
   * *Considered:* repairing the map (cutting the snapped pieces again until nothing crosses).
     That is the right long-term fix and a larger change to the merge; the check stays useful
     after it, as the guard that the map is what the faces assume.
4. **Fill and stroke remember themselves for the session** (#292, #393).
   `element.set_fill` and `element.set_stroke` take `on: true | false`. Off remembers each
   shape's paint in the session (`PaintMemory`, bounded, not saved and not part of undo); on
   puts it back, or the tool's when there is nothing to remember. With only raw lines selected,
   a fill goes to what they surround (#291): the fill that was turned off there, else the area
   they close off, as the Paint Bucket fills it.

## Consequences

* A document can hold raw shapes that have not merged yet (after a floating paste, while they
  are selected, and if the file is saved in that moment). They draw the same; the next move or
  deselect merges them. Merges already tolerated such shapes (a moved selection during a drag).
* `edit.paste` returns `floating: true` when shapes are waiting.
* The `float` and `on` parameters are part of the command vocabulary plug-ins and saved commands
  see.
