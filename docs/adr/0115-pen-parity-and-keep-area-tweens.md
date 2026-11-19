# ADR 0115: Pen parity and tweens that keep area

* Status: accepted (2026-11-18)
* Relates to: ADR 0038 (motion path points), ADR 0042, 0045 (Subselection and raw points),
  ADR 0016 (motion tweens), ADR 0081 (eases per property group), ADR 0083 (Motion Editor),
  ADR 0089 (Lottie keyframes), user issues #319 and #408, roadmap M1

## Context

Two requests from animators coming from Animate.

**The Pen (#319).** AniCraft's Pen placed corners, pulled mirrored handles and closed a path
on its first point. It lacked what Animate's Pen does on the point just drawn and on paths
already there: clicking the last point to break its handles (so the next segment leaves it as
a sharp corner), Alt-dragging it to draw its curve again, and, on a drawn path, the pointer
that says "click here to make a corner / delete this point / add a point". Behaviour was
learned from the issue's description and Adobe's public help pages for the Pen; nothing was
copied.

**Keep area (#408).** A tween moves scale X and scale Y in straight lines. From 100 × 100 % to
200 × 50 % the halfway frame is 150 × 75 %: 12.5 % more area than either key, so a squash
bulges. Animators hide it with extra keys.

## Decision

### The Pen

1. **On the last point** of the path being drawn:
   * a **click** takes its handle ahead away, keeping the one behind: the curve arriving
     stays, the next segment starts as a corner. On a point with no handle ahead the click
     **finishes** the open path, so a double-click still finishes, as before;
   * a **drag** from it pulls a new handle ahead alone;
   * **Alt-drag** draws both of its handles again. The segment as it was stays in view,
     faint, until the button is let go.
2. **Shift** keeps a new point on a multiple of 45° from the point before, and a handle on a
   multiple of 45° from its point. It wins over snapping, as it does for the Line tool.
3. **The segment a click would add** is previewed as it will curve, from the last point's
   handle, not as a straight dashed line.
4. **On a path whose points show** (a selected drawing object or selected merged shapes),
   with no path being drawn:
   * a click on a **smooth point** makes it a corner; a click on a **corner** deletes it (so
     two clicks delete a smooth point, as in Animate);
   * a click on the **outline** between points adds a point (Alt-click still does, and is the
     only way on a motion path);
   * a click on an **end of an open drawing object** picks the path up: the Pen draws on from
     there, and finishing extends that object instead of adding another. Clicking its other
     end closes it.
   **Ctrl-click (⌘)** selects the path under the pointer, so its points show.
5. **Only selected paths are edited.** A click on an unselected line starts a new path there,
   as it always did. In merge drawing, starting a line on another line (a T-junction) is the
   common case, and Animate behaves the same way: its Pen edits the paths whose points are
   showing. A path just drawn is left unselected for the same reason.
6. **Pointer marks.** egui has no pen cursors, so the stage draws a mark by the crosshair: a
   plus (add), a minus (delete), a caret (make a corner), a ring (close), a slash (continue)
   and a square (finish). They are drawn in the selection colour over white, from the theme.
7. **Commands.** The shell holds only the path under construction (view state). Every
   document change is an existing command (`draw.path`, `path.add_point`,
   `path.convert_point`, `path.delete_points`, `raw.add_point`, `raw.convert_points`,
   `raw.delete_points`) or the new **`path.extend {id, d, end?: "end" | "start", close?}`**,
   which continues an open path (one subpath, not closed) with a piece given in stage space.
   The piece's own start is not kept: it joins the path's end exactly. From the start, the
   piece is turned round and put in front, so the path keeps its direction (stroke profiles
   and dashes run the same way as before). A refused piece leaves the path as it was.

The interaction is split into `pen_press`, `pen_drag`, `pen_release` and `pen_hover`, which
take positions and modifier keys and are tested without an egui event loop.

### Keep area

1. **A flag per tween**: `Tween.keep_area` and `MotionTween.keep_area`, `#[serde(default)]`
   and not written when false. Files without it are byte-for-byte as before, and older files
   open. One command sets it on whatever tween the span has:
   **`timeline.set_keep_area {keep_area, layer?, frame?}`**.
2. **Scale moves geometrically.** With the flag on, each scale axis goes from its key to the
   next as `a · (b / a)^t` (`geom::lerp_geometric`) in place of `a + (b − a) · t`, where `t`
   is the eased progress of the scale group (ADR 0081) or of the property key's segment. The
   product of two axes moved this way is the geometric interpolation of the keys' products,
   so **keys of one area keep that area on every frame**, exactly up to rounding (tested to
   2 · 10⁻⁵), and keys of different areas change it by an even ratio. Rotation, skew and
   position are untouched (the area factor of the matrix is `sx · sy`).
   * An ease that overshoots (Back, Elastic) extrapolates in the same ratio and never
     crosses zero.
   * An axis whose keys differ in sign (a flip through zero), or with a zero key, can't go
     geometrically and tweens linearly as before. A flipped axis that stays flipped keeps its
     sign.
3. **On a motion tween, an axis without keys follows.** A motion tween keys each property on
   its own, so an animator who stretches only the width has said nothing about the height.
   With Keep area, a scale axis that isn't animated (no track, or fewer than two keys) is
   derived from the animated one: `follower · driver(0) / driver(f)`, which holds the area of
   the span's first frame. When both axes have keys, both curves are the animator's and
   rule 2 applies. `MotionTween::scale_at` is the one place this is computed; `props_at`,
   `apply`, `split_at` and the Motion Editor use it.
4. **A drag of one axis doesn't key the follower.** Transform tools set both scale factors
   on every step of a drag, putting the untouched axis back where the gesture found it. To
   the key recorder that looked like an edit of the follower (it shows a derived value), and
   the first step of a drag would have ended the following. So, while an axis follows, an
   edit that leaves it at its own resting value is not keyed. Setting it to anything else
   keys it, and from then on it is the animator's.
5. **`evaluate` stays pure**, and off is bit-exact: the flag adds a branch after the existing
   interpolation and nothing changes when it is false (tested bit for bit against the lerp
   of before, for classic and motion tweens).
6. **The Motion Editor draws what plays**: a scale curve of a tween that keeps area is
   sampled through `shown_value`, so it shows the geometric curve between keys, or the
   derived curve of a following axis with a note saying which axis it follows. Properties and
   the Motion Editor's footer have the **Keep area** checkbox.
7. **Exports bake it.** Every exporter samples `evaluate` per frame. The Lottie exporter
   writes eased keys only where they reproduce the sampled frames (ADR 0089); no cubic ease
   draws a geometric curve, so it falls back to one hold key per frame by its own check
   (tested).

## Alternatives considered

* **Linear on one axis and the inverse on the other** (`sy = area / sx`) for classic tweens.
  It ignores the end key's second axis and jumps at the end key when the keys differ in area.
  The geometric form is symmetric, reaches both keys, and is what #408 asks for.
* **Editing any path under the Pen, selected or not.** Simpler to discover, but it would
  break starting a path on existing artwork, the usual way to draw in merge mode.
* **A custom OS cursor** for the Pen's states. egui has none; a mark beside the crosshair
  says the same and needs no platform code.
* **A new document field for "derived" scale keys.** Not needed: the derived curve is a
  function of the keys already there.

## Consequences

* Two more fields in the file format, both absent when off.
* `Progress` (the classic tween's eased progress per group) carries `keep_area`.
* The Pen's click on a smooth last point now makes it a corner instead of finishing; a second
  click, a double-click on a corner, Enter or Esc finish. The tool's hint says so.
* Retiming a motion tween that keeps area (`motion/retime.rs`) inserts keys at the straight
  line's value, not the geometric one. Splitting a span is exact.

## Not in this slice

* Continuing a merged (raw) line as the same stroke: a path started on its end merges with it
  as any touching stroke does.
* Keep area per property key or per segment; a "volume" control that exaggerates the squash.
* Keep area when importing Animate files (XFL has no such option).
* Pen cursors as real OS cursors; converting a corner back to smooth by dragging from it with
  the Pen (Subselection's Make Smooth does it).
