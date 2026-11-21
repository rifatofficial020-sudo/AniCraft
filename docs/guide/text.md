# Text

Titles, captions, speech bubbles, scores and name boxes: this chapter covers the Text tool, the
fonts it uses, the three kinds of text, and how text looks when you export.

A note on keys: AniCraft uses **Ctrl** on Windows and Linux and **⌘** (Command) on macOS, and
**Alt** is **⌥** (Option) on macOS. So **Ctrl+B** is **⌘B** on a Mac.

## Typing text

1. Pick the **Text** tool (**T**).
2. Click on the stage where the text should start, and type. **Enter** starts a new line.
3. Press **Esc**, or click somewhere else, to finish.

To change text later, click it with the Text tool, or double-click it with the **Selection**
tool (**V**). Each edit is one undo step.

Text is artwork like any other: you can move, scale, rotate and fade it, tween it, put it in a
symbol, and use it as a mask.

## Text properties

Select some text to set its look in **Properties**. With the Text tool in your hand and nothing
selected, the same rows set what new text gets. New text keeps the last style you used, as in
Animate.

| Row | What it sets |
|---|---|
| **Font** | The typeface: any font on your computer. |
| **Size** | The size in pixels, with the text colour beside it. |
| **Style** | **B** for bold and **I** for italic, then **Left**, **Centre** or **Right** alignment. |
| **Spacing** | **Letters** (letter spacing) and **Lines** (line spacing), in pixels. |
| **Width** | Turn on **Wrap** to wrap lines at a fixed width, and set the width. With Wrap off, each paragraph stays on one line. |
| **Position** | **Normal**, **Superscript** or **Subscript**, then **U** for underline. |
| **Link** | A web address the characters link to in SVG exports. |
| **Indent** | How far the first line of a paragraph starts in (negative hangs it out). |
| **Margins** | Space kept clear at the left and right of each line, in pixels. |

Bold and italic use the matching face of the font (for example, its Bold or Italic file). If a
font has no bold or italic face, AniCraft doesn't fake one.

With the text selected (not being typed in), a change applies to the whole block. Where the
characters differ, the row shows the first one's value and says **mixed**.

## Styling words and characters

One block can mix styles, as in Animate:

1. Click into the text with the **Text** tool (or double-click it).
2. Select the characters: drag across them, or hold **Shift** with the arrow keys.
3. Set their style in **Properties**: the section at the top is named after your selection, for
   example **Text: 6 characters selected**. Font, size, colour, bold, italic, letter spacing,
   superscript and subscript, underline and link all apply to just those characters, and the
   stage shows the change at once (each change is one undo step).

Things to know:

* **Typing continues the style before the caret.** Type after a red word and the new letters
  are red. To start a new style, set it in Properties with nothing selected, then type.
* **Paragraph settings** (alignment, indent, margins and line spacing) apply to every paragraph
  your selection touches. Press **Enter** inside a paragraph and both halves keep its settings.
* **Copy and paste keeps the styles inside AniCraft.** Text pasted from other apps comes in
  plain, in the style of the text before it.
* While you type, the box over the stage shows the colours, sizes, italics, underlines and
  superscripts in AniCraft's interface font; the stage shows the real fonts and alignment.
* **Superscript and subscript** use the size and offset the font itself suggests.
* Text set by a script in a dynamic or input field uses the field's first style.

## Fonts on your computer

The **Font** menu lists the fonts installed on your computer, plus Inter, which comes with
AniCraft. There is nothing to set up: install a font in your system as usual and it shows up the
next time AniCraft starts.

### Your text looks the same everywhere

AniCraft saves the shapes of the letters with the document. So a file you send to someone looks
exactly the same on their computer, even if they don't have the font, and so do your exports.

If you edit text whose font isn't on this computer, AniCraft lays it out again with the default
font and tells you, so nothing changes behind your back.

### Missing characters

If a font doesn't have a character you type (an emoji, or a letter from another script), the
character shows as an empty box. AniCraft doesn't borrow it from another font yet, so pick a font
that covers your language.

## Static, dynamic and input text

As in Animate, text comes in three types. Choose one in **Properties › Text type**.

| Type | What it's for |
|---|---|
| **Static text** | Text that's part of the artwork: titles, captions, labels. This is what the Text tool makes. |
| **Dynamic text** | Text a script changes while the movie plays, such as a score or a timer. |
| **Input text** | A box viewers type into, such as a name or an answer. |

