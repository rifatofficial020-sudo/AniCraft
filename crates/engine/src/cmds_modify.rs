//! Clipboard (cut/copy/paste), transforms (free transform, rotate 90°, flip) and alignment.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session, TransformOrigin};
use anicraft_doc::{Element, ElementId, ElementKind, Layer, LayerId, LayerKind};
use anicraft_geom::{Affine, Rect, Vec2};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c("edit.cut", "Cut", "Edit", Some("Ctrl+X"), "{}", true, cut as crate::RunFn),
        c("edit.copy", "Copy", "Edit", Some("Ctrl+C"), "{} (the selection on every layer, each layer's artwork kept apart)", false, copy),
        c("edit.paste", "Paste in Center", "Edit", Some("Ctrl+V"), "{x?, y?, float?: true} (centre of the pasted artwork; default: offset 10 px). With float (as the editor pastes) raw shapes stay whole and selected on top, and go into the drawing when they are deselected or dropped after a move; without it they cut and join what they land on at once. Artwork copied from one layer lands on the active layer; from several, back on those layers when they are all here, else on the active layer and the ones below it, creating missing ones", true, paste),
        c("edit.paste_in_place", "Paste in Place", "Edit", Some("Ctrl+Shift+V"), "{float?: true} (layers and float as edit.paste)", true, paste_in_place),
        c("modify.transform", "Transform", "", None, "{a, b, c, d, e, f, origin?: selection|registration|center (default selection)} affine applied in stage space (free transform), to the selection on every layer. With origin registration or center, the matrix's scale, rotation and skew are applied to each selected object about its own point instead (see modify.transform_origin) and its move (e, f) is left out", true, transform),
        c("modify.rotate_cw", "Rotate 90° CW", "Modify/Transform", Some("Ctrl+Shift+9"), ABOUT_PARAMS, true, |s, p| about_center(s, p, Affine::rotate(std::f32::consts::FRAC_PI_2))),
        c("modify.rotate_ccw", "Rotate 90° CCW", "Modify/Transform", Some("Ctrl+Shift+7"), ABOUT_PARAMS, true, |s, p| about_center(s, p, Affine::rotate(-std::f32::consts::FRAC_PI_2))),
        c("modify.flip_h", "Flip Horizontal", "Modify/Transform", None, ABOUT_PARAMS, true, |s, p| about_center(s, p, Affine::scale(-1.0, 1.0))),
        c("modify.flip_v", "Flip Vertical", "Modify/Transform", None, ABOUT_PARAMS, true, |s, p| about_center(s, p, Affine::scale(1.0, -1.0))),
        c(
            "modify.transform_origin",
            "Next Transformation Origin",
            "Modify/Transform",
            None,
            "{mode?: selection|registration|center (default: the next one)} what several selected objects turn, scale and skew about with Free Transform and Modify › Transform: selection (one point for them all, as one arrangement; the default), registration (each about its own point: a symbol instance's or group's registration point, the centre of anything else) or center (each about the centre of its own box). Tool state, not part of the document → {mode}",
            false,
            transform_origin,
        ),
        c(
            "modify.transformation_point",
            "Set Transformation Point",
            "",
            None,
            "{x, y} (stage space, as modify.transform) moves the selected symbol instances' transformation point there (the white circle Free Transform shows): what each then turns, scales and skews about with Free Transform, the Modify › Transform commands, the Properties fields and in tweens. It is kept with the instance, in the instance's own space, so it travels with it. Anything else has no point of its own to keep (ADR 0111) → {set, x, y}",
            true,
            set_transformation_point,
        ),
        c(
            "modify.reset_transformation_point",
            "Reset Transformation Point",
            "Modify/Transform",
            None,
            "{} puts the selected symbol instances' transformation point back at the centre (none kept: tools use the centre of the box, tweens the registration point) → {reset}",
            true,
            reset_transformation_point,
        ),
        c("modify.remove_transform", "Remove Transform", "Modify/Transform", Some("Ctrl+Shift+Z"), "{}", true, |s, p| s.each_layer(true, |s| remove_transform(s, p))),
        c("align.left", "Left", "Modify/Align", Some("Ctrl+Alt+1"), "{to_stage?}", true, |s, p| align(s, p, Edge::Left)),
        c("align.hcenter", "Horizontal Center", "Modify/Align", Some("Ctrl+Alt+2"), "{to_stage?}", true, |s, p| align(s, p, Edge::HCenter)),
        c("align.right", "Right", "Modify/Align", Some("Ctrl+Alt+3"), "{to_stage?}", true, |s, p| align(s, p, Edge::Right)),
        c("align.top", "Top", "Modify/Align", Some("Ctrl+Alt+4"), "{to_stage?}", true, |s, p| align(s, p, Edge::Top)),
        c("align.vcenter", "Vertical Center", "Modify/Align", Some("Ctrl+Alt+5"), "{to_stage?}", true, |s, p| align(s, p, Edge::VCenter)),
        c("align.bottom", "Bottom", "Modify/Align", Some("Ctrl+Alt+6"), "{to_stage?}", true, |s, p| align(s, p, Edge::Bottom)),
        c("align.distribute_h", "Distribute Horizontal Centers", "Modify/Align", None, "{}", true, |s, _| distribute(s, true)),
        c("align.distribute_v", "Distribute Vertical Centers", "Modify/Align", None, "{}", true, |s, _| distribute(s, false)),
    ]
}

