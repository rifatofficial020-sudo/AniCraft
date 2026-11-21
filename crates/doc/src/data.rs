//! Data tables that drive animation (ADR 0118).
//!
//! A document keeps **tables** of numbers ([`DataTable`]: named columns, one row per frame or,
//! with a key column, one row per keyed time such as an age or a depth) and **bindings**
//! ([`Binding`]): a column of a table drives one property of one symbol instance over a span
//! of frames. Both live in [`Document::data`], apart from the timelines: a binding names its
//! instance by id, so it follows the instance through keyframes and tweens without touching
//! either, and editing or re-importing a table changes what every frame shows with no keyframe
//! to rebuild.
//!
//! Bindings are applied inside [`crate::evaluate`] (time stays a function): after tweens and
//! poses have placed a layer's elements, each bound instance gets the bound properties' values
//! for the frame. A binding whose table or column is missing, or whose column has no number,
//! leaves the instance as it is. Everything here is pure and bounded; nothing panics on any
//! table, binding or frame.

use crate::library::FolderId;
use crate::motion::{Property, Props};
use crate::{Document, Element, ElementId, ElementKind, LoopMode};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::Arc;

/// Unique id of a data table (from the document's id counter).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataId(pub u64);

/// Most tables in a document.
pub const MAX_TABLES: usize = 256;
/// Most columns in a table.
pub const MAX_COLUMNS: usize = 256;
/// Most rows in a table.
pub const MAX_ROWS: usize = 100_000;
/// Most cells (rows × columns) in a table.
pub const MAX_CELLS: usize = 1_000_000;
/// Longest column name (characters).
pub const MAX_COLUMN_NAME: usize = 64;
/// Longest note of where a table came from (characters).
pub const MAX_SOURCE: usize = 256;
/// Most bindings in a document.
pub const MAX_BINDINGS: usize = 10_000;
/// Longest frame span a binding maps (and the last frame it may start on).
pub const MAX_SPAN: u32 = 100_000;
/// Largest scale or offset of a binding, and the largest value it gives.
pub const MAX_VALUE: f64 = 1.0e9;

/// One named column of numbers. `None` is a gap: a cell with no number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataColumn {
    pub name: String,
    #[serde(default)]
    pub values: Vec<Option<f64>>,
}

/// A table of numbers kept in the document: a Library item (ADR 0118).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataTable {
    pub id: DataId,
    pub name: String,
    #[serde(default)]
    pub columns: Vec<DataColumn>,
    /// The column rows are looked up by (an age, a depth, a time). `None`: by row number, the
    /// first row being 0, so a binding of as many frames as rows shows one row per frame. A
    /// table with a key keeps its rows in the key's ascending order, rows without a key last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The name of the file it was imported from (the name only, never a folder), shown in the
    /// viewer and offered when importing again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The Library folder it is in (`None`: the top level). Kept here, not in the Library
    /// tree's placements, so builds from before data tables still open the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<FolderId>,
}

fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// A column name as tables keep it: trimmed, without control characters, at most
/// [`MAX_COLUMN_NAME`] characters. Empty when nothing is left.
pub fn clean_column_name(name: &str) -> String {
    name.trim().chars().filter(|c| !c.is_control()).take(MAX_COLUMN_NAME).collect::<String>().trim().to_string()
}

impl DataTable {
    /// A table of `columns`, repaired to the rules ([`DataTable::normalize`]).
    pub fn new(id: DataId, name: impl Into<String>, columns: Vec<DataColumn>, key: Option<String>) -> DataTable {
        let mut t = DataTable { id, name: name.into(), columns, key, source: String::new(), folder: None };
        t.normalize();
        t
    }

    /// How many rows the table has (every column has as many).
    pub fn rows(&self) -> usize {
        self.columns.iter().map(|c| c.values.len()).max().unwrap_or(0)
    }

