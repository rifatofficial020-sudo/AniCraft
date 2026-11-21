//! Data tables that drive animation (ADR 0118), in the shell.
//!
//! * In the Library a table is an item like the others: its row, a preview of its first rows,
//!   its key column and Import Again; a double-click opens the **table viewer**, a window with
//!   every row and column (only the cells in view are drawn).
//! * A symbol instance's Properties get a **Data** section: Bind to Data adds a binding of a
//!   property to a column; its table, column, span of frames, keys, scale, offset and
//!   interpolation are edited in place, live on the stage; Bake to Keyframes writes what the
//!   bindings show as plain keyframes.
//!
//! Everything here runs `data.*` commands; the only state is which table the viewer shows.

use crate::a11y::{self, Named};
use crate::app::App;
use crate::theme::{self, FORM_LABEL_DOCK, Section, T, Type, color, font, form_row, text, with_type};
use anicraft_engine::doc::data::{MAX_SPAN, MAX_VALUE};
use anicraft_engine::doc::{Binding, BoundProperty, DataId, DataTable, ElementId, ElementKind, Interp, LibraryItem, SymbolKind};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, WidgetType, pos2, vec2};
use serde_json::{Value, json};

/// The engine's English for what the section lists, for the catalogs: translated where drawn.
#[cfg_attr(not(test), allow(dead_code))]
pub const TEXTS: [&str; 9] = [tr_noop!("X"), tr_noop!("Y"), tr_noop!("Scale X"), tr_noop!("Scale Y"), tr_noop!("Rotation"), tr_noop!("Alpha"), tr_noop!("Frame shown"), tr_noop!("Linear"), tr_noop!("Step")];

/// Width of a column of the grid, of its row numbers, and the height of a row.
const CELL_W: f32 = 92.0;
const NUMBER_W: f32 = 52.0;
const ROW_H: f32 = 22.0;

/// What a table is, for its tooltip and screen readers (“Data table · 200 rows × 5 columns”).
pub fn table_detail(t: &DataTable) -> String {
    let rows = trn!("{n} row", "{n} rows", t.rows());
    let columns = trn!("{n} column", "{n} columns", t.columns.len());
    tr!("Data table · {rows} × {columns}", rows = rows, columns = columns)
}

/// A table's Library icon: a small grid.
pub fn paint_table_icon(p: &egui::Painter, r: Rect, c: egui::Color32) {
    p.rect_stroke(r, 2.0, Stroke::new(1.5, c), StrokeKind::Inside);
    let y = r.top() + r.height() * 0.38;
    p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, c));
    p.line_segment([pos2(r.center().x, r.top()), pos2(r.center().x, r.bottom())], Stroke::new(1.0, c));
}

