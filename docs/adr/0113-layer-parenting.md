# ADR 0113: Layer parenting

* Status: accepted (2026-11-18)
* Relates to: ADR 0043, 0084, 0098 (bones), ADR 0027, 0082 (camera and layer depth), ADR 0016
  (motion tweens), ADR 0044 (selections across layers), ADR 0045 (editing symbols in place),
  ADR 0004 (destructive actions), user issue #388, roadmap M7

## Context

Animators coming from Animate rig cut-out characters with **layer parenting**: the forearm
layer follows the upper-arm layer, the hand follows the forearm, and each layer keeps its own
keyframes and tweens on the main timeline. It needs no symbol nesting and no armature, which
is why studios prefer it for dialogue animation (#388). AniCraft had bones (ADR 0043, 0098),
which pose instances from joint angles, but no way for one plain layer to follow another.

Behaviour was learned from Adobe's public help page on layer parenting and from the user's
description; nothing was copied.

Two things made this more than a transform multiply:

* `Layer.parent` already means "the folder or mask this layer sits in", so the rig link needs
  its own field;
* every command and tool treats an element's transform as stage space. Once a layer follows a
  parent, its stored transforms are in the parent's space, and all of that code would be wrong
  on such layers.

## Decision

1. **The model: `Layer.rig_parent: Option<LayerId>`** (`#[serde(default, skip_serializing_if =
   none)]`, so files without parenting are byte-for-byte as before and older files open). A
   layer names at most one parent layer on the same timeline. It works on a scene's timeline
   and inside a symbol's.

2. **The parent is the parent layer's lead object: the first (bottom-most) object on that
   layer at the frame**, as it shows there (tweened, on its motion guide, posed by its bones
   and springs). One rule for every kind of object; a layer in a rig normally holds one. On a
   frame where the parent layer is empty the lead is the identity, so the child sits in its
   grandparent's space (or the stage's). A hidden parent layer still leads.