// ----- clipboard --------------------------------------------------------------------------------

/// Copied artwork: elements as they look at the playhead, kept apart by the layer they came from,
/// plus every Library item they need, so a paste into another document is complete (ADR 0112).
#[derive(Clone, Default)]
pub struct Clipboard {
    /// One entry per layer the selection was on, top to bottom.
    pub layers: Vec<ClipLayer>,
    /// The symbols, bitmaps and sounds the artwork uses, and the document it was copied in.
    pub carried: crate::transfer::Carried,
}

/// One layer's copied artwork.
#[derive(Clone)]
pub struct ClipLayer {
    /// The layer it came from, and its name (for a layer a paste has to create).
    pub layer: LayerId,
    pub name: String,
    pub elements: Vec<Element>,
    /// The keyframe (its first frame) it was copied from with Edit Multiple Frames (ADR 0051),
    /// where Paste in Place on Multiple Frames puts it back; `None` for a copy on one frame.
    pub frame: Option<u32>,
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.elements.is_empty())
    }
    /// Every copied element, layer by layer.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.layers.iter().flat_map(|l| l.elements.iter())
    }
    pub fn len(&self) -> usize {
        self.layers.iter().map(|l| l.elements.len()).sum()
    }
}

fn copy(s: &mut Session, _: &Value) -> Result<Value> {
    let layers = s.selected_layers();
    if layers.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    // What you see is what you copy: the evaluated state at the playhead (tweens applied).
    let mut copied = Vec::with_capacity(layers.len());
    for l in layers {
        let elements: Vec<Element> = s.elements_on(l).iter().filter(|e| s.selection.contains(l, e.id)).map(|e| (**e).clone()).collect();
        let name = s.timeline().layer(l).map(|x| x.name.clone()).unwrap_or_default();
        if !elements.is_empty() {
            copied.push(ClipLayer { layer: l, name, elements, frame: None });
        }
    }
    let all: Vec<Element> = copied.iter().flat_map(|c| c.elements.iter().cloned()).collect();
    let carried = crate::transfer::carry(s, &all, []);
    let n = all.len();
    let layers = copied.len();
    s.clipboard = Clipboard { layers: copied, carried };
    Ok(json!({"copied": n, "layers": layers}))
}

fn cut(s: &mut Session, p: &Value) -> Result<Value> {
    copy(s, p)?;
    crate::cmds_draw::delete_selection(s, p)?;
    Ok(json!({"cut": s.clipboard.len()}))
}

fn clipboard_bounds(s: &Session) -> Rect {
    s.clipboard.elements().fold(Rect::EMPTY, |r, e| r.union(&clip_element_bounds(s, e)))
}