/// A number as the grid shows it: plain, to four decimals, or scientific when very large or
/// small.
pub fn fmt_num(v: f64) -> String {
    let a = v.abs();
    if a != 0.0 && !(1e-4..1e9).contains(&a) {
        return format!("{v:.3e}");
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

fn cell_text(v: Option<f64>) -> String {
    v.map_or_else(|| "–".to_string(), fmt_num)
}

/// The Library's preview of a table: its first rows.
pub fn preview(app: &App, ui: &egui::Ui, r: Rect, id: DataId) {
    let Some(t) = app.s.doc.data_table(id) else { return };
    let p = ui.painter().with_clip_rect(r);
    let cols = ((r.width() / 64.0).floor() as usize).clamp(1, t.columns.len().max(1));
    let w = r.width() / cols as f32;
    let rows = ((r.height() / 15.0).floor() as usize).saturating_sub(1);
    for (i, c) in t.columns.iter().take(cols).enumerate() {
        let x = r.left() + (i + 1) as f32 * w - 4.0;
        let is_key = t.key.as_deref() == Some(c.name.as_str());
        let name: String = c.name.chars().take(10).collect();
        p.text(pos2(x, r.top()), Align2::RIGHT_TOP, name, font(Type::Caption), if is_key { T.accent } else { color(Type::Label) });
        for (j, v) in c.values.iter().take(rows).enumerate() {
            p.text(pos2(x, r.top() + (j + 1) as f32 * 15.0), Align2::RIGHT_TOP, cell_text(*v), font(Type::DataSmall), color(Type::Data));
        }
    }
    p.line_segment([pos2(r.left(), r.top() + 14.0), pos2(r.right(), r.top() + 14.0)], Stroke::new(1.0, T.separator));
}

/// The key column's chooser: Row number, or one of the columns.
fn key_combo(app: &mut App, ui: &mut egui::Ui, id: DataId, place: &str) {
    let Some((key, names)) = app.s.doc.data_table(id).map(|t| (t.key.clone(), t.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>())) else { return };
    let shown = key.clone().unwrap_or_else(|| tr!("Row number").to_string());
    let mut pick: Option<Option<String>> = None;
    egui::ComboBox::from_id_salt(egui::Id::new("data_key").with(place))
        .width(theme::fit(ui, 150.0))
        .selected_text(text(Type::Body, shown))
        .show_ui(ui, |ui| {
            if ui.selectable_label(key.is_none(), tr!("Row number")).clicked() && key.is_some() {
                pick = Some(None);
            }
            for n in &names {
                if ui.selectable_label(key.as_deref() == Some(n.as_str()), n.as_str()).clicked() && key.as_deref() != Some(n.as_str()) {
                    pick = Some(Some(n.clone()));
                }
            }
        })
        .response
        .on_hover_text(tr!("How rows are looked up: by row number (a row per frame), or by a column such as an age, a depth or a time"))
        .set_name(tr!("Key column"));
    if let Some(k) = pick {
        app.run("data.set_key", json!({"table": id.0, "key": k}));
    }
}

/// Asks for a file and imports it over table `id` (its bindings stay).
fn import_again(app: &mut App, id: DataId) {
    // (A plain import clears the table to replace, so it is named after asking.)
    app.invoke("data.import_table");
    app.data_reimport = Some(id);
}

/// The Library's settings of a table: where it came from, its key column, the viewer and
/// Import Again.
pub fn library_properties(app: &mut App, ui: &mut egui::Ui, id: DataId) {
    let Some((detail, source, uses)) = app.s.doc.data_table(id).map(|t| (table_detail(t), t.source.clone(), app.s.doc.data.uses(id))) else { return };
    ui.label(text(Type::Caption, detail));
    if !source.is_empty() {
        ui.label(text(Type::Caption, tr!("From {file}", file = source)));
    }
    if uses > 0 {
        ui.label(text(Type::Caption, trn!("Drives {n} property", "Drives {n} properties", uses)));
    }
    form_row(ui, FORM_LABEL_DOCK, tr!("Key"), |ui| key_combo(app, ui, id, "library"));
    ui.horizontal_wrapped(|ui| {
        if ui.button(tr!("View Table")).on_hover_text(tr!("Shows every row and column (or double-click the table in the list)")).clicked() {
            app.data_view = Some(id);
        }
        if ui.button(tr!("Import Again…")).on_hover_text(tr!("Reads the file again, or a revised one: the table's values change and everything bound to it follows")).clicked() {
            import_again(app, id);
        }
    });
}

/// Every row and column of `t`, scrolling both ways with the column names and row numbers in
/// place; only the cells in view are drawn.
fn grid(ui: &mut egui::Ui, t: &DataTable) {
    let (rows, cols) = (t.rows(), t.columns.len());
    let size = vec2(NUMBER_W + cols as f32 * CELL_W, (rows + 1) as f32 * ROW_H);
    egui::ScrollArea::both().id_salt("data_grid").auto_shrink([false, false]).show_viewport(ui, |ui, view| {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
        a11y::control(ui, &resp, WidgetType::Label, &tr!("{name}: {detail}", name = t.name, detail = table_detail(t)), None);
        let p = ui.painter_at(ui.clip_rect());
        let o = rect.min;
        let (top, left) = (o.y + view.min.y, o.x + view.min.x);
        let r0 = ((view.min.y / ROW_H).floor().max(0.0) as usize).min(rows);
        let r1 = ((view.max.y / ROW_H).ceil().max(0.0) as usize).min(rows);
        let c0 = (((view.min.x - NUMBER_W) / CELL_W).floor().max(0.0) as usize).min(cols);
        let c1 = ((view.max.x / CELL_W).ceil().max(0.0) as usize).min(cols);
        for r in r0..r1 {
            let y = o.y + (r + 1) as f32 * ROW_H;
            if r % 2 == 1 {
                p.rect_filled(Rect::from_min_size(pos2(left, y), vec2(view.width(), ROW_H)), 0.0, T.cell_tint);
            }
            for (c, column) in t.columns.iter().enumerate().take(c1).skip(c0) {
                let v = column.values.get(r).copied().flatten();
                let x = o.x + NUMBER_W + (c + 1) as f32 * CELL_W - 8.0;
                p.text(pos2(x, y + ROW_H / 2.0), Align2::RIGHT_CENTER, cell_text(v), font(Type::Data), if v.is_some() { color(Type::Data) } else { T.text_faint });
            }
        }
        // The column names stay at the top, the row numbers at the left.
        p.rect_filled(Rect::from_min_size(pos2(left, top), vec2(view.width(), ROW_H)), 0.0, T.header);
        for (c, column) in t.columns.iter().enumerate().take(c1).skip(c0) {
            let x = o.x + NUMBER_W + (c + 1) as f32 * CELL_W - 8.0;
            let is_key = t.key.as_deref() == Some(column.name.as_str());
            let name: String = if column.name.chars().count() > 13 { format!("{}…", column.name.chars().take(12).collect::<String>()) } else { column.name.clone() };
            p.text(pos2(x, top + ROW_H / 2.0), Align2::RIGHT_CENTER, name, font(Type::Label), if is_key { T.accent } else { color(Type::Body) });
        }
        p.rect_filled(Rect::from_min_size(pos2(left, top + ROW_H), vec2(NUMBER_W, view.height() - ROW_H)), 0.0, T.header);
        for r in r0..r1 {
            let y = o.y + (r + 1) as f32 * ROW_H;
            if y + ROW_H > top + ROW_H {
                p.text(pos2(left + NUMBER_W - 8.0, y + ROW_H / 2.0), Align2::RIGHT_CENTER, (r + 1).to_string(), font(Type::DataSmall), T.text_dim);
            }
        }
        p.rect_filled(Rect::from_min_size(pos2(left, top), vec2(NUMBER_W, ROW_H)), 0.0, T.header);
        p.line_segment([pos2(left, top + ROW_H), pos2(left + view.width(), top + ROW_H)], Stroke::new(1.0, T.separator));
        p.line_segment([pos2(left + NUMBER_W, top), pos2(left + NUMBER_W, top + view.height())], Stroke::new(1.0, T.separator));
    });
}

/// The table viewer window, while a table is open in it.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.data_view else { return };
    let Some(t) = app.s.doc.data.tables.iter().find(|t| t.id == id).cloned() else {
        app.data_view = None;
        return;
    };
    let mut open = true;
    let at = crate::windows::work_area(ctx).center() - vec2(280.0, 220.0);
    let shown = crate::windows::new(ctx, tr!("Data Table")).id(egui::Id::new("data_table_view")).open(&mut open).default_pos(at).default_size([560.0, 400.0]).min_width(320.0).resizable(true).collapsible(false).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(text(Type::Heading, t.name.as_str()));
            ui.label(text(Type::Caption, table_detail(&t)));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(text(Type::Label, tr!("Key")));
            key_combo(app, ui, id, "viewer");
            if ui.button(tr!("Import Again…")).on_hover_text(tr!("Reads the file again, or a revised one: the table's values change and everything bound to it follows")).clicked() {
                import_again(app, id);
            }
            let uses = app.s.doc.data.uses(id);
            if uses > 0 {
                ui.label(text(Type::Caption, trn!("Drives {n} property", "Drives {n} properties", uses)));
            }
        });
        ui.add_space(4.0);
        if t.rows() == 0 {
            ui.label(text(Type::Caption, tr!("This table has no rows.")));
        } else {
            grid(ui, &t);
        }
    });
    a11y::name_title_bar(&shown, tr!("Data Table"));
    if !open {
        app.data_view = None;
    }
}

