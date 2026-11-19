# ADR 0117: User themes from a folder

* Status: accepted (2026-11-18)
* Relates to: ADR 0005 (UI guidelines), ADR 0008 (visual language), ADR 0022 and 0064 (high
  contrast, following the system), ADR 0077 (the light theme and the palette system), ADR 0094
  (portable mode), ADR 0004 (destructive actions); user issue #340; roadmap M8

## Context

The interface has three palettes over one set of roles in `src/theme.rs` (Graphite, Light, High
contrast). Every colour change meant editing that file and rebuilding. Issue #340 asks for themes
that install like plug-ins: drop a file in a folder, pick it in Preferences, edit it and reload,
with coloured icons and gradients for people who miss the tinted chrome of older animation tools.

The triage of #340 split it into four requests: themes from a folder, coloured and gradient
icons, distinct pressed and focus states, and bevel or gradient surfaces. This ADR decides the
first, and how far colour in icons and gradients go **within the palette**. It does not take on
new control states or surface styles: those change what every painter draws, not which colours it
draws with, and they are separate decisions.

Two things could go wrong with themes from files: a theme that makes the editor unreadable (and
then can't be switched off, because Preferences is unreadable too), and a format that lets a file
reach into individual widgets, after which no built-in change is safe.

## Decisions

1. **A theme is a small data file that names colours for the palette's roles.** JSON, one file
   per theme, in `themes` in the settings folder (the portable `data` folder in a portable copy,
   ADR 0094; `ANICRAFT_THEMES_DIR` points it elsewhere). JSON because the preferences, saved
   commands and documents already are, the reader is already a dependency, and unknown keys can be
   refused exactly.

   ```json
   {
     "anicraft_theme": 1,
     "name": "Ember",
     "author": "…",
     "base": "dark",
     "colors": { "panel": "#221a20", "accent": "#d6336c", "stage_shadow": "#00000096" }
   }
   ```

   * `anicraft_theme` is the format's version. A file with a higher number than the build reads
     is refused ("update AniCraft"), not half-read.
   * `base` is `dark`, `light` or `high-contrast` (default `dark`): the built-in palette the
     theme starts from. Roles the file doesn't name keep the base's colour, and what is not a
     colour comes from the base too: whether egui starts from its dark or light look, the title
     bar, control outlines and hairlines, corner radii, shadow strength.
   * `colors` maps **role names** (the fields of `theme::Tokens` that hold a colour, 50 today:
     `panel`, `text_dim`, `accent`, `span`, `tween`, `playhead`…) to `#rrggbb`, or `#rrggbbaa`
     for the four roles that are laid over others (`stage_shadow`, `stage_edge`, `cell_tint`,
     `icon_accent`). There is no key for a widget, a size, a font, a state or a file path: a
     theme can't address anything the built-in palettes can't.
   * The role list is one macro (`colour_roles!`) that generates the names, the reader and the
     writer; a test reads `Tokens` from its source and fails if a colour field is not in it.

2. **All or nothing.** A file is read whole and checked whole before any of it is used. An
   unknown key, an unknown role, a colour that isn't hex, a see-through colour on a solid role, a
   file over 64 KB or one that isn't JSON refuses the theme with a message that names the key or
   role. Nothing is skipped silently and nothing is applied in part: a typo in a role name is an
   error the author sees, not a colour that quietly stays the base's.

3. **Contrast is computed, and text below WCAG AA refuses the theme.** The pairs a palette draws
   together moved from the guidelines test into `theme::shortfalls`, so the built-in palettes and
   a user's theme are checked by the same table (WCAG 2.x relative luminance):
   * the built-ins against the house rules as before (body text 7:1 and so on);
   * a user's theme against WCAG AA: text 4.5:1 on every surface it is drawn on (`text`,
     `text_dim`, `text_faint`, `accent_text`, `error`, `ok` on panels, headers and fields; text on
     filled buttons; marks and letters on timeline blocks), icons 3:1, and the focus ring 3:1
     (1.4.11, 2.4.7). Any of these below its minimum **refuses** the theme, with the pair and the
     numbers: "text_dim on panel has a contrast of 2.47:1; it needs 4.5:1."
   * what doesn't stop the interface being read (a timeline block against the grid, a checkbox
     edge, waveforms) is **noted** under the theme in Preferences, not refused.
   * `base` must agree with the colours: light text on a `light` base is refused with the fix
     ("use "base": "dark""), because the base decides the egui look and the title bar.

   A refused theme is still listed, disabled, with its reason, so the author sees it where they
   look for it. If the theme in use stops passing (edited and reloaded, or deleted), the built-in
   palette it was based on shows and the status bar says why; the choice is kept, so mending the
   file and pressing Reload brings it back.

