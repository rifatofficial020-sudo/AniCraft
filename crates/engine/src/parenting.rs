//! Layer parenting (ADR 0113): the commands, and editing a layer that follows a parent.
//!
//! A layer with a parent layer keeps its artwork in the space of the parent's lead object, so
//! the document's coordinates there are not the stage's. Commands and tools keep working in
//! **stage space** all the same, through one choke point:
//!
//! * [`Session::editable_keyframe`] hands a command the keyframe with its artwork placed as the
//!   stage shows it (`space · own`), and remembers what it handed out;
//! * when the command is done ([`Session::settle_rig`]), whatever it changed or added goes back
//!   into the layer's space (`space⁻¹ · shown`), and whatever it left alone gets its exact
//!   stored transform back (no drift, no un-sharing);
//! * what commands and the shell read ([`Session::elements_on`], [`Session::visible_elements`])
//!   is placed the same way.
//!
//! The space a command sees is the one from **before** it ran, and what it stores is for the
//! space **after** it, so moving a parent and its child together moves each once.
//!
//! A link that a command breaks as a side effect (deleting the parent layer, turning it into
//! a guide) leaves the child where it shows at the playhead ([`Session::rig_end`]).

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{Element, Keyframe, LayerId};
use anicraft_geom::Affine;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c(
            "layer.set_parent",
            "Parent Layer",
            "",
            None,
            "{parent: layer id | null (null removes the parent), layer? (default: the active layer), layers?: [ids] (several at once), keep?: bool (default true)} layer parenting (ADR 0113): the layer's artwork follows the first object on the parent layer at every frame (position, rotation, scale, skew; not colour or filters), while keeping its own keys and tweens. With `keep` the layer stays where it is on stage at the playhead: every key is re-expressed for the new parent, in one undo step. A layer can't follow itself or a layer that follows it; folders, guides, the camera and armature layers can't follow, and folders, guides and the camera can't lead",
            true,
            set_parent as crate::RunFn,
        ),
        c("layer.remove_parent", "Remove Parent", "Modify/Timeline", None, "{layer? (default: the active layer), layers?: [ids], keep?: bool (default true)} the layer stops following its parent layer and stays where it is on stage at the playhead (layer.set_parent with parent: null)", true, remove_parent),
        c("layer.parents", "Layer Parents", "", None, "{frame?} → {layers: [{layer, name, parent, parent_name, ancestors: [ids, nearest first], children: [ids], can_follow, can_lead, space: [a, b, c, d, tx, ty] (the layer's own coordinates → the stage's at the frame)}]} top to bottom", false, parents),
    ]
}

/// The layers a parenting command works on: `layers`, else `layer`, else the active one.
fn targets(s: &Session, p: &Value) -> Result<Vec<LayerId>> {
    let ids: Vec<LayerId> = match opt_u64_list(p, "layers")? {
        Some(list) if !list.is_empty() => list.into_iter().map(LayerId).collect(),
        _ => vec![opt_u64(p, "layer")?.map_or(s.layer, LayerId)],
    };
    if ids.len() > 1000 {
        return Err(EngineError::Param("too many layers (limit 1000)".into()));
    }
    for id in &ids {
        if s.timeline().layer(*id).is_none() {
            return Err(EngineError::State(format!("no layer with id {}", id.0)));
        }
    }
    Ok(ids)
}

fn set_parent(s: &mut Session, p: &Value) -> Result<Value> {
    let parent = match p.get("parent") {
        None => return Err(EngineError::Param("param 'parent' is required: a layer id, or null to remove the parent".into())),
        Some(Value::Null) => None,
        Some(v) => Some(LayerId(v.as_u64().ok_or_else(|| EngineError::Param("param 'parent' must be a layer id or null".into()))?)),
    };
    let keep = opt_bool(p, "keep")?.unwrap_or(true);
    let ids = targets(s, p)?;
    link(s, &ids, parent, keep)
}

fn remove_parent(s: &mut Session, p: &Value) -> Result<Value> {
    let keep = opt_bool(p, "keep")?.unwrap_or(true);
    let ids = targets(s, p)?;
    link(s, &ids, None, keep)
}

