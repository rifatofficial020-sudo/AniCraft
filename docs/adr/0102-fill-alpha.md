# ADR 0102: Fill alpha

* Status: accepted (2026-11-17; an outside contribution by Koolskull, pull request #354, merged with the M8 batch)
* Relates to: ADR 0074 (the fill picker and gradients), ADR 0078 (swatches)

## Context

In Flash, the colour you picked for a fill was not only a hue. The mixer had an alpha, and a
light wash was how people built glass, smoke, shadows and fades without a second shape. A
gradient did the same per stop: one end of a ramp could be the colour and the other end
nothing, or two stops could be the same colour at two different strengths. That is a different
thing from no fill. No fill means the shape has no interior. A clear fill is still there. You
can select it, erase it, and use it as an alpha mask.

The document already stores that alpha on a solid colour and on every gradient stop, and the
renderer already paints it. The solid fill picker threw it away and hid the Alpha control. A
gradient stop kept its alpha in the file, and the picker could edit it, but the stop's marker
was always drawn opaque, so a fade did not show on the handles.

## Decision

1. **A solid fill and a stroke carry alpha.** The colour picker (Tools squares, the Colour
   panel, Properties) shows a labeled Alpha row: a slider and a percent, 0% clear, 100%
   opaque. Hex may be `#rrggbb` or `#rrggbbaa`. Choosing a swatch keeps the alpha you had.
   None is still no fill, not a clear one.
2. **Each gradient stop keeps its own alpha.** The ramp blends them, including through
   Linear RGB (the alpha is not gamma-corrected). The stop's marker is drawn at that alpha,
   and its name says the percent.
3. **A brush stroke that is not fully opaque stays its own shape, and you see that alpha
   while the pen is down.** Merge drawing would cut away the colour underneath, so a wash
   could not show what is behind it, and two washes of one colour would join instead of
   darkening where they cross. Opaque ink still joins. Modify › Group bundles the separate
   strokes. The ink drawn before the pen lifts is already that alpha, as one coverage, so
   lifting does not change it. Pressure that drives opacity uses the stroke's average, the
   same average the finished stroke keeps.

## Consequences

* An older file with no alpha byte stays opaque.
* A saved solid that omitted alpha still reads as opaque.
* Alpha 0 is a fill the hit test and the eraser can still find. Removing the fill is None.
* A wash does not join the raw drawing. Group still bundles those strokes. An opaque stroke
  joins as before.

## Notes from review (the batch that merged this, M8)

* **Where this differs from Animate.** In Animate a see-through brush stroke with Object
  Drawing off still merges: it replaces the paint under it. Here it stays its own shape
  (Decision 3), because a wash that cuts away what it covers can't do what a wash is for. The
  cost is that such strokes don't join into one shape; Modify › Group bundles them.
* **Only the brushes do this.** A rectangle, oval or Paint Bucket fill with a see-through
  colour and Object Drawing off still merges as before (it cuts what it covers, then shows the
  stage through). Not yet: the same rule for the other drawing tools, if artists ask for it.
* **Nothing in the file changes.** `Rgba` always had alpha, `#rrggbbaa` was already read by the
  commands, and the renderers already draw it (CPU oracle and GPU stage, SVG, HTML canvas and
  Lottie exports). Older files open unchanged; no version bump.
* **Live preview.** The pen-down wash is one coverage image (at most 2048 px on its longer
  side, reused as one texture), so its cost is bounded for any stroke.
