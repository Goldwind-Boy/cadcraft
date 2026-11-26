//! Parts List (明细栏 / BOM) management dialog, opened from the Window menu. Mirrors the Zhongwang
//! parts-list table: rows for 序号 / 图号 / 名称 / 数量 / 材料 / 单重 / 总重 / 备注 / 零件类型, with
//! append / delete-row and inline field editing. OK writes the edited rows back through the
//! `bom.*` engine commands and regenerates the on-sheet parts table; Cancel discards the edits.

use egui::Id;
use serde_json::{Value, json};

use crate::CadApp;

/// Part-type dropdown values (same as the engine's list).
const PART_KINDS: [&str; 7] = [
    "标准件",
    "外购件",
    "借用件",
    "外协零件",
    "外委部件",
    "自制零件",
    "自制部件",
];

/// One editable parts-list row (local copy in the dialog; written back on OK).
#[derive(Clone, Debug, Default)]
struct Row {
    code: String,
    name: String,
    qty: i64,
    material: String,
    weight: String,
    total_weight: String,
    note: String,
    kind: String,
}

#[derive(Clone, Debug, Default)]
struct State {
    rows: Vec<Row>,
    /// Whether the ledger has been loaded into the local copy yet.
    loaded: bool,
}

fn row_from_json(v: &Value) -> Row {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    Row {
        code: s("code"),
        name: s("name"),
        qty: v.get("qty").and_then(Value::as_i64).unwrap_or(1).max(1),
        material: s("material"),
        weight: s("weight"),
        total_weight: s("totalWeight"),
        note: s("note"),
        kind: s("kind"),
    }
}

/// Write the edited rows back to the ledger, then regenerate the on-sheet parts table.
fn apply(app: &mut CadApp, st: &State) {
    let _ = app.run("bom.clear", json!({}));
    for row in &st.rows {
        // Merge key: the model name, falling back to the drawing number when the name is blank.
        let block = if !row.name.is_empty() { row.name.clone() } else { row.code.clone() };
        let _ = app.run(
            "bom.add",
            json!({
                "block": block,
                "name": row.name,
                "code": row.code,
                "qty": row.qty.max(1),
                "material": row.material,
                "weight": row.weight,
                "totalWeight": row.total_weight,
                "note": row.note,
                "kind": row.kind,
            }),
        );
    }
    // Reuse the previous table's insertion point; nothing is drawn if no table exists yet.
    let _ = app.run("bom.table", json!({}));
    let _ = app.session.zoom_extents();
    app.set_status("明细栏已更新");
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = Id::unique("bom_dialog");
    let mut st: State = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();

    if !st.loaded {
        if let Ok(list) = app.run("bom.list", json!({})) {
            st.rows = list
                .get("items")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(row_from_json).collect())
                .unwrap_or_default();
        }
        st.loaded = true;
    }

    let mut is_open = *open;
    let mut close = false;

    egui::Window::new("明细管理")
        .id(id)
        .open(&mut is_open)
        .collapsible(false)
        .default_size(egui::vec2(760.0, 360.0))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("bom_grid").striped(true).num_columns(10).show(ui, |ui| {
                    for h in ["序号", "图号", "名称", "数量", "材料", "单重", "总重", "备注", "类型", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();

                    let mut remove: Option<usize> = None;
                    for (idx, row) in st.rows.iter_mut().enumerate() {
                        ui.label(format!("{}", idx + 1));
                        ui.add(egui::TextEdit::singleline(&mut row.code).desired_width(90.0));
                        ui.add(egui::TextEdit::singleline(&mut row.name).desired_width(120.0));
                        ui.add(egui::DragValue::new(&mut row.qty).range(1..=9999).speed(1.0));
                        ui.add(egui::TextEdit::singleline(&mut row.material).desired_width(80.0));
                        ui.add(egui::TextEdit::singleline(&mut row.weight).desired_width(60.0));
                        ui.add(egui::TextEdit::singleline(&mut row.total_weight).desired_width(60.0));
                        ui.add(egui::TextEdit::singleline(&mut row.note).desired_width(80.0));
                        egui::ComboBox::from_id_salt(format!("bom_kind_{idx}"))
                            .selected_text(if row.kind.is_empty() { "标准件" } else { &row.kind })
                            .width(110.0)
                            .show_ui(ui, |ui| {
                                for k in PART_KINDS {
                                    ui.selectable_value(&mut row.kind, k.to_string(), k);
                                }
                            });
                        if ui.button("✕").clicked() {
                            remove = Some(idx);
                        }
                        ui.end_row();
                    }
                    if let Some(idx) = remove {
                        st.rows.remove(idx);
                    }
                });
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("＋追加").clicked() {
                    st.rows.push(Row { kind: "标准件".into(), qty: 1, ..Row::default() });
                }
                ui.add_space(12.0);
                if ui.button("确定").clicked() {
                    apply(app, &st);
                    close = true;
                }
                if ui.button("取消").clicked() {
                    close = true;
                }
            });
        });

    *open = is_open && !close;
    if close {
        ctx.data_mut(|d| d.remove::<State>(id));
    } else {
        ctx.data_mut(|d| d.insert_temp(id, st));
    }
}