// ----- bindings ---------------------------------------------------------------------------------

/// The selected symbol instance, when exactly one is selected: its id and whether its symbol is
/// a graphic.
fn bindable(app: &App) -> Option<(ElementId, bool)> {
    let sel = app.s.selected_elements();
    let [(_, e)] = sel.as_slice() else { return None };
    match &e.kind {
        ElementKind::Symbol { symbol, .. } => Some((e.id, app.s.doc.symbol(*symbol).is_some_and(|s| s.kind == SymbolKind::Graphic))),
        _ => None,
    }
}

/// Whether Modify › Data › Bind to Data has something to bind: one instance and a table.
pub fn can_bind(app: &App) -> bool {
    bindable(app).is_some() && !app.s.doc.data.tables.is_empty()
}

/// Whether the selection has data bindings (to remove or bake).
pub fn has_bindings(app: &App) -> bool {
    app.s.selection.ids().any(|id| !app.s.doc.data.bindings_of(id).is_empty())
}

fn binding_params(b: &Binding) -> Value {
    json!({"element": b.element.0, "property": b.property.key(), "table": b.table.0, "column": b.column, "start": b.start, "frames": b.frames, "from": b.from, "to": b.to, "interpolation": b.interp.key(), "scale": b.scale, "offset": b.offset})
}

