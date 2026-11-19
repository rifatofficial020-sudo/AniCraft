# ADR 0099: Filter colour and stepped tracks, built-in presets, and filters on text

* Status: accepted (2026-11-16)
* Relates to: roadmap M5 (filters, last slice), ADR 0025 (bevel and gradient filters), ADR 0032
  (GPU filters), ADR 0056 (colour effect properties in motion tweens), ADR 0066 (dynamic and
  input text), ADR 0068 (brush presets: built-in ones and the user's), ADR 0072 (filters in the
  HTML page), ADR 0081 (classic tweens ease filters as a group), ADR 0091 (filter lists,
  presets and tweens), ADR 0092 (Motion Editor filter tracks), ADR 0004 (destructive actions),
  ADR 0009 and 0014 (XFL import)

## Context

ADR 0091 and 0092 left four things for later: filter **colours** as motion-tween tracks,
**quality and switches** as tracks, **built-in presets**, and **filters on text**. ADR 0091 also
made a classic tween hold the first keyframe's filters whenever the two lists differed, and
called that Animate's behaviour. It is not quite: Adobe's public help for animated filters says
that when the two keyframes' filters don't match, Animate pairs what matches and gives the
other keyframe a filter "that has no parameters" with the colour of the existing one, so the
unmatched filter fades in or out; and that Knockout, Inner and the Type are taken from the first
keyframe when they differ. Animate also offers filters on text as on movie clips and buttons.

Behaviour was learnt from Adobe's public help pages and by observation. No Adobe code, preset
or asset was used; the built-in presets' values are AniCraft's own.

## Decision

### 1. A filter's colour is four tracks, in the tint's units and space

`FilterParam` gains `Color(FilterColor, Channel)`:

* `FilterColor` is the filter's own field: `Color` (Drop Shadow, Glow), `Shadow` and
  `Highlight` (Bevel), and `Stop(n)` for a gradient filter's ramp stop by its place (0 to 14).
* `Channel` is red, green, blue (0–255) or alpha (percent): exactly the units of the colour
  effect's `tint_red`/`tint_green`/`tint_blue` and `alpha` (ADR 0056).

Each channel is a `Property` like any other (`filter.0.color_red`, `filter.1.shadow_color_alpha`,
`filter.0.stop_2_blue`), so it keys, eases, retimes, splits and joins with no new machinery,
and the files need nothing new (`filter_tracks`, ADR 0092). The colour therefore travels as a
tint travels: each channel of the straight sRGB colour on its own curve. With linear eases the
midpoint is `Rgba::lerp`'s, which is also what classic tweens give a filter colour; a test pins
the two together. One colour space for every colour that tweens was preferred over a better
one for filters only: an artist who matches a glow to a tint must see them stay matched.

A whole colour is addressed without a channel where that is the natural unit:
`motion.set_key {property: "filter.0.color", color: "#rrggbbaa"}` keys the four channels in one
undo step, `motion.remove_key` and `motion.set_ease` take the same name, and `motion.info`
reports `filter.0.color` as `#rrggbbaa` beside the channels.

Ramp stops are addressed by place. A stop the ramp doesn't have does nothing (as a track whose
filter went, ADR 0092). Stop *positions* are not tracks.

### 2. Quality and the switches are stepped tracks

`FilterParam::Quality` (1 to 3), `Inner`, `Knockout` and `HideObject` (0 or 1) are *stepped*
(`Property::is_stepped`): `Track::value_at` returns the key before the frame, never a value in
between, and exactly the next key's value from its frame on. Keys are whole numbers (rounded
when set and when a file is repaired). An ease on such a segment would do nothing, so
`motion.set_ease` refuses one named alone and skips them when easing every property. This is
Animate's rule (switches never interpolate) made explicit as hold keys, so an artist can turn
Knockout on at frame 20 instead of being limited to the first keyframe's setting.

The Type of a bevel or gradient filter (inner, outer, full) and a filter's on/off are not
tracks: they stay the object's own.

`Filter::params` now lists everything a tween can animate (numbers, colour channels, quality,
switches), so auto-key (ADR 0092 §5) and F6 cover colours and switches without change: editing
a glow's colour or ticking Knockout in Properties inside a tween keys just what changed.

### 3. Classic tweens pair lists that differ, and the rest fade

`lerp_filters` replaces ADR 0091 §6's "the first keyframe's filters hold":

* **Same kinds in the same order**: pairwise, as before. Numbers and colours travel; quality,
  switches, the Type and on/off stay the first keyframe's.
* **Otherwise** (`filter::pairing`): the longest run of kinds both lists hold in the same order
  is paired (a longest common subsequence over at most 64 entries a side; among equal runs the
  earliest filters pair). A filter with no partner travels to, or from, `Filter::neutral()`:
  itself with no blur, strength or distance (an Adjust Color at rest), keeping its colours,
  angle, quality and switches. So an added filter fades in, a removed one fades out, and a
  filter replaced by another kind cross-fades with it. The tween applies them in the merged
  order.
* **On the first keyframe itself** (`t` = 0) the result is exactly its own list, so what the
  artist authored is what shows, even when a fading-in filter has Hide Object or Knockout.

Deliberately not copied: Animate "first processes the clip with the most filters"; we pair by
order, which is deterministic and keeps filters the artist didn't touch paired.