fn clip_element_bounds(s: &Session, e: &Element) -> Rect {
    // Symbols may come from another document: look them up in the clipboard first.
    let mut doc = s.doc.clone();
    for sym in &s.clipboard.carried.symbols {
        if doc.symbol(sym.id).is_none() {
            doc.library.push(sym.clone());
        }
    }
    e.bounds(&doc)
}

/// The layers a paste puts each copied layer's artwork on (ADR 0044). One copied layer goes on
/// the active layer. Several go back on the layers they came from when every one of them is in
/// this scene (same id and name) and can be drawn on; else on the active layer and the drawable
/// layers below it, in order, with missing layers created at the bottom named after the copied
/// ones, as Paste Frames does.
fn paste_targets(s: &mut Session) -> Result<Vec<LayerId>> {
    // Each layer the artwork came from once (a copy with Edit Multiple Frames has one entry per
    // keyframe, ADR 0051), in order.
    let mut src: Vec<(LayerId, String)> = Vec::new();
    for c in &s.clipboard.layers {
        if !src.iter().any(|(l, _)| *l == c.layer) {
            src.push((c.layer, c.name.clone()));
        }
    }
    let dst = layer_targets(s, &src)?;
    Ok(s.clipboard.layers.iter().map(|c| src.iter().position(|(l, _)| *l == c.layer).and_then(|i| dst.get(i)).copied().unwrap_or(s.layer)).collect())
}

/// [`paste_targets`] for each source layer (its id and name).
fn layer_targets(s: &mut Session, src: &[(LayerId, String)]) -> Result<Vec<LayerId>> {
    let n = src.len();
    if n <= 1 {
        return Ok(vec![s.layer]);
    }
    let tl = s.timeline();
    let drawable = |s: &Session, l: &Layer| s.selectable(l.id) && !matches!(l.kind, LayerKind::Folder | LayerKind::Camera | LayerKind::Armature);
    let home: Vec<LayerId> = src.iter().filter_map(|(id, name)| tl.layer(*id).filter(|l| l.name == *name && drawable(s, l)).map(|l| l.id)).collect();
    if home.len() == n {
        return Ok(home);
    }
    let at = s.active_layer_index().unwrap_or(0);
    let mut out = vec![s.layer];
    out.extend(tl.layers.iter().skip(at + 1).filter(|l| drawable(s, l)).map(|l| l.id).take(n - 1));
    let names: Vec<String> = src.iter().skip(out.len()).map(|(_, name)| name.clone()).collect();
    for name in names {
        if s.timeline().layers.len() >= 1000 {
            return Err(EngineError::State("too many layers (limit 1000)".into()));
        }
        let id = LayerId(s.doc.alloc_id());
        let count = s.timeline().layers.len();
        let mut layer = Layer::new(id, name);
        if let Some(k) = layer.keyframe_mut(0) {
            k.duration = s.frame_count().max(1);
        }
        layer.color = crate::cmds_timeline::layer_color(count + 1);
        let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
        tl.layers.push(Arc::new(layer));
        tl.repair_masks();
        out.push(id);
    }
    Ok(out)
}

/// Whether pasting the clipboard would put a symbol inside itself: inside an open symbol, an
/// instance of it (or of one using it).
pub(crate) fn clipboard_nests(s: &Session) -> bool {
    // (Symbols from another document come in as new ones: they can't hold the open one.)
    s.clipboard.carried.origin == s.origin
        && s.clipboard.layers.iter().flat_map(|c| &c.elements).any(|e| matches!(e.kind, anicraft_doc::ElementKind::Symbol { symbol, .. } if crate::editing::would_nest(s, symbol)))
}