    /// The column called `name`: exactly, else without case.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name).or_else(|| self.columns.iter().position(|c| same_name(&c.name, name)))
    }

    pub fn column(&self, name: &str) -> Option<&DataColumn> {
        self.column_index(name).and_then(|i| self.columns.get(i))
    }

    /// The key column, when the table has one.
    pub fn key_column(&self) -> Option<&DataColumn> {
        self.key.as_deref().and_then(|k| self.column(k))
    }

    /// Whether the table breaks none of the rules [`DataTable::normalize`] repairs.
    pub fn is_normal(&self) -> bool {
        let rows = self.rows();
        let cols = self.columns.len();
        let names_ok = self.columns.iter().enumerate().all(|(i, c)| !c.name.is_empty() && clean_column_name(&c.name) == c.name && !self.columns.iter().take(i).any(|o| same_name(&o.name, &c.name)));
        let sizes_ok = cols <= MAX_COLUMNS && rows <= MAX_ROWS && rows.saturating_mul(cols) <= MAX_CELLS && self.columns.iter().all(|c| c.values.len() == rows);
        let cells_ok = self.columns.iter().all(|c| c.values.iter().all(|v| v.is_none_or(f64::is_finite)));
        let key_ok = match &self.key {
            None => true,
            Some(k) => self.columns.iter().any(|c| c.name == *k) && self.key_column().is_some_and(|c| keys_sorted(&c.values)),
        };
        names_ok && sizes_ok && cells_ok && key_ok && self.source.chars().count() <= MAX_SOURCE && !self.source.contains(['/', '\\'])
    }

    /// Repairs the table: column names cleaned and made unique (a nameless one is `Column n`),
    /// at most [`MAX_COLUMNS`] columns, [`MAX_ROWS`] rows and [`MAX_CELLS`] cells, short columns
    /// padded with gaps, cells that aren't finite turned into gaps, a key that names no column
    /// dropped, and with a key the rows put in its ascending order (stable; rows without a key
    /// last). Idempotent.
    pub fn normalize(&mut self) {
        if self.is_normal() {
            return;
        }
        self.columns.truncate(MAX_COLUMNS);
        let mut names: Vec<String> = Vec::with_capacity(self.columns.len());
        for (i, c) in self.columns.iter_mut().enumerate() {
            let mut base = clean_column_name(&c.name);
            if base.is_empty() {
                base = format!("Column {}", i + 1);
            }
            let mut name = base.clone();
            let mut n = 2usize;
            // Bounded: at most as many names are taken as there are columns.
            while names.iter().any(|o| same_name(o, &name)) && n <= MAX_COLUMNS + 2 {
                let stem: String = base.chars().take(MAX_COLUMN_NAME.saturating_sub(5)).collect();
                name = format!("{stem} {n}");
                n += 1;
            }
            c.name = name.clone();
            names.push(name);
        }
        let cols = self.columns.len().max(1);
        let rows = self.rows().min(MAX_ROWS).min(MAX_CELLS / cols);
        for c in &mut self.columns {
            c.values.resize(rows, None);
            for v in &mut c.values {
                if v.is_some_and(|x| !x.is_finite()) {
                    *v = None;
                }
            }
        }
        self.source = self.source.rsplit(['/', '\\']).next().unwrap_or("").chars().filter(|c| !c.is_control()).take(MAX_SOURCE).collect();
        self.key = self.key.as_deref().and_then(|k| self.column_index(k)).and_then(|i| self.columns.get(i)).map(|c| c.name.clone());
        if let Some(keys) = self.key_column().map(|c| c.values.clone())
            && !keys_sorted(&keys)
        {
            let mut order: Vec<usize> = (0..rows).collect();
            let key = |i: usize| keys.get(i).copied().flatten();
            order.sort_by(|&a, &b| match (key(a), key(b)) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });
            for c in &mut self.columns {
                c.values = order.iter().map(|&i| c.values.get(i).copied().flatten()).collect();
            }
        }
    }

    /// The first and last key a binding maps a span onto by default: the key column's lowest
    /// and highest, or the first and last row numbers. `None` for a table with no rows (or no
    /// keyed row).
    pub fn key_range(&self) -> Option<(f64, f64)> {
        match self.key_column() {
            Some(k) => {
                let n = k.values.partition_point(Option::is_some);
                let first = k.values.first().copied().flatten()?;
                let last = n.checked_sub(1).and_then(|i| k.values.get(i)).copied().flatten()?;
                Some((first, last))
            }
            None => self.rows().checked_sub(1).map(|last| (0.0, last as f64)),
        }
    }

    /// Column `column`'s value at `key` (a row number when the table has no key column):
    /// between the nearest rows with a number on either side, straight across ([`Interp::Linear`])
    /// or held from the earlier one ([`Interp::Step`]). Before the first such row and after the
    /// last, theirs. Gaps are skipped. `None` when the column has no number at all.
    pub fn sample(&self, column: usize, key: f64, interp: Interp) -> Option<f64> {
        let vals = &self.columns.get(column)?.values;
        if !key.is_finite() {
            return None;
        }
        let keys = self.key_column().map(|c| c.values.as_slice());
        // The rows that have a key, and the first of them whose key is past `key`.
        let (n, i) = match keys {
            Some(k) => {
                let n = k.partition_point(Option::is_some).min(vals.len());
                (n, k.get(..n)?.partition_point(|x| x.is_some_and(|x| x <= key)))
            }
            None => {
                let n = vals.len();
                (n, if key < 0.0 { 0 } else if key >= n as f64 { n } else { (key.floor() as usize + 1).min(n) })
            }
        };
        let key_of = |j: usize| match keys {
            Some(k) => k.get(j).copied().flatten(),
            None => Some(j as f64),
        };
        let value = |j: usize| vals.get(j).copied().flatten();
        let lo = (0..i).rev().find(|&j| value(j).is_some());
        let hi = (i..n).find(|&j| value(j).is_some());
        match (lo, hi) {
            (None, None) => None,
            (Some(a), None) => value(a),
            (None, Some(b)) => value(b),
            (Some(a), Some(b)) => match interp {
                Interp::Step => value(a),
                Interp::Linear => {
                    let (va, vb, ka, kb) = (value(a)?, value(b)?, key_of(a)?, key_of(b)?);
                    let span = kb - ka;
                    let v = if span > 0.0 { va + (vb - va) * ((key - ka) / span).clamp(0.0, 1.0) } else { va };
                    v.is_finite().then_some(v)
                }
            },
        }
    }
}

