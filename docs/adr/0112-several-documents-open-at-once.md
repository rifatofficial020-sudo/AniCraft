# ADR 0112: Several documents open at once

Status: accepted (M8, first slice). Fixes #420.

## Context

AniCraft edited one document per window. Opening or creating another replaced it, after asking
about unsaved changes. Animate keeps each open document in a tab above the stage, and artists
move artwork between files all the time: a character from one scene file into the next, a
button from a library file into a project. Issue #420 asks for the same.

The shell was built around one `Session` (`App::s`) and some 150 fields of view state in `App`,
read directly by every panel and tool. The engine's `Session` holds the document, its undo
history, selection and playhead, and also things that belong to the editor rather than to a
document (the clipboards, the tool style, the plug-ins, recent files, brush and filter presets).
Autosave (ADR 0015) was one recovery session per running editor. Pasting into "another
document" worked only after New or Open in the same session, and only by accident of ids: each
document counts its own ids, so a copied symbol's id may name a different symbol in the other
file.

## Decision

### 1. The shown document stays where it was; the others are parked

`App::s` and `App`'s view fields remain the document being edited, so no panel, tool or test
changes how it reads them. `src/tabs.rs` adds `Tabs`: one slot per open document. The slot of a
document that isn't shown holds a `Parked`: its whole `Session`, its `DocView` and its autosave.
Showing another tab exchanges the two (`std::mem::swap`, O(1): nothing is copied or
serialized).

`DocView` is declared once, by a macro that lists the `App` fields that belong to a document
with the value a newly opened one gets: zoom and pan, timeline scroll and zoom, the frame
selection, Library and Movie Explorer state, the Motion Editor's property, keys and scroll, the
point, bone, pin and width selections, the Actions panel's place. Everything else in `App`
(the tool in hand and its options, the dock layout, preferences, onion-skin settings, windows
such as Find, Publish and Test Movie) is the window's and stays. Before a document is put
aside, what is in progress on it is finished: text being typed and a pen path are committed,
playback stops, an open gesture ends its undo step, and windows that edit one keyframe or layer
(Edit Envelope, Custom Ease, Layer Properties) close. A modal dialog, a running publish or a
plug-in export holds the document: the tab can't be left until it is done, and the status bar
says so.

