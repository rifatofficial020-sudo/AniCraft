# ADR 0130: Text that mixes directions, and letters from a fallback face

* Status: accepted (2026-11-23), built
* Relates to: ADR 0012 (text as stored outlines), ADR 0085 (styled runs and paragraphs),
  ADR 0066 (text fields), ADR 0070 and ADR 0107 (text on a path), ADR 0125 (backlog item P-008),
  outside PRs #39 and #40 (SoldierPePe)

## Context

Two things made text read wrong.

**Mixed directions.** Layout shaped each run of a paragraph as one stretch, in the direction of
its first letter. A number or a Latin word inside Arabic or Hebrew came out reversed ("2026" as
"6202"), and a wrapped right-to-left paragraph was broken from its visual left, so its last
words came first.

**Missing letters.** A block was drawn with the faces its runs chose and nothing else. Arabic
in a face without Arabic letters (Inter, the default where the app embeds it; Consolas) showed
as boxes, and nothing said so.

Outside PRs #39 and #40 fixed both on the single-style layout that existed before ADR 0085.
That layout is gone: text is now runs with their own faces, sizes and paints, in paragraphs
with their own alignment and margins, and it is also set along paths and typed into fields
while a movie plays. This ADR rebuilds both fixes on that layout and keeps the PRs' ideas and
tests.

The constraints are the old ones: text is stored as outlines, so nothing downstream of layout
(the renderers, every exporter, the file) knows about directions or fonts; layout must not
panic on any text; and a document must not change because of which fonts a computer has until
its text is edited there.

## Decision

1. **The order of work is the bidi algorithm's** (Unicode Standard Annex #9). Embedding levels
   are resolved once for the text; text is shaped and broken into lines in reading order; each
   line is then put in the order it is displayed. Levels and reordering come from the shared
   `craft-text` crate's `bidi` module (storytold/craft-libs, on `unicode-bidi`), not from an
   app copy. A paragraph's direction is its first strong character's (rules P2 and P3).
2. **A run is shaped in stretches of one direction and one script.** Each stretch gets its
   direction from its level and its script from its letters; characters every script shares
   (spaces, digits, punctuation, marks) take the script before them. Each stretch is shaped
   with the letters around it as context, so Arabic letters join across a change of style or
   face as they do inside one. Script itemisation uses the shaper's own script table: no new
   dependency.
3. **Lines are made of shaped clusters.** A cluster is what the shaper says belongs together:
   a letter with its marks, or a ligature. Lines break after the last space that fits, or
   between the clusters of a word wider than the line, never inside a cluster; reordering moves
   clusters whole. Text is shaped once: a line takes its clusters as they were shaped, so a
   word broken across lines in the middle (only when it is wider than the line) keeps the
   joining forms it had.
4. **Letters a run's face lacks come from a fallback face, cluster by cluster.** After
   shaping, a cluster with a glyph its face has no letter for is shaped again in another face:
   the first of `FALLBACK_FAMILIES` (faces with wide coverage that come with Windows, macOS and
   common Linux desktops, then the system faces for Chinese, Japanese, Korean, Indic and Thai
   text, then symbols) that has every drawn character of the cluster, in the run's weight and
   slant; when no listed face has all of it, any installed face that has the letter. A letter
   and its marks always go together. The rest of the block keeps its face, and so do the line's
   height and baseline: a fallback face's letters sit on the line the run's face makes.
   Invisible format characters (joiners, direction marks, variation selectors) are never
   missing. Limits: 16 fallback faces per block, and 16 searches of every installed face per
   layout (each remembered).
5. **The user is told.** `text.create`, `text.edit` and the other text commands return a note
   when letters were drawn from another family (naming it) and when characters are in no
   installed font and show as boxes (how many). The block's font stays the one chosen.
6. **Carets know where clusters stand.** A laid-out line carries its paragraph's direction and
   each cluster's box in display order. A caret's place is the edge its cluster is read from
   (the right edge of a right-to-left letter). In a field while a movie plays, Left and Right
   move the caret the way the key points through text that mixes directions, crossing to the
   line before or after at a line's edge by the paragraph's direction; a selection is still a
   range of characters and is highlighted as one stretch per direction it runs through. Lines
   that read left to right throughout behave exactly as before.
7. **Nothing downstream changes.** Static text, text on a path and fields store the outlines
   layout made, in display order, so the CPU and GPU renderers and every exporter draw them as
   they draw any text. Text on a path takes each glyph's place on its line as it is displayed.
   No new field in the document: files are unchanged.
8. **Tests use fonts made in memory.** `crates/text/src/testfont.rs` writes small TrueType
   fonts (boxes for letters) from the OpenType specification, so mixed directions and fallback
   are tested the same on every computer instead of being skipped where a font is missing.

## Consequences

* Text in one direction lays out as it did, except where it mixes scripts: a stretch in
  another script is now shaped with that script's rules.
* Letter spacing follows each cluster, not each glyph, so a letter's marks stay on it.
* `Layout::missing` counts characters no face has; before, it counted characters the run's own
  face lacked.
* Which fallback face is used depends on the computer. That is the same rule as for fonts
  (ADR 0012): the outline is stored, so a document only changes where it is edited.
* One caret per boundary: at a change of direction a boundary has two places it could be
  shown, and we show it at the edge of the letter after it (the line's end at the edge of the
  letter before it). Typing a letter of the other direction there can make it appear away from
  the caret. Editors with two carets or a caret affinity avoid that; we don't have either yet.

## Not in this slice

* A direction setting for a paragraph (it is always the first strong character's), and
  alignment to the paragraph's start: a right-to-left paragraph is aligned as its paragraph
  style says, left by default, as left-to-right text is.
* The stage editor (the field beside the block) is egui's text editor, which does not reorder;
  the block updates live and reads correctly. Typing in the block itself is P-019, which will
  use the caret boxes from point 6.
* Script-set text in the HTML export's own player (ADR 0107) is laid out by the page's script
  from embedded glyph tables, one glyph per character and left to right; authored text there is
  outlines and reads correctly.
* The direction SVG text asks for (`direction`, `unicode-bidi`) on import.
* Reshaping a word broken in its middle, hyphenation and kashida justification.
* The interface's own right-to-left layout (P-034).

## Alternatives considered

* **Depending on `unicode-bidi` directly**, as PR #39 did. The shared crate already wraps it
  with the line rules (L1 and L2) on slices of levels, which is what a layout made of clusters
  needs, and rule 9 of the Craft standards asks apps to use shared code.
* **Choosing a face per character before shaping**, as PR #40 did, with the Unicode class of
  each character deciding what a mark attaches to. Shaping first lets the shaper say what a
  cluster is, needs no extra character tables, costs nothing when the face has every letter,
  and treats a letter the shaper can compose from parts as present.
* **Reshaping every line** after breaking, as PR #39 did. It shapes all text twice; shaping
  once and breaking between clusters gives the same result at every break that is at a space.
* **A bundled font for tests.** A font file is an asset with a licence and a provenance row;
  the fonts written in `testfont.rs` are code.