### Field settings

Dynamic and input text are **fields**, with a few more rows in Properties:

* **Instance**: the name scripts use for the field, for example `score_txt`. Letters, digits and
  `_` only.
* **Lines**: **Single line**, **Multiline**, **Multiline no wrap**, or (input text only)
  **Password**, which hides what the viewer types.
* **Box**: the field's width (**W**) and height (**H**) in pixels.
* **Show border**: draws a line round the field and fills its box.
* **Selectable** (dynamic text): viewers can select the text while the movie plays.
* **Max characters** (input text): the most characters a viewer may type. **0** means no limit.

### Setting and reading fields from a script

A frame script can set a dynamic field's text, add to it, and read what a viewer typed into an
input field. For example:

```actionscript
score_txt.text = "Score: " + score;
```

See [Interactivity](interactivity.md) for how to add scripts.

### Formatted text from a script

A script can set a field's text with formatting, using a small set of HTML tags:

```actionscript
score_txt.htmlText = "Score: <b>12</b> <font color=\"#cc0000\">new best!</font>";
```

The tags are `<b>`, `<i>`, `<u>`, `<font face="…" size="…" color="#RRGGBB">`, `<a href="…">`,
`<p align="left|center|right">` and `<br>`. A size may be a step from the size around it, such as
`size="+4"`. Anything else is left out safely: other tags are dropped and their text kept,
scripts and styles inside the text are dropped, and a link that could run a script is no link.
Reading `htmlText` gives the field's text back with its formatting. Setting plain `text` puts the
field back in its own style.

### Limiting what viewers type

For an input field, **Max characters** in Properties limits how much a viewer may type. A script
can change it, and can also limit *which* characters go in:

```actionscript
name_txt.maxChars = 12;
name_txt.restrict = "A-Za-z ";   // letters and spaces only
```

`restrict` lists the characters allowed, with ranges such as `0-9`. A `^` means "but not what
follows": `"A-Z^Q"` is capitals except Q. Set it to `null` to allow everything again. These
limits apply to typing; a script can still set any text.

### Doing something when a viewer types

An input field can react each time its text changes. Without a script: select the field, open
**Actions**, choose the field's tab and click **Add change event**, then pick actions as for a
button. In a script:

```actionscript
name_txt.addEventListener(Event.CHANGE, onChange);

function onChange(e:Event):void {
	hello_txt.text = "Hello, " + e.target.text;
}
```

The event comes once for every edit the viewer makes. Moving the caret or selecting text doesn't
send it, and neither does a script that sets the text.

### Trying it out

Choose **Control › Test Movie** (**Ctrl+Enter**). Click an input field and type. You can move
the caret with the arrow keys, **Home** and **End**, select with **Shift** or the pointer, delete
with **Backspace** and **Delete**, start a new line with **Enter** in a multiline field, and press
**Tab** to go to the next field. Select All, Copy, Paste and your system's input methods work too.
Screen readers find input fields as text boxes.

Not there yet for fields: scripts that scroll a field or set its selection, links you can
click while the movie plays, Cut, double-clicking to select a word, a blinking caret and
scrolling a field with the mouse wheel.

## Text on a path

Static text can follow the outline of a shape: an arc, a circle, a wave.

1. Select the text block.
2. In **Properties**, under **Path**, click **Attach to Path**.
3. Click the shape on the stage. (Press **Esc** to give up.)

The text now runs along a copy of the shape's outline, standing on it. The shape itself stays as
it was: delete or hide it if you only want the text. With a shape and the text both selected,
**Modify › Text › Attach Text to Path** does the same in one go.

* **Offset** moves the text along the path. The text's alignment says how: left-aligned text
  starts at the offset, centred text is centred on it, right-aligned text ends at it.
* **Flip** runs the text the other way along the path, on its other side (under an arc instead
  of over it).
* **Release** turns it back into lines of text where it started.
* **Edit Path** switches to the Subselection tool so you can reshape the path (below).

### Reshaping the path

The path belongs to the text, so you can reshape it where it is. Select the text and choose the
**Subselection** tool (**A**), or click **Edit Path** in Properties. The path shows its points:

* drag a point or one of its handles, and the text follows as you drag;
* double-click the path to add a point; select points and press **Delete** to remove them;
* right-click a point to make it smooth or a corner.

Each drag is one step to undo. The original shape, if you kept it, doesn't change.

