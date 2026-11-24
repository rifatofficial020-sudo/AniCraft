# ADR 0128: One allowance of work for a frame and for a player step, and frames that say what they left out

* Status: accepted (2026-11-21)
* Relates to: `docs/roadmap.md` (P-002), ADR 0024 (the player), ADR 0125 (the backlog), outside
  pull request #13, AGENTS.md rule 15

## Context

A symbol may hold instances of other symbols, 32 deep (`MAX_SYMBOL_DEPTH`). That cap stops a
library that refers to itself in a cycle. It does not stop one that doubles: 32 symbols, each
holding two instances of the next, are 2^32 shapes. The editor can't build that by accident, but
a damaged or hostile `.anicraft` or `.fla` can hold it, and "never crash" covers such files.

Pull request #497 gave one evaluation, and one bounds query, an allowance of 1,000,000 element
visits (`MAX_EVAL_ELEMENTS`). Outside pull request #13 had proposed a budget first and went
further in what it counted. Looking at `main` after #497, four things were still open.

1. **Work that is not an element visit.** A timeline's layers are looked at on every visit of
   its symbol and cost nothing, so a symbol with thousands of empty layers multiplied by
   nesting. An outline of a million commands is measured once per bounds query of each instance.
   A symbol is found by walking the library, so a library of a hundred thousand symbols
   multiplied every lookup.
2. **The player's step.** Test Movie rebuilds the instance tree after every change in a step
   (up to 10,000 actions) and evaluates every interactive instance for every pointer event (up
   to 256), each with a whole frame's allowance of its own. The tree was capped at 10,000
   instances but looked at every plain element of every one of them for nothing.
3. **Nobody was told.** A frame cut short drew in part and looked like lost artwork. An export
   wrote the partial picture and reported success.
4. **Outside PR #13** conflicted with all of it and needed a decision.

## Decision

### 1. One unit of work, and what costs more than one

The allowance stays 1,000,000 units for one evaluation or one bounds query. A unit is one
element or one layer looked at. Two things cost more:

* a shape costs one more unit for every 256 commands of its outline
  (`PATH_COMMANDS_PER_UNIT`), where it is drawn and where it is measured;
* finding a symbol costs one more unit for every 1,024 library entries passed on the way
  (`LIBRARY_ENTRIES_PER_UNIT`).

An ordinary document pays nothing extra: outlines are tens of commands and libraries hundreds of
symbols, and the heaviest benchmark scene (50,000 shapes) uses a twentieth of the allowance.
Once the allowance is spent nothing more is taken from it, so the cut is clean: what was reached
is drawn, the rest is not.

#13 charged one unit for every library entry passed. That bounds the same hostile case, but an
honest document with 2,000 symbols and 5,000 instances would spend ten times the allowance on
lookups alone and lose artwork. Dividing by 1,024 keeps the bound (a lookup in a library of
100,000 symbols costs about 100 units, so at most 10,000 of them happen) and leaves honest
documents whole. A map from id to symbol would remove the cost altogether; it is a larger change
to the document model and is not needed for the bound.

### 2. A player step has an allowance of its own

`MAX_WORK_PER_STEP` is four frames' worth (4,000,000 units). Every rebuild of the instance
tree and every hit test in one `step`, `start` or `input` takes from it, in the same units: the
tree pays for each layer and each element it looks at, and hit tests evaluate from the step's
allowance instead of a fresh one each. When it is spent the step stops as it does when its
action or fuel budget is spent, with one line in the Output panel:

> Stopped this frame early: the movie holds more than one frame may look at, so some instances
> and buttons were left out (symbols nested in themselves?)

The queries the shell makes between steps (`tree`, `hit`, `field_at`, `hit_overlay`) each have
one frame's allowance for everything they do; `hit_overlay` used to give every button its own.

Four frames, not one, because an honest step does look at the movie several times: after a
`gotoAndPlay`, after a button changes state, for a pointer that moved. A movie that needs more
than four full passes in one step is one no machine plays in time.

### 3. A list that was cut short says so, and so does whoever shows or writes it

`DisplayList::left_out` is set by every evaluation whose allowance ran out, and kept when lists
are appended (onion skins, the hit-area overlay). It is a plain field of the result:
`evaluate` stays a pure function of the document and the frame.

* **The stage** says once, in the status bar, as an error that stays: "Some artwork on this
  frame is left out: it holds more than AniCraft draws at once. Nothing is lost. Look for
  symbols nested many times over." It says it again only after a frame that was whole. The
  check is skipped while playing, where it would cost an evaluation every frame; the frame
  playback stops on is checked.
* **Commands** that write frames still write them, and add `"left_out": true` and a
  `"warning"` to their result: `file.export_png`, `file.export_svg`,
  `file.export_png_sequence`, `file.export_svg_sequence`, `file.export_sprite_sheet`,
  `file.export_gif`, `file.export_apng`, `file.export_webp`, `file.export_video`. A result
  without the keys is a whole frame, so nothing that reads results today changes.
  `anicraft-cli info` reports it for the frame shown, and the MCP `render` tool says it in the
  picture's caption.
* An export is not refused. A partial picture with a warning is more use to someone repairing a
  damaged file than no picture, and the warning is in the result an agent or a script reads.

### 4. What was taken from #13, and what was not

Taken, as ideas, into the allowance `main` already had: counting layers, symbol lookups and
outline commands, and making exhaustion something a caller can see. Its author is credited in
`AUTHORS.md` and the changelog.

Not taken: its commits (they rewrite the same functions #497 rewrote, from a base two hundred
commits old); the per-entry lookup charge (above); a public budget type that callers pass in
(no caller outside `anicraft-doc` needs to share one, and the player does it inside the crate);
charging both keyframes of a tween before interpolating (the tweened elements are charged when
they are visited, outline included, which bounds how often the work repeats); and refusing
**Paste** at a point when the clipboard's bounds were cut short (the paste still lands, offset by
the bounds of what was reached, which is the same partial answer every other bounds query gives).

## Consequences

* A hostile document costs a bounded amount everywhere it is looked at: about 0.1 s for a frame
  or a bounds query and under 0.3 s for a whole player step in a debug build on the development
  machine (the tests that build such documents run in 0.07 s to 0.31 s).
* Ordinary documents are unchanged. Evaluating the 50,000-shape stress scene takes 11.7 ms a
  frame with the change and 11.9 ms without (release build, best of three; the machine's noise
  is larger than the difference).
* `DisplayList` has one more public field. Code that builds one field by field names it.
* Not yet: the same note from the Lottie and HTML exporters and from exporter plug-ins (they
  walk the document their own way), a notice inside the Test Movie window when the picture
  (and not the step) is cut short, and a count of how much was left out.