fn paste_with(s: &mut Session, offset: Vec2, float: bool) -> Result<Value> {
    if s.clipboard.is_empty() {
        return Err(EngineError::State("the clipboard is empty".into()));
    }
    if clipboard_nests(s) {
        return Err(EngineError::State("a symbol can't be pasted inside itself".into()));
    }
    let map = crate::transfer::bring(s, &s.clipboard.carried.clone());
    let targets = paste_targets(s)?;
    let copied = s.clipboard.layers.clone();
    let mut sel = crate::Selection::default();
    let mut all = Vec::new();
    let mut merged = false;
    let mut floating = Vec::new();
    for (c, &layer) in copied.iter().zip(&targets) {
        let (fresh, m) = s.with_layer(layer, |s| paste_layer(s, &c.elements, offset, float, &map))?;
        merged |= m;
        if float {
            // The raw shapes among them: every pasted element that can merge.
            floating.extend(c.elements.iter().zip(&fresh).filter(|(e, _)| crate::merge::mergeable(e).is_some()).map(|(_, &id)| (layer, id)));
        }
        all.extend(fresh.iter().map(|e| e.0));
        sel.extend_on(layer, fresh);
    }
    s.selection = sel;
    // Floating shapes wait for the selection to let go of them (`Session::settle_floating`).
    s.floating = (!floating.is_empty()).then_some(crate::Floating { items: floating, frame: s.frame });
    let mut layers: Vec<LayerId> = Vec::new();
    for l in &targets {
        if !layers.contains(l) {
            layers.push(*l);
        }
    }
    if let Some(&first) = layers.first()
        && layers.len() > 1
    {
        s.layer = first;
    }
    let mut v = json!({"pasted": all, "merged": merged});
    if s.floating.is_some() {
        v["floating"] = json!(true);
    }
    if layers.len() > 1 {
        v["layers"] = json!(layers.iter().map(|l| l.0).collect::<Vec<_>>());
    }
    Ok(v)
}

/// Pastes `elements` moved by `offset` into the active layer's keyframe at the playhead; their
/// new ids, and whether raw shapes merged. With `float` raw shapes go on top whole, like
/// objects, so the new ids are in the order of `elements`.
/// `map`: what the Library items they use are called in this document (ADR 0112).
pub(crate) fn paste_layer(s: &mut Session, elements: &[Element], offset: Vec2, float: bool, map: &crate::transfer::IdMap) -> Result<(Vec<ElementId>, bool)> {
    let mut fresh = Vec::new();
    let mut out = Vec::new();
    let mut raw = Vec::new();
    for e in elements.iter().cloned() {
        let mut e = e;
        e.id = ElementId(s.doc.alloc_id());
        map.element(&mut e);
        e.transform = Affine::translate(offset.x, offset.y).mul(&e.transform);
        // Raw shapes go into the drawing where they land, cutting and joining what they touch
        // (ADR 0042); objects go on top.
        if !float && crate::merge::mergeable(&e).is_some() {
            raw.push(e);
        } else {
            fresh.push(e.id);
            out.push(Arc::new(e));
        }
    }
    let kf = s.editable_keyframe()?;
    if kf.elements.len() + out.len() + raw.len() > 100_000 {
        return Err(EngineError::State("too many elements on this keyframe".into()));
    }
    kf.elements.extend(out);
    let merged = !raw.is_empty();
    if merged {
        fresh.extend(crate::merge::merge_in(s, raw)?);
    }
    Ok((fresh, merged))
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    let offset = match (opt_f32(p, "x")?, opt_f32(p, "y")?) {
        (Some(x), Some(y)) => {
            let b = clipboard_bounds(s);
            if b.is_empty() { Vec2::new(10.0, 10.0) } else { Vec2::new(x, y) - b.center() }
        }
        _ => Vec2::new(10.0, 10.0),
    };
    paste_with(s, offset, opt_bool(p, "float")?.unwrap_or(false))
}

fn paste_in_place(s: &mut Session, p: &Value) -> Result<Value> {
    paste_with(s, Vec2::ZERO, opt_bool(p, "float")?.unwrap_or(false))
}

// ----- transforms -------------------------------------------------------------------------------

/// Applies `m` (stage space) to every selected element: `t' = m · t`.
pub(crate) fn apply(s: &mut Session, m: &Affine) -> Result<usize> {
    apply_each(s, |_| *m)
}

