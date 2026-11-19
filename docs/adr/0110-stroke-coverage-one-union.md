# ADR 0110: A stroke's coverage is that of one outline

* Status: accepted (2026-11-17)
* Relates to: ADR 0002 (render pipeline), ADR 0018 (variable-width strokes), ADR 0019 and 0029
  (the GPU stage draws the oracle's fills), ADR 0046 (caps, joins and dashes), ADR 0109
  (Convert Lines to Fills, where this was found)

## Context

The CPU oracle works out a pixel's coverage by adding up the signed area every edge sweeps
across it and folding the sum by the fill rule (`anicraft_render::raster`). That is exact when
the loops it is given don't overlap. It is not exact where two loops each cover part of one
pixel: both parts are added, as if they lay side by side.

The stroker gave it exactly that. A stroke was a quad per segment, a disc or a fan at every
bend and a half disc at every end, all wound the same way so that "non-zero fills their
union". Inside the stroke that holds (2 folds to 1). Along its edge it does not:

* at a round join or cap, the disc's rim and the quad's edge run together for a few pixels,
  and both cover the same part of each: up to about half a pixel too dark;
* at every flattening joint of a wide curve, the two quads overlap in a thin wedge on the
  inside of the bend;
* where dashes' round caps run into one another, and where a line crosses itself, the
  corners of the overlap.

A line converted to a fill (ADR 0109) is drawn without any of this, so it looked slightly
lighter at its edges than the line it came from, and corners of outlined rectangles showed as
dark knots at small sizes. The GPU stage draws the oracle's prepared polygons with the same
arithmetic (ADR 0019), so it had the same error.

## Decision

### 1. The stroker hands over loops that don't overlap

`anicraft_render::stroke` still means the same stroke: a trapezoid per segment, the join on
the outside of each bend, the cap at each open end, dashes cut by arc length. It now builds
it as an outline, in two steps.

**Each run of segments (a subpath, or one dash) is its own outline.** The left edges
forwards, the end's cap, the right edges backwards, the start's cap; a closed run is two
loops, the left edges one way and the right edges the other. Between two segments:

* on the outside of a bend, the join's own points (an arc, a miter's tip, or nothing for a
  bevel);
* on a nearly straight joint (most joints of a flattened curve, where the old stroker left a
  gap narrower than 0.05 px rather than add a join), the middle of that gap;
* on the inside of a bend, the point where the two edges cross. Where they are too short to
  cross (a line wider than its curve is tight, a hairpin), the outline goes through the
  bend's own point instead, which is what the separate pieces did.

Every part of such an outline is wound the same way as the pieces were, so where it overlaps
itself or another run the winding numbers add and never cancel: no hole can open.

**The loops are then united** by `anicraft_geom::polygons::union`: the area with non-zero
winding, as loops that don't overlap, outer boundaries one way round and holes the other.
The union is taken on an integer grid (the `i_overlay` crate, MIT OR Apache-2.0): points snap
to about 2³⁰ steps across the stroke's bounds and from there the arithmetic is exact, so edges
that lie on one another (a line drawn there and back), touch, or cross at a shallow angle come
out right, and the same on every platform.

Two cases skip the union because their outline can't overlap: one convex loop (a straight
line with its caps), and a convex closed line of one width none of whose bends had to go
through its own point (its outer loop is the convex polygon grown, its inner loop the same
edges moved inwards, each still there and in order, so convex too and inside the other). That
is every rectangle, oval and rounded rectangle.

The result is what every caller gets: the oracle and, through `prepare_scene`, the GPU stage;
the outlines vector exports write for variable-width strokes (fewer, cleaner subpaths); the
Paint Bucket's walls; the stage's selection overlay.

### 2. Why not the other ways

* **Unite the old pieces.** Right, and far too slow to do every frame: a wide curve's 211
  overlapping pieces took 1.6 ms with `i_overlay` and 13 ms with the planar map of merge
  drawing (`anicraft_geom::outline::unite`, built to keep curves for editing, not for
  speed), against 20 µs to stroke it. An outline that is already nearly clean unites in
  about 60 ns an edge.
* **A rasterizer that resolves winding inside a pixel** (sub-scanlines, or exact per-pixel
  clipping). It would have to be written twice, for the oracle and in WGSL, and kept in step
  to 1/255; changing the polygons fixes both renderers with one piece of code they already
  share.
