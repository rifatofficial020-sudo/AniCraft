# ADR 0118: Data tables that drive animation

* Status: accepted (2026-11-18)
* Relates to: ADR 0001 (the persistent document), ADR 0016 (motion tweens), ADR 0050 (the
  Library), ADR 0111 (the transformation point), ADR 0114 (audio to keyframes), ADR 0017
  (plug-ins), ADR 0004 (destructive actions), issues #416, #417, #418 (split from #414),
  roadmap M6

## Context

Three requests from one user, a scientist animating a lake-sediment record: keep the numbers
behind an animation in the document (#416), let a column of them drive an instance's position,
scale, rotation or alpha over a span of frames (#417), and let a column pick the frame a graphic
symbol shows (#418). Today a script can turn numbers into keyframes, but then the document holds
only the keyframes, and every revision of the data means writing them again.

Animate has nothing like this; After Effects has data-driven animation through expressions.
AniCraft has no expression language and should not grow one for this: a chart, a caption
counter or a record played over time needs "this number, at this frame, is that property", and
nothing more.

## Decision

1. **The document keeps tables and bindings, apart from the timelines**
   (`Document::data`, `anicraft_doc::data`). A **table** has an id, a Library name, named columns
   of `f64` cells where a cell may be a gap, and an optional **key column**. A **binding** names
   a symbol instance by element id, one property, a table by id, a column by name, a span of
   frames, the keys the span runs between, an interpolation, and a scale and offset. The field
   is left out of files with no data, so those files are byte for byte what they were, and
   `FORMAT_VERSION` does not change: an older build opens a file with data and ignores it (and
   drops it if it saves, as with every optional field before this one).

   Bindings are kept in the document, keyed by element id, not on the element. An instance is
   copied onto every keyframe of its layer, and a binding on the element would have to be kept
   the same on all of them; keyed by id, one binding follows the instance through keyframes and
   tweens and touches neither. It also keeps `Element` as it is for the hundred places that
   build one. The cost: a copy, paste or duplicate makes a new id and so does not carry the
   bindings (not yet, below), and bindings of an instance that was deleted stay until the file
   is next opened (undo brings the instance back with them).

2. **A row per frame, or a row per keyed time.** Without a key column, rows are looked up by row
   number, and a binding of as many frames as the table has rows shows exactly one row per
   frame (the arithmetic multiplies before it divides, so frame *n* is row *n* to the bit; a
   test walks 997 rows). With a key column (an age, a depth, a time) rows are looked up by its
   value, and a table with a key keeps its rows in the key's ascending order, rows without a key
   last, so a lookup is a binary search. Playing a record from old to young is a binding whose
   last key is before its first, not a second ordering of the table.

3. **Reading a column.** Between the nearest rows with a number on either side: straight across
   (**Linear**) or held from the earlier row (**Step**, which gives regime holds with no new
   ease). Gaps are skipped, so a patchy column still draws. Before the first number and after
   the last, theirs. A column with no number, a missing column or a missing table gives no
   value, and the instance stays as it is placed: a binding never hides or breaks its instance.

4. **A binding replaces the property's value; it does not add to it.** Value = column × scale +
   offset, in the property's own units (pixels, percent, degrees), within the range a motion
   tween's curve may take. X and Y are the registration point; scale and rotation turn about
   the instance's transformation point when it has one (ADR 0111), which is how a bar grows
   from its foot. The code that places the instance is the motion tween's
   (`motion::placed`), so a bound instance and a tweened one agree. Alpha keeps the rest of the
   colour effect. The other properties still tween: a bound Y on an instance whose X is tweened
   moves along both.

   **Before the span the instance shows the span's first frame, after it the last.** A chart
   whose data ends should hold its last value, not snap back to where the bar was drawn. The
   span exists to map frames onto keys, not to switch the binding on and off.

5. **The frame a graphic shows** (#418) is one more bound property, `frame`: the value, rounded,
   is the 0-based frame of the symbol (as `first_frame` is in every command), and the instance is
   shown as Single Frame whatever its looping says, since a frame picked by data and a loop
   offset can't both be meant. Only a graphic symbol's instance takes it (a movie clip plays on
   its own).

6. **Only symbol instances are bound**, as only they tween. A shape's own space is the stage's,
   so scaling one about its origin would throw it across the stage; Convert to Symbol is one
   key away and gives the artist a registration point to scale about.

7. **Applied inside `evaluate`, purely.** `eval_timeline` hands each layer's elements, after
   tweens, motion paths and poses, to `Document::bound_elements`, which swaps in the bound
   properties for that timeline's frame. A document with no bindings pays one emptiness check
   per layer; one with bindings pays a binary search per element (bindings are kept sorted by
   element and property). Scrubbing, the stage, the oracle, the GPU stage, Test Movie and every
   export that draws through `evaluate` (PNG, sequences, GIF, APNG, WebP, video, sprite sheets,
   SVG frames, the HTML canvas page's baked frames) therefore agree, with nothing to keep in
   step. Bindings apply on whatever timeline holds the instance, a symbol's own included.

   Editing tools read the authored element (`Session::elements_on`), so dragging a bound
   instance moves what is under the binding; the selection box follows what is drawn, since it
   comes from the display list. Properties shows the authored values and, in the Data section,
   what each binding gives at the playhead.

8. **Bake to Keyframes** (`data.bake`) writes what the bindings show as plain keyframes and
   removes them, for people who want keys, and for exports that write keys from the document's
   structure (Lottie). It inserts a keyframe where the bound value changes on a held span, one
   per frame inside a classic tween (the tween is kept on both sides of each cut), and sets the
   instance on every keyframe that holds it. A looping graphic cut onto a new keyframe carries
   on from the frame it was showing. Tests compare every frame of the movie before and after.
   It refuses armature layers (poses place their objects), locked layers and motion tween spans
   (keyframes would cut the tween up; its curves are the right target, later), and changes
   nothing when it refuses.

9. **Tables are Library items** (`LibraryItem::Data`): listed, renamed, moved into folders,
   counted (a use per binding), found by Select Unused Items and deleted like the others;
   deleting a table deletes the bindings that read it (undoable, and counted in the
   confirmation as uses). Two differences. Table names are unique among tables whatever folder
   they are in, because commands name a table by name. And a table keeps its own folder
   (`DataTable::folder`), not an entry in the Library tree's placements: an older build reads
   those placements strictly, and one it doesn't know would make the whole file unreadable.

10. **Import** (`data.import_table`, File › Import › Import Data Table…) reads CSV, TSV or JSON,
    up to 32 MB. Delimited text goes through **craft-csv** (craft-libs: encoding and delimiter
    detection and a bounded record parser), as the shared-libraries rule asks; what a field
    means is ours. The first record names the columns unless all of it is numbers; a cell that
    isn't a finite number is a gap; a column with no number at all is left out and named in the
    report; a single decimal comma is read in semicolon and tab files. JSON may be an array of
    records, an array of rows, an object of columns, or `{columns, rows}`; a small serde visitor
    keeps numbers only and stops storing at the limits, so a hostile file costs no more memory
    than the largest table. `data.set_table` takes the same values inline for scripts, MCP and
    plug-ins.

    **Importing over a table keeps its id**, so every binding stays (bindings name columns by
    name, so reordered columns are fine). A binding whose column is gone rests, and the report
    names it. The table remembers the file's name only, never its folder, so a shared document
    doesn't carry someone's paths.

11. **Limits.** 256 tables; 256 columns, 100 000 rows and 1 000 000 cells a table; 64-character
    column names; 10 000 bindings; spans of at most 100 000 frames; scale, offset and keys
    within ±1e9. Opening a file repairs data and never refuses the file for it: tables with
    repeated ids are dropped, names made usable and unique, short columns padded, non-finite
    cells turned into gaps, keyed rows sorted, bindings sorted and made one per property, and
    bindings of instances that are nowhere dropped.

12. **Commands, in a new `data` namespace**: `data.import_table`, `data.set_table`,
    `data.list`, `data.get`, `data.set_cell`, `data.set_key`, `data.remove`, `data.bind`,
    `data.unbind`, `data.bindings`, `data.bake`. All work headless, over the CLI and MCP.
    **Plug-ins** may call all of them but `data.import_table`, which reads a file (as
    `swatch.import` is refused); they pass values to `data.set_table`.

13. **In the shell.** A table shows in the Library with a preview of its first rows, its key
    column and Import Again; a double-click opens the **table viewer**, a window that draws
    only the cells in view. A symbol instance's Properties get a **Data** section: Bind to Data
    adds a binding, whose table, column, frames, keys, scale, offset and interpolation are
    edited in place and show on the stage as they change, with no dialog; Bake to Keyframes
    sits beside it. Modify › Data has Bind to Data, Remove Data Bindings and Bake Data to
    Keyframes.

## Measured

Test profile, Windows 10 (`cargo test -p anicraft-engine data_timings -- --ignored --nocapture`):

* Import: 100 000 rows of 10 columns from 3.2 MB of CSV in 63 ms.
* `evaluate` of a frame with 200 instances: 15 µs unbound, 82 µs with each bound to a column of a
  1 000-row keyed table (about 0.33 µs a binding: a binary search and a decompose and compose).
  The unbound figure includes the new step: one emptiness check per layer.
* Bake of those 200 instances over 40 frames: 39 keyframes in 4.9 ms.

## Not yet

* Bindings that follow copy, paste, Duplicate and Duplicate Symbol.
* Baking inside a motion tween (as its property curves), and a warning from Lottie export when
  a document has live bindings (it writes keys from the document's structure; bake first).
* Editing cells in the viewer (`data.set_cell` does it headless), adding and renaming columns,
  text columns that drive captions.
* Binding other properties (skew, tint, filter values), shapes, text and group members.
* Expressions between columns, and audio amplitude written into a table (#332's other half).
* XFL has no counterpart; nothing is imported or exported there.

## Consequences

A revision of the data is one Import Again. The numbers travel with the document. `evaluate`
gains one more pure step, and the file format one more optional field. The Library has a fourth
kind of item, so every `match` on `LibraryItem` now says what a table means there.
