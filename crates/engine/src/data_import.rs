//! Reading a data table (ADR 0118) from CSV, TSV or JSON text.
//!
//! Delimited text goes through `craft-csv` (craft-libs): encoding and delimiter detection and a
//! bounded record parser; what a field means is decided here. JSON is read by a small serde
//! visitor that keeps numbers only and stops storing at the table's limits, so a hostile file
//! costs no more memory than the largest table. Nothing here touches a document or a file.
//!
//! * The first record names the columns, unless every field of it is a number (then the
//!   columns are `Column 1`, `Column 2`, …).
//! * A cell that isn't a finite number is a gap. A column with no number at all (sample ids,
//!   notes) is left out and reported.
//! * JSON may be an array of records (`[{"age": 1, "pinus": 3}, …]`), an array of rows
//!   (`[["age", "pinus"], [1, 3], …]`), an object of columns (`{"age": [1, …], "pinus": [3, …]}`)
//!   or `{"columns": [names], "rows": [[…], …]}`; a plain array of numbers is one column.

use anicraft_doc::DataColumn;
use anicraft_doc::data::{MAX_CELLS, MAX_COLUMNS, MAX_COLUMN_NAME, MAX_ROWS, clean_column_name};
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;

/// Largest file `data.import_table` reads.
pub const MAX_FILE_BYTES: u64 = 32 << 20;

/// How a table file is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Comma, semicolon or tab separated: the delimiter is detected.
    Delimited,
    Tsv,
    Json,
}

impl Format {
    pub fn from_key(s: &str) -> Option<Option<Format>> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(None),
            "csv" => Some(Some(Format::Delimited)),
            "tsv" | "tab" => Some(Some(Format::Tsv)),
            "json" => Some(Some(Format::Json)),
            _ => None,
        }
    }

    /// The format of a file named `name` holding `text`: by its extension, else by how the text
    /// starts.
    pub fn detect(name: &str, text: &str) -> Format {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "json" => Format::Json,
            "tsv" | "tab" => Format::Tsv,
            "csv" => Format::Delimited,
            _ if text.trim_start().starts_with(['{', '[']) => Format::Json,
            _ => Format::Delimited,
        }
    }
}

/// What a file held.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The columns with numbers, each as long as the table has rows.
    pub columns: Vec<DataColumn>,
    /// Columns left out because they held no number.
    pub skipped: Vec<String>,
    /// Cells of the kept columns with no number.
    pub gaps: usize,
    /// The file held more rows, columns or cells than a table keeps.
    pub truncated: bool,
}

impl Parsed {
    pub fn rows(&self) -> usize {
        self.columns.first().map_or(0, |c| c.values.len())
    }
}

/// A number as tables write it: plain or scientific, with an optional sign and percent sign;
/// with `decimal_comma` (semicolon or tab separated files) a single comma is the decimal mark.
/// `None` for anything else, and for infinities and NaN.
pub fn parse_number(s: &str, decimal_comma: bool) -> Option<f64> {
    let s = s.trim().trim_end_matches('%').trim_end();
    if s.is_empty() || s.len() > 64 || !s.bytes().any(|b| b.is_ascii_digit()) || s.bytes().any(|b| b.is_ascii_alphabetic() && !matches!(b, b'e' | b'E')) {
        return None;
    }
    let v = if decimal_comma && s.bytes().filter(|b| *b == b',').count() == 1 && !s.contains('.') { s.replace(',', ".").parse::<f64>() } else { s.parse::<f64>() };
    v.ok().filter(|v| v.is_finite())
}

/// A table being read: columns grow together, a row at a time.
#[derive(Default)]
struct Builder {
    names: Vec<String>,
    cols: Vec<Vec<Option<f64>>>,
    /// Each column by its name without case.
    by_name: HashMap<String, usize>,
    rows: usize,
    truncated: bool,
}

impl Builder {
    /// The column called `name` (made when new, padded to the rows so far). `None` past the
    /// column limit.
    fn column(&mut self, name: &str) -> Option<usize> {
        let key = clean_column_name(name).to_lowercase();
        if let Some(&i) = self.by_name.get(&key) {
            return Some(i);
        }
        self.add(name)
    }

    fn add(&mut self, name: &str) -> Option<usize> {
        if self.cols.len() >= MAX_COLUMNS {
            self.truncated = true;
            return None;
        }
        let i = self.cols.len();
        let clean = clean_column_name(name);
        let clean = if clean.is_empty() { format!("Column {}", i + 1) } else { clean };
        self.by_name.entry(clean.to_lowercase()).or_insert(i);
        self.names.push(clean);
        self.cols.push(vec![None; self.rows]);
        Some(i)
    }

