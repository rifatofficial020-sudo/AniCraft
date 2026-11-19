//! Layer parenting (ADR 0113): a layer's artwork follows the object on another layer.
//!
//! A layer may name a **parent layer** ([`Layer::rig_parent`]; not the folder or mask it sits
//! in, which is [`Layer::parent`]). At every frame the child's artwork is placed in the space of
//! the parent layer's **lead object**: the first (bottom-most) object on the parent layer at
//! that frame, as it shows there (tweened, guided, posed), itself placed by the parent's own
//! parent, and so on up the chain. Position, rotation, scale and skew are followed; colour
//! effects, filters and blending are not.
//!
//! Everything here is a pure function of the timeline and the frame.

use crate::{Keyframe, Layer, LayerId, LayerKind, Timeline, eval};
use anicraft_geom::Affine;
use std::sync::Arc;

/// Longest chain of parent layers that is followed (and kept).
pub const MAX_RIG_DEPTH: usize = 64;

/// Why one layer can't be parented to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigError {
    /// A layer isn't on this timeline.
    Missing,
    /// A layer can't be its own parent.
    SameLayer,
    /// The parent already follows the child (directly or through other layers).
    Cycle,
    /// The child's kind holds no artwork that could follow (folder, camera, guide), or is posed
    /// by its own bones (armature).
    ChildKind,
    /// The parent's kind holds no object to follow (folder, camera, guide).
    ParentKind,
    /// The chain of parents would be longer than [`MAX_RIG_DEPTH`].
    TooDeep,
}

impl std::fmt::Display for RigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RigError::Missing => "no such layer on this timeline",
            RigError::SameLayer => "a layer can't be its own parent",
            RigError::Cycle => "that layer already follows this one: a layer can't be parented to a layer that follows it",
            RigError::ChildKind => "only normal and mask layers can follow a parent layer (folders, guides, the camera and armature layers can't)",
            RigError::ParentKind => "a folder, guide or camera layer has no object to follow: pick a layer with artwork as the parent",
            RigError::TooDeep => "that chain of parent layers would be too long",
        })
    }
}

impl Layer {
    /// Whether this layer's artwork can follow a parent layer.
    pub fn can_follow(&self) -> bool {
        matches!(self.kind, LayerKind::Normal | LayerKind::Mask)
    }

    /// Whether other layers can follow this layer's lead object.
    pub fn can_lead(&self) -> bool {
        matches!(self.kind, LayerKind::Normal | LayerKind::Mask | LayerKind::Armature)
    }

    /// Places every keyframe's artwork by `m` (applied after each object's own transform): what
    /// keeps a layer where it is on stage when its space changes by `m`'s inverse (ADR 0113).
    /// A motion tween's keys and path move with its target. Touches nothing when `m` is the
    /// identity.
    pub fn place_all(&mut self, m: &Affine) {
        if *m == Affine::IDENTITY || !m.is_finite() {
            return;
        }
        for k in &mut self.keyframes {
            if k.elements.is_empty() {
                continue;
            }
            place_keyframe(Arc::make_mut(k), m);
        }
    }
}

/// One keyframe's artwork placed by `m` (see [`Layer::place_all`]).
fn place_keyframe(k: &mut Keyframe, m: &Affine) {
    let target = k.motion.as_ref().and_then(|mt| mt.target_index(&k.elements));
    // The motion tween first: its keys are read against the target as it was.
    if let (Some(i), Some(mt)) = (target, k.motion.as_mut())
        && let Some(base) = k.elements.get(i)
    {
        mt.place(base, m);
    }
    for e in &mut k.elements {
        let t = m.mul(&e.transform);
        if t.is_finite() {
            Arc::make_mut(e).transform = t;
        }
    }
}

impl Timeline {
    /// Whether any layer follows a parent layer.
    pub fn has_rig(&self) -> bool {
        self.layers.iter().any(|l| l.rig_parent.is_some())
    }

    /// The parent layers of `id`, nearest first (bounded; a cycle in a damaged file stops it).
    pub fn rig_ancestors(&self, id: LayerId) -> Vec<LayerId> {
        let mut out = Vec::new();
        let mut cur = self.layer(id).and_then(|l| l.rig_parent);
        while let Some(p) = cur {
            if out.len() >= MAX_RIG_DEPTH || p == id || out.contains(&p) {
                break;
            }
            out.push(p);
            cur = self.layer(p).and_then(|l| l.rig_parent);
        }
        out
    }

    /// The layers that follow `id` directly, top to bottom.
    pub fn rig_children(&self, id: LayerId) -> Vec<LayerId> {
        self.layers.iter().filter(|l| l.rig_parent == Some(id)).map(|l| l.id).collect()
    }

    /// Every layer that follows `id`, directly or through other layers, top to bottom.
    pub fn rig_descendants(&self, id: LayerId) -> Vec<LayerId> {
        self.layers.iter().filter(|l| l.id != id && self.rig_ancestors(l.id).contains(&id)).map(|l| l.id).collect()
    }