/// Applies a stage-space transform of its own to every selected element on the active layer.
fn apply_each(s: &mut Session, of: impl Fn(ElementId) -> Affine) -> Result<usize> {
    let els = s.selected_elements_mut()?;
    let n = els.len();
    for e in els {
        let m = of(e.id);
        if !m.is_finite() || m.determinant().abs() < 1e-8 {
            return Err(EngineError::Param("the transform is degenerate (it would collapse the artwork)".into()));
        }
        let t = m.mul(&e.transform);
        let d = t.decompose();
        if !t.is_finite() || d.sx.abs() > 10_000.0 || d.sy.abs() > 10_000.0 || d.sx.abs() < 1e-4 || d.sy.abs() < 1e-4 {
            return Err(EngineError::Param("the transform would make an element too large or too small".into()));
        }
        e.transform = t;
    }
    Ok(n)
}

fn transform(s: &mut Session, p: &Value) -> Result<Value> {
    let m = Affine { m: [f32_req(p, "a")?, f32_req(p, "b")?, f32_req(p, "c")?, f32_req(p, "d")?, f32_req(p, "e")?, f32_req(p, "f")?] };
    match origin_param(p)?.unwrap_or(TransformOrigin::Selection) {
        TransformOrigin::Selection => s.each_layer(true, |s| Ok(json!({"transformed": apply(s, &m)?}))),
        mode => about_own(s, &m, mode),
    }
}

/// Remove Transform: scale, rotation and skew go back to none; each element keeps its
/// position (registration point).
fn remove_transform(s: &mut Session, _: &Value) -> Result<Value> {
    let els = s.selected_elements_mut()?;
    let n = els.len();
    for e in els {
        let t = e.transform.translation();
        e.transform = Affine::translate(t.x, t.y);
    }
    Ok(json!({"reset": n}))
}

/// What the Modify › Transform commands take.
const ABOUT_PARAMS: &str = "{origin?: selection|registration|center (default: the mode set with modify.transform_origin; selection unless changed)} about the centre of the whole selection, or each selected object about its own point";

/// Param `origin`.
fn origin_param(p: &Value) -> Result<Option<TransformOrigin>> {
    match opt_str(p, "origin")? {
        None => Ok(None),
        Some(key) => TransformOrigin::from_key(key).map(Some).ok_or_else(|| EngineError::Param(format!("param 'origin' must be selection, registration or center, not '{key}'"))),
    }
}

/// `m` about the centre of the whole selection's bounds, on every layer it is on, or (by the
/// transformation origin, ADR 0109) its scale, rotation and skew on each object about its own
/// point.
fn about_center(s: &mut Session, p: &Value, m: Affine) -> Result<Value> {
    let mode = origin_param(p)?.unwrap_or(s.style.transform_origin);
    if mode != TransformOrigin::Selection {
        return about_own(s, &m, mode);
    }
    let b = s.selection_bounds();
    if b.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    // One instance with a transformation point of its own turns about it (ADR 0111).
    let c = s.transformation_point().filter(|(_, own)| *own).map_or(b.center(), |(at, _)| at);
    let full = Affine::translate(c.x, c.y).mul(&m).mul(&Affine::translate(-c.x, -c.y));
    s.each_layer(true, |s| Ok(json!({"transformed": apply(s, &full)?})))
}

/// `modify.transformation_point`: the selected instances' point, given in stage space, kept in
/// each instance's own.
fn set_transformation_point(s: &mut Session, p: &Value) -> Result<Value> {
    let at = Vec2::new(f32_req(p, "x")?, f32_req(p, "y")?);
    if !at.is_finite() {
        return Err(EngineError::Param("params 'x' and 'y' must be numbers".into()));
    }
    change_transformation_point(s, Some(at)).map(|n| json!({"set": n, "x": at.x, "y": at.y}))
}

fn reset_transformation_point(s: &mut Session, _: &Value) -> Result<Value> {
    change_transformation_point(s, None).map(|n| json!({"reset": n}))
}

