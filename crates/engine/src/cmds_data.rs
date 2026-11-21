//! Data tables that drive animation (ADR 0118): the `data.*` commands.
//!
//! * Tables: `data.import_table` reads a CSV, TSV or JSON file ([`crate::data_import`]),
//!   `data.set_table` takes the values inline (scripts, MCP, plug-ins); importing or setting a
//!   table that is already there replaces its values and keeps its id, so every binding stays.
//!   `data.list` and `data.get` read them; `data.set_cell` and `data.set_key` edit one.
//! * Bindings: `data.bind` lets a column drive a property of a symbol instance over a span of
//!   frames, `data.unbind` takes that away, `data.bindings` lists them with the values at the
//!   playhead, and `data.bake` writes what the bindings show as plain keyframes.
//!
//! The model, the sampling and what a binding does to its instance are `anicraft_doc::data`
//! (pure, applied inside `evaluate`); this module checks params and edits the document.

use crate::data_import::{self, Format, Parsed};
use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::data::{MAX_BINDINGS, MAX_SPAN, MAX_TABLES, MAX_VALUE, with_values};
use anicraft_doc::motion::MAX_KEYS;
use anicraft_doc::{Binding, BoundProperty, DataId, DataTable, Document, Element, ElementId, ElementKind, Interp, LayerId, LayerKind, LibraryItem, LoopMode, SymbolId, SymbolKind, Timeline};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Most rows `data.get` returns at once.
pub const MAX_GET_ROWS: usize = 10_000;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, params, mutates, run| CommandSpec { id, label, menu, shortcut: None, params, mutates, run };
    vec![
        c(
            "data.import_table",
            "Import Data Table…",
            "File/Import",
            "{path (.csv, .tsv or .json, up to 32 MB: the first record names the columns; cells that aren't numbers become gaps, and a column with no number is left out), name? (default the file's name), table? (id or name of a table to replace: its bindings stay), key? (the column rows are looked up by, e.g. an age or a time; default by row number, or the replaced table's key), format?: auto|csv|tsv|json} → {table, name, rows, columns, key, skipped, gaps, truncated, replaced, bindings, missing (columns bindings read that are gone)}",
            true,
            import_table as crate::RunFn,
        ),
        c(
            "data.set_table",
            "Set Data Table",
            "",
            "{name, columns: [{name, values: [number | null, ..]}, ..] (in this order) | {name: [..], ..} (sorted by name) | [names] with rows: [[..], ..], key?, table? (id or name of the table to replace; default the table called `name`)} the table's values inline (null is a gap); replacing a table keeps its bindings → as data.import_table",
            true,
            set_table,
        ),
        c("data.list", "Data Tables", "", "{} → {tables: [{table, name, rows, columns: [names], key, source, folder, bindings}], bindings (how many in the document)}", false, list),
        c("data.get", "Data Table Values", "", "{table (id or name), from? (first row, 0-based), count? (rows, at most 10,000)} → {table, name, key, rows, from, columns: [{name, values: [number | null, ..]}]}", false, get),
        c("data.set_cell", "Set Data Cell", "", "{table (id or name), column, row (0-based), value (number, or null for a gap)}: every binding that reads the cell shows the new value", true, set_cell),
        c("data.set_key", "Set Key Column", "", "{table (id or name), key (a column's name, or null: rows are looked up by row number)}: a table with a key keeps its rows in the key's ascending order", true, set_key),
        c("data.remove", "Delete Data Table", "", "{table (id or name)} deletes the table and the bindings that read it (undoable) → {deleted, uses}", true, remove),
        c(
            "data.bind",
            "Bind to Data…",
            "Modify/Data",
            "{property: x|y|scale_x|scale_y|rotation|alpha|frame (x, y pixels; scales and alpha percent; rotation degrees; frame: the 0-based frame of its symbol a graphic instance shows), table (id or name), column, element? (a symbol instance's id; default the selected one), start? (the span's first frame, 0-based; default the first frame the instance is on), frames? (the span's length; default one frame per row, or with a key column the frames the instance is on), from?, to? (the keys the first and last frame show; default the table's first and last; to before from plays it backwards), interpolation?: linear|step, scale?, offset? (value = column × scale + offset)} one binding per property of an instance; before the span it shows the first frame's value, after it the last's → {element, property, table, column, start, frames, value}",
            true,
            bind,
        ),
        c("data.unbind", "Remove Data Bindings", "Modify/Data", "{element? (default every selected instance), property? (default all of them)} → {removed}", true, unbind),
        c("data.bindings", "Data Bindings", "", "{element? (default every binding in the document)} → {bindings: [{element, property, table, table_name, column, start, frames, from, to, interpolation, scale, offset, live (the table and column exist), value (at the playhead, or null)}]}", false, bindings),
        c(
            "data.bake",
            "Bake Data to Keyframes",
            "Modify/Data",
            "{element? (default every selected instance with bindings), property? (default all of them)} writes what the bindings show as keyframes on the instance's layer (one where the value changes; one per frame inside a classic tween) and removes the bindings: every frame looks as it did → {keys, elements, removed}",
            true,
            bake,
        ),
    ]
}