* **Skip the union when a curve "can't" overlap itself** (no fold, turning less than half a
  circle, and so on). Each such rule we tried has a knife edge where a line just touches
  itself and is called clean. The point of this change is that coverage is exact, so only
  the two provable cases above skip it.
* **Write the union ourselves.** The hard part is not the sweep, it is staying consistent
  when edges nearly coincide: one wrong inside/outside call leaves a loop open, and an open
  loop is a streak across the picture. `i_overlay` does this with integer snapping and is
  what the `geo` crate uses for its boolean operations. craft-libs has no polygon boolean.

### 3. A cache is not part of this

The task allowed for caching the union per stroke if it cost. Measured (below), scenes of
ordinary outlined shapes are no slower, and a scene of nothing but self-crossing scribbles
is 4 to 8 % slower. Fills are not cached per shape today (only whole frames are, ADR 0002),
so a stroke cache would be new global state in the oracle with its own memory policy, for a
few per cent on an unusual scene. Left for when a profile asks for it.

## Consequences

* **Pictures change at stroke edges, lighter, never elsewhere.** 14 of the 31 XFL corpus
  goldens were regenerated (9 past the tolerance, 5 by a few pixels): the outlines of ovals
  and rounded rectangles, round caps, the dark knots at outlined rectangles' corners. At the
  corpus's 50 % scale a 1 px outline's edge pixel changes by up to 116/255.
* **Tests.** Over 81 pairs of tangled paths and styles (wide waves, hairpins, knots, a line
  there and back, crossing subpaths, a ring smaller than its width; round, miter, bevel,
  dashes with overlapping round caps, dots, width profiles): no sample point is covered
  twice, the loops cover exactly what the pieces covered, the pixels sum to the outline's
  area, and non-zero and even-odd give the same picture to 1/255. A see-through line that
  crosses itself is nowhere darker than its own alpha. The GPU stage matches the oracle on a
  scene of such lines within the existing 1/255.
* **Against Convert Lines to Fills** (`a_line_is_drawn_as_its_converted_fill_is`): with
  miters, bevels, flat or square ends and dashes on straight runs, the line and its fill are
  the same polygons and no pixel changes at all (0.0000; it was up to 0.5). With curves,
  round joins or round caps the worst pixel is 0.05 to 0.09: the line is flattened along its
  middle and the fill along its edge, each within the renderer's 0.1 px, so they are two
  slightly different polygons. Getting those to 1/255 means building both from the same
  curves, which is a different change (see Not yet).
* **Speed** (median of 80 renders, 32 threads, before → after): 2,000 stroked ovals, stars
  and rounded rectangles at 1080p 9.3 → 9.0 ms; the demo scene 2.5 → 2.6 ms; seven tangled
  lines at 400 × 300 0.51 → 0.63 ms; 300 self-crossing scribbles at 720p 5.9 → 6.4 ms. The
  stroker itself costs about three times what it did (20 → 60 µs for a wide curve), and the
  rasterizers get 3 to 4 times fewer edges (703,000 → 254,000 in the first scene), which
  pays most of it back.
* **One new dependency**, `i_overlay` 9.2 with `i_float`, `i_shape`, `i_tree` and
  `i_key_sort` (MIT; no build script, no unsafe in our use, no default features). It sits in
  `anicraft-geom`. Its input is sanitized first (finite points, coordinates under 10¹²); the
  command fuzz and the stroker's degenerate-input tests run through it.
* The stroker's public functions keep their names and arguments; they return an outline's
  loops where they returned overlapping pieces. Nothing depended on the pieces but tests.

## Not yet

* One geometry for a line and its converted fill on curves (stroke from the curve's offset
  curves, as `anicraft_geom::outline` does, instead of from the flattened middle), which
  would bring that comparison to 1/255 everywhere.
* A per-stroke cache of outlines for strokes that don't move between frames.
* Dashes measured along the curve rather than its flattening (a dash's end can sit a
  fraction of a pixel from where the converted fill puts it).
* Mask artwork and fills that overlap themselves under non-zero have the same arithmetic;
  fills are drawn as the artist's own outline says, and a soft mask's strokes go through
  this stroker already.