/// Whether keys are in ascending order with the rows that have none last.
fn keys_sorted(keys: &[Option<f64>]) -> bool {
    keys.windows(2).all(|w| match (w.first().copied().flatten(), w.get(1).copied().flatten()) {
        (Some(a), Some(b)) => a <= b,
        (None, Some(_)) => false,
        _ => true,
    })
}

/// How a binding reads a column between two rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interp {
    /// Straight from one row's value to the next.
    #[default]
    Linear,
    /// Each row's value is held until the next row.
    Step,
}

impl Interp {
    pub const ALL: [Interp; 2] = [Interp::Linear, Interp::Step];

    pub fn key(self) -> &'static str {
        match self {
            Interp::Linear => "linear",
            Interp::Step => "step",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Interp::Linear => "Linear",
            Interp::Step => "Step",
        }
    }
    pub fn from_key(s: &str) -> Option<Interp> {
        match s.trim().to_ascii_lowercase().as_str() {
            "linear" | "smooth" => Some(Interp::Linear),
            "step" | "hold" | "stepped" => Some(Interp::Step),
            _ => None,
        }
    }
    fn is_linear(&self) -> bool {
        *self == Interp::Linear
    }
}

/// A property of an instance a column can drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundProperty {
    /// Registration point, pixels in the layer's space.
    X,
    Y,
    /// Percent (100 = natural size).
    ScaleX,
    ScaleY,
    /// Degrees, clockwise on screen.
    Rotation,
    /// Opacity, percent.
    Alpha,
    /// The frame of its symbol a graphic instance shows, counted from 0 (0 is the symbol's
    /// first frame) and held as a single frame.
    Frame,
}

impl BoundProperty {
    pub const ALL: [BoundProperty; 7] = [BoundProperty::X, BoundProperty::Y, BoundProperty::ScaleX, BoundProperty::ScaleY, BoundProperty::Rotation, BoundProperty::Alpha, BoundProperty::Frame];