fn state(msg: impl Into<String>) -> EngineError {
    EngineError::State(msg.into())
}

fn param(msg: impl Into<String>) -> EngineError {
    EngineError::Param(msg.into())
}

fn short(s: &str) -> String {
    s.chars().take(60).collect()
}

/// The table param `key` names: an id, or a name (without case).
fn table_param(s: &Session, p: &Value, key: &str) -> Result<Option<DataId>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => s.doc.data_table_named(name).map(|t| Some(t.id)).ok_or_else(|| state(format!("no data table named '{}' (data.list shows them)", short(name)))),
        Some(v) => {
            let id = v.as_u64().map(DataId).ok_or_else(|| param(format!("param '{key}' must be a data table's id or name")))?;
            s.doc.data_table(id).map(|t| Some(t.id)).ok_or_else(|| state(format!("no data table with id {}", id.0)))
        }
    }
}

fn table_req(s: &Session, p: &Value) -> Result<DataId> {
    table_param(s, p, "table")?.ok_or_else(|| param("param 'table' is required (a data table's id or name)"))
}

fn table_mut(s: &mut Session, id: DataId) -> Result<&mut DataTable> {
    Arc::make_mut(&mut s.doc.data).tables.iter_mut().find(|t| t.id == id).map(Arc::make_mut).ok_or_else(|| state("the data table went away"))
}

fn table_json(d: &Document, t: &DataTable) -> Value {
    json!({
        "table": t.id.0,
        "name": t.name,
        "rows": t.rows(),
        "columns": t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        "key": t.key,
        "source": t.source,
        "folder": t.folder.map(|f| f.0),
        "bindings": d.data.uses(t.id),
    })
}

/// A name no other table has (commands name tables by name) and nothing else at the Library's
/// top level has.
fn fresh_name(d: &Document, wanted: &str) -> String {
    let base = d.unique_name(None, if wanted.trim().is_empty() { "Table" } else { wanted });
    if d.data_table_named(&base).is_none() {
        return base;
    }
    (2..=MAX_TABLES + 2).map(|n| format!("{base} {n}")).find(|c| d.data_table_named(c).is_none() && !d.name_taken(None, c, None)).unwrap_or(base)
}

/// The key a store sets: `None` keeps what the table has, `Some(None)` clears it.
type KeyParam = Option<Option<String>>;

fn key_param(p: &Value) -> Result<KeyParam> {
    match p.get("key") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(k)) => Ok(Some(Some(k.clone()))),
        Some(_) => Err(param("param 'key' must be a column's name, or null")),
    }
}