### Several lines on a path

Press **Enter** while typing to start a new line. The first line sits on the path; each further
line runs alongside it, one line height further from the path on the side under the first line's
letters, and keeps its own letter spacing as the path bends. **Line spacing** moves the lines
apart, the alignment places every line at the offset (starting, centred or ending there), and
**Flip** moves the whole block to the path's other side. Where the path bends more tightly than
a line is far from it, that line's letters crowd together: ease the bend or use fewer lines.
Text on a path takes up to 64 lines.

Changing the words, font, size or spacing keeps the text on its path. On a closed shape the text
wraps round; on an open one, letters that would run past the ends don't show. The selected text
shows the path it follows. Text on a path is always static text, and it is drawn from its own
outlines like all text, so it looks the same on every computer.

## Filters on text

Text takes the same filters as movie clips and buttons: **Drop Shadow**, **Blur**, **Glow**,
**Bevel**, **Gradient Glow**, **Gradient Bevel** and **Adjust Color**. Select the text and
choose **Add filter** under **Filters** in Properties; you don't need to make a symbol first.
Everything in [Filters and blend modes](drawing.md#filters-and-blend-modes) works here too: the
order, the eye that switches a filter off, copying and pasting filters, and presets, the
built-in ones included.

Filters work on static, dynamic and input text. On a field, the box, the border and the text
are filtered together, so a drop shadow falls from the whole field. Changing the words, the
font or the text type keeps the filters. They tween between keyframes and in the Motion Editor
like a movie clip's (see [Tweens](tweens.md#animating-filters)). **Modify › Break Apart** turns
the text into a shape, which has no filters.

## Turning text into shapes

To reshape the letters themselves, select the text and choose **Modify › Break Apart**
(**Ctrl+B**). The text becomes an ordinary shape that you can edit with the Subselection tool,
recolour and morph. Text in several colours becomes one shape per colour. It is no longer
editable as text, so keep a copy if you might want to change the words.

## Text in exports

Because the letter shapes are saved with the document, text comes out exactly as you see it on
the stage in every picture export: PNG, PNG sequences, GIF, animated PNG and WebP, video, sprite
sheets, SVG and Lottie. Fields show their authored text, box and border there. Each colour in a
block is drawn in its colour, and in SVG, characters with a link are a link.

In the **HTML canvas** export, the movie plays in a web browser:

* static text, and the authored text of fields, is drawn exactly as in AniCraft;
* input fields become real text boxes over the movie, so the browser's input methods, on-screen
  keyboards on phones and tablets, and the clipboard all work. Filters on an input field show
  on its box and border there; what the viewer types is drawn by the browser, without them
  (the export report says so);
* text that a script or the viewer changes is drawn with the fonts in the viewer's browser. If
  the viewer doesn't have your font, it uses a similar one, and the export report tells you this
  can happen.

To keep your own font for text a script changes, **embed** the characters it will show. Select
the dynamic field and, in Properties, type them into **Embed** or use **Add** (capitals, lower
case, numerals, punctuation, or the characters in the field's text). The page then carries the
outlines of just those letters and draws the field with them in every browser. A few things to
know:

* AniCraft reads the font's own licence setting first. If the font doesn't allow embedding, or
  only allows the whole font, nothing is embedded and the export report says so; the field
  uses the browser's fonts as before.
* Embedded letters are drawn one by one, without kerning or ligatures, so spacing can differ
  slightly from Test Movie.
* Only dynamic fields use them. An input field is a real text box, drawn by the browser.
* A character you didn't embed makes that word fall back to the browser's font.

AniCraft doesn't put font files in the web page: it has no way to make a web font that keeps to
every licence. Embedding only matters for text that changes while the movie plays.

See [Exporting and Publish](exporting-and-publish.md) for every format.

## Text in Animate files

When you open an Animate document, its text comes across as text you can edit, with its font,
size, colour, bold and italic, superscript and subscript, links, alignment, indents, margins,
spacing, position and wrapping. Each Animate text block stays one block, with every run's style
kept, and its filters. Dynamic and input fields come across as fields, with their names, boxes,
borders and line types. Fonts that aren't on
your computer use the default font, and the status bar names them. See
[Opening Animate files](opening-animate-files.md).

Not there yet: vertical text and text along a path.

## Next steps

* Make text react to the viewer: [Interactivity](interactivity.md).
* Animate a title: [Tweens](tweens.md).