    /// The name used in files and commands.
    pub fn key(self) -> &'static str {
        match self {
            BoundProperty::X => "x",
            BoundProperty::Y => "y",
            BoundProperty::ScaleX => "scale_x",
            BoundProperty::ScaleY => "scale_y",
            BoundProperty::Rotation => "rotation",
            BoundProperty::Alpha => "alpha",
            BoundProperty::Frame => "frame",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            BoundProperty::X => "X",
            BoundProperty::Y => "Y",
            BoundProperty::ScaleX => "Scale X",
            BoundProperty::ScaleY => "Scale Y",
            BoundProperty::Rotation => "Rotation",
            BoundProperty::Alpha => "Alpha",
            BoundProperty::Frame => "Frame shown",
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            BoundProperty::X | BoundProperty::Y => "px",
            BoundProperty::ScaleX | BoundProperty::ScaleY | BoundProperty::Alpha => "%",
            BoundProperty::Rotation => "°",
            BoundProperty::Frame => "",
        }
    }
    pub fn from_key(s: &str) -> Option<BoundProperty> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        let s = match s.as_str() {
            "opacity" => "alpha",
            "scalex" => "scale_x",
            "scaley" => "scale_y",
            "rotate" => "rotation",
            "first_frame" | "firstframe" => "frame",
            other => other,
        };
        BoundProperty::ALL.into_iter().find(|p| p.key() == s)
    }
    /// The motion property this is (`None` for the frame shown).
    fn property(self) -> Option<Property> {
        match self {
            BoundProperty::X => Some(Property::X),
            BoundProperty::Y => Some(Property::Y),
            BoundProperty::ScaleX => Some(Property::ScaleX),
            BoundProperty::ScaleY => Some(Property::ScaleY),
            BoundProperty::Rotation => Some(Property::Rotation),
            BoundProperty::Alpha => Some(Property::Alpha),
            BoundProperty::Frame => None,
        }
    }
    /// `v` kept within what the property takes (as a motion tween's curve is).
    pub fn clamp(self, v: f32) -> f32 {
        match self.property() {
            Some(p) => {
                let r = p.range();
                v.clamp(*r.start(), *r.end())
            }
            None => v.round().clamp(0.0, MAX_SPAN as f32),
        }
    }
}

fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A column of a table driving one property of one instance over a span of frames.
///
/// The span's first frame shows the key `from`, its last the key `to`, the frames between keys
/// in a straight line between the two; the column read there ([`DataTable::sample`]) times
/// `scale` plus `offset` is the property's value. Before the span the instance shows the first
/// frame's value and after it the last's, so a chart doesn't snap back when its data ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub element: ElementId,
    pub property: BoundProperty,
    pub table: DataId,
    pub column: String,
    /// The span's first frame, on the timeline the instance is on.
    #[serde(default)]
    pub start: u32,
    /// How many frames the span has (at least 1).
    #[serde(default = "one_frame")]
    pub frames: u32,
    /// The key the first frame shows. `None`: the table's first ([`DataTable::key_range`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    /// The key the last frame shows. `None`: the table's last. May be before `from`, to play a
    /// record backwards (ages run from old to young).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    #[serde(default, skip_serializing_if = "Interp::is_linear")]
    pub interp: Interp,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: f64,
}

fn one_frame() -> u32 {
    1
}

impl Binding {
    /// The key frame `frame` shows in `table`: exact at the span's ends, and exact on every
    /// frame of a span with one frame per row.
    pub fn key_at(&self, table: &DataTable, frame: u32) -> Option<f64> {
        let (first, last) = table.key_range()?;
        let (from, to) = (self.from.unwrap_or(first), self.to.unwrap_or(last));
        let den = self.frames.max(1) - 1;
        let num = frame.saturating_sub(self.start).min(den);
        let key = if num == 0 {
            from
        } else if num >= den {
            to
        } else {
            // Multiplied before dividing: with one frame per row this is the row number exactly.
            from + (to - from) * f64::from(num) / f64::from(den)
        };
        key.is_finite().then_some(key)
    }

    /// The property's value at `frame`, in the property's units and range. `None` when the
    /// table or column is missing or has no number: the instance stays as placed.
    pub fn value_at(&self, data: &DataSet, frame: u32) -> Option<f32> {
        let table = data.table(self.table)?;
        let column = table.column_index(&self.column)?;
        let v = table.sample(column, self.key_at(table, frame)?, self.interp)? * self.scale + self.offset;
        v.is_finite().then(|| self.property.clamp(v.clamp(-MAX_VALUE, MAX_VALUE) as f32))
    }

    /// Whether the binding breaks none of the rules [`Binding::sanitize`] repairs.
    fn is_sane(&self) -> bool {
        let fin = |v: f64| v.is_finite() && v.abs() <= MAX_VALUE;
        self.start <= MAX_SPAN && (1..=MAX_SPAN).contains(&self.frames) && fin(self.scale) && fin(self.offset) && self.from.is_none_or(fin) && self.to.is_none_or(fin) && clean_column_name(&self.column) == self.column
    }

