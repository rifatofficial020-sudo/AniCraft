# ADR 0109: Shape commands, the picker's eyedropper and transformation origins

* Status: accepted (2026-11-17)
* Relates to: ADR 0031 (merge drawing), ADR 0018 (variable-width strokes), ADR 0046 (stroke
  caps, joins and dashes), ADR 0034 (the Eyedropper tool), ADR 0102 (fill alpha), ADR 0073
  (groups), ADR 0042 (raw drawings as one shape); issues #179, #288, #296

## Context

Three reports from artists coming from Animate asked for things its Modify menu, colour picker
and Free Transform tool do:

* **Modify › Shape** was missing **Convert Lines to Fills** (#179), and with it **Expand
  Fill** and **Soften Fill Edges**, which Animate keeps beside it.
* With a colour picker open, Animate's pointer is an eyedropper: a click on the stage takes
  the colour there. In AniCraft the click closed the picker and selected the artwork (#288).
* With several objects selected, Free Transform always treated them as one arrangement.
  Animators with hinged parts (clock hands, limbs) want each to turn on its own point (#296).

Behaviour was learned from Adobe's public help pages and from watching the program; nothing
was copied.

## Decision

### 1. A stroke's outline is built from its curves, in `anicraft_geom::outline`

The renderer strokes by filling many small polygons along the flattened line
(`anicraft_render::stroke`). Uniting those polygons would give the right area, as thousands of
straight pieces. A converted line is artwork the artist goes on to edit, so the outline is
built from the curves instead:

* Every segment is covered by **ribbons**, closed pieces between two cross-sections. A
  ribbon's long sides are the cubics with the offset curve's ends and end derivatives
  (Hermite); a ribbon is halved until both sides are within 0.0025 units of the true offset
  and the segment turns less than 45° across it. A line of one width is one exact quad.
* Corners get the join and open ends the cap **by the renderer's rules** (round, miter with
  its limit, bevel; butt, round, square; dashes by arc length restarting per subpath, a
  zero-length dash as a dot; a width profile's left and right widths by the fraction of the
  subpath's length in the shape's own space), so the outline covers what was drawn.
* The pieces overlap, and one may fold over itself where the line is wider than the curve is
  tight. Each is filled on its own and their **union** is taken by the planar map of merge
  drawing (ADR 0031), which keeps exact sub-curves. Pieces that don't touch are united apart
  and long runs in halves, so the map's 20,000-segment limit bounds how tangled a line is, not
  how long. The union runs scaled up (16 to 256 times) so the map's 0.2-unit snap is far below
  a pixel and a hairline keeps its shape.

A straight line with flat ends becomes four points; with round ends, two lines and four
quarter circles; a circle's line, two rings of a few curves each.

*Rejected:* uniting the renderer's polygons and fitting curves to the result (corners and
straight edges come back approximate, and the planar map is asked to unite thousands of
discs); a signed distance field traced by marching squares, as the Brush does (ADR 0033: right
for a soft, freehand sweep, wrong for a mitered corner or a flat cap that must stay exact).

### 2. Expand and Inset are the boundary's band added or taken away

The band within `d` of a fill's boundary is the boundary stroked `2d` wide with round joins
(the same machinery). The fill united with the band is the fill **expanded**; the fill with
the band subtracted is the fill **inset**. Both are exact offsets: expanding rounds convex
corners (a disc rolled round the shape), insetting keeps them, holes shrink as the fill grows,
and a fill thinner than `2d` insets to nothing. A fill is first made into loops that don't
overlap, so the edges inside a self-crossing outline are not offset.

**Soften Fill Edges** offsets the fill `steps + 1` times and subtracts neighbours, giving the
core and one band per step; band `i` of `n` (1 next to the core) gets `(n + 1 - i) / (n + 1)`
of the fill's alpha, on a solid colour or on every stop of a gradient. Expand puts the bands
round the fill; Inset takes them from it, so the last band ends exactly on the old outline.
A bitmap fill has no alpha to fade and is refused with a message. Animate's exact alphas are
not documented; this ramp is linear and never reaches zero, which is what its result looks
like.

### 3. The commands work where the artist looks, and store where the shape lives

`modify.lines_to_fills`, `modify.expand_fill {distance, direction}` and
`modify.soften_fill_edges {distance, steps, direction}` act on the selection on every layer.

* Geometry is worked out in the **keyframe's space** (a line's width and an offset are stage
  distances, however the shape is scaled, skewed or mirrored) and stored back through the
  inverse of the element's transform. The element keeps its transform, so tweens and a
  gradient's placing are untouched.
* **Raw shapes stay raw** and are merged with the drawing again (ADR 0031): a converted line's
  fill joins a touching fill of its colour and cuts others; softened bands, each a different
  alpha and so a different style, stay apart.
* **Drawing objects stay objects.** A line-only object becomes a fill in place (same element,
  same id). An object with a fill and a line keeps its fill and gets the outline as a second
  object above it. Expanding an object moves its one outline, so its line follows; in merged
  drawing the lines are elements of their own and stay.