/// Puts `parsed` in the document: over table `target` (or the one called `name`), keeping its
/// id, name, folder and bindings; else as a new table.
fn store(s: &mut Session, name: Option<&str>, target: Option<DataId>, parsed: Parsed, key: KeyParam, source: &str) -> Result<Value> {
    let Parsed { columns, skipped, gaps, truncated } = parsed;
    if let Some(Some(k)) = &key
        && !columns.iter().any(|c| c.name.to_lowercase() == k.trim().to_lowercase())
    {
        let have: Vec<&str> = columns.iter().map(|c| c.name.as_str()).take(20).collect();
        return Err(param(format!("the table has no column '{}' to be its key (it has: {})", short(k), have.join(", "))));
    }
    let target = target.or_else(|| name.and_then(|n| s.doc.data_table_named(n)).map(|t| t.id));
    let (id, replaced) = match target {
        Some(id) => {
            let t = table_mut(s, id)?;
            t.columns = columns;
            match key {
                Some(k) => t.key = k.map(|k| k.trim().to_string()),
                // The key it had, while the column is still there.
                None => t.key = t.key.take().filter(|k| t.columns.iter().any(|c| c.name.to_lowercase() == k.to_lowercase())),
            }
            if !source.is_empty() {
                t.source = source.to_string();
            }
            t.normalize();
            (id, true)
        }
        None => {
            if s.doc.data.tables.len() >= MAX_TABLES {
                return Err(state(format!("a document keeps at most {MAX_TABLES} data tables; delete one first")));
            }
            let name = match name {
                Some(n) => {
                    let n = anicraft_doc::library::clean_name(n).map_err(|e| param(e.to_string()))?;
                    if s.doc.name_taken(None, &n, None) { fresh_name(&s.doc, &n) } else { n }
                }
                None => fresh_name(&s.doc, "Table"),
            };
            let id = DataId(s.doc.alloc_id());
            let mut t = DataTable::new(id, name, columns, key.flatten().map(|k| k.trim().to_string()));
            t.source = source.to_string();
            t.normalize();
            Arc::make_mut(&mut s.doc.data).tables.push(Arc::new(t));
            (id, false)
        }
    };
    let d = &s.doc;
    let t = d.data_table(id).ok_or_else(|| state("the data table went away"))?;
    // Columns that bindings read and the table no longer has: those bindings rest until it does.
    let mut missing: Vec<&str> = d.data.bindings.iter().filter(|b| b.table == id && t.column_index(&b.column).is_none()).map(|b| b.column.as_str()).collect();
    missing.sort_unstable();
    missing.dedup();
    let mut out = table_json(d, t);
    out["skipped"] = json!(skipped);
    out["gaps"] = json!(gaps);
    out["truncated"] = json!(truncated);
    out["replaced"] = json!(replaced);
    out["missing"] = json!(missing);
    Ok(out)
}

fn import_table(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(str_req(p, "path")?);
    let format = match opt_str(p, "format")? {
        None => None,
        Some(f) => Format::from_key(f).ok_or_else(|| param(format!("unknown format '{}' (auto, csv, tsv or json)", short(f))))?,
    };
    let target = table_param(s, p, "table")?;
    let key = key_param(p)?;
    if crate::vfs::len(&path)? > data_import::MAX_FILE_BYTES {
        return Err(param(format!("{} is larger than {} MB, the most a data table is read from", path.display(), data_import::MAX_FILE_BYTES >> 20)));
    }
    let file: String = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let parsed = data_import::parse(&crate::vfs::read(&path)?, &file, format).map_err(|e| param(format!("{}: {e}", path.display())))?;
    let stem: String = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().replace('/', "-").chars().take(128).collect());
    let name = match opt_str(p, "name")? {
        Some(n) => Some(n.to_string()),
        None if target.is_some() => None,
        None => Some(if stem.trim().is_empty() { "Table".to_string() } else { stem }),
    };
    store(s, name.as_deref(), target, parsed, key, &file)
}

