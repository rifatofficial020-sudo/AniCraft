# ADR 0085: Per-character and paragraph styles in text

* Status: accepted (2026-11-16), first slice built
* Relates to: ADR 0012 (text), ADR 0014 (text from Animate documents), ADR 0066 (dynamic and
  input fields), ADR 0070 (text on a path), ADR 0004 (destructive actions), roadmap M5

## Context

ADR 0012 gave a text block one style. Animate's static text mixes styles freely: a bold word, a
coloured name, a superscript in "mc²", a centred title above left-aligned body text. Animate
documents store this as `DOMTextRun`s, each with its own `DOMTextAttrs`, and our XFL import split
such a field into one block per style, losing anything mixed within a line. The constraints of
ADR 0012 still hold: rendering and every export use only stored outlines (no fonts at render
time), `evaluate` stays pure, older files must open and save unchanged.

## Decision

1. **Runs and paragraphs on the block.** `TextBlock` gains `runs: Vec<TextRun>` (a character
   count and a `CharStyle`: font, size, bold, italic, colour, letter spacing, baseline shift,
   underline, link) and `paragraphs: Vec<ParagraphStyle>` (alignment, first-line indent, left
   and right margins, line spacing), both `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
   **Empty means "all in the block's own style"**, so every older file opens unchanged and a
   block whose characters share one style saves byte for byte as before (a test pins the JSON).
   When there are runs, the block's own style fields mirror the first run and first paragraph,
   so code that asks "the block's style" (the player laying out a script's text, Find, exports
   of fields) keeps working. `TextBlock::set_styles` is the one way to set them: it fits runs to
   the text, joins neighbours in one style, caps them at 10,000 (`MAX_TEXT_RUNS`; the rest takes
   the last style), sanitizes values, and drops runs and paragraphs that add nothing.
2. **Run edits are pure functions** in `anicraft_doc::rich_text` (`type_into_runs`,
   `replace_runs`, `restyle_runs`, `slice_runs`, `edit_paragraphs`, `summarize`, `diff_chars`),
   shared by the engine, the shell and the tests. **Typing continues the style of the character
   before the caret** (the first character's at the start), as Animate and word processors do. A
   line break splits a paragraph into two with its style; joining keeps the first's. A change of
   text given as a whole (`text.edit {text}`, Find and Replace) is taken as one edit: the longest
   common start and end are kept and the middle replaced.
3. **Layout per run** in `anicraft-text` (`Fonts::layout_rich`): each stretch of a paragraph in
   one run is shaped on its own with `harfrust` in its own face (faces read once per layout,
   glyph outlines cached per face, glyph and size), lines break greedily at spaces across runs,
   a line makes room for its tallest run, and paragraphs are aligned, indented and narrowed by
   their margins. **One run in one paragraph lays out exactly as before**: the old layout's
   outline, box, lines, carets and glyphs were compared bit for bit on 288 inputs (sizes,
   spacing, widths, alignments, line breaks, ligatures) before the old code was removed.
   Kerning across a run boundary is not applied (each run is shaped alone).
4. **Superscript and subscript use the font's own OS/2 metrics** (`ySuperscriptYSize`,
   `ySuperscriptYOffset`, and the subscript pair, per the OpenType specification), falling back
   to 0.65 em raised 0.35 em (lowered 0.15 em) when a font leaves them out. The glyphs are drawn
   smaller and shifted; the line keeps the run's full-size height so a superscript doesn't push
   lines apart. Underlines use the font's `post` underline position and thickness.
5. **Colour pieces without duplicating outlines.** The outline is still one path, now laid out
   as one piece per paint (colour and link) one after another; `fills: Vec<TextFill>` records each
   piece's colour, link and command count. `evaluate` emits one display item per piece; the
   pieces are cut from the outline once and cached in memory (never saved, invalidated on copy
   or when the outline changes), so a frame costs what it did. A single piece in the block's
   colour keeps no fills (files unchanged). Every renderer and export (CPU oracle, GPU stage,
   PNG, GIF, video, SVG, Lottie, HTML canvas, sprite sheets) therefore draws each colour with no
   change. Break Apart gives one shape per colour.
6. **Links.** A run's `url` is kept (unsafe schemes — `javascript:`, `vbscript:`, `data:` — are
   refused by the commands and dropped on import). The display list carries a sparse
   `links: Vec<(item, url)>`; SVG export wraps those items in `<a href>`. We chose outlines with
   links over SVG `<text>`/`<tspan>` output: tspans would make the SVG depend on the viewer's
   fonts and break ADR 0012's "looks the same everywhere"; selectable SVG text, if wanted, will
   be an export option.
7. **Fields and paths keep runs.** A dynamic or input field's authored text is laid out with its
   runs (ADR 0066 allows authored styles); the HTML page draws its pieces (`pp`). Text a script
   or viewer sets uses the field's first style; `htmlText` is later. Text on a path keeps its
   runs' faces, sizes, colours and shifts; underlines aren't drawn on a path yet.
8. **Commands.** `text.style {start?, end?, …character keys}`, `text.paragraph {start?, end?,
   …paragraph keys}` (the paragraphs the range touches), `text.styles {start?, end?}` (runs,
   paragraphs, and what the range shares, a mixed value as `null`), and `runs`/`paragraphs` on
   `text.create` and `text.edit` (character keys on `text.edit` restyle every run, keeping the
   rest). All live in the `text` namespace, which plug-ins may already call. Each change is one
   undo step; the panic hunt covers hostile runs, ranges, sizes and links.
9. **The shell.** While typing on the stage, the editor keeps the runs and paragraphs with the
   text and shows each run's colour, size (at the zoom), italic, underline and baseline shift in
   the interface font (egui can't show the document's faces; the stage, updated by each style
   change in the same frame, shows the real result). Properties gains a section named after the
   selection ("Text: 6 characters selected") with the character and paragraph rows; values the
   selection doesn't share show the first one's and say **mixed**; at a caret, a style picked
   applies to what is typed next. A click in that section keeps the editor open and its
   selection. Copy and Cut remember the selection's runs; a paste of exactly that text gets
   them back, anything else pastes plain in the style before it (the system clipboard keeps
   plain text, so other apps are unaffected).
10. **XFL import** reads every `DOMTextRun` into one block per field: face (PostScript name to
    family, bold and italic), size, colour and alpha, letter spacing, `characterPosition` and
    `url` per run; `alignment`, `indent`, `leftMargin`, `rightMargin` and `lineSpacing` per
    paragraph from the run the paragraph starts in. Justified text is still drawn left-aligned
    and reported. The corpus test now expects one block for the two-style title field.

## Consequences

* Text with mixed styles round-trips through `.anicraft`, XFL import and every export, edits
  stay live and undoable, and documents without mixed styles are unchanged on disk.
* A block costs one display item per colour instead of one.
* Fields from Animate show their authored styles; scripts still set text in one style.
* Not yet: justified alignment, kerning across runs, underlines on a path, `htmlText`, links
  that click in Test Movie and the HTML page, SVG `<text>` output, the stage editor showing the
  real faces, bold and paragraph alignment while typing, and tab stops.