    /// Repairs the binding's numbers and name.
    pub fn sanitize(&mut self) {
        let fin = |v: f64, or: f64| if v.is_finite() { v.clamp(-MAX_VALUE, MAX_VALUE) } else { or };
        self.start = self.start.min(MAX_SPAN);
        self.frames = self.frames.clamp(1, MAX_SPAN);
        self.scale = fin(self.scale, 1.0);
        self.offset = fin(self.offset, 0.0);
        self.from = self.from.filter(|v| v.is_finite()).map(|v| fin(v, 0.0));
        self.to = self.to.filter(|v| v.is_finite()).map(|v| fin(v, 0.0));
        self.column = clean_column_name(&self.column);
    }
}

/// The document's data: its tables and what they drive.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DataSet {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<Arc<DataTable>>,
    /// Sorted by instance, then property: one binding per property of an instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
}

impl DataSet {
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty() && self.bindings.is_empty()
    }

    pub fn table(&self, id: DataId) -> Option<&DataTable> {
        self.tables.iter().find(|t| t.id == id).map(|t| &**t)
    }

    /// The bindings of instance `id`, in property order.
    pub fn bindings_of(&self, id: ElementId) -> &[Binding] {
        let a = self.bindings.partition_point(|b| b.element < id);
        let n = self.bindings.get(a..).map_or(0, |rest| rest.partition_point(|b| b.element == id));
        self.bindings.get(a..a + n).unwrap_or(&[])
    }

    /// Adds `b`, replacing the binding of the same property of the same instance. `false`
    /// when the document can't hold another.
    pub fn bind(&mut self, mut b: Binding) -> bool {
        b.sanitize();
        let at = self.bindings.partition_point(|o| (o.element, o.property) < (b.element, b.property));
        let full = self.bindings.len() >= MAX_BINDINGS;
        match self.bindings.get_mut(at) {
            Some(o) if o.element == b.element && o.property == b.property => *o = b,
            _ if full => return false,
            _ => self.bindings.insert(at, b),
        }
        true
    }

    /// Removes instance `id`'s binding of `property` (every binding of it for `None`). How many
    /// went.
    pub fn unbind(&mut self, id: ElementId, property: Option<BoundProperty>) -> usize {
        let before = self.bindings.len();
        self.bindings.retain(|b| b.element != id || property.is_some_and(|p| p != b.property));
        before - self.bindings.len()
    }

    /// How many bindings read table `id`.
    pub fn uses(&self, id: DataId) -> usize {
        self.bindings.iter().filter(|b| b.table == id).count()
    }

    /// Whether the set breaks none of the rules [`DataSet::sanitize`] repairs.
    fn is_sane(&self) -> bool {
        let tables_ok = self.tables.len() <= MAX_TABLES && self.tables.iter().enumerate().all(|(i, t)| t.is_normal() && !self.tables.iter().take(i).any(|o| o.id == t.id));
        let sorted = self.bindings.windows(2).all(|w| match (w.first(), w.get(1)) {
            (Some(a), Some(b)) => (a.element, a.property) < (b.element, b.property),
            _ => true,
        });
        tables_ok && sorted && self.bindings.len() <= MAX_BINDINGS && self.bindings.iter().all(Binding::is_sane)
    }

    /// Repairs loaded data: tables with unique ids, each normal; bindings sane, sorted, one
    /// per property of an instance, at most [`MAX_BINDINGS`]. `true` when something changed.
    pub fn sanitize(&mut self) -> bool {
        if self.is_sane() {
            return false;
        }
        let mut seen = std::collections::HashSet::new();
        self.tables.retain(|t| seen.insert(t.id));
        self.tables.truncate(MAX_TABLES);
        for t in &mut self.tables {
            if !t.is_normal() {
                Arc::make_mut(t).normalize();
            }
        }
        for b in &mut self.bindings {
            b.sanitize();
        }
        self.bindings.sort_by_key(|b| (b.element, b.property));
        self.bindings.dedup_by_key(|b| (b.element, b.property));
        self.bindings.truncate(MAX_BINDINGS);
        true
    }
}

/// `el` with `values` (each a property and its value, in the property's units) in place of its
/// own: scaled and turned about its transformation point when it has one (ADR 0111), as a motion
/// tween's curves do; alpha keeps the rest of its colour effect; a frame makes a graphic
/// instance show that frame of its symbol and hold it.
pub fn with_values(el: &Element, values: &[(BoundProperty, f32)]) -> Element {
    let mut out = el.clone();
    let base = Props::of(el);
    let mut props = base;
    let mut moved = false;
    for &(p, v) in values {
        let v = p.clamp(v);
        match p {
            BoundProperty::Alpha => out.color = el.color.with_alpha((v / 100.0).clamp(0.0, 1.0)).sanitized(),
            BoundProperty::Frame => {
                if let ElementKind::Symbol { loop_mode, first_frame, .. } = &mut out.kind {
                    *loop_mode = LoopMode::SingleFrame;
                    *first_frame = v as u32;
                }
            }
            _ => {
                if let Some(q) = p.property() {
                    props.set(q, v);
                    moved = true;
                }
            }
        }
    }
    if moved {
        let m = crate::motion::placed(el, &base, &props);
        if m.is_finite() {
            out.transform = m;
        }
    }
    out
}