    /// Column `i` by position (made as `Column n` when the header was shorter).
    fn nth(&mut self, i: usize) -> Option<usize> {
        while self.cols.len() <= i {
            self.add("")?;
        }
        Some(i)
    }

    /// Whether another row fits.
    fn room(&mut self) -> bool {
        let ok = self.rows < MAX_ROWS && (self.rows + 1).saturating_mul(self.cols.len().max(1)) <= MAX_CELLS;
        if !ok {
            self.truncated = true;
        }
        ok
    }

    /// Ends the row: every column gets a cell (a gap where the row had none).
    fn end_row(&mut self) {
        self.rows += 1;
        for c in &mut self.cols {
            c.resize(self.rows, None);
        }
    }

    fn set(&mut self, col: usize, v: Option<f64>) {
        if let Some(c) = self.cols.get_mut(col) {
            c.resize(self.rows + 1, None);
            if let Some(slot) = c.get_mut(self.rows) {
                *slot = v;
            }
        }
    }

    fn finish(mut self) -> Result<Parsed, String> {
        for c in &mut self.cols {
            c.resize(self.rows, None);
        }
        let mut columns = Vec::new();
        let mut skipped = Vec::new();
        for (name, values) in self.names.into_iter().zip(self.cols) {
            if values.iter().any(Option::is_some) { columns.push(DataColumn { name, values }) } else { skipped.push(name) }
        }
        if columns.is_empty() {
            return Err(if skipped.is_empty() { "the file has no rows of numbers".to_string() } else { "the file has no numbers: a data table keeps numeric columns (cells that aren't numbers become gaps)".to_string() });
        }
        skipped.truncate(MAX_COLUMNS);
        let gaps = columns.iter().map(|c| c.values.iter().filter(|v| v.is_none()).count()).sum();
        Ok(Parsed { columns, skipped, gaps, truncated: self.truncated })
    }
}

/// Reads a table from the bytes of a file called `name` (for its extension), as `format` or as
/// detected.
pub fn parse(bytes: &[u8], name: &str, format: Option<Format>) -> Result<Parsed, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("the file is larger than {} MB, the most a data table is read from", MAX_FILE_BYTES >> 20));
    }
    let text = craft_csv::decode_text(bytes);
    match format.unwrap_or_else(|| Format::detect(name, &text)) {
        Format::Json => parse_json(&text),
        Format::Tsv => parse_delimited(&text, Some('\t')),
        Format::Delimited => parse_delimited(&text, None),
    }
}

/// Reads delimited text; `delimiter` `None` detects comma, semicolon or tab.
pub fn parse_delimited(text: &str, delimiter: Option<char>) -> Result<Parsed, String> {
    let delim = delimiter.unwrap_or_else(|| char::from(craft_csv::sniff_delimiter(text, '"')));
    let decimal_comma = delim != ',';
    let mut b = Builder::default();
    let mut first = true;
    craft_csv::parse_records(text, delim, '"', MAX_COLUMNS + 1, |record| {
        // A blank line is no row.
        if record.iter().all(|f| f.trim().is_empty()) {
            return true;
        }
        if record.len() > MAX_COLUMNS {
            b.truncated = true;
        }
        if first {
            first = false;
            let header = record.iter().any(|f| !f.trim().is_empty() && parse_number(f, decimal_comma).is_none());
            if header {
                for f in record.iter().take(MAX_COLUMNS) {
                    // (A repeated name still gets its own column: `add`, not `column`.)
                    b.add(f);
                }
                return true;
            }
        }
        if !b.room() {
            return false;
        }
        for (i, f) in record.iter().take(MAX_COLUMNS).enumerate() {
            if let Some(c) = b.nth(i) {
                b.set(c, parse_number(f, decimal_comma));
            }
        }
        b.end_row();
        true
    });
    b.finish()
}

// ----- JSON -------------------------------------------------------------------------------------

/// How many cells may still be stored, and whether any were dropped.
struct Limits {
    cells: Cell<usize>,
    truncated: Cell<bool>,
}

impl Limits {
    fn take(&self) -> bool {
        match self.cells.get() {
            0 => {
                self.truncated.set(true);
                false
            }
            n => {
                self.cells.set(n - 1);
                true
            }
        }
    }
}

#[derive(Debug)]
enum JCell {
    Num(f64),
    /// Text that isn't a number (its first characters: a column name, or a gap).
    Text(String),
    Gap,
}