/// The inline values of `data.set_table` as JSON text one of [`data_import::parse_json`]'s
/// shapes reads.
fn inline_json(p: &Value) -> Result<String> {
    let cols = p.get("columns").filter(|v| !v.is_null()).ok_or_else(|| param("param 'columns' is required: {name: [numbers], ..}, [{name, values}, ..], or [names] with 'rows'"))?;
    let v = match (cols, p.get("rows").filter(|v| !v.is_null())) {
        (Value::Array(_), Some(rows @ Value::Array(_))) => json!({"columns": cols, "rows": rows}),
        (_, Some(_)) => return Err(param("with 'rows' (a list of rows), 'columns' is the list of column names")),
        (Value::Object(_), None) => cols.clone(),
        (Value::Array(list), None) => {
            // Written by hand: the columns keep the order they were given in (an object's
            // members reach a command sorted by name).
            let mut parts = Vec::with_capacity(list.len());
            for (i, c) in list.iter().enumerate() {
                let name = c.get("name").and_then(Value::as_str).map_or_else(|| format!("Column {}", i + 1), str::to_string);
                let values = c.get("values").filter(|v| v.is_array()).ok_or_else(|| param("each of 'columns' is {name, values: [numbers]}"))?;
                parts.push(format!("{}: {values}", Value::String(name)));
            }
            return Ok(format!("{{{}}}", parts.join(", ")));
        }
        _ => return Err(param("param 'columns' must be {name: [numbers], ..}, [{name, values}, ..], or [names] with 'rows'")),
    };
    serde_json::to_string(&v).map_err(|e| param(e.to_string()))
}

fn set_table(s: &mut Session, p: &Value) -> Result<Value> {
    let target = table_param(s, p, "table")?;
    let name = opt_str(p, "name")?;
    if target.is_none() && name.is_none() {
        return Err(param("param 'name' is required (the table's name), or 'table' to replace one"));
    }
    let key = key_param(p)?;
    let parsed = data_import::parse_json(&inline_json(p)?).map_err(param)?;
    store(s, name, target, parsed, key, "")
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let d = &s.doc;
    let tables: Vec<Value> = d.data.tables.iter().map(|t| table_json(d, t)).collect();
    Ok(json!({"tables": tables, "bindings": d.data.bindings.len()}))
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let id = table_req(s, p)?;
    let t = s.doc.data_table(id).ok_or_else(|| state("the data table went away"))?;
    let rows = t.rows();
    let from = (opt_u64(p, "from")?.unwrap_or(0) as usize).min(rows);
    let count = opt_u64(p, "count")?.map_or(MAX_GET_ROWS, |n| n as usize).min(MAX_GET_ROWS).min(rows - from);
    let columns: Vec<Value> = t.columns.iter().map(|c| json!({"name": c.name, "values": c.values.iter().skip(from).take(count).collect::<Vec<_>>()})).collect();
    Ok(json!({"table": t.id.0, "name": t.name, "key": t.key, "rows": rows, "from": from, "count": count, "columns": columns}))
}

fn set_cell(s: &mut Session, p: &Value) -> Result<Value> {
    let id = table_req(s, p)?;
    let column = str_req(p, "column")?.to_string();
    let row = opt_u64(p, "row")?.ok_or_else(|| param("param 'row' is required (0-based)"))? as usize;
    let value = match p.get("value") {
        None => return Err(param("param 'value' is required (a number, or null for a gap)")),
        Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| param("param 'value' must be a finite number, or null for a gap"))?),
    };
    let t = table_mut(s, id)?;
    let rows = t.rows();
    let ci = t.column_index(&column).ok_or_else(|| param(format!("the table has no column '{}'", short(&column))))?;
    let slot = t.columns.get_mut(ci).and_then(|c| c.values.get_mut(row)).ok_or_else(|| param(format!("the table has {rows} rows (0 to {})", rows.saturating_sub(1))))?;
    *slot = value;
    // A changed key may move the row.
    t.normalize();
    Ok(json!({"table": id.0, "column": column, "row": row, "value": value}))
}