3. **`evaluate` composes the parent's whole transform, purely.** A layer's *space* at a frame
   is `space(parent) · lead(parent).transform`; the identity without a parent
   (`Timeline::rig_spaces`). `eval_timeline`, the player's hit-testing walk and the HTML
   canvas export place a layer's artwork by `view · space` (the camera's per-layer view first,
   ADR 0082, so depth, attaching and the camera keep working). Position, rotation, scale and
   skew follow; **colour effects, filters, blending, opacity and visibility do not** (they
   stay each layer's own, as in Animate). Masks, guides and folders are untouched: they go by
   `Layer.parent`. Every export draws the evaluated list (or, for HTML, bakes each layer's
   space per frame as `r`), so all of them match the stage with baked transforms.
   * The child's keys are **in the parent object's space**, as in any scene graph. We
     considered storing stage-space keys plus a bind matrix; it evaluates the same but makes
     "what does this key mean" depend on a hidden matrix, and breaks down when the parent
     layer swaps its object.
   * Bounded: a chain is at most `MAX_RIG_DEPTH` = 64 layers; resolving is memoised per
     evaluation (each layer's space once); a cycle in a damaged file stops at the bound and
     non-finite spaces read as the identity.

4. **What can be linked.** Followers: normal and mask layers. Leaders: normal, mask and
   armature layers. Folders, guides and the camera hold no object to follow and no artwork
   that could; an armature layer is posed by its bones, in stage space, and can't follow (it
   can lead, so a prop layer can follow a rig's root). A layer can't follow itself or a layer
   that follows it. `Timeline::rig_check` is the one rule; `Timeline::repair_rig` (run with
   `repair_masks`, so on load and after every layer command) drops links that don't hold.

5. **Parenting keeps the layer where it is on stage at the playhead.** `layer.set_parent`
   multiplies every keyframe's objects by `new_space⁻¹ · old_space` taken **at the current
   frame**: one constant correction for the whole layer, one undo step. The layer is attached
   as it stands now and from then on moves rigidly with the parent, as After Effects and
   Animate users expect. (The alternative, correcting each keyframe by the spaces at its own
   frame, would keep old stage animation but make the result depend on where the keys are and
   still jump on held frames; it is not what "attach this to that" means.) Motion tweens are
   carried too: a move shifts their X/Y keys and path; a turn or scale rewrites the transform
   keys at each keyed frame, keeping whole turns of rotation (`MotionTween::place`).
   `keep: false` links without correcting. When the parent's object is scaled to nothing at
   the playhead there is no correction, and the command says so.

6. **Commands keep working in stage space, through one choke point.**
   `Session::editable_keyframe` is where every artwork edit at the playhead gets its keyframe.
   On a follower it hands the keyframe over with each object placed as the stage shows it
   (`space · own`); when the command is done, what it changed or added goes back into the
   layer's space (`space⁻¹ · shown`) and what it left alone gets its stored `Arc` back (no
   float drift, no un-sharing, no false "document changed"). What commands and the shell read
   (`Session::elements_on`, `selected_elements`, `visible_elements`) is placed the same way,
   and bounds and hit tests already come from the display list. So Move, Transform, the
   Properties fields, drawing, paste, auto-key in classic tweens and motion-tween property
   keys all mean stage space on a follower without knowing about parenting.
   * The space a command reads is the one from **before** it ran; what it stores is for the
     space **after** it. Selecting an arm and its hand and dragging them moves each once
     (`each_layer` runs parents before followers).
   * A link broken as a side effect (deleting the parent layer, turning it into a guide)
     leaves the follower where it showed at the playhead; it does not jump.
   * A parent scaled to nothing at the playhead refuses edits there with a message rather
     than storing a wrong transform.

7. **Commands** (namespace `layer`, which plug-ins may already call):
   `layer.set_parent {parent: id | null, layer?, layers?, keep?}`,
   `layer.remove_parent {layer?, layers?, keep?}` (Modify › Timeline › Remove Parent) and
   `layer.parents {frame?}` (each layer's parent, ancestors, children and space). All routine
   and undoable (ADR 0004 tier 3). `file.new_sample {which: "arm"}` opens an arm of three
   parented layers.

8. **The shell: a parenting view in the timeline.** A toggle beside the **Layers** heading
   (and View › Parenting View) adds a parent column to the layer list: each layer has a dot in
   its colour, joined to its parent's dot by a line, stepping in with each link like a tree.
   Drag from a dot to another layer to parent it (valid rows light up; a refused drop says
   why); click the dot, or focus it and press Enter or Space, for a menu with **No Parent**
   and every layer it could follow. The same menu is **Parent** in every layer's right-click
   menu, view open or not. The column is added to the layer list's width while open, so names
   keep their room. Each dot is a named button for assistive technology ("Parent of Hand:
   Forearm"). The toggle is a preference (view state), not a document edit.

## XFL

Not imported. XFL's `parentLayerIndex` is how a layer names the **folder or mask** it sits in,
and the importer already reads it as that. Adobe's public pages describe layer parenting as a
feature but not how an XFL file stores it (which attribute links the layers, and whether the
child's matrices are stored in the stage's space or the parent's, which decides where every
object lands). Without a public description we don't guess: documents import as before, with
parented layers as plain layers. Reported, as for the camera (ADR 0082).

## Consequences

* Documents without parenting evaluate exactly as before and allocate nothing extra
  (`has_rig` is one pass over the layers).
* Measured (release, 60 layers each with a classic tween, one object per layer): 13.4 µs per
  `evaluate` unparented, 25.5 µs with all 60 in one chain. A leader is evaluated twice (once
  for its lead transform, once to draw); an armature leader with springs pays its simulation
  twice.
* A follower's coordinates in the file, and in `doc.info`, are its parent's; commands and the
  editor show stage coordinates.

## Not yet

* **Changing the parent over time** (Animate lets a keyframe change the parent): one parent
  per layer for the whole timeline.
* **Edit Multiple Frames** on a follower edits other frames in the parent's space; the
  **motion path** overlay and **Asset Warp** pins of a follower are drawn in the parent's
  space, not on the stage; tools other than Selection and Free Transform are untested on a
  turned follower.
* A **guided** follower follows its motion guide in the parent's space (the guide rides with
  the parent, though the guide layer's artwork is drawn where it is).
* Armature layers as followers; picking which object of a parent layer leads; a visible
  hierarchy on the stage; parenting several selected layers from the view in one drag.
* XFL import of layer parenting (no public format description).

## Questions for the lead

* Is the constant correction at the playhead (decision 5) the behaviour we want for layers
  that already have animation, or should existing keys keep their stage positions key by key?