impl Document {
    pub fn data_table(&self, id: DataId) -> Option<&DataTable> {
        self.data.table(id)
    }

    /// The table called `name` (without case), the first in Library order.
    pub fn data_table_named(&self, name: &str) -> Option<&DataTable> {
        self.data.tables.iter().find(|t| t.name == name).or_else(|| self.data.tables.iter().find(|t| same_name(&t.name, name))).map(|t| &**t)
    }

    /// What `el`'s bindings make of it at `frame` of the timeline it is on: `None` when it has
    /// none, or none of them has a value there.
    pub fn bound_element(&self, el: &Element, frame: u32) -> Option<Element> {
        let bindings = self.data.bindings_of(el.id);
        if bindings.is_empty() || !matches!(el.kind, ElementKind::Symbol { .. }) {
            return None;
        }
        let mut values = [(BoundProperty::X, 0.0f32); BoundProperty::ALL.len()];
        let mut n = 0;
        for b in bindings {
            if let (Some(v), Some(slot)) = (b.value_at(&self.data, frame), values.get_mut(n)) {
                *slot = (b.property, v);
                n += 1;
            }
        }
        (n > 0).then(|| with_values(el, values.get(..n).unwrap_or(&[])))
    }

    /// A layer's elements at `frame` with the document's bindings applied (see
    /// [`Document::bound_element`]). Costs one check when the document binds nothing.
    pub fn bound_elements<'a>(&self, els: Cow<'a, [Arc<Element>]>, frame: u32) -> Cow<'a, [Arc<Element>]> {
        if self.data.bindings.is_empty() {
            return els;
        }
        let mut els = els;
        for i in 0..els.len() {
            let Some(bound) = els.get(i).and_then(|e| self.bound_element(e, frame)) else { continue };
            if let Some(slot) = els.to_mut().get_mut(i) {
                *slot = Arc::new(bound);
            }
        }
        els
    }

    /// Repairs loaded data (see [`DataSet::sanitize`]); tables also get usable, unique Library
    /// names and folders that exist, and bindings of instances that are nowhere in the document
    /// are dropped. Ids count towards `max_id`. Touches the data only if something needs it.
    pub(crate) fn sanitize_data(&mut self, max_id: &mut u64) {
        if self.data.is_empty() {
            return;
        }
        let mut data = (*self.data).clone();
        let mut changed = data.sanitize();
        let mut names: Vec<String> = Vec::new();
        for (i, t) in data.tables.iter_mut().enumerate() {
            *max_id = (*max_id).max(t.id.0);
            let cleaned = crate::library::clean_name(&t.name).unwrap_or_else(|_| format!("Table {}", i + 1));
            let mut name = cleaned.clone();
            let mut n = 2usize;
            while names.iter().any(|o| same_name(o, &name)) && n <= MAX_TABLES + 2 {
                name = format!("{} {n}", cleaned.chars().take(crate::library::MAX_NAME_CHARS - 8).collect::<String>());
                n += 1;
            }
            let folder = t.folder.filter(|f| self.folder(*f).is_some());
            if name != t.name || folder != t.folder {
                let t = Arc::make_mut(t);
                t.name = name.clone();
                t.folder = folder;
                changed = true;
            }
            names.push(name);
        }
        if !data.bindings.is_empty() {
            let mut live = std::collections::HashSet::new();
            for tl in self.scenes.iter().map(|s| &s.timeline).chain(self.library.iter().map(|s| &s.timeline)) {
                for e in tl.layers.iter().flat_map(|l| l.keyframes.iter()).flat_map(|k| k.elements.iter()) {
                    live.insert(e.id);
                }
            }
            let before = data.bindings.len();
            data.bindings.retain(|b| live.contains(&b.element));
            changed |= data.bindings.len() != before;
        }
        if changed {
            self.data = Arc::new(data);
        }
    }
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