/// The first column of `t` that isn't its key.
fn first_column(t: &DataTable) -> Option<&str> {
    t.columns.iter().find(|c| t.key.as_deref() != Some(c.name.as_str())).or(t.columns.first()).map(|c| c.name.as_str())
}

/// Binds `property` of the selected instance to the first table's first column (the span and
/// keys as `data.bind` chooses them), to be changed in Properties.
pub fn add_binding(app: &mut App, property: BoundProperty) {
    let Some((el, _)) = bindable(app) else {
        app.status_err(tr!("Select one symbol instance to bind to data."));
        return;
    };
    // The table the instance's other bindings read, else the first.
    let table = app.s.doc.data.bindings_of(el).first().map(|b| b.table).or_else(|| app.s.doc.data.tables.first().map(|t| t.id));
    let Some((table, column, name)) = table.and_then(|id| app.s.doc.data_table(id)).and_then(|t| first_column(t).map(|c| (t.id, c.to_string(), t.name.clone()))) else {
        app.status_err(tr!("There is no data table yet: import one with File › Import › Import Data Table."));
        return;
    };
    if app.run("data.bind", json!({"element": el.0, "property": property.key(), "table": table.0, "column": column})).is_some() {
        app.show_panel(crate::dock::Panel::Properties);
        app.status_ok(tr!("{property} follows {table} · {column}. Change it under Data in Properties. {undo} to undo.", property = crate::i18n::t(property.label()), table = name, column = column, undo = app.undo_key()));
    }
}

/// Modify › Data › Bind to Data: the first property the instance hasn't bound yet (Scale Y
/// first, a chart's bar).
pub fn bind_from_menu(app: &mut App) {
    let Some((el, graphic)) = bindable(app) else {
        app.status_err(tr!("Select one symbol instance to bind to data."));
        return;
    };
    let bound: Vec<BoundProperty> = app.s.doc.data.bindings_of(el).iter().map(|b| b.property).collect();
    let order = [BoundProperty::ScaleY, BoundProperty::ScaleX, BoundProperty::X, BoundProperty::Y, BoundProperty::Rotation, BoundProperty::Alpha, BoundProperty::Frame];
    match order.into_iter().find(|p| !bound.contains(p) && (graphic || *p != BoundProperty::Frame)) {
        Some(p) => add_binding(app, p),
        None => app.status_ok(tr!("Every property of this instance is bound already: change them under Data in Properties.")),
    }
}