/// Sets (stage space) or clears the transformation point of every selected instance, on every
/// layer the selection is on; answers how many.
fn change_transformation_point(s: &mut Session, at: Option<Vec2>) -> Result<usize> {
    let keeps = |e: &Element| matches!(e.kind, ElementKind::Symbol { .. });
    if s.selection.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    if !s.selected_elements().iter().any(|(_, e)| keeps(e)) {
        return Err(EngineError::State("only a symbol instance keeps a transformation point: select one (Modify › Convert to Symbol makes one)".into()));
    }
    let mut n = 0;
    s.each_layer(false, |s| {
        if !s.selected_elements().iter().any(|(_, e)| keeps(e)) {
            return Ok(Value::Null);
        }
        for e in s.selected_elements_mut()?.into_iter().filter(|e| keeps(e)) {
            let own = match at {
                None => None,
                Some(at) => {
                    let inv = e.transform.inverse().ok_or_else(|| EngineError::State("the instance is flattened to nothing: it has no point to place".into()))?;
                    let own = inv.apply(at);
                    if !own.is_finite() || own.x.abs() > anicraft_doc::MAX_PIVOT || own.y.abs() > anicraft_doc::MAX_PIVOT {
                        return Err(EngineError::Param("the point is too far from the instance".into()));
                    }
                    Some(own)
                }
            };
            e.set_pivot(own);
            n += 1;
        }
        Ok(Value::Null)
    })?;
    Ok(n)
}

impl Session {
    /// The point Free Transform shows and turns the selection about (stage space), and whether
    /// it is one the selected instance keeps (ADR 0111): a single selected element's own
    /// transformation point when it has one, else the centre of its box (which its transform
    /// carries); for several elements, the centre of the selection's bounds. `None` with
    /// nothing selected.
    pub fn transformation_point(&self) -> Option<(Vec2, bool)> {
        let (frame, bounds) = self.transform_frame()?;
        if let [(_, e)] = self.selected_elements().as_slice()
            && let Some(p) = e.pivot()
        {
            let at = e.transform.apply(p);
            if at.is_finite() {
                return Some((at, true));
            }
        }
        Some((frame.apply(bounds.center()), false))
    }

    /// The selection's box as Free Transform draws it (ADR 0111): a box and the transform that
    /// places it on the stage. One selected element shows its own box, turned, scaled and
    /// skewed as it is (the box of what it draws at the playhead, in its own space, under its
    /// transform); anything else (several elements, raw pieces, a flattened element) the
    /// upright bounds of the selection under the identity. `None` with nothing to show.
    pub fn transform_frame(&self) -> Option<(Affine, Rect)> {
        let selected = self.selected_elements();
        if let [(layer, e)] = selected.as_slice()
            && !matches!(e.kind, ElementKind::Camera)
            && let Some(inv) = e.transform.inverse()
        {
            let mut own = Rect::EMPTY;
            for item in self.display_list().items.iter().filter(|i| i.layer == *layer && i.element == e.id) {
                let pad = item.stroke.as_ref().map_or(0.0, anicraft_doc::Stroke::reach);
                own = own.union(&item.path.bounds().inflate(pad).transformed(&inv.mul(&item.transform)));
            }
            if !own.is_empty() && own.min.is_finite() && own.max.is_finite() {
                return Some((e.transform, own));
            }
        }
        let b = self.selection_bounds();
        (!b.is_empty()).then_some((Affine::IDENTITY, b))
    }
}

/// `modify.transform_origin`: sets the mode, or moves to the next one.
fn transform_origin(s: &mut Session, p: &Value) -> Result<Value> {
    let mode = match opt_str(p, "mode")? {
        None => s.style.transform_origin.next(),
        Some(key) => TransformOrigin::from_key(key).ok_or_else(|| EngineError::Param(format!("param 'mode' must be selection, registration or center, not '{key}'")))?,
    };
    s.style.transform_origin = mode;
    Ok(json!({"mode": mode.key()}))
}