4. **Preferences › Appearance › Theme** lists Follow system, the three built-ins, then the
   user's themes by name (with author and file name), each a radio button applied live on click
   like the built-ins. Under them: **Open Themes Folder**, **Reload** and **Export Current
   Theme**. Export writes every role of the look in use to `my-theme.json` (numbered if taken) in
   the folder and lists it at once: the way to start a theme is to export one and change colours,
   not to read documentation. None of this is modal.

5. **Where the choice is kept.** `theme_file` (the file's name without `.json`) beside the
   existing `theme`, which then holds the base palette. An older build reads the base and shows a
   built-in look; a file name with a path separator is dropped. A theme chosen by hand is not
   overridden by the system's contrast setting, as Light chosen by hand isn't (ADR 0077).

6. **Icons take colour from one role: `icon_accent`.** The icons are single-colour outlines
   (Lucide's grid). A palette may set `icon_accent`, and then an icon's **closed shapes are
   filled with it under the outline**: two-tone icons, from the same artwork, at any scale. It is
   see-through in the built-ins (nothing is drawn; they are unchanged), may be translucent, and
   is checked: the outline must still reach 3:1 against the tint over the surface it sits on, at
   rest, under the pointer and on the active tool. The fill is drawn only under icons painted in
   the icon roles (`icon`, `icon_hot`, `on_active`); a disabled icon or a chevron drawn in a text
   colour stays an outline. One role, applied in `icons::paint`, so every icon follows it and no
   panel decides for itself.

   Multi-colour icon artwork and icon packs in a theme (#340, proposal 5) are **not** in this
   decision: they need a fuller SVG reader and a per-theme icon cache, and the MPL-2.0 rasteriser
   the issue mentions stays out under the permissive-only policy.

7. **Gradients exist where the palette has a role for one: `accent_end`.** The accent is drawn
   as a thin square bar wherever it marks structure: a section band's edge, a dock panel's and a
   tab strip's edge, the active tab's line, a picked row's edge. Those bars now go through one
   helper, `theme::accent_bar`, which draws `accent` flat, or a two-stop gradient from `accent`
   to `accent_end` along the bar when a palette sets it. The built-ins have `accent_end` equal to
   `accent` (flat, the same draw call as before), and a theme that changes only `accent` gets a
   flat bar in its accent, never a gradient to its base's. No widget builds a gradient of its
   own; a second gradient would be a second role and a second helper, decided the same way.
   Gradient or bevelled surfaces (#340, proposal 4) are not decided here.

8. **Tokens stay a plain reference.** `T.panel` is read tens of thousands of times a frame, from
   any thread. The built-ins are constants; a user's tokens are kept for the life of the program
   (a few hundred bytes for each distinct set ever applied, the same set kept once), the set in
   use sits behind a lock, and each thread remembers it until a counter says it changed, so a
   read takes no lock. No `unsafe`. Under test the look stays per thread, as before (ADR 0077).

9. **A theme is view state, so it has no engine command** (golden rule 2 leaves view state to
   the shell). What a script or an assistant needs is reachable without a window:
   `anicraft --check-theme <file>` prints whether a theme would be used, or why not, and exits 0
   or 1; `anicraft --theme <file>.json` uses a theme for one run (screenshots).

10. **Limits.** 64 KB a file, 200 themes listed (the rest counted and said), names and authors
    up to 64 characters without control characters, no subfolders read, no file written except
    by Export.

## Consequences

* A new colour role must be added to the `colour_roles!` list (a test fails until it is) and is
  then settable by themes with no further work; a new role that pairs with another needs a line
  in `theme::shortfalls`, as before.
* Built-in UI changes must hold under colours they didn't choose. The rules already forbid
  assuming a colour or a direction (ADR 0077); the contrast table is what guarantees the pairs.
  Check UI changes with a user theme as well as `--theme light`.
* The built-in palettes are pinned by a test over every role's bytes: this change altered none of
  them, and nothing alters one by accident.
* Reading the folder is synchronous at start and on Reload (200 small files at most).

## Not yet

* Theme packages with their own icons, multi-colour and gradient icon artwork (#340).
* Separate pressed and focus looks, and bevel or gradient surface styles (#340).
* Metrics in a theme (tool button size, bar thickness).
* Installing a theme by dropping it on the window, and removing one from Preferences (the folder
  is one click away).
* A file watcher: Reload is a button.
* Themes in the browser build (it has no folder).
* Following the system's accent colour; a light high-contrast palette (ADR 0077).

## Question for the lead

#340 stays open: this decides and builds the folder, the format, the checks and colour in icons
within the palette, but not icon packs, new control states or surface styles. If those are not
wanted at all, the issue can be closed against this ADR.