/// The Data section of a symbol instance's Properties: its bindings, each edited in place.
pub fn instance_section(app: &mut App, ui: &mut egui::Ui, el: ElementId) {
    let Some((id, graphic)) = bindable(app).filter(|(id, _)| *id == el) else { return };
    let bindings: Vec<Binding> = app.s.doc.data.bindings_of(id).to_vec();
    let has_tables = !app.s.doc.data.tables.is_empty();
    if bindings.is_empty() && !has_tables {
        return;
    }
    ui.add_space(4.0);
    let count = bindings.len().to_string();
    theme::section(ui, "prop_data", Section::Rule, tr!("Data"), (!bindings.is_empty()).then_some(count.as_str()), |ui| {
        for b in &bindings {
            ui.push_id(("data_binding", b.property.key()), |ui| binding_rows(app, ui, b));
            ui.add_space(4.0);
        }
        ui.horizontal_wrapped(|ui| {
            let free: Vec<BoundProperty> = BoundProperty::ALL.into_iter().filter(|p| !bindings.iter().any(|b| b.property == *p) && (graphic || *p != BoundProperty::Frame)).collect();
            ui.add_enabled_ui(has_tables && !free.is_empty(), |ui| {
                ui.menu_button(tr!("Bind to Data"), |ui| {
                    for p in &free {
                        if ui.button(crate::i18n::t(p.label())).clicked() {
                            ui.close();
                            add_binding(app, *p);
                        }
                    }
                })
                .response
                .on_hover_text(tr!("A column of a data table drives one of this instance's properties, frame by frame"));
            });
            if !bindings.is_empty() && ui.button(tr!("Bake to Keyframes")).on_hover_text(tr!("Writes what the bindings show as plain keyframes and removes the bindings: every frame looks as it did")).clicked() {
                bake(app, id);
            }
        });
    });
}

/// Bake Data to Keyframes for instance `el` (or the selection), with a status line.
pub fn bake(app: &mut App, el: ElementId) {
    if let Some(v) = app.run("data.bake", json!({"element": el.0})) {
        let n = v["keys"].as_u64().unwrap_or(0);
        app.status_ok(trn!("Baked the data into {n} new keyframe. {undo} to undo.", "Baked the data into {n} new keyframes. {undo} to undo.", n, undo = app.undo_key()));
    }
}