* Limits: distance up to 1,000, 1 to 50 steps, 2,000 outlines per command, 60,000 pieces and
  20,000 dashes per outline. Past them the command declines with a message; nothing is half
  done. Shapes with an asset warp are skipped.
* One undo step each, undoable, so no confirmation (ADR 0004). The `modify` namespace already
  has a plug-in policy; plug-ins and saved commands may call them.

### 4. The picker's eyedropper samples what is drawn

`paint.sample {x, y, scale}` renders one pixel of the current display list with the CPU
renderer, over the stage colour, and returns it. It is the colour as drawn: gradients at the
point, a bitmap's pixel, lines, symbols with colour effects, blends and filters, on any layer.
(`paint.pick`, the Eyedropper tool, picks a *style* from one shape; ADR 0034 listed "picking a
bitmap's pixel colour" as not yet.)

In the app, while a colour picker **popup** is open, the stage takes no tool input: the
pointer is a crosshair with a chip showing the sampled colour, and a click hands the colour to
the picker, which takes it (keeping its alpha), adds it to the recent colours and closes, as
Animate's does. Esc closes the picker without picking. The picker is a plain widget with no
access to the document, so the two meet through the frame's shared data by frame number: the
picker says it is showing, the stage says the pointer is over it (the popup then ignores that
click instead of closing on it) and hands back the colour. Every swatch in the app uses the
one picker, so every swatch has the eyedropper.

*Not taken:* sampling outside the AniCraft window (the issue's "superbonus"): there is no
portable, permission-free screen capture, and Animate removed it too.

### 5. Transformation origins are tool state, and objects are what the artist sees as one

`TransformOrigin` (`Selection`, `Registration`, `Center`) lives in `ToolStyle`, like Object
Drawing: set or cycled by `modify.transform_origin`, kept from one selection to the next, not
part of the document. A new session starts with `Selection` (Animate's behaviour, and what
scripts already get).

* `modify.transform {a..f, origin?}` keeps its meaning without `origin`. With `registration`
  or `center` the matrix's linear part (scale, rotation, skew) is applied to each object about
  its own point and its translation is ignored; Free Transform sends the mode with every
  change but a move. Rotate 90° and Flip follow the mode, or their own `origin` param.
* An **object** is one element, or the selected raw pieces of one drawing that touch (a fill
  and the lines on its edge: the units Align already uses, #301), so a drawing keeps its
  outline.
* A symbol instance's or a group's **own point** is its registration point. Shapes, drawings,
  bitmaps and text have no point of their own to set, so they use their centre in both modes.
  The **centre** is the centre of the object's box in its own space, carried by its
  transform. That point stays put under a transform about itself, so a drag sent as many small
  changes ends exactly where one transform would (tested with thirty steps).
* The stage marks each object's point while Free Transform is in hand, and Properties shows
  **Transform about** in the tool's options. The cycling command is in Modify › Transform and
  can be given a key; it has none by default, as the common keys are taken.

*Rejected:* a draggable transformation point stored per element (Animate has one). It is a
document-model change touching every exporter's notion of an instance, and the issue asks for
modes, not a new point. The enum leaves room for it.

## Found on the way

Checking the outline against the stroker's own polygons at high zoom showed two things in
`anicraft_render::stroke`:

* **Fixed here:** from about 2000 % zoom, a round join on a gentle bend of a wide dashed or
  profiled line could sweep the long way round, a whole disc instead of a sliver. The side to
  sweep was chosen from the difference of two nearly equal unit vectors, which is noise at
  that scale. It is now chosen that way only where the path nearly turns back.
* **Not changed:** the renderer adds its stroke polygons' coverage pixel by pixel, so a pixel
  partly covered by two overlapping polygons (at every flattening joint of a wide curve) is
  drawn darker than the stroke's true coverage, by up to half a pixel's worth. A converted
  line is drawn as one fill without that, so its edge is very slightly lighter than the line
  was. The tests therefore compare with the union of the stroker's polygons, not with its
  pixels.

## Consequences

* Measured (the engine's tests): against the union of the stroker's polygons at 2400 %, the
  fills cover the same area as the lines to within a pixel (0.00 to 0.98 px) for round, flat
  and square caps, miters, bevels, dashes, dots, width profiles, dashed profiles, tight
  curves, closed outlines and a mirrored, skewed shape, on 200 to 10,000 px of line. Rectangle
  and circle offsets match their closed forms to 0.5 px² and 0.15 px of radius.
* The command fuzz runs 16 s longer (168 to 184 s) for the new cases.
* `Shape`, the file format and the exporters are unchanged: the results are ordinary shapes.
* Not yet: a draggable transformation point; origins with Edit Multiple Frames and in the
  Properties Position/Scale/Rotation fields; Convert Lines to Fills on text outlines and
  inside symbols without opening them; a live preview in the two dialogs; miter or bevel
  corners for Expand Fill; sampling through Preview Through Camera and the artwork around an
  open symbol; remembering the transformation origin between sessions.
