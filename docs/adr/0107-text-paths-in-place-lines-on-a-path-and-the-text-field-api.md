# ADR 0107: Editing text paths in place, lines on a path, and the text-field API

* Status: accepted (2026-11-20), built
* Relates to: ADR 0070 (text on a path), ADR 0066 (dynamic and input text fields), ADR 0085
  (styled runs and paragraphs), ADR 0012 (text as stored outlines), ADR 0013 and ADR 0052 (the
  ActionScript subset), ADR 0024 (the pure player), ADR 0028 (HTML canvas export), ADR 0045
  (point editing), roadmap M5

## Context

Three text slices left the same things "for later". ADR 0070: to reshape the path a text
follows you had to release the text, reshape a shape and attach again; and line breaks became
spaces. ADR 0066: fields had `text`, `appendText` and `length` only, no `htmlText`, no change
events, no `restrict`, and the HTML page drew script-set text in whatever font the viewer's
browser had. ADR 0085: runs existed, but a script could not set them.

The constraints are the old ones. Text is stored as outlines and rendering never needs fonts
(ADR 0012). Time is a function: the player is pure, and the HTML page's player is a line-by-line
port of it, held to it by parity tests. Nothing may crash on hostile input, and HTML is the
most hostile input the app takes from a script.

## Decision

### Editing the path in place

1. **The point commands edit the text's own path.** `path.move_points`, `path.add_point`,
   `path.delete_points` and `path.convert_point` take a text-on-a-path element as their `id`
   and work on the copy of the path the block holds (`TextBlock::on_path`), in the same
   `(command, slot)` addressing and stage-space deltas as on a shape. After the edit the glyphs
   are set along the new path and the block's box is refitted, with the element's transform
   moved the other way, so the path's points stay where they are on the stage and their indices
   don't change during a drag. We chose this over new `text.path_*` commands: the Subselection
   tool, the Pen's add and delete, smooth and corner conversion, arrow-key nudges, the marquee
   and every script that already edits points work on text paths with no new surface to learn
   or fuzz.
2. **The shell shows a text path's points like a shape's.** With the Subselection tool, a
   selected text on a path shows its anchors and handles; a drag is one undo step (the existing
   gesture coalescing) and the text follows every frame of it. Properties › Path gains **Edit
   Path**, which switches to the Subselection tool, because artists look for the feature at the
   text, not at the tool.
3. **A path with no length is refused**, and the text stays as it was (the command fails before
   anything is stored). Plain text still has no points; the message now says to attach it to a
   path.

### Several lines on a path

4. **Line breaks are lines again.** The text is laid out in its lines (each paragraph's line
   spacing kept; wrapping, indents and margins have no meaning on a path and are ignored). The
   first line with glyphs sits on the path, as before, bit for bit. **Each further line runs
   along the curve parallel to the path**, its baseline as far from the path as it is from the
   first line's baseline in the layout, on the side away from the glyphs' tops: below the first
   line, as lines of text are. *Flip* walks the path backwards, so the whole block changes side
   together.
5. **A line is spaced along its own parallel curve, not the path.** Glyph middles are placed by
   arc length on the offset curve (sampled: at most 2,048 samples per line, about one per two
   pixels), so letters keep their advances inside a bend instead of piling up, and spread
   outside one. Every line is anchored at the same place on the path (the offset's point,
   carried across to the parallel curve), on its own width, by the block's alignment: left
   starts there, centre is centred there, right ends there. On a closed path each line wraps
   round its own length. Where the parallel curve folds to nothing (a line through the centre of
   a circle) that line is left out. At most 64 lines are set; the rest are left out and counted.
6. **Animate has no equivalent** (it has no text on a path at all), and SVG's `textPath` is one
   line. The rule above is the one that keeps every line's letters at their own spacing; where a
   bend is tighter than a line's distance from the path, that line's letters do bunch, as any
   offset curve does. Leading blank lines don't move the first line off the path.

### `htmlText`

7. **The subset Flash text fields took**, mapped onto ADR 0085's runs and paragraphs, both
   ways: `<b>`, `<i>`, `<u>`, `<font face size color letterspacing>` (a size may be relative,
   `+2`), `<a href>`, `<p align>` (justify reads as left), `<br>`, and the entities `&lt;`,
   `&gt;`, `&amp;`, `&quot;`, `&apos;`, `&nbsp;` and numeric ones. It lives in
   `anicraft_doc::html_text` as two pure functions, `parse` and `write`.
8. **Reading is a sanitiser.** Nothing of the input is ever kept as markup: a tag outside the
   subset is dropped and its text kept (as Flash does); `<script>` and `<style>` go with their
   content; comments and declarations go; a link that could run script (`javascript:`,
   `vbscript:`, `data:`, however spelt) is dropped and its text stays; character codes that
   aren't characters, and control characters, are dropped. Bounds: 65,536 characters read,
   16,384 kept, 64 levels of style nesting, 16 attributes a tag, sizes 1 to 2,000 px, letter
   spacing ±1,000 px. A fuzz test holds that any input reads without panic, with runs and
   paragraphs that fit the text, and that written HTML reads back to the same text and writes
   the same HTML again.
9. **Writing is canonical**: one `<p align>` per line and one `<font face size color>` per run,
   with `<a>`, `<b>`, `<i>`, `<u>` inside. Numbers have two decimals at most. What this HTML has
   no place for is left out and documented: baseline shifts, opacity, indents, margins and line
   spacing. A line break made by `</p>` takes the style of the text before it.