/// One binding: its name with Remove, then what it reads and how.
fn binding_rows(app: &mut App, ui: &mut egui::Ui, b: &Binding) {
    let label = crate::i18n::t(b.property.label());
    let table = app.s.doc.data_table(b.table).cloned();
    let value = b.value_at(&app.s.doc.data, app.s.frame);
    let mut next = b.clone();
    ui.horizontal(|ui| {
        ui.label(text(Type::Body, label).strong());
        let shown = match value {
            Some(v) => format!("{}{}", fmt_num(f64::from(v)), b.property.unit()),
            None => tr!("no value").to_string(),
        };
        with_type(ui, Type::Data, |ui| ui.label(text(Type::Data, shown)).on_hover_text(tr!("What the binding gives at the playhead (the instance stays as placed when the table or column is missing or has no number)")));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::timeline::icon_button(ui, &tr!("Remove the {property} binding", property = label), None, crate::icons::Icon::Trash).on_hover_text(tr!("Removes this binding: the property is the instance's own again")).clicked() {
                app.run("data.unbind", json!({"element": b.element.0, "property": b.property.key()}));
            }
        });
    });
    let tables: Vec<(DataId, String)> = app.s.doc.data.tables.iter().map(|t| (t.id, t.name.clone())).collect();
    form_row(ui, FORM_LABEL_DOCK, tr!("Table"), |ui| {
        let shown = table.as_ref().map_or_else(|| tr!("(missing)").to_string(), |t| t.name.clone());
        egui::ComboBox::from_id_salt("data_bind_table")
            .width(theme::fit(ui, 150.0))
            .selected_text(text(Type::Body, shown))
            .show_ui(ui, |ui| {
                for (tid, name) in &tables {
                    if ui.selectable_label(*tid == b.table, name.as_str()).clicked() && *tid != b.table {
                        next.table = *tid;
                        // A column of the same name if the new table has one, else its first.
                        if let Some(t) = app.s.doc.data_table(*tid)
                            && t.column_index(&b.column).is_none()
                        {
                            next.column = first_column(t).unwrap_or("").to_string();
                        }
                        (next.from, next.to) = (None, None);
                    }
                }
            })
            .response
            .set_name(tr!("Data table"));
    });
    form_row(ui, FORM_LABEL_DOCK, tr!("Column"), |ui| {
        let live = table.as_ref().is_some_and(|t| t.column_index(&b.column).is_some());
        let shown = if live { b.column.clone() } else { tr!("{column} (missing)", column = b.column).to_string() };
        egui::ComboBox::from_id_salt("data_bind_column")
            .width(theme::fit(ui, 150.0))
            .selected_text(text(Type::Body, shown))
            .show_ui(ui, |ui| {
                for c in table.iter().flat_map(|t| &t.columns) {
                    if ui.selectable_label(c.name == b.column, c.name.as_str()).clicked() {
                        next.column = c.name.clone();
                    }
                }
            })
            .response
            .set_name(tr!("Column"));
    });
    form_row(ui, FORM_LABEL_DOCK, tr!("Frames"), |ui| {
        with_type(ui, Type::Data, |ui| {
            let mut first = next.start + 1;
            if ui.add(egui::DragValue::new(&mut first).range(1..=MAX_SPAN + 1).speed(0.2)).on_hover_text(tr!("The first frame of the span the table is played over")).named(tr!("First frame of the span")).changed() {
                next.start = first.saturating_sub(1);
            }
            ui.label(text(Type::Caption, tr!("for")));
            ui.add(egui::DragValue::new(&mut next.frames).range(1..=MAX_SPAN).speed(0.2).suffix(" f")).on_hover_text(tr!("How many frames the span has. Before it the instance shows the first frame's value, after it the last's")).named(tr!("Frames in the span"));
        });
    });
    // The keys the span runs between: the table's first and last unless set.
    let range = table.as_ref().and_then(|t| t.key_range());
    let keyed = table.as_ref().is_some_and(|t| t.key.is_some());
    form_row(ui, FORM_LABEL_DOCK, if keyed { tr!("Keys") } else { tr!("Rows") }, |ui| {
        with_type(ui, Type::Data, |ui| {
            let (lo, hi) = range.unwrap_or((0.0, 0.0));
            // Row numbers show from 1, as the viewer numbers them.
            let shift = if keyed { 0.0 } else { 1.0 };
            let (mut from, mut to) = (next.from.unwrap_or(lo) + shift, next.to.unwrap_or(hi) + shift);
            let speed = if keyed { ((hi - lo).abs() / 200.0).max(0.01) } else { 0.1 };
            if ui.add(egui::DragValue::new(&mut from).range(-MAX_VALUE..=MAX_VALUE).speed(speed).max_decimals(3)).on_hover_text(tr!("What the span's first frame shows")).named(tr!("First key of the span")).changed() {
                next.from = Some(from - shift);
            }
            ui.label(text(Type::Caption, tr!("to")));
            if ui.add(egui::DragValue::new(&mut to).range(-MAX_VALUE..=MAX_VALUE).speed(speed).max_decimals(3)).on_hover_text(tr!("What the span's last frame shows. Before the first plays the table backwards")).named(tr!("Last key of the span")).changed() {
                next.to = Some(to - shift);
            }
            if (next.from.is_some() || next.to.is_some()) && ui.small_button(tr!("All")).on_hover_text(tr!("The whole table, however long it gets")).clicked() {
                (next.from, next.to) = (None, None);
            }
        });
    });
    form_row(ui, FORM_LABEL_DOCK, tr!("Value"), |ui| {
        with_type(ui, Type::Data, |ui| {
            ui.add(egui::DragValue::new(&mut next.scale).range(-MAX_VALUE..=MAX_VALUE).speed(0.01).max_decimals(4).prefix("× ")).on_hover_text(tr!("The column's number times this")).named(tr!("Scale of the value"));
            ui.add(egui::DragValue::new(&mut next.offset).range(-MAX_VALUE..=MAX_VALUE).speed(0.5).max_decimals(3).prefix("+ ")).on_hover_text(tr!("Added after scaling. The result is the property: pixels, percent, degrees or a frame counted from 0")).named(tr!("Offset of the value"));
        });
    });
    form_row(ui, FORM_LABEL_DOCK, tr!("Between"), |ui| {
        egui::ComboBox::from_id_salt("data_bind_interp")
            .width(theme::fit(ui, 150.0))
            .selected_text(text(Type::Body, crate::i18n::t(b.interp.label())))
            .show_ui(ui, |ui| {
                for i in Interp::ALL {
                    if ui.selectable_label(i == b.interp, crate::i18n::t(i.label())).clicked() {
                        next.interp = i;
                    }
                }
            })
            .response
            .on_hover_text(tr!("Between two rows: Linear goes straight from one value to the next; Step holds each row's value until the next row"))
            .set_name(tr!("Between rows"));
    });
    if next != *b {
        crate::panels::live(app, tr_noop!("Data Binding"), "data.bind", binding_params(&next));
    }
}