fn set_key(s: &mut Session, p: &Value) -> Result<Value> {
    let id = table_req(s, p)?;
    let key = key_param(p)?.ok_or_else(|| param("param 'key' is required (a column's name, or null)"))?;
    let t = table_mut(s, id)?;
    t.key = match key {
        None => None,
        Some(k) => Some(t.column(&k).map(|c| c.name.clone()).ok_or_else(|| param(format!("the table has no column '{}'", short(&k))))?),
    };
    t.normalize();
    Ok(json!({"table": id.0, "key": t.key}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let id = table_req(s, p)?;
    Ok(crate::cmds_library::remove(s, &[LibraryItem::Data(id)], &[]))
}

/// Where an instance is: its layer, the first frame it is on and how many frames from there to
/// the end of the last keyframe that holds it.
struct Found {
    layer: LayerId,
    first: u32,
    life: u32,
    el: Arc<Element>,
    /// On the timeline being edited (else on another scene's, or inside a symbol).
    here: bool,
}

fn find_in(tl: &Timeline, id: ElementId) -> Option<Found> {
    for l in &tl.layers {
        let mut found: Option<Found> = None;
        for k in &l.keyframes {
            let Some(e) = k.elements.iter().find(|e| e.id == id) else { continue };
            match &mut found {
                Some(f) => f.life = k.end().saturating_sub(f.first),
                None => found = Some(Found { layer: l.id, first: k.start, life: k.duration.max(1), el: e.clone(), here: false }),
            }
        }
        if found.is_some() {
            return found;
        }
    }
    None
}

fn find(s: &Session, id: ElementId) -> Option<Found> {
    if let Some(f) = find_in(s.timeline(), id) {
        return Some(Found { here: true, ..f });
    }
    s.doc.scenes.iter().map(|x| &x.timeline).chain(s.doc.library.iter().map(|x| &x.timeline)).find_map(|tl| find_in(tl, id))
}

/// The instances a command works on: `element`, else the selected ones.
fn targets(s: &Session, p: &Value) -> Result<Vec<ElementId>> {
    if let Some(id) = opt_u64(p, "element")? {
        return Ok(vec![ElementId(id)]);
    }
    let sel: Vec<ElementId> = s.selection.ids().collect();
    if sel.is_empty() {
        return Err(state("nothing is selected: select an instance, or pass 'element'"));
    }
    Ok(sel)
}

fn property_param(p: &Value) -> Result<Option<BoundProperty>> {
    match opt_str(p, "property")? {
        None => Ok(None),
        Some(k) => BoundProperty::from_key(k).map(Some).ok_or_else(|| param(format!("unknown property '{}' ({})", short(k), BoundProperty::ALL.map(BoundProperty::key).join(", ")))),
    }
}

fn opt_f64(p: &Value, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|f| f.is_finite() && f.abs() <= MAX_VALUE).map(Some).ok_or_else(|| param(format!("param '{key}' must be a finite number within ±1,000,000,000"))),
    }
}

fn binding_json(d: &Document, b: &Binding, frame: u32) -> Value {
    let table = d.data_table(b.table);
    json!({
        "element": b.element.0,
        "property": b.property.key(),
        "table": b.table.0,
        "table_name": table.map(|t| t.name.as_str()),
        "column": b.column,
        "start": b.start,
        "frames": b.frames,
        "from": b.from,
        "to": b.to,
        "interpolation": b.interp.key(),
        "scale": b.scale,
        "offset": b.offset,
        "live": table.is_some_and(|t| t.column_index(&b.column).is_some()),
        "value": b.value_at(&d.data, frame),
    })
}

