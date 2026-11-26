//! Title Block (标题栏) edit dialog, mirroring the Zhongwang "标题栏编辑" window: a two-column
//! form listing each fillable cell's prompt on the left and its current value on the right. The
//! dialog keeps a local copy of the values; OK writes them back through the `title.props` /
//! `title.set` engine commands, Cancel discards the edits.

use egui::Id;
use serde_json::{Value, json};

use crate::CadApp;

/// One editable title-block row (local copy in the dialog; written back on OK).
#[derive(Clone, Debug, Default)]
struct Row {
    tag: String,
    prompt: String,
    value: String,
}

#[derive(Clone, Debug, Default)]
struct State {
    rows: Vec<Row>,
    /// Reference handle returned by `title.props` (null when no title block is on the sheet).
    handle: Option<String>,
    /// Whether the ledger has been loaded into the local copy yet.
    loaded: bool,
}

fn row_from_json(v: &Value) -> Row {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    Row { tag: s("tag"), prompt: s("prompt"), value: s("value") }
}

/// Write the edited values back onto the title-block reference.
fn apply(app: &mut CadApp, st: &State) {
    let mut values = serde_json::Map::new();
    for row in &st.rows {
        values.insert(row.tag.clone(), json!(row.value));
    }
    let params = match &st.handle {
        Some(h) => json!({ "handle": h, "values": values }),
        None => json!({ "values": values }),
    };
    match app.run("title.set", params) {
        Ok(_) => app.set_status("标题栏已更新"),
        Err(e) => app.set_status(e),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = Id::unique("title_dialog");
    let mut st: State = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();

    if !st.loaded {
        if let Ok(props) = app.run("title.props", json!({})) {
            st.handle = props.get("handle").and_then(Value::as_str).map(|s| s.to_string());
            st.rows = props
                .get("items")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(row_from_json).collect())
                .unwrap_or_default();
        }
        st.loaded = true;
    }

    let mut is_open = *open;
    let mut close = false;

    egui::Window::new("标题栏编辑")
        .id(id)
        .open(&mut is_open)
        .collapsible(false)
        .resizable(false)
        .default_size(egui::vec2(360.0, 320.0))
        .show(ctx, |ui| {
            if st.handle.is_none() {
                ui.label(egui::RichText::new("当前图中未找到标题栏，请先用图幅设置插入标题栏。").color(crate::theme::Tokens::get().text_faint));
                ui.separator();
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("title_grid").num_columns(2).spacing(egui::vec2(16.0, 8.0)).show(ui, |ui| {
                    ui.strong("显示名称");
                    ui.strong("填写内容");
                    ui.end_row();
                    for row in &mut st.rows {
                        ui.label(&row.prompt);
                        ui.add(egui::TextEdit::singleline(&mut row.value).desired_width(200.0));
                        ui.end_row();
                    }
                });
            });

            ui.separator();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(app.session.state().is_ok(), |ui| {
                    if ui.button("确定").clicked() {
                        apply(app, &st);
                        close = true;
                    }
                });
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