/// What a finished `data.import_table` did, for the status bar; the table becomes the
/// Library's current item.
pub fn imported(app: &mut App, v: &Value) {
    let Some(id) = v["table"].as_u64().map(DataId) else { return };
    app.library_sel = Some(LibraryItem::Data(id));
    let name = v["name"].as_str().unwrap_or("");
    let (rows, cols) = (v["rows"].as_u64().unwrap_or(0), v["columns"].as_array().map_or(0, Vec::len));
    let rows = trn!("{n} row", "{n} rows", rows);
    let cols = trn!("{n} column", "{n} columns", cols);
    let mut msg = if v["replaced"].as_bool() == Some(true) {
        tr!("{name} now has {rows} × {columns}; what is bound to it follows.", name = name, rows = rows, columns = cols)
    } else {
        tr!("{name} is in the Library: {rows} × {columns}. Select an instance and choose Modify › Data › Bind to Data.", name = name, rows = rows, columns = cols)
    };
    let skipped: Vec<&str> = v["skipped"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !skipped.is_empty() {
        let shown = skipped.iter().take(4).copied().collect::<Vec<_>>().join(", ");
        msg = format!("{msg} {}", trn!("Left out {n} column with no numbers ({names}).", "Left out {n} columns with no numbers ({names}).", skipped.len(), names = shown));
    }
    let missing: Vec<&str> = v["missing"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !missing.is_empty() {
        msg = format!("{msg} {}", tr!("Bindings still read columns that are gone: {names}.", names = missing.join(", ")));
    }
    if v["truncated"].as_bool() == Some(true) {
        msg = format!("{msg} {}", tr!("The file held more than a table keeps; the rest was left out."));
    }
    app.status_ok(msg);
}

/// A sample for screenshots: six bars on a baseline, each a graphic instance whose height
/// follows a column of a table of twenty-four rows (a row a frame), one bar selected with its
/// binding in Properties; with `viewer`, the table viewer open on the table.
pub fn sample_chart(app: &mut App, viewer: bool) {
    app.perform("file.new", json!({"width": 960, "height": 540, "fps": 24}));
    // i18n-exempt: layer, symbol, table and column names are document data (a screenshot sample)
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"];
    let columns: Vec<Value> = months
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let values: Vec<f64> = (0..24).map(|f| (120.0 + 90.0 * ((f as f64) * 0.26 + i as f64 * 1.1).sin() + 12.0 * i as f64).round()).collect();
            json!({"name": m, "values": values})
        })
        .collect();
    let Some(table) = app.run("data.set_table", json!({"name": "Rainfall", "columns": columns})).and_then(|v| v["table"].as_u64()) else { return };
    app.run("draw.line", json!({"x0": 120, "y0": 440, "x1": 840, "y1": 440, "stroke": "#1b2a4a", "stroke_width": 3}));
    app.run("timeline.insert_frame", json!({"frame": 23}));
    app.run("layer.new", json!({"name": "Bars"}));
    app.run("draw.rect", json!({"x": 150, "y": 340, "w": 80, "h": 100, "fill": "#3d8bfd", "stroke": null, "object": true}));
    let symbol = app.run("modify.convert_to_symbol", json!({"name": "Bar", "kind": "graphic"})).and_then(|v| v["symbol"].as_u64());
    let mut bars: Vec<u64> = app.s.selection.ids().map(|e| e.0).collect();
    for i in 1..months.len() {
        if let Some(el) = app.run("library.place", json!({"symbol": symbol, "x": 190 + 110 * i, "y": 390})).and_then(|v| v["element"].as_u64()) {
            bars.push(el);
        }
    }
    app.run("timeline.insert_frame", json!({"frame": 23}));
    for (i, (el, month)) in bars.iter().zip(months).enumerate() {
        // Each bar grows from its foot on the baseline.
        app.run("element.select", json!({"ids": [el]}));
        app.run("modify.transformation_point", json!({"x": 190 + 110 * i, "y": 440}));
        app.run("data.bind", json!({"element": el, "property": "scale_y", "table": table, "column": month}));
    }
    if let Some(el) = bars.get(2) {
        app.run("data.bind", json!({"element": el, "property": "alpha", "table": table, "column": "Mar", "scale": 0.3, "offset": 40}));
        app.run("element.select", json!({"ids": [el]}));
    }
    app.run("timeline.goto", json!({"frame": 9}));
    app.library_sel = Some(LibraryItem::Data(DataId(table)));
    // Room for the Data section under the instance's own properties.
    for p in [crate::dock::Panel::Align, crate::dock::Panel::History] {
        app.dock.close(p);
    }
    if viewer {
        app.show_panel(crate::dock::Panel::Library);
        app.data_view = Some(DataId(table));
    } else {
        // The whole of the instance's properties, the Data section last.
        for p in [crate::dock::Panel::Library, crate::dock::Panel::MovieExplorer] {
            app.dock.close(p);
        }
    }
    app.show_panel(crate::dock::Panel::Properties);
    app.s.mark_saved();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_shown_plainly() {
        for (v, s) in [(0.0, "0"), (1.0, "1"), (-2.5, "-2.5"), (0.123456, "0.1235"), (1234.5, "1234.5"), (1e12, "1.000e12"), (0.00001, "1.000e-5"), (-0.00001, "-1.000e-5"), (-0.00000001 * 1e3, "-1.000e-5")] {
            assert_eq!(fmt_num(v), s, "{v}");
        }
        assert_eq!(cell_text(None), "–");
    }

    #[test]
    fn every_engine_label_is_listed_for_the_catalogs() {
        for p in BoundProperty::ALL {
            assert!(TEXTS.contains(&p.label()), "{}", p.label());
        }
        for i in Interp::ALL {
            assert!(TEXTS.contains(&i.label()), "{}", i.label());
        }
    }
}