fn bind(s: &mut Session, p: &Value) -> Result<Value> {
    let property = property_param(p)?.ok_or_else(|| param(format!("param 'property' is required ({})", BoundProperty::ALL.map(BoundProperty::key).join(", "))))?;
    let table = table_req(s, p)?;
    let column = str_req(p, "column")?;
    let ids = targets(s, p)?;
    let [id] = ids.as_slice() else { return Err(state("a binding drives one instance: select one, or pass 'element'")) };
    let found = find(s, *id).ok_or_else(|| state(format!("no object with id {} in the document", id.0)))?;
    let ElementKind::Symbol { symbol, .. } = &found.el.kind else {
        return Err(state("data drives symbol instances: convert the artwork with Modify › Convert to Symbol first"));
    };
    if property == BoundProperty::Frame && s.doc.symbol(*symbol).is_none_or(|x| x.kind != SymbolKind::Graphic) {
        return Err(state("only a graphic symbol's instance shows a frame picked by data (a movie clip plays on its own): change the symbol's type to Graphic"));
    }
    let t = s.doc.data_table(table).ok_or_else(|| state("the data table went away"))?;
    let ci = t.column_index(column).ok_or_else(|| {
        let have: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).take(20).collect();
        param(format!("table '{}' has no column '{}' (it has: {})", t.name, short(column), have.join(", ")))
    })?;
    let column = t.columns.get(ci).map(|c| c.name.clone()).unwrap_or_default();
    let start = opt_u32(p, "start")?.unwrap_or(found.first);
    let frames = match opt_u32(p, "frames")? {
        Some(n) => n,
        // One frame per row; with a key, the record over the frames the instance is on.
        None if t.key.is_some() => found.life,
        None => t.rows() as u32,
    };
    if start > MAX_SPAN || !(1..=MAX_SPAN).contains(&frames.max(1)) {
        return Err(param(format!("'start' is at most {MAX_SPAN} and 'frames' between 1 and {MAX_SPAN}")));
    }
    let interp = match opt_str(p, "interpolation")? {
        None => Interp::Linear,
        Some(k) => Interp::from_key(k).ok_or_else(|| param(format!("unknown interpolation '{}' (linear or step)", short(k))))?,
    };
    let b = Binding { element: *id, property, table, column, start, frames: frames.max(1), from: opt_f64(p, "from")?, to: opt_f64(p, "to")?, interp, scale: opt_f64(p, "scale")?.unwrap_or(1.0), offset: opt_f64(p, "offset")?.unwrap_or(0.0) };
    if !Arc::make_mut(&mut s.doc.data).bind(b.clone()) {
        return Err(state(format!("a document keeps at most {MAX_BINDINGS} data bindings")));
    }
    Ok(binding_json(&s.doc, &b, s.frame))
}

fn unbind(s: &mut Session, p: &Value) -> Result<Value> {
    let property = property_param(p)?;
    let ids = targets(s, p)?;
    if !ids.iter().any(|id| s.doc.data.bindings_of(*id).iter().any(|b| property.is_none_or(|q| q == b.property))) {
        return Err(state("no data binding to remove there"));
    }
    let data = Arc::make_mut(&mut s.doc.data);
    let removed: usize = ids.iter().map(|id| data.unbind(*id, property)).sum();
    Ok(json!({"removed": removed}))
}

fn bindings(s: &mut Session, p: &Value) -> Result<Value> {
    let d = &s.doc;
    let list: Vec<Value> = match opt_u64(p, "element")? {
        Some(id) => d.data.bindings_of(ElementId(id)).iter().map(|b| binding_json(d, b, s.frame)).collect(),
        None => d.data.bindings.iter().map(|b| binding_json(d, b, s.frame)).collect(),
    };
    Ok(json!({"bindings": list}))
}