/// The point each selected object is transformed about in `mode`, with the elements that go
/// with it (ADR 0109). An object is one element; the selected raw pieces of one drawing (a
/// fill and the lines on its edge, ADR 0042) are one object, so a drawing keeps its outline.
///
/// * A symbol instance's or a group's own point is its registration point (its origin).
/// * The centre is the centre of the object's own box: of its artwork in its own space,
///   carried by its transform. Turning or scaling an object about that point leaves the point
///   where it is, so a drag applied a little at a time ends where one transform would.
/// * Anything without a registration point of its own (shapes, drawings, bitmaps, text) is
///   transformed about its centre in both modes.
pub fn transform_origins(s: &Session, mode: TransformOrigin) -> Vec<(Vec<Item>, Vec2)> {
    let els: std::collections::HashMap<Item, Arc<Element>> = s.selected_elements().into_iter().map(|(l, e)| ((l, e.id), e)).collect();
    selected_units(s)
        .into_iter()
        .map(|(items, bounds)| {
            let members: Vec<&Arc<Element>> = items.iter().filter_map(|i| els.get(i)).collect();
            let at = match members.as_slice() {
                [e] if mode == TransformOrigin::Registration && matches!(e.kind, ElementKind::Symbol { .. } | ElementKind::Group { .. }) => e.transform.translation(),
                // The box of the object's artwork in its first element's own space (one
                // element: its own box), which the transform carries along.
                [first, ..] => match first.transform.inverse() {
                    Some(inv) => {
                        let own = members.iter().fold(Rect::EMPTY, |r, e| r.union(&e.local_bounds(&s.doc).transformed(&inv.mul(&e.transform))));
                        if own.is_empty() { bounds.center() } else { first.transform.apply(own.center()) }
                    }
                    None => bounds.center(),
                },
                [] => bounds.center(),
            };
            (items, at)
        })
        .collect()
}

/// The scale, rotation and skew of `m` applied to each selected object about its own point.
fn about_own(s: &mut Session, m: &Affine, mode: TransformOrigin) -> Result<Value> {
    let [a, b, c, d, _, _] = m.m;
    let linear = Affine { m: [a, b, c, d, 0.0, 0.0] };
    let mut by: std::collections::HashMap<Item, Affine> = std::collections::HashMap::new();
    for (items, at) in transform_origins(s, mode) {
        let full = Affine::translate(at.x, at.y).mul(&linear).mul(&Affine::translate(-at.x, -at.y));
        by.extend(items.into_iter().map(|i| (i, full)));
    }
    if by.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    s.each_layer(true, |s| {
        let layer = s.layer;
        Ok(json!({"transformed": apply_each(s, |id| by.get(&(layer, id)).copied().unwrap_or(Affine::IDENTITY))?}))
    })
}

// ----- align ------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Edge {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

/// A selected element: its layer and id.
pub type Item = (LayerId, ElementId);

/// What align and distribute place: the elements of one unit (see [`selected_units`]) and their
/// combined bounds.
type Unit = (Vec<Item>, Rect);

fn moves_for(units: &[Unit], target: Rect, edge: Edge) -> Vec<(Item, Vec2)> {
    units
        .iter()
        .flat_map(|(items, b)| {
            let d = match edge {
                Edge::Left => Vec2::new(target.min.x - b.min.x, 0.0),
                Edge::HCenter => Vec2::new(target.center().x - b.center().x, 0.0),
                Edge::Right => Vec2::new(target.max.x - b.max.x, 0.0),
                Edge::Top => Vec2::new(0.0, target.min.y - b.min.y),
                Edge::VCenter => Vec2::new(0.0, target.center().y - b.center().y),
                Edge::Bottom => Vec2::new(0.0, target.max.y - b.max.y),
            };
            items.iter().map(move |id| (*id, d))
        })
        .collect()
}

