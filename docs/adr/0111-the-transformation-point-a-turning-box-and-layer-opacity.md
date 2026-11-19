# ADR 0111: The transformation point, a turning box and layer opacity

* Status: accepted (2026-11-17)
* Relates to: ADR 0109 (transformation origins, which left the draggable point out), ADR 0073
  (groups), ADR 0029 (fade groups on the GPU stage), ADR 0028 (HTML canvas export), ADR 0061
  (Layer Properties), ADR 0004 (destructive actions); issues #302, #280, #178, #183, #175, #279

## Context

Six reports from artists coming from Animate:

* The **transformation point** (they call it the anchor or pivot point) could not be moved:
  not with the Selection tool, not with Free Transform, not in Properties (#302, #280, #178,
  #183). In Animate it is the white circle Free Transform shows; you drag it, even outside the
  box, and the object then turns about it. A limb hinged at the shoulder needs it.
* A rotated symbol showed an upright box, so you could not see that it was rotated, and its
  handles stretched it along the stage's axes, which skews it (#175).
* There was no **layer opacity** (#279).

ADR 0109 rejected a stored point because that slice only needed modes for several objects.
These reports need the point itself.

Behaviour was learned from Adobe's public help pages on transforming objects and on layers,
from the public tutorial video linked in #280, and from watching the program; nothing was
copied.

## Decision

### 1. A symbol instance keeps its transformation point; everything else has one while the tool is held

`ElementKind::Symbol` gains `pivot: Option<Vec2>`, in the instance's own space (so it travels
with the instance, as Animate's does), read through `Element::pivot()`. It is saved only when
set; older files open with none. Points that are not finite, or more than 10⁶ units out, count
as none.

* `None` means "never moved". Tools then use the **centre of the instance's box**, and tweens
  the **registration point**, exactly as before this ADR, so no existing file changes.
* `modify.transformation_point {x, y}` (stage space) sets it on the selected instances;
  `modify.reset_transformation_point` (Modify › Transform) clears it. Both are ordinary
  undoable commands, and a drag of the circle is one gesture, so one undo step.
* A raw shape, a drawing object, a group, text, a bitmap or several elements have **one point
  for the selection while Free Transform is the tool** (shell state in `src/xform.rs`, like a
  marquee). It goes with the selection when a Free Transform change or a move carries the
  artwork, and is dropped when the tool or the selection changes. Raw pieces are cut and
  joined by every merge, so there is no lasting element to keep a point on; groups and the
  rest could keep one later (the accessor leaves room), and Animate itself forgets a group's
  point when it is deselected.

*Rejected:* a field on `Element` for every kind. It is the more general model, but only an
instance needs a lasting point today, the field would touch over a hundred element literals
across parallel tracks, and `Element::pivot()` hides where it lives, so widening it later is
not a file-format break (a new optional key).

### 2. What turns about the point

One rule, everywhere: **a new scale, rotation or skew leaves the point where it is.**

* **Free Transform**: a rotation turns about the point; Alt scales and skews about it
  (without Alt the opposite handle or edge stays put, as before and as in Animate).
* **Modify › Transform** (Rotate 90°, Flip) in the default origin mode turn one selected
  instance about its own point.
* **Properties** (`element.set_transform`): a new Scale or Rotation is applied about the
  point; a Position still places the registration point. Properties shows the point as
  **Pivot** X and Y with **Reset**, which is also the keyboard and screen-reader way to set it.
* **Classic tweens** (`doc::evaluate`, pure): when either keyframe's instance has a point, the
  point travels in a straight line from where the first keyframe has it to where the next
  does (by the position ease), and the instance's scale, rotation and skew are interpolated
  as before and placed round it. A keyframe without a point takes the other's. So an arm
  keyed at two angles swings on its shoulder instead of sliding between two placements.
* **Motion tweens**: the position tracks carry the point, and the rotation, scale and skew
  tracks turn the target about it.

Every renderer and export draws evaluated frames, so the swing is the same in the PNG, GIF,
video, SVG, Lottie and HTML exports and on both stages with no change to them.

ADR 0109's modes are unchanged. With *each object about its own point* the circle shows each
object's point and is not draggable.

### 3. One element shows its own box

`Session::transform_frame()` answers a box and the transform that places it. For one selected
element it is the box of what the element draws at the playhead, in the element's own space,
under the element's transform: turned, scaled and skewed as the element is. For several
elements, raw pieces or Edit Multiple Frames it is the upright bounds under the identity, as
before.

Free Transform draws that box and works in it (`xform::Frame`): handles are hit where the box
is, a handle's scale and an edge's skew are computed in the box's own space and conjugated back
(`M · L · M⁻¹`), so a turned instance stretches along its own axes and stays unskewed; a
rotation is computed on the stage (a turn is a turn however the box is scaled); the resize
arrows follow the box's direction; snapping uses the handles' real positions. The Selection
tool's plain box stays upright, as Animate's does.

The dragged point snaps to the box's eight handles and its centre within 8 screen pixels (Alt:
freely) and may leave the box (#178). A double-click on it puts it back at the centre.

The point is grabbed before the box, as in Animate: a press on the white circle moves the
point, not the object. Dragging an object by its exact centre therefore moves the point; any
other place inside the box moves the object.

### 4. Layer opacity is in the movie, and a layer fades as one

`Layer::opacity` (`0..=1`, saved only below 1). In `doc::evaluate` a layer below full opacity
puts its artwork in a **fade group** (`DisplayList::groups`, the mechanism a translucent
instance already uses): the layer is composited together and then shown at its opacity, so
overlapping parts do not show through each other. A folder's opacity multiplies into the
layers in it. Mask artwork ignores opacity (as it ignores an instance's) and guide artwork is
never in the movie. Because it is a group in the display list, the CPU oracle, the GPU stage,
SVG and Lottie needed no change; the HTML canvas export writes each layer's opacity and its
player opens the same group (`player.js`), checked item for item against the Rust evaluator.

`layer.set_opacity {opacity: 0..100}` and `opacity` on `layer.set_properties` /
`layer.properties`; Layer Properties has a slider and a field; the layer's right-click menu
has **Opacity ›** 100, 75, 50, 25, 0 %; a faded layer shows its percentage beside its name
and says it to screen readers.

**Not Animate's meaning, on purpose.** Animate's Layer Properties offers *Visibility ›
Transparent*, an authoring aid that is not published. The report asks for layer opacity as
other animation and paint programs have it, and an opacity that silently disappears in the
export would be a trap, so AniCraft's is part of the movie. *Rejected:* multiplying the
opacity into each element's alpha (cheaper, but overlapping artwork on the layer shows
through itself, which is not what "the layer at 50 %" means), and an authoring-only
transparency (a second, similar-looking setting can come later if artists ask for a tracing
aid).

## Consequences

* Measured (the sample scene, 1280 × 720, frame 7; RTX 5090 for the GPU): seven faded layers
  cost the CPU oracle 2.0 ms a frame (2.9 → 4.9 ms); the GPU stage is unchanged (1.8 and
  1.4 ms, within noise). GPU against the oracle on faded layers and a faded folder, four
  frames at four scales: at most 6 of 2,073,600 pixels differ, by 1/255.
* Analytic tests: a tween about a moved point keeps the point within 0.001 px on every frame
  and turns 9° a frame over a 90° span; Properties' rotation and scale keep it fixed; a turned
  box's side handle changes one scale only (no skew, rotation kept); layer opacity pixels
  match `1 − a` over white exactly, and the overlap of two shapes on a faded layer shows only
  the top one.
* Files: two optional keys (`pivot` on an instance, `opacity` on a layer). Older files open
  unchanged; older versions ignore both.
* A press on the exact centre of Free Transform's box now moves the point (see 3).
* Not yet: the point of an Animate file's instances (XFL `transformationPoint`: importing it
  changes how imported classic tweens move, so it needs the corpus goldens checked first);
  a kept point for groups, text and drawing objects; the point with Edit Multiple Frames and
  inside ADR 0109's each-object modes; motion guides carrying the point (they carry the
  registration point); Alt-rotate about the opposite corner; a turned box for the Selection
  tool; opacity keyed over time; an authoring-only layer transparency; layer opacity from
  Animate files.

## Questions for the lead

* Layer opacity is published (4). If the project would rather match Animate's
  authoring-only transparency, the model stays and only `evaluate`'s movie path changes.