/// Bakes instance `id`'s bindings (of `property`, or all) into keyframes on its layer. How many
/// keyframes were made.
fn bake_one(s: &mut Session, id: ElementId, property: Option<BoundProperty>) -> Result<usize> {
    let found = find(s, id).ok_or_else(|| state(format!("no object with id {} in the document", id.0)))?;
    if !found.here {
        return Err(state("the instance is on another timeline: open the scene or symbol it is in, then bake"));
    }
    let bound: Vec<Binding> = s.doc.data.bindings_of(id).iter().filter(|b| property.is_none_or(|q| q == b.property)).cloned().collect();
    let data = s.doc.data.clone();
    let values_at = |f: u32| -> Vec<(BoundProperty, f32)> { bound.iter().filter_map(|b| b.value_at(&data, f).map(|v| (b.property, v))).collect() };
    let tl = s.timeline();
    let layer = tl.layer(found.layer).ok_or_else(|| state("the layer went away"))?;
    if layer.kind == LayerKind::Armature {
        return Err(state(format!("'{}' is an armature layer: its poses place its objects, so data can't be baked there", layer.name)));
    }
    if layer.locked || tl.locked_by_folder(layer.id) {
        return Err(state(format!("layer '{}' is locked", layer.name)));
    }
    // The frames that get a keyframe.
    let mut cuts: Vec<u32> = Vec::new();
    for (i, k) in layer.keyframes.iter().enumerate() {
        if !k.elements.iter().any(|e| e.id == id) {
            continue;
        }
        if k.motion.is_some() {
            return Err(state(format!("layer '{}' has a motion tween on frame {}: keyframes would cut it up. Remove the motion tween first (Insert › Remove Tween)", layer.name, k.start + 1)));
        }
        let tweened = k.tween.is_some() && layer.keyframes.get(i + 1).is_some_and(|n| n.start == k.end());
        let mut prev = values_at(k.start);
        for f in k.start.saturating_add(1)..k.end() {
            let v = values_at(f);
            if tweened || v != prev {
                cuts.push(f);
                if cuts.len() > MAX_KEYS {
                    return Err(state(format!("that is more than {MAX_KEYS} keyframes at once: shorten the binding's span, or use step interpolation")));
                }
            }
            prev = v;
        }
    }
    // A graphic instance cut onto a new keyframe carries on from the frame it was showing.
    let lengths: HashMap<SymbolId, u32> = s.doc.library.iter().filter(|x| x.kind == SymbolKind::Graphic).map(|x| (x.id, x.timeline.frame_count())).collect();
    let (first, end) = (found.first, found.first.saturating_add(found.life));
    let layer = s.layer_mut(Some(found.layer))?;
    for &f in &cuts {
        // (The keyframe it is cut from: an earlier cut may have made it.)
        let from = layer.keyframe_at(f).map_or(f, |k| k.start);
        crate::cmds_timeline::insert_keyframe_at(layer, f, true)?;
        let Some(k) = layer.keyframe_starting_at(f).and_then(|i| layer.keyframe_mut(i)) else { continue };
        for e in &mut k.elements {
            let ElementKind::Symbol { symbol, loop_mode, first_frame, .. } = &e.kind else { continue };
            let Some(&len) = lengths.get(symbol) else { continue };
            let shown = loop_mode.local_frame(f - from, *first_frame, len);
            if *loop_mode != LoopMode::SingleFrame
                && shown != *first_frame
                && let ElementKind::Symbol { first_frame, .. } = &mut Arc::make_mut(e).kind
            {
                *first_frame = shown;
            }
        }
    }
    // Every keyframe that holds the instance gets it as the bindings show it there.
    let at = layer.keyframes.partition_point(|k| k.start < first);
    let count = layer.keyframes.iter().skip(at).take_while(|k| k.start < end).count();
    for i in at..at + count {
        let Some(k) = layer.keyframe_mut(i) else { continue };
        let vals = values_at(k.start);
        if vals.is_empty() {
            continue;
        }
        if let Some(slot) = k.elements.iter_mut().find(|e| e.id == id) {
            let next = with_values(slot, &vals);
            if next != **slot {
                *slot = Arc::new(next);
            }
        }
    }
    Ok(cuts.len())
}

fn bake(s: &mut Session, p: &Value) -> Result<Value> {
    let property = property_param(p)?;
    let ids: Vec<ElementId> = targets(s, p)?.into_iter().filter(|id| s.doc.data.bindings_of(*id).iter().any(|b| property.is_none_or(|q| q == b.property))).collect();
    if ids.is_empty() {
        return Err(state("no data binding to bake there"));
    }
    let mut keys = 0;
    for id in &ids {
        keys += bake_one(s, *id, property)?;
    }
    let data = Arc::make_mut(&mut s.doc.data);
    let removed: usize = ids.iter().map(|id| data.unbind(*id, property)).sum();
    Ok(json!({"keys": keys, "elements": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "removed": removed}))
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod tests;