Rejected: an `App` per document (every panel would need to know which one it draws, and the
window's own state would have to be shared between them), and moving the per-document fields
into a struct inside `App` (a change to thousands of lines of `app.x` for no behaviour).

### 2. Which documents are open is the window's state, not a command

Tabs are like zoom and the tool in hand: they are in no document and nothing about them is
saved or undone, so they live in the shell (golden rule 2's exception for view state). The
engine, the CLI and MCP stay one document per session; `--control` (ADR 0104) edits the
document that is shown.

The engine gains only what any multi-document front end needs:

* `Session::take_shared(&mut other)` hands the editor's share from one session to the next: the
  four clipboards, the tool and text style, the view aids, plug-ins, recent files, brushes and
  filter presets. The shell calls it on every switch.
* `Session::origin`: a number no other document in the process has, new with every New and
  Open. A copy remembers it.
* `Session::drop_caches()` and `RenderService::forget_pictures()`, for point 6.

### 3. New and Open bring a tab

`file.new`, `file.new_sample`, `file.new_from_template`, `file.open`, `file.open_recent` and
`file.recover`, however they are invoked (menu, start screen, a drop, the system handing over a
file, the Recover window), run in a fresh session in a new tab, at the end of the bar. Nothing
is asked about the document that was shown: it stays open. If the command fails, the tab made
for it goes and the document that was shown is back, with the error in the status bar.

* A file that is already open is shown instead of opened twice (paths are compared after
  `canonicalize`).
* The document the app starts with (the sample, or a blank page) gives way to the first
  document opened while it is untouched, as a word processor's blank page does. A document the
  user made or opened never gives way.
* Closing the last tab leaves a blank page and, when the preference is on, the start screen,
  as Animate shows its home screen with nothing open.
* At most 32 documents are open at once (`tabs::MAX_TABS`): each keeps an undo history and an
  autosave writer.

### 4. The tab bar

Above the edit bar, across the window (the edit bar and everything below belong to the shown
document). Each tab has the file's name ("Untitled", "Untitled 2", … until saved), a dot before
the name while it has unsaved changes, and a × to close it. Click shows the document; middle
click closes it; a drag moves the tab past its neighbours; right-click has Close, Close
Others, Close All and Reveal in Finder / Show in File Explorer / Show in Folder
(`platform::reveal_in_folder`). The tooltip is the file's path. Tabs narrow when they don't
fit, then scroll with the wheel; the shown one is brought into view.

Keys: **Ctrl+Tab** and **Ctrl+Shift+Tab** (the Control key on every system: ⌘+Tab is the Mac's
own), **Ctrl+PageDown** and **Ctrl+PageUp**, **Ctrl+W** for File › Close and **Ctrl+Alt+W** for
File › Close All, as in Animate; the last four are in the keymap and can be changed. With the
Test Movie window focused, Ctrl+W still closes that window. The Window menu lists the open
documents, the shown one ticked.

For screen readers the bar is a tab list ("Open documents") whose children are tabs, each
selected or not, named by the document ("…, unsaved changes" when it has them), with a named
Close button.

### 5. Unsaved changes (ADR 0004)

Closing loses work for good, so it always asks, and asks about the document by name:

* **Closing a tab** with unsaved changes shows that document and asks: Save, Don't Save,
  Cancel. Close Others and Close All close the saved ones at once and ask about each unsaved
  one in turn.
* **Quitting** asks once for all of them: the question lists every document with unsaved
  changes. **Save All** saves each in turn (one without a file asks for its name; cancelling
  that stops the quit), **Don't Save** quits, **Cancel** keeps everything open.

One small state machine (`tabs::CloseFlow`, moved on each frame by `tabs::tick`) replaces the
window's three close flags. A save that is cancelled or fails ends the flow with everything
still open.

### 6. Memory stays bounded

A parked document keeps its document tree and its undo history (bounded by the undo budget, per
document) and nothing else: on every switch the stage's tile textures and the render service's
pictures (in memory and on the GPU) are dropped, and the parked session drops its display list
and symbol picture. The render cache and the tile textures are one shared, budgeted set, so
graphics memory doesn't grow with the number of open documents. Switching back draws the
visible tiles again, which takes one frame.

### 7. Autosave and recovery cover every open document

Each open document is its own recovery session (`autosave::DocSaver`: its own id, lock, copy
and writer thread, ADR 0015 unchanged underneath). Unsaved work is copied the moment its
document is put aside, since nothing ticks a parked document. Closing a tab cleanly removes its
copy; a crash leaves one copy per document with unsaved changes, and the Recover window, which
already listed several, restores each into a tab of its own.

### 8. Paste across documents brings the Library items

A copy (artwork or frames) carries the symbols, bitmaps and sounds it uses, found by one
bounded walk (`transfer::carry`), and its document's `origin`. Pasting into the same document
is as before. Pasting into another (`transfer::bring`):

* every carried item gets a new id from the target's counter, and the pasted elements, the
  groups inside them and the timelines of the carried symbols are pointed at the new ids;
* a name the Library's top level already has is made unique ("Ball" → "Ball 2");
* a bitmap or sound the target already has, with the same name and data, is used instead;
* a symbol this session already brought in is used again while it is unchanged on both sides
  (`Arc` identity of the source symbol and of the copy made here), so pasting twice doesn't
  fill the Library with copies, and a symbol edited since comes as a new item rather than
  silently replacing work.

It is one undo step with the paste. Before this, a cross-document paste reused whatever symbol
happened to have the copied id, and never brought bitmaps.

Layer and element ids inside a brought symbol are kept (the id counter moves past them): they
are only looked up within their own timeline.

## Consequences

* Opening a document never asks about the one that was open, and never replaces it.
* Tests that expected "opening over unsaved work asks first" now expect a new tab.
* Undo memory is per document: 32 open documents with full histories use 32 budgets. A shared
  budget is possible later.
* The guide's pictures show the tab bar.

## Not in this slice

* Tearing a tab off into its own window, and tabs side by side (the issue's second picture).
* The Library panel showing another open document's Library, and dragging items from it;
  Animate's "Resolve Library Conflict" choice (replace instead of rename).
* Reopening the documents that were open when AniCraft last quit.
* Left and Right arrows between tabs when one has keyboard focus (Ctrl+Tab does it).
* A shared undo budget across documents.
* Several documents in the CLI, MCP or the control port.

## Measured

Windows 10, the dev profile (optimized), the sample animation in three tabs, 300 switches
(`cargo test --bin anicraft switching_cost -- --ignored --nocapture`): showing another tab
takes 0.2 µs (median and 95th percentile; the exchange and the cache drops), 1.1 ms at worst
(the first). The stage then draws again: the status bar read 6.3 ms for the sample's visible
tiles on the GPU (RTX 5090, Vulkan) in the screenshot run.