### 4. Built-in presets: always there, hidden, never deleted

Eight presets written in `filter_library::builtins()` from the filters' own parameters: Soft
Shadow, Hard Shadow, Neon Glow, Outline, Emboss, Engrave, Soft Focus, Greyscale.

* They follow the brushes' rule (ADR 0068): a built-in's name can't be saved over, renamed to,
  renamed from or deleted; the engine refuses with the reason (never a confirmation, since
  nothing irreversible can happen). A preset in a file or in older preferences with a
  built-in's name is numbered ("Emboss 2").
* **Hidden, not removed**: `filter.preset_hide {name, hidden?}` takes one off the presets menu
  and puts it back. A hidden preset still applies by name, so scripts, the CLI, MCP and
  plug-ins that name one work on every computer. Hiding is reversible, so it asks nothing.
* What is hidden lives in the session's library (`FilterLibrary::hidden`) and in the
  preferences under a key of its own, `filter_presets_hidden`. The `filter_presets` list stays
  the plain list older builds read.
* `filter.preset_list` returns `builtin` (name, filters, hidden) beside `presets` (the user's),
  so existing callers see what they saw. Plug-ins may list and apply; they may not hide
  (`plugin_may_call`): the menu is the artist's.
* Names are English in commands and files; the interface translates them where it draws them.

### 5. Filters on text

`TextBlock` gains `filters: Vec<FilterEntry>` (left out of files when empty, so older files and
older builds are unaffected), on every text: static, dynamic and input, as Animate offers.

* `ElementKind::filters()`/`filters_mut()` give the one list of whatever takes filters; the
  motion tween, the filter commands, the Motion Editor and Properties go through them, so text
  gets the list, the order, on/off, copy and paste, presets, classic tweens and motion-tween
  tracks with no second implementation.
* `evaluate` draws text with active filters in a group of its own with the filters on it, as
  for a movie clip: a field's background, clipped text, selection, caret and border are
  filtered as one, and the text's alpha fades the group after the filters. Mask artwork takes
  none. So the CPU oracle, the GPU stage and every display-list export draw them **with no
  renderer change**; a GPU parity test covers filtered text all the same.
* The text commands that lay a block out again (`text.edit`, `text.field`, `text.path`, Find
  and Replace) carry the list over.
* The HTML page places elements itself: text items carry `fx`/`fm` like instances and the
  page's player opens the same group. An input field's typed text is a real form element (ADR
  0066), drawn by the browser, so only its box, border and authored text are filtered; the
  export report says so.
* XFL: a text field's `<filters>` are the records an instance holds, so they import with the
  same reader, switched-off ones switched off, instead of being reported as skipped.

### 6. Interface

* **Motion Editor**: after a filter's numbers, one row per colour (Colour, Shadow, Highlight,
  Stop 1…) with a swatch showing the colour at the playhead (picking a colour keys it there)
  and a diamond for its four keys. Clicking the row lists its Red, Green, Blue and Alpha rows,
  ordinary curves; while one is shown, a strip along the foot of the graph shows the whole
  colour over the span. Quality reads Low, Medium, High; switches are tick boxes; their curves
  are steps, without ease handles, and the footer says they hold.
* **Properties**: the Filters section appears for text. The presets menu lists the built-in
  presets, then the user's, and **Built-in Presets** ticks which ones the menu shows.
* All of it is named for screen readers ("Glow: Colour · Red") and translatable.

## Tests

* Doc: a colour's midpoint equal to `Rgba::lerp`'s and each channel on its own ease; stepped
  values held, exact at the key, whole after repair, kept by a split; every value's name
  reading back; pairing (added, removed, replaced, reordered, hostile lengths) and `neutral`;
  text drawn in a filtered group, its file round trip and old files; text tweening its filters.
* Engine, through the commands: a glow's colour to its midpoint in the display list and the
  pixels, equal to a still clip; a switch stepping exactly at its key in the pixels; auto-key
  of a colour, a switch and a ramp stop with one undo step; F6 and the document round trip;
  lists that differ fading in; the built-in presets present, applying, read-only and hidden;
  a text's drop shadow in the pixels, kept through edits; `panic_hunt_every_command` green.
* Render: the page's display list equal to `evaluate`'s for filtered text, groups opening and
  closing round it, the input-field note. XFL: a field's filters imported.
* GPU: filtered text against the oracle (max 1/255, mean 0.003–0.005 at three scales).
* Shell: the Motion Editor's rows and names; hidden presets through the preferences; the
  Filters row comparing text. Screenshots: `--open motioncolour`, `--open textshadow`.

## Consequences

* ADR 0091 §6 is superseded for lists that differ, and ADR 0092's "colours, quality and
  switches stay the object's own" no longer holds. A document whose classic tween crossed two
  different filter lists now fades between them instead of holding; nothing else changes in
  existing files.
* F6 inside a motion tween keys a filter's colours and switches too: up to 86 tracks for a
  filter with a full ramp. Bounded (16 slots), and only where the artist asked to pin a pose.
* Not yet: the Type and on/off as tracks; ramp stop positions as tracks; filter tracks from
  Animate's XFL motion tweens; a text field's typed text filtered in the HTML page; filters on
  text in the SVG importer; editing the built-in presets' values (save a copy under a name).
