//! Sheet Setup dialog (GB 图幅设置), opened from the Window menu. Picks a GB format /
//! orientation / margin, mounts the title block at the inner frame's lower-right corner, and draws
//! everything through the engine's `sheet` and `insert` commands. The parts list itself is
//! generated from the BOM ledger, not mounted as a static column block here.
//!
//! The catalog root is resolved the same way the Standard Parts palette does ([`crate::parts`]);
//! column blocks live in `sheets/` under that root.

use std::path::PathBuf;

use egui::{Color32, Stroke, StrokeKind, Vec2};
use serde_json::json;

use crate::CadApp;

/// GB basic format: code, B short edge, L long edge, side margin c, no-binding edge e (mm).
const FORMATS: [(&str, f64, f64, f64, f64); 5] = [
    ("A0", 841.0, 1189.0, 10.0, 20.0),
    ("A1", 594.0, 841.0, 10.0, 20.0),
    ("A2", 420.0, 594.0, 10.0, 10.0),
    ("A3", 297.0, 420.0, 5.0, 10.0),
    ("A4", 210.0, 297.0, 5.0, 10.0),
];
/// Drawing-scale denominators offered as `1:n`.
const RATIOS: [u32; 6] = [1, 2, 5, 10, 20, 50];
/// Title block column block file (relative to the catalog root).
const TITLE_FILE: &str = "sheets/标题栏-1.dxf";

/// Dialog options, kept in egui temp memory between frames while open.
#[derive(Clone, Debug, PartialEq)]
struct SheetState {
    /// `A0`..`A4` or `custom`.
    size: String,
    portrait: bool,
    /// Keep the 25 mm binding margin on the left; otherwise an even edge on all sides.
    bind: bool,
    /// Drawing scale denominator n (`1:n`); the engine scales existing bare geometry by old/new
    /// and backfills the title-block SCALE cell with `1:n`.
    ratio: u32,
    custom_w: f64,
    custom_h: f64,
    title: bool,
}

impl Default for SheetState {
    fn default() -> Self {
        SheetState {
            size: "A3".into(),
            portrait: false,
            bind: true,
            ratio: 1,
            custom_w: 297.0,
            custom_h: 420.0,
            title: true,
        }
    }
}

/// Outer (W, H) in mm for the current options.
fn dims(st: &SheetState) -> (f64, f64) {
    let (b, l) = FORMATS
        .iter()
        .find(|(k, _, _, _, _)| *k == st.size)
        .map(|(_, b, l, _, _)| (*b, *l))
        .unwrap_or((st.custom_h.min(st.custom_w), st.custom_h.max(st.custom_w)));
    if st.portrait { (b, l) } else { (l, b) }
}

fn format_row(code: &str) -> Option<(f64, f64)> {
    FORMATS.iter().find(|(k, _, _, _, _)| *k == code).map(|(_, b, l, _, _)| (*b, *l))
}

/// Resolve a column-block DXF under the catalog root, checking it exists.
fn column_file(root: &Option<PathBuf>, rel: &str) -> Option<PathBuf> {
    let root = root.as_ref()?;
    let f = root.join(rel);
    f.exists().then_some(f)
}

