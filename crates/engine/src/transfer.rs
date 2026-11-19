//! Library items that travel with copied artwork into another document (ADR 0112).
//!
//! A copy keeps the symbols, bitmaps and sounds the artwork uses, and which document it was made
//! in ([`Session::origin`]). Pasted back into that document, the artwork uses the items that are
//! already there. Pasted into another one, ids mean nothing (each document counts its own), so
//! every item comes along under a new id, with a new name where the Library's top level already
//! has that name, and the pasted artwork is pointed at the new ids:
//!
//! * a bitmap or sound the document already has, name and data alike, is used as it is;
//! * a symbol this session already brought in, unchanged on both sides since, is used again, so
//!   pasting the same thing twice doesn't fill the Library with copies.

use crate::Session;
use anicraft_doc::{Bitmap, BitmapId, Element, ElementKind, Keyframe, LibraryItem, Paint, Sound, SoundId, Symbol, SymbolId, Timeline};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ORIGIN: AtomicU64 = AtomicU64::new(1);

/// A number no other open document has (in this process): what a copy remembers of where it
/// was made.
pub(crate) fn fresh_origin() -> u64 {
    NEXT_ORIGIN.fetch_add(1, Ordering::Relaxed)
}

/// The most Library items one copy carries (a bounded walk; libraries may be cyclic).
const MAX_CARRIED: usize = 10_000;
/// The most brought-in symbols a session remembers; past it, it starts over.
const MAX_REMEMBERED: usize = 4096;

/// The Library items copied artwork uses, and the document they are from.
#[derive(Clone, Debug, Default)]
pub struct Carried {
    /// [`Session::origin`] of the document the copy was made in.
    pub origin: u64,
    pub symbols: Vec<Arc<Symbol>>,
    pub bitmaps: Vec<Arc<Bitmap>>,
    pub sounds: Vec<Arc<Sound>>,
}

/// A symbol brought in from another document: the symbol as it was there, and as it is here.
pub(crate) struct Brought {
    origin: u64,
    from: Arc<Symbol>,
    here: Arc<Symbol>,
}

/// Calls `f` for each Library item `e` uses itself (not what its symbols use), groups included.
fn element_refs(e: &Element, f: &mut impl FnMut(LibraryItem), depth: usize) {
    match &e.kind {
        ElementKind::Symbol { symbol, .. } => f(LibraryItem::Symbol(*symbol)),
        ElementKind::Shape(s) => {
            if let Some(Paint::Bitmap { bitmap, .. }) = &s.fill {
                f(LibraryItem::Bitmap(*bitmap));
            }
        }
        ElementKind::Group { members } if depth < anicraft_doc::MAX_GROUP_DEPTH => {
            for k in members.layers.iter().flat_map(|l| l.keyframes.iter()) {
                for m in &k.elements {
                    element_refs(m, f, depth + 1);
                }
            }
        }
        ElementKind::Group { .. } | ElementKind::Text(_) | ElementKind::Camera => {}
    }
}

/// Everything in the Library that `roots` and the keyframe sounds `sounds` reach: the symbols
/// placed (and the ones inside them), the bitmaps filling shapes and the sounds on keyframes, on
/// the copied artwork and inside those symbols.
pub(crate) fn carry(s: &Session, roots: &[Element], sounds: impl IntoIterator<Item = SoundId>) -> Carried {
    let mut items: Vec<LibraryItem> = sounds.into_iter().map(LibraryItem::Sound).collect();
    for e in roots {
        element_refs(e, &mut |i| items.push(i), 0);
    }
    let mut seen: HashSet<LibraryItem> = HashSet::new();
    let mut out = Carried { origin: s.origin, ..Carried::default() };
    while let Some(item) = items.pop() {
        if seen.len() >= MAX_CARRIED || !seen.insert(item) {
            continue;
        }
        match item {
            LibraryItem::Symbol(id) => {
                let Some(sym) = s.doc.library.iter().find(|x| x.id == id) else { continue };
                anicraft_doc::library::visit_refs(&sym.timeline, |i| items.push(i));
                out.symbols.push(sym.clone());
            }
            LibraryItem::Bitmap(id) => out.bitmaps.extend(s.doc.bitmaps.iter().find(|b| b.id == id).cloned()),
            LibraryItem::Sound(id) => out.sounds.extend(s.doc.sounds.iter().find(|x| x.id == id).cloned()),
        }
    }
    out
}

/// What each carried item is called in the document it was brought into. Empty for a paste into
/// the document the copy came from: nothing changes there.
#[derive(Default)]
pub(crate) struct IdMap {
    symbols: HashMap<SymbolId, SymbolId>,
    bitmaps: HashMap<BitmapId, BitmapId>,
    sounds: HashMap<SoundId, SoundId>,
}