impl JCell {
    fn number(&self) -> Option<f64> {
        match self {
            JCell::Num(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum JItem {
    Cell(JCell),
    Row(Vec<JCell>),
    Record(Vec<(String, JCell)>),
}

fn text_cell(s: &str) -> JCell {
    match parse_number(s, false) {
        Some(v) => JCell::Num(v),
        None => JCell::Text(s.chars().take(MAX_COLUMN_NAME * 2).collect()),
    }
}

fn num_cell(v: f64) -> JCell {
    if v.is_finite() { JCell::Num(v) } else { JCell::Gap }
}

/// One scalar; an array or object where a cell belongs is read past and is a gap.
struct CellSeed;

impl<'de> DeserializeSeed<'de> for CellSeed {
    type Value = JCell;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<JCell, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for CellSeed {
    type Value = JCell;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a number")
    }
    fn visit_f64<E>(self, v: f64) -> Result<JCell, E> {
        Ok(num_cell(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<JCell, E> {
        Ok(num_cell(v as f64))
    }
    fn visit_u64<E>(self, v: u64) -> Result<JCell, E> {
        Ok(num_cell(v as f64))
    }
    fn visit_str<E>(self, v: &str) -> Result<JCell, E> {
        Ok(text_cell(v))
    }
    fn visit_bool<E>(self, _: bool) -> Result<JCell, E> {
        Ok(JCell::Gap)
    }
    fn visit_unit<E>(self) -> Result<JCell, E> {
        Ok(JCell::Gap)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<JCell, A::Error> {
        while a.next_element::<IgnoredAny>()?.is_some() {}
        Ok(JCell::Gap)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<JCell, A::Error> {
        while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(JCell::Gap)
    }
}

/// A row: a scalar, an array of scalars or an object of them.
struct ItemSeed<'a>(&'a Limits);

impl<'de> DeserializeSeed<'de> for ItemSeed<'_> {
    type Value = JItem;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<JItem, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ItemSeed<'_> {
    type Value = JItem;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a row of numbers")
    }
    fn visit_f64<E>(self, v: f64) -> Result<JItem, E> {
        Ok(JItem::Cell(num_cell(v)))
    }
    fn visit_i64<E>(self, v: i64) -> Result<JItem, E> {
        Ok(JItem::Cell(num_cell(v as f64)))
    }
    fn visit_u64<E>(self, v: u64) -> Result<JItem, E> {
        Ok(JItem::Cell(num_cell(v as f64)))
    }
    fn visit_str<E>(self, v: &str) -> Result<JItem, E> {
        Ok(JItem::Cell(text_cell(v)))
    }
    fn visit_bool<E>(self, _: bool) -> Result<JItem, E> {
        Ok(JItem::Cell(JCell::Gap))
    }
    fn visit_unit<E>(self) -> Result<JItem, E> {
        Ok(JItem::Cell(JCell::Gap))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<JItem, A::Error> {
        let mut row = Vec::new();
        while let Some(c) = a.next_element_seed(CellSeed)? {
            if row.len() < MAX_COLUMNS && self.0.take() {
                row.push(c);
            } else {
                self.0.truncated.set(true);
            }
        }
        Ok(JItem::Row(row))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<JItem, A::Error> {
        let mut rec = Vec::new();
        while let Some(k) = a.next_key::<String>()? {
            let c = a.next_value_seed(CellSeed)?;
            if rec.len() < MAX_COLUMNS && self.0.take() {
                rec.push((k, c));
            } else {
                self.0.truncated.set(true);
            }
        }
        Ok(JItem::Record(rec))
    }
}

/// An array of rows (anything else is read past and is empty).
struct ItemsSeed<'a>(&'a Limits);

impl<'de> DeserializeSeed<'de> for ItemsSeed<'_> {
    type Value = Vec<JItem>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Vec<JItem>, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ItemsSeed<'_> {
    type Value = Vec<JItem>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array")
    }
    fn visit_f64<E>(self, _: f64) -> Result<Vec<JItem>, E> {
        Ok(Vec::new())
    }
    fn visit_i64<E>(self, _: i64) -> Result<Vec<JItem>, E> {
        Ok(Vec::new())
    }
    fn visit_u64<E>(self, _: u64) -> Result<Vec<JItem>, E> {
        Ok(Vec::new())
    }
    fn visit_str<E>(self, _: &str) -> Result<Vec<JItem>, E> {
        Ok(Vec::new())
    }
    fn visit_bool<E>(self, _: bool) -> Result<Vec<JItem>, E> {
        Ok(Vec::new())
    }
    fn visit_unit<E>(self) -> Result<Vec<JItem>, E> {
        Ok(Vec::new())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Vec<JItem>, A::Error> {
        while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Vec::new())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Vec<JItem>, A::Error> {
        let mut out = Vec::new();
        while let Some(item) = a.next_element_seed(ItemSeed(self.0))? {
            // One more than the rows a table keeps: the first may name the columns.
            let fits = out.len() <= MAX_ROWS && (!matches!(item, JItem::Cell(_)) || self.0.take());
            if fits {
                out.push(item);
            } else {
                self.0.truncated.set(true);
            }
        }
        Ok(out)
    }
}

enum Top {
    Rows(Vec<JItem>),
    Object(Vec<(String, Vec<JItem>)>),
}

struct TopSeed<'a>(&'a Limits);

impl<'de> DeserializeSeed<'de> for TopSeed<'_> {
    type Value = Top;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Top, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for TopSeed<'_> {
    type Value = Top;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of rows or an object of columns")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Top, A::Error> {
        ItemsSeed(self.0).visit_seq(a).map(Top::Rows)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Top, A::Error> {
        let mut out = Vec::new();
        while let Some(k) = a.next_key::<String>()? {
            let v = a.next_value_seed(ItemsSeed(self.0))?;
            if out.len() < MAX_COLUMNS + 2 {
                out.push((k, v));
            } else {
                self.0.truncated.set(true);
            }
        }
        Ok(Top::Object(out))
    }
}

/// Adds `items` as rows: arrays by position (the first naming the columns when it is all
/// text, or `names` when given), objects by name.
fn rows_into(b: &mut Builder, items: Vec<JItem>, names: Option<Vec<String>>) {
    let mut first = names.is_none();
    if let Some(names) = names {
        for n in names.iter().take(MAX_COLUMNS) {
            b.add(n);
        }
    }
    for item in items {
        let was_first = std::mem::replace(&mut first, false);
        match item {
            JItem::Row(row) => {
                if was_first && !row.is_empty() && row.iter().all(|c| matches!(c, JCell::Text(_))) {
                    for c in &row {
                        if let JCell::Text(t) = c {
                            b.add(t);
                        }
                    }
                    continue;
                }
                if !b.room() {
                    return;
                }
                for (i, c) in row.iter().enumerate() {
                    if let Some(col) = b.nth(i) {
                        b.set(col, c.number());
                    }
                }
                b.end_row();
            }
            JItem::Record(rec) => {
                if !b.room() {
                    return;
                }
                for (k, c) in &rec {
                    if let Some(col) = b.column(k) {
                        b.set(col, c.number());
                    }
                }
                b.end_row();
            }
            // A scalar among rows: a row of one cell.
            JItem::Cell(c) => {
                if !b.room() {
                    return;
                }
                if let Some(col) = b.nth(0) {
                    b.set(col, c.number());
                }
                b.end_row();
            }
        }
    }
}

/// Reads a table from JSON text (see the module's notes for the shapes).
pub fn parse_json(text: &str) -> Result<Parsed, String> {
    let limits = Limits { cells: Cell::new(MAX_CELLS + MAX_COLUMNS), truncated: Cell::new(false) };
    let mut de = serde_json::Deserializer::from_str(text);
    let top = TopSeed(&limits).deserialize(&mut de).map_err(|e| format!("not a JSON table ({e}): a data table is an array of rows or records, or an object of columns"))?;
    de.end().map_err(|e| format!("not a JSON table ({e})"))?;
    let mut b = Builder { truncated: limits.truncated.get(), ..Builder::default() };
    match top {
        Top::Rows(items) => {
            if !items.is_empty() && items.iter().all(|i| matches!(i, JItem::Cell(_))) {
                b.add("value");
            }
            rows_into(&mut b, items, None);
        }
        Top::Object(mut entries) => {
            let is_rows = |items: &[JItem]| items.iter().any(|i| !matches!(i, JItem::Cell(_)));
            if let Some(at) = entries.iter().position(|(k, v)| k == "rows" && is_rows(v)) {
                let names = entries.iter().find(|(k, _)| k == "columns").map(|(_, v)| {
                    v.iter()
                        .map(|i| match i {
                            JItem::Cell(JCell::Text(t)) => t.clone(),
                            JItem::Cell(JCell::Num(n)) => n.to_string(),
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>()
                });
                let rows = entries.get_mut(at).map(|e| std::mem::take(&mut e.1)).unwrap_or_default();
                rows_into(&mut b, rows, names.filter(|n| !n.is_empty()));
            } else {
                // Columns: the table is as long as the longest.
                let rows = entries.iter().map(|(_, v)| v.len()).max().unwrap_or(0).min(MAX_ROWS);
                for (name, items) in &entries {
                    if is_rows(items) {
                        continue;
                    }
                    let Some(col) = b.add(name) else { break };
                    if let Some(c) = b.cols.get_mut(col) {
                        *c = items.iter().take(rows).map(|i| if let JItem::Cell(c) = i { c.number() } else { None }).collect();
                    }
                }
                let cols = b.cols.len().max(1);
                let keep = rows.min(MAX_CELLS / cols);
                b.truncated |= keep < entries.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
                b.rows = keep;
                for c in &mut b.cols {
                    c.resize(keep, None);
                }
            }
        }
    }
    b.finish()
}

#[cfg(test)]
#[path = "data_import_tests.rs"]
mod tests;