/// Run the `sheet` command, then insert the chosen column blocks at the inner frame's corner.
fn apply(app: &mut CadApp, st: &SheetState) {
    let mut params = json!({
        "size": st.size,
        "orientation": if st.portrait { "portrait" } else { "landscape" },
        "bind": st.bind,
        "ratio": st.ratio,
    });
    if st.size == "CUSTOM" {
        params["width"] = json!(st.custom_w);
        params["height"] = json!(st.custom_h);
    }

    let summary = match app.run("sheet", params) {
        Ok(v) => v,
        Err(e) => {
            app.set_status(e);
            return;
        }
    };

    // Inner frame lower-right corner, and the column block heights the frame command reports.
    let corner = summary
        .get("innerRight")
        .and_then(|v| v.as_array())
        .and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)))
        .unwrap_or((0.0, 0.0));

    let root = crate::parts::repo_root();

    if st.title {
        if let Some(file) = column_file(&root, TITLE_FILE) {
            let _ = app.run(
                "insert",
                json!({ "name": "标题栏-1", "file": file.to_string_lossy(), "at": [corner.0, corner.1], "scale": 1.0, "rotation": 0.0 }),
            );
            // Backfill the title-block scale cell with the chosen 1:n.
            let _ = app.run("title.set", json!({ "values": { "SCALE": format!("1:{}", st.ratio) } }));
        } else {
            app.set_status(format!("标题栏块未找到: {TITLE_FILE}"));
        }
    }

    let _ = app.session.zoom_extents();
    app.set_status("图幅设置完成");
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::unique("sheet_setup_dialog");
    let mut st: SheetState = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let mut is_open = *open;
    let mut close = false;

    egui::Window::new("图幅设置")
        .id(id)
        .open(&mut is_open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // ------------------------------------------------------ left: options
                ui.vertical(|ui| {
                    ui.heading("图幅");
                    egui::Grid::new("sheet_grid").num_columns(2).show(ui, |ui| {
                        ui.label("样式");
                        ui.label("GB");
                        ui.end_row();

                        ui.label("幅面");
                        egui::ComboBox::from_id_salt("sheet_size")
                            .selected_text(st.size.clone())
                            .show_ui(ui, |ui| {
                                for code in ["A0", "A1", "A2", "A3", "A4", "CUSTOM"] {
                                    ui.selectable_value(&mut st.size, code.to_string(), code);
                                }
                            });
                        ui.end_row();

                        if st.size == "CUSTOM" {
                            ui.label("宽度 (mm)");
                            ui.add(egui::DragValue::new(&mut st.custom_w).range(1.0..=5000.0).speed(1.0));
                            ui.end_row();
                            ui.label("高度 (mm)");
                            ui.add(egui::DragValue::new(&mut st.custom_h).range(1.0..=5000.0).speed(1.0));
                            ui.end_row();
                        } else if let Some((b, l)) = format_row(&st.size) {
                            ui.label("尺寸");
                            ui.label(format!("{b:.0} × {l:.0}"));
                            ui.end_row();
                        }

                        ui.label("布置");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut st.portrait, false, "横置");
                            ui.radio_value(&mut st.portrait, true, "纵置");
                        });
                        ui.end_row();

                        ui.label("装订边");
                        ui.checkbox(&mut st.bind, "左侧留装订边 25 mm");
                        ui.end_row();

                        ui.label("绘图比例");
                        egui::ComboBox::from_id_salt("sheet_ratio")
                            .selected_text(format!("1:{}", st.ratio))
                            .show_ui(ui, |ui| {
                                for n in RATIOS {
                                    ui.selectable_value(&mut st.ratio, n, format!("1:{n}"));
                                }
                            });
                        ui.end_row();
                    });

                    ui.separator();
                    ui.heading("栏目");
                    ui.checkbox(&mut st.title, "标题栏");
                    ui.label("附加栏 / 代号栏 / 参数栏：本期暂不提供块");
                });

                ui.separator();

                // ---------------------------------------------------- right: preview
                preview(ui, &st);
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
                ui.label(format!("比例 1:{}", st.ratio));
            });
        });

    *open = is_open && !close;
    ctx.data_mut(|d| d.insert_temp(id, st));
}

/// Schematic frame preview scaled to a fixed box, outer boundary + inner frame + title block.
fn preview(ui: &mut egui::Ui, st: &SheetState) {
    let (w64, h64) = dims(st);
    let w = w64 as f32;
    let h = h64 as f32;
    let box_size = Vec2::new(170.0, 150.0);
    let scale = (box_size.x / w).min(box_size.y / h);
    let fw = w * scale;
    let fh = h * scale;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(box_size.x, box_size.y), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    let origin = rect.left_top() + Vec2::new((box_size.x - fw) / 2.0, (box_size.y - fh) / 2.0);
    let outer = egui::Rect::from_min_size(origin, Vec2::new(fw, fh));
    painter.rect_stroke(outer, 2.0, Stroke::new(1.0, Color32::from_gray(120)), StrokeKind::Inside);

    // Margins (schematic): binding 25 on the left, side margin c otherwise.
    let (_, _, c64, e64) = FORMATS
        .iter()
        .find(|(k, _, _, _, _)| *k == st.size)
        .map(|row| (row.1, row.2, row.3, row.4))
        .unwrap_or((297.0, 420.0, 10.0, 10.0));
    let (ml, mr, mb, mt) = if st.bind { (25.0, c64, c64, c64) } else { (e64, e64, e64, e64) };
    let (ml, mr, mb, mt) = (ml as f32, mr as f32, mb as f32, mt as f32);
    let inner = egui::Rect::from_min_size(
        origin + Vec2::new(ml * scale, mt * scale),
        Vec2::new((w - ml - mr) * scale, (h - mb - mt) * scale),
    );
    painter.rect_stroke(inner, 2.0, Stroke::new(1.6, Color32::from_gray(220)), StrokeKind::Inside);

    // Title block at the inner frame's lower-right corner.
    if st.title {
        let th = 48.0 * scale;
        let tw = 180.0 * scale;
        let tr = egui::Rect::from_min_size(
            egui::pos2(inner.right() - tw, inner.bottom() - th),
            Vec2::new(tw, th),
        );
        painter.rect_stroke(tr, 1.0, Stroke::new(1.2, Color32::from_gray(200)), StrokeKind::Inside);
    }
}