    /// Whether `child` may take `parent` as its parent layer.
    pub fn rig_check(&self, child: LayerId, parent: LayerId) -> Result<(), RigError> {
        let (c, p) = (self.layer(child).ok_or(RigError::Missing)?, self.layer(parent).ok_or(RigError::Missing)?);
        if child == parent {
            return Err(RigError::SameLayer);
        }
        if !c.can_follow() {
            return Err(RigError::ChildKind);
        }
        if !p.can_lead() {
            return Err(RigError::ParentKind);
        }
        let above = self.rig_ancestors(parent);
        if above.contains(&child) {
            return Err(RigError::Cycle);
        }
        // The chain above the parent, the parent, and the deepest chain below the child.
        let below = self.layers.iter().map(|l| self.rig_ancestors(l.id)).filter_map(|a| a.iter().position(|x| *x == child).map(|i| i + 1)).max().unwrap_or(0);
        if above.len() + 1 + below > MAX_RIG_DEPTH {
            return Err(RigError::TooDeep);
        }
        Ok(())
    }

    /// The lead object's transform on the layer at `index` at `frame`: its first object as it
    /// shows there, in the layer's own space. The identity on an empty frame.
    fn rig_lead(&self, index: usize, frame: u32, fps: f32) -> Affine {
        let Some(layer) = self.layers.get(index).filter(|l| l.can_lead()) else { return Affine::IDENTITY };
        let walk = layer.parent.and_then(|p| self.layer(p)).filter(|g| g.kind == LayerKind::Guide).and_then(|g| eval::guide_walk(g, frame));
        eval::evaluate_shown(layer, frame, walk.as_ref(), fps).and_then(|(_, els)| els.first().map(|e| e.transform)).filter(Affine::is_finite).unwrap_or(Affine::IDENTITY)
    }

    /// Each layer's space at `frame`, by layer index: the transform from the layer's own
    /// coordinates to the timeline's (the identity for a layer without a parent layer). `fps`
    /// times an armature parent's springs, as the evaluator does.
    pub fn rig_spaces(&self, frame: u32, fps: f32) -> Vec<Affine> {
        let mut memo: Vec<Option<Affine>> = vec![None; self.layers.len()];
        (0..self.layers.len()).map(|i| self.rig_resolve(i, frame, fps, &mut memo, 0)).collect()
    }

    /// The space a layer following `parent` has at `frame`: `parent`'s own space times its
    /// lead object's transform there.
    pub fn rig_child_space(&self, parent: LayerId, frame: u32, fps: f32) -> Affine {
        let Some(i) = self.layers.iter().position(|l| l.id == parent) else { return Affine::IDENTITY };
        let mut memo: Vec<Option<Affine>> = vec![None; self.layers.len()];
        let w = self.rig_resolve(i, frame, fps, &mut memo, 0).mul(&self.rig_lead(i, frame, fps));
        if w.is_finite() { w } else { Affine::IDENTITY }
    }

    /// The space of layer `id` at `frame` (see [`Timeline::rig_spaces`]).
    pub fn rig_space(&self, id: LayerId, frame: u32, fps: f32) -> Affine {
        let Some(i) = self.layers.iter().position(|l| l.id == id) else { return Affine::IDENTITY };
        if self.layers.get(i).is_none_or(|l| l.rig_parent.is_none()) {
            return Affine::IDENTITY;
        }
        let mut memo: Vec<Option<Affine>> = vec![None; self.layers.len()];
        self.rig_resolve(i, frame, fps, &mut memo, 0)
    }

    fn rig_resolve(&self, i: usize, frame: u32, fps: f32, memo: &mut Vec<Option<Affine>>, depth: usize) -> Affine {
        if let Some(Some(w)) = memo.get(i) {
            return *w;
        }
        let parent = self.layers.get(i).filter(|l| l.can_follow()).and_then(|l| l.rig_parent).and_then(|p| self.layers.iter().position(|l| l.id == p));
        let w = match parent {
            Some(p) if p != i && depth < MAX_RIG_DEPTH => {
                let w = self.rig_resolve(p, frame, fps, memo, depth + 1).mul(&self.rig_lead(p, frame, fps));
                if w.is_finite() { w } else { Affine::IDENTITY }
            }
            _ => Affine::IDENTITY,
        };
        if let Some(slot) = memo.get_mut(i) {
            *slot = Some(w);
        }
        w
    }

    /// Drops parent-layer links that can't hold: to a layer that is gone, to itself, between
    /// kinds that can't be linked, round a cycle, or past [`MAX_RIG_DEPTH`]. Touches only the
    /// layers that change.
    pub fn repair_rig(&mut self) {
        if !self.has_rig() {
            return;
        }
        for i in 0..self.layers.len() {
            let Some((id, p)) = self.layers.get(i).and_then(|l| Some((l.id, l.rig_parent?))) else { continue };
            // Checked as a new link would be: the layer's own link is taken away first.
            if let Some(l) = self.layers.get_mut(i) {
                Arc::make_mut(l).rig_parent = None;
            }
            if self.rig_check(id, p).is_ok()
                && let Some(l) = self.layers.get_mut(i)
            {
                Arc::make_mut(l).rig_parent = Some(p);
            }
        }
    }
}