/// Moves each element by its own amount, on its own layer.
fn translate_each(s: &mut Session, moves: &[(Item, Vec2)]) -> Result<usize> {
    let mut layers: Vec<LayerId> = Vec::new();
    for ((l, _), _) in moves {
        if !layers.contains(l) {
            layers.push(*l);
        }
    }
    let mut n = 0;
    for layer in layers {
        let mine: Vec<(ElementId, Vec2)> = moves.iter().filter(|((l, _), d)| *l == layer && (d.x != 0.0 || d.y != 0.0)).map(|((_, id), d)| (*id, *d)).collect();
        if mine.is_empty() {
            continue; // untouched layers stay shared with the undo snapshot
        }
        n += s.with_layer(layer, |s| -> Result<usize> {
            let kf = s.editable_keyframe()?;
            let mut n = 0;
            for (id, d) in &mine {
                for e in kf.elements_mut_where(|e| e == *id) {
                    e.transform = Affine::translate(d.x, d.y).mul(&e.transform);
                    n += 1;
                }
            }
            Ok(n)
        })?;
    }
    Ok(n)
}

/// What align and distribute place, over every layer (alignment works across layers): each
/// selected object on its own, and each raw drawing as one (#301). A raw drawing is the selected
/// raw pieces on a layer that touch, such as a fill and the strokes on its edges (ADR 0042), so a
/// shape keeps its outline where it goes.
fn selected_units(s: &Session) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    let mut raw: Vec<(LayerId, Vec<Item>, Vec<Rect>)> = Vec::new();
    for (l, e) in s.selected_elements() {
        let b = s.element_bounds_on(l, e.id);
        if b.is_empty() {
            continue;
        }
        if crate::merge::mergeable(&e).is_none() {
            units.push((vec![(l, e.id)], b));
            continue;
        }
        match raw.iter_mut().find(|(k, ..)| *k == l) {
            Some((_, ids, boxes)) => {
                ids.push((l, e.id));
                boxes.push(b);
            }
            None => raw.push((l, vec![(l, e.id)], vec![b])),
        }
    }
    for (_, ids, boxes) in raw {
        for group in crate::merge::touching_groups(&boxes) {
            let items: Vec<Item> = group.iter().filter_map(|&i| ids.get(i).copied()).collect();
            let b = group.iter().filter_map(|&i| boxes.get(i)).fold(Rect::EMPTY, |r, b| r.union(b));
            units.push((items, b));
        }
    }
    units
}

fn align(s: &mut Session, p: &Value, edge: Edge) -> Result<Value> {
    let units = selected_units(s);
    if units.is_empty() {
        return Err(EngineError::State("nothing is selected".into()));
    }
    // One element (or one raw drawing) aligns to the stage: there is nothing else to align it with.
    let to_stage = opt_bool(p, "to_stage")?.unwrap_or(units.len() == 1);
    let target = if to_stage {
        Rect::new(0.0, 0.0, s.doc.stage.width as f32, s.doc.stage.height as f32)
    } else {
        units.iter().fold(Rect::EMPTY, |r, (_, b)| r.union(b))
    };
    let moves = moves_for(&units, target, edge);
    let n = translate_each(s, &moves)?;
    Ok(json!({"moved": n, "to_stage": to_stage}))
}

fn distribute(s: &mut Session, horizontal: bool) -> Result<Value> {
    let mut units = selected_units(s);
    if units.len() < 3 {
        return Err(EngineError::State("select at least three elements to distribute".into()));
    }
    let key = |b: &Rect| if horizontal { b.center().x } else { b.center().y };
    units.sort_by(|a, b| key(&a.1).total_cmp(&key(&b.1)));
    let (Some(first), Some(last)) = (units.first(), units.last()) else { return Ok(json!({"moved": 0})) };
    let (a, z) = (key(&first.1), key(&last.1));
    let step = (z - a) / (units.len() - 1) as f32;
    let moves: Vec<(Item, Vec2)> = units
        .iter()
        .enumerate()
        .flat_map(|(i, (items, b))| {
            let d = a + step * i as f32 - key(b);
            let d = if horizontal { Vec2::new(d, 0.0) } else { Vec2::new(0.0, d) };
            items.iter().map(move |id| (*id, d))
        })
        .collect();
    let n = translate_each(s, &moves)?;
    Ok(json!({"moved": n}))
}