impl IdMap {
    fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.bitmaps.is_empty() && self.sounds.is_empty()
    }

    /// Points `e` (and a group's members) at the items as they are here.
    pub(crate) fn element(&self, e: &mut Element) {
        if !self.is_empty() {
            self.element_in(e, 0);
        }
    }

    fn element_in(&self, e: &mut Element, depth: usize) {
        match &mut e.kind {
            ElementKind::Symbol { symbol, .. } => {
                if let Some(to) = self.symbols.get(symbol) {
                    *symbol = *to;
                }
            }
            ElementKind::Shape(s) => {
                if let Some(Paint::Bitmap { bitmap, .. }) = &mut s.fill
                    && let Some(to) = self.bitmaps.get(bitmap)
                {
                    *bitmap = *to;
                }
            }
            ElementKind::Group { members } if depth < anicraft_doc::MAX_GROUP_DEPTH => self.timeline_in(members, depth + 1),
            ElementKind::Group { .. } | ElementKind::Text(_) | ElementKind::Camera => {}
        }
    }

    /// Points a copied keyframe's artwork and sound at the items as they are here.
    pub(crate) fn keyframe(&self, k: &mut Keyframe) {
        if !self.is_empty() {
            self.keyframe_in(k, 0);
        }
    }

    fn keyframe_in(&self, k: &mut Keyframe, depth: usize) {
        for e in &mut k.elements {
            // Only what refers to the Library is copied to be changed.
            if matches!(e.kind, ElementKind::Symbol { .. } | ElementKind::Group { .. }) || matches!(&e.kind, ElementKind::Shape(s) if matches!(s.fill, Some(Paint::Bitmap { .. }))) {
                self.element_in(Arc::make_mut(e), depth);
            }
        }
        if let Some(snd) = &mut k.sound
            && let Some(to) = self.sounds.get(&snd.sound)
        {
            snd.sound = *to;
        }
    }

    fn timeline_in(&self, tl: &mut Timeline, depth: usize) {
        for l in &mut tl.layers {
            for k in &mut Arc::make_mut(l).keyframes {
                self.keyframe_in(Arc::make_mut(k), depth);
            }
        }
    }
}

/// Moves the document's id counter past every id `sym` uses inside, so new ids stay unique.
fn ids_past(max_id: &mut u64, sym: &Symbol) {
    *max_id = (*max_id).max(sym.id.0);
    for l in &sym.timeline.layers {
        *max_id = (*max_id).max(l.id.0);
        for e in l.keyframes.iter().flat_map(|k| k.elements.iter()) {
            *max_id = (*max_id).max(e.id.0);
        }
    }
}

/// Makes sure the document has what `c` carries, and says what each item is called here.
pub(crate) fn bring(s: &mut Session, c: &Carried) -> IdMap {
    if c.origin == s.origin {
        same_document(s, c);
        return IdMap::default();
    }
    let mut map = IdMap::default();
    for b in &c.bitmaps {
        // The same picture under the same name is the same Library item.
        let here = s.doc.bitmaps.iter().find(|x| x.name == b.name && x.width == b.width && x.height == b.height && x.smoothing == b.smoothing && (Arc::ptr_eq(&x.png, &b.png) || x.png == b.png)).map(|x| x.id);
        let id = here.unwrap_or_else(|| {
            let mut copy = (**b).clone();
            copy.id = BitmapId(s.doc.alloc_id());
            copy.name = s.doc.unique_name(None, &b.name);
            let id = copy.id;
            s.doc.bitmaps.push(Arc::new(copy));
            id
        });
        map.bitmaps.insert(b.id, id);
    }
    for x in &c.sounds {
        let here = s.doc.sounds.iter().find(|y| y.name == x.name && y.format == x.format && y.frames == x.frames && (Arc::ptr_eq(&y.data, &x.data) || y.data == x.data)).map(|y| y.id);
        let id = here.unwrap_or_else(|| {
            let mut copy = (**x).clone();
            copy.id = SoundId(s.doc.alloc_id());
            copy.name = s.doc.unique_name(None, &x.name);
            let id = copy.id;
            s.doc.sounds.push(Arc::new(copy));
            id
        });
        map.sounds.insert(x.id, id);
    }
    // Symbols: first what each is called here (they refer to each other), then the symbols.
    let mut fresh: Vec<(&Arc<Symbol>, SymbolId)> = Vec::new();
    for sym in &c.symbols {
        // One brought in before, the same there and here as it was then, is used again.
        let known = s.brought.iter().find(|b| b.origin == c.origin && Arc::ptr_eq(&b.from, sym)).filter(|b| s.doc.library.iter().any(|x| Arc::ptr_eq(x, &b.here))).map(|b| b.here.id);
        let id = known.unwrap_or_else(|| {
            let id = SymbolId(s.doc.alloc_id());
            fresh.push((sym, id));
            id
        });
        map.symbols.insert(sym.id, id);
    }
    let mut max_id = 0u64;
    for (sym, id) in fresh {
        let mut copy = (**sym).clone();
        copy.id = id;
        copy.name = s.doc.unique_name(None, &sym.name);
        map.timeline_in(&mut copy.timeline, 0);
        ids_past(&mut max_id, &copy);
        let here = Arc::new(copy);
        s.doc.library.push(here.clone());
        if s.brought.len() >= MAX_REMEMBERED {
            s.brought.clear();
        }
        s.brought.push(Brought { origin: c.origin, from: sym.clone(), here });
    }
    s.doc.next_id = s.doc.next_id.max(max_id.saturating_add(1));
    map
}

/// A paste into the document the copy was made in: only what was deleted since comes back,
/// under the id it had.
fn same_document(s: &mut Session, c: &Carried) {
    let mut max_id = 0u64;
    for sym in &c.symbols {
        if s.doc.symbol(sym.id).is_none() {
            ids_past(&mut max_id, sym);
            s.doc.library.push(sym.clone());
        }
    }
    for b in &c.bitmaps {
        if s.doc.bitmap(b.id).is_none() {
            max_id = max_id.max(b.id.0);
            s.doc.bitmaps.push(b.clone());
        }
    }
    for x in &c.sounds {
        if s.doc.sound(x.id).is_none() {
            max_id = max_id.max(x.id.0);
            s.doc.sounds.push(x.clone());
        }
    }
    s.doc.next_id = s.doc.next_id.max(max_id.saturating_add(1));
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