10. **In the player** a field's state gains its styles (`FieldState::rich`). `field.htmlText =
    …` parses against the field's own style, sets the text and the styles and lays the field
    out run by run, one piece per colour (`FieldLook::pieces`), through the host
    (`ScriptHost::lay_out_field` now takes the styles). `field.text = …` puts the field back in
    its own style, as ActionScript's does. Reading `htmlText` writes the styles the text has:
    what a script set, the authored runs while the text is still the authored one, else the
    field's own. Typing into a styled input field continues the style before the caret.

### Change events, `maxChars` and `restrict`

11. **`Event.CHANGE`** (`ListenEvent::Change`): `field.addEventListener(Event.CHANGE, f)` on a
    text field. It fires **once per edit the viewer makes that changes the text** (a typed
    character, a paste, a deletion, a line break), after the edit, with the field as `target`
    and `currentTarget`. Caret moves, selecting, and keys that change nothing fire nothing, and
    neither does a script setting the text, as in ActionScript. Listening on the stage, a
    timeline or another kind of instance is a problem at compile time or a line in Output.
12. **The action IR has the same event**: `TextField::on_change`, a list of actions that run in
    the timeline the field is on, before the listeners (as an instance's handlers run before
    its listeners). `actions.set {element, event: "change", actions}` sets it on an input
    field, `actions.clear` removes it, `actions.list` shows it, and the Actions panel's
    Instance tab edits it for a selected input field.
13. **`maxChars`** is read and set (0: no limit; the authored value until a script sets it).
    **`restrict`** is read and set in ActionScript's own notation (listed characters, `a-z`
    ranges, `^` to exclude what follows, `\` to escape; `null`: anything; `""`: nothing). Both
    limit only what the viewer types. Typing only what a field refuses does nothing: it does
    not delete the selection. `restrict` has no authoring field, because Animate has none.
14. **Not built here, by choice**: `scrollV`/`maxScrollV` (the field's scroll is the caret's
    doing today and the HTML page's input elements scroll themselves; a script-set scroll needs
    both to agree), `setSelection`, `TextEvent.TEXT_INPUT`, focus events, `textColor` and
    `setTextFormat`.

### The HTML page

15. **`player.js` mirrors all of it**: the HTML reader and writer, the run edits, `restrict`,
    the change event. The parity tests compare output and state step by step on a movie that
    uses every member, and a second test feeds 400 generated HTML strings (hostile pieces
    included) through both readers and writers and compares text, runs, paragraphs and written
    HTML exactly.
16. **Styled text is laid out by a function with no canvas in it** (`fieldLayout`, in
    `player.js`, so the tests run it): lines broken where the text breaks and wrapped at spaces
    across runs, each line making room for its tallest stretch, paragraphs aligned on their
    own. `canvas.js` draws each stretch in its colour, with its underline. Plain text in a
    browser font is drawn exactly as before.
17. **An input field in the page stays a native element** (ADR 0066's reasons hold), so it
    shows its text in one style, and its change event fires from the element's `input` event,
    with `maxChars` and `restrict` applied to what the element holds.

### Embedded fonts in the HTML export

18. **Outlines, not font files.** There is no permissive pure-Rust WOFF2 or font-subset encoder
    in the tree or in craft-libs, so the page cannot carry a font file. It carries what AniCraft
    carries everywhere else: outlines. A dynamic field has **Embed**, a list of characters
    (`TextField::embed`, each once, at most 1,024; Properties offers A–Z, a–z, 0–9, punctuation
    and "the characters in this text"). At export the engine reads those characters' glyphs
    from the field's face (`Fonts::glyph_table`: outline and advance at 1,000 px to the em) and
    hands them to the exporter (`HtmlOptions::fonts`); the render crate still has no fonts. The
    page draws a stretch of changed text from the embedded outlines when the face has every
    character of it, and with the browser's fonts otherwise.
19. **The font's licence bits decide** (`OS/2` `fsType`, per the OpenType specification;
    `Embedding::from_fs_type`). The mapping is the three-case rule proposed for the shared
    library in craft-libs issue #82, so it can be replaced by the shared function unchanged:
    restricted without Preview & Print or Editable, or bitmap-only, is **Forbidden**; "no
    subsetting" is **Whole**; anything else is **Subset**. Only Subset is embedded, since the
    page carries a subset of outlines. A face that is refused, a font that isn't installed (a
    fallback face's glyphs are never embedded in its place), characters the face lacks and
    input fields with characters to embed are each named in the export's report, and the result
    lists every face with what happened to it.
20. **What it costs and what it doesn't do.** Glyphs are drawn one per character: no kerning,
    ligatures or complex shaping, so embedded text can differ a little from Test Movie, which
    shapes. Input fields don't use it. Runs a script styles in another face use that face only
    if a field embeds it. A page carries at most 16,384 glyphs and 32 faces.

## Consequences

* Text on a path is reshaped where it stands, with the tool artists already use for points, and
  can run to several lines.
* Movies that format a score or a message with `htmlText`, validate a name as it is typed or
  limit a field's characters play in Test Movie and in the HTML page, the same in both.
* A dynamic field can look the same in every browser for the characters the author chose, when
  the font allows it; the page grows by the outlines embedded.
* `.anicraft` files without the new settings are byte for byte as before: `on_change` and
  `embed` are saved only when set, and text on a path keeps its format (its text now keeps
  its line breaks, which older builds drew as spaces).
* `ScriptHost::lay_out_field` and the script `World` trait each gained a parameter or a method;
  hosts outside the tree must follow.
* Not yet: `scrollV`, selection and focus members, text formats set by scripts, links that
  click in Test Movie and the page, underlines on a path, compensating a line's spacing where
  its parallel curve folds, styled text inside the page's input elements, kerning in embedded
  text, `htmlText` and embedded characters from Animate documents (`renderAsHTML`, embedded
  font items), and a web font file when a permissive encoder exists.