/// Gives each of `ids` the parent layer `parent` (or none), keeping it in place at the playhead
/// when `keep`.
fn link(s: &mut Session, ids: &[LayerId], parent: Option<LayerId>, keep: bool) -> Result<Value> {
    let (frame, fps) = (s.frame, s.doc.stage.fps);
    let mut changed = Vec::new();
    for &id in ids {
        let tl = s.timeline();
        let name = |l: LayerId| tl.layer(l).map_or_else(String::new, |x| x.name.clone());
        if tl.layer(id).is_some_and(|l| l.rig_parent == parent) {
            continue;
        }
        if let Some(par) = parent {
            tl.rig_check(id, par).map_err(|e| EngineError::State(format!("'{}' can't follow '{}': {e}", name(id), name(par))))?;
        }
        let place = if keep {
            let old = tl.rig_space(id, frame, fps);
            let new = parent.map_or(Affine::IDENTITY, |par| tl.rig_child_space(par, frame, fps));
            let back = new.inverse().ok_or_else(|| EngineError::State(format!("the object on '{}' is scaled to nothing at this frame, so nothing can be placed by it: parent the layer at a frame where it shows", parent.map(name).unwrap_or_default())))?;
            back.mul(&old)
        } else {
            Affine::IDENTITY
        };
        let layer = s.layer_mut(Some(id))?;
        layer.rig_parent = parent;
        layer.place_all(&place);
        s.rig_handled.push(id);
        changed.push(id.0);
    }
    Ok(json!({"layers": changed, "parent": parent.map(|p| p.0), "kept": keep}))
}

fn parents(s: &mut Session, p: &Value) -> Result<Value> {
    let frame = opt_u32(p, "frame")?.unwrap_or(s.frame).min(crate::MAX_FRAME);
    let tl = s.timeline();
    let spaces = tl.rig_spaces(frame, s.doc.stage.fps);
    let rows: Vec<Value> = tl
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let space = spaces.get(i).copied().unwrap_or(Affine::IDENTITY);
            json!({
                "layer": l.id.0, "name": l.name,
                "parent": l.rig_parent.map(|p| p.0),
                "parent_name": l.rig_parent.and_then(|p| tl.layer(p)).map(|x| x.name.clone()),
                "ancestors": tl.rig_ancestors(l.id).iter().map(|a| a.0).collect::<Vec<_>>(),
                "children": tl.rig_children(l.id).iter().map(|a| a.0).collect::<Vec<_>>(),
                "can_follow": l.can_follow(), "can_lead": l.can_lead(),
                "space": space.m.to_vec(),
            })
        })
        .collect();
    Ok(json!({"layers": rows}))
}

/// The layer spaces a command started with (see the module notes).
#[derive(Clone, Debug)]
pub(crate) struct RigCmd {
    scene: usize,
    depth: usize,
    frame: u32,
    /// Every layer of the timeline being edited: its parent layer and its space then.
    layers: Vec<(LayerId, Option<LayerId>, Affine)>,
}

/// A keyframe handed to a command in stage space.
#[derive(Clone, Debug)]
pub(crate) struct RigEdit {
    layer: LayerId,
    /// The keyframe's first frame, and the playhead then.
    start: u32,
    frame: u32,
    /// Each object as stored, and as it was handed out.
    shown: Vec<(Arc<Element>, Element)>,
}

/// What [`Session::rig_begin`] hands to [`Session::rig_end`]: the enclosing command's state.
pub(crate) struct RigOuter(Option<RigCmd>, Vec<RigEdit>, Vec<LayerId>);

impl Session {
    /// A command starts: the layer spaces it sees are the ones of this moment.
    pub(crate) fn rig_begin(&mut self) -> RigOuter {
        let outer = RigOuter(self.rig_cmd.take(), std::mem::take(&mut self.rig_edits), std::mem::take(&mut self.rig_handled));
        let tl = self.timeline();
        if tl.has_rig() {
            let spaces = tl.rig_spaces(self.frame, self.doc.stage.fps);
            let layers = tl.layers.iter().zip(spaces).map(|(l, w)| (l.id, l.rig_parent, w)).collect();
            self.rig_cmd = Some(RigCmd { scene: self.scene, depth: self.editing.len(), frame: self.frame, layers });
        }
        outer
    }

    /// A command is done: what it edited in stage space goes back into its layers' spaces, and
    /// a layer whose parent link the command broke as a side effect stays where it showed.
    pub(crate) fn rig_end(&mut self, outer: RigOuter) {
        self.settle_rig();
        if let Some(cmd) = self.rig_cmd.take().filter(|c| c.scene == self.scene && c.depth == self.editing.len()) {
            let handled = std::mem::take(&mut self.rig_handled);
            let scene = self.scene;
            if let Ok(tl) = crate::editing::edited_mut(&mut self.doc, scene, &self.editing) {
                tl.repair_rig();
                for (id, was, space) in &cmd.layers {
                    if was.is_none() || handled.contains(id) {
                        continue;
                    }
                    if tl.layer(*id).is_some_and(|l| l.rig_parent.is_none())
                        && let Some(l) = tl.layer_mut(*id)
                    {
                        l.place_all(space);
                    }
                }
            }
        }
        self.rig_cmd = outer.0;
        self.rig_edits = outer.1;
        self.rig_handled = outer.2;
    }

    /// The space of `layer` at the playhead: its own coordinates → the stage's (the timeline's,
    /// while a symbol is open). The identity for a layer without a parent layer. While a
    /// command runs this is the space the command started with.
    pub fn layer_space(&self, layer: LayerId) -> Affine {
        if let Some(c) = self.rig_cmd.as_ref().filter(|c| c.scene == self.scene && c.depth == self.editing.len() && c.frame == self.frame)
            && let Some((_, _, w)) = c.layers.iter().find(|(id, _, _)| *id == layer)
        {
            return *w;
        }
        let tl = self.timeline();
        if tl.layer(layer).is_none_or(|l| l.rig_parent.is_none()) {
            return Affine::IDENTITY;
        }
        tl.rig_space(layer, self.frame, self.doc.stage.fps)
    }

    /// `els` (in `layer`'s own space) as the stage shows them: placed by the layer's space.
    /// The very same elements for a layer without a parent layer.
    pub(crate) fn shown_on(&self, layer: LayerId, els: Vec<Arc<Element>>) -> Vec<Arc<Element>> {
        let w = self.layer_space(layer);
        if w == Affine::IDENTITY {
            return els;
        }
        els.into_iter().map(|e| Arc::new(Element { transform: w.mul(&e.transform), ..(*e).clone() })).collect()
    }

    /// Why artwork on `layer` can't be edited at the playhead because of its parent (`None`
    /// when it can): the parent's object is scaled to nothing there.
    pub(crate) fn rig_blocked(&self, layer: LayerId) -> Option<EngineError> {
        let w = self.layer_space(layer);
        (w != Affine::IDENTITY && w.inverse().is_none()).then(|| EngineError::State("this layer's parent is scaled to nothing at this frame, so its artwork can't be placed here: edit it at another frame, or remove the parent (Modify › Timeline › Remove Parent)".into()))
    }

    /// Hands `kf` (on `layer`) to the running command in stage space, once per command.
    pub(crate) fn rig_conjugate(edits: &mut Vec<RigEdit>, layer: LayerId, frame: u32, w: Affine, kf: &mut Keyframe) {
        if w == Affine::IDENTITY || edits.iter().any(|e| e.layer == layer && e.start == kf.start) {
            return;
        }
        let mut shown = Vec::with_capacity(kf.elements.len());
        for slot in &mut kf.elements {
            let placed = Element { transform: w.mul(&slot.transform), ..(**slot).clone() };
            let stored = std::mem::replace(slot, Arc::new(placed.clone()));
            shown.push((stored, placed));
        }
        edits.push(RigEdit { layer, start: kf.start, frame, shown });
    }

    /// Puts what the running command edited in stage space back into its layers' spaces (see
    /// the module notes). Runs before a motion tween's edit becomes keys, and when the
    /// command ends.
    pub(crate) fn settle_rig(&mut self) {
        if self.rig_edits.is_empty() {
            return;
        }
        let edits = std::mem::take(&mut self.rig_edits);
        let (scene, fps) = (self.scene, self.doc.stage.fps);
        for e in edits {
            // The space the layer has now: its parent may have moved in this command.
            let back = self.timeline().rig_space(e.layer, e.frame, fps).inverse().unwrap_or(Affine::IDENTITY);
            let Some(layer) = crate::editing::edited_mut(&mut self.doc, scene, &self.editing).ok().and_then(|tl| tl.layer_mut(e.layer)) else { continue };
            let target = layer.keyframe_starting_at(e.start).or_else(|| layer.keyframe_index_at(e.frame));
            for (i, k) in layer.keyframes.iter_mut().enumerate() {
                // Untouched objects get their stored selves back, wherever the command left them.
                let touched = |x: &Arc<Element>| e.shown.iter().find(|(_, placed)| **x == *placed).map(|(stored, _)| stored.clone());
                let here = Some(i) == target;
                if !here && !k.elements.iter().any(|x| touched(x).is_some()) {
                    continue;
                }
                let k = Arc::make_mut(k);
                for slot in &mut k.elements {
                    match touched(slot) {
                        Some(stored) => *slot = stored,
                        // Changed or new on the keyframe the command edited: into the layer's space.
                        None if here => {
                            let t = back.mul(&slot.transform);
                            if t.is_finite() {
                                Arc::make_mut(slot).transform = t;
                            }
                        }
                        None => {}
                    }
                }
            }
        }
    }
}
