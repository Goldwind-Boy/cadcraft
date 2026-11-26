//! The Standard Parts palette: browse the GB fastener / bearing catalog and insert a chosen
//! block into the active drawing. The catalog is read from an external folder (CSV manifest +
//! per-block DXF), resolved from the `CADCRAFT_PARTS_ROOT` environment variable, a `标准件库`
//! folder next to the executable, or the built-in fallback path.
//!
//! Layout mirrors the Zhongwang mechanical-part出库 dialog: a left category tree, a middle
//! spec table, and a right preview/options column. Insertion goes through the engine's `insert`
//! command ([`CadApp::run`]); the block DXF defines (or redefines) the block `name`, and `at`
//! places its base point.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{ColorImage, RichText, TextureHandle, TextureOptions};
use serde_json::{json, Value};

use crate::CadApp;
use crate::theme::Tokens;

/// Built-in catalog root when neither the environment variable nor a folder beside the exe exists.
const FALLBACK_ROOT: &str = r"D:\CADcraft\标准件库";
/// Catalog manifest file, relative to the root.
const MANIFEST: &str = "parts/型号清单.csv";
/// Per-block GB dimensions, relative to the root.
const DIM_JSON: &str = "preview/catalog_data.json";

/// One catalog entry: a named block backed by a single-block DXF.
#[derive(Clone, Debug, PartialEq)]
struct PartEntry {
    category: String,
    name: String,
    /// DXF path relative to the catalog root, forward slashes.
    rel_path: String,
    /// GB dimensions keyed by field (d, s, k, …) from the dimension JSON.
    dim: HashMap<String, f64>,
}

/// Resolve the catalog root: environment override, a `标准件库` folder next to the executable,
/// then the built-in fallback. Returns `None` when the manifest cannot be found anywhere.
pub(crate) fn repo_root() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("CADCRAFT_PARTS_ROOT") {
        let p = PathBuf::from(env);
        if p.join(MANIFEST).exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for up in [0usize, 1, 2, 3] {
            let mut cand = dir.to_path_buf();
            for _ in 0..up {
                cand.pop();
            }
            let guess = cand.join("标准件库");
            if guess.join(MANIFEST).exists() {
                return Some(guess);
            }
        }
    }
    let fallback = PathBuf::from(FALLBACK_ROOT);
    fallback.join(MANIFEST).exists().then_some(fallback)
}

/// Load the per-block GB dimension map: block name -> field -> value, from the dimension JSON.
fn load_dim_map(root: &Path) -> HashMap<String, HashMap<String, f64>> {
    let Ok(text) = std::fs::read_to_string(root.join(DIM_JSON)) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    let Some(cats) = value.get("cats").and_then(|v| v.as_object()) else {
        return out;
    };
    for entries in cats.values() {
        let Some(arr) = entries.as_array() else { continue };
        for e in arr {
            let Some(name) = e.get("name").and_then(|v| v.as_str()) else { continue };
            let mut dim = HashMap::new();
            if let Some(obj) = e.get("dim").and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    let num = v.as_f64().or_else(|| v.as_i64().map(|i| i as f64));
                    if let Some(num) = num {
                        dim.insert(k.clone(), num);
                    }
                }
            }
            // Nominal length lives at the top level; surface it under "L" so preview callouts can use it.
            if let Some(l) = e.get("L").and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))) {
                dim.insert("L".to_string(), l);
            }
            out.insert(name.to_string(), dim);
        }
    }
    out
}

/// Parse the manifest CSV. Columns: `序号,类别,块名,文件,基点`; the base-point field is quoted
/// and may contain a comma, so it is joined back after splitting.
fn load_catalog(root: &Path) -> Vec<PartEntry> {
    let Ok(text) = std::fs::read_to_string(root.join(MANIFEST)) else {
        return Vec::new();
    };
    let dim_map = load_dim_map(root);
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.strip_prefix('\u{feff}').unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        if index == 0 {
            continue; // header
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 4 {
            continue;
        }
        let category = cols[1].trim().to_string();
        let name = cols[2].trim().to_string();
        let rel_path = cols[3].trim().trim_matches('"').replace('\\', "/");
        if name.is_empty() || rel_path.is_empty() {
            continue;
        }
        let dim = dim_map.get(&name).cloned().unwrap_or_default();
        out.push(PartEntry { category, name, rel_path, dim });
    }
    // 类别内按直径/边长数值、再按公称长度升序（对齐中望规格表顺序）；类别顺序保持清单首现顺序。
    let mut cat_order: Vec<String> = Vec::new();
    for e in &out {
        if !cat_order.contains(&e.category) {
            cat_order.push(e.category.clone());
        }
    }
    out.sort_by(|a, b| {
        let ia = cat_order.iter().position(|c| c == &a.category).unwrap_or(0);
        let ib = cat_order.iter().position(|c| c == &b.category).unwrap_or(0);
        let sa = *a.dim.get("d").or_else(|| a.dim.get("d1")).or_else(|| a.dim.get("b")).unwrap_or(&0.0);
        let sb = *b.dim.get("d").or_else(|| b.dim.get("d1")).or_else(|| b.dim.get("b")).unwrap_or(&0.0);
        let la = parse_spec(&a.name).1.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let lb = parse_spec(&b.name).1.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        (ia, sa, la).partial_cmp(&(ib, sb, lb)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Distinct categories in catalog order.
fn categories(catalog: &[PartEntry]) -> Vec<String> {
    let mut out = Vec::new();
    for p in catalog {
        if !out.contains(&p.category) {
            out.push(p.category.clone());
        }
    }
    out
}

/// Split a block name into a spec token and an optional length token.
/// `螺栓-M10x35` -> (`M10`, `35`); `螺母-M10` -> (`M10`, None); `键-b10x8x25` -> (`b10`, `25`).
fn parse_spec(name: &str) -> (String, Option<String>) {
    let body = name.split('-').nth(1).unwrap_or(name);
    let parts: Vec<&str> = body.split('x').collect();
    let spec = parts.first().unwrap_or(&body).to_string();
    let len = if parts.len() > 1 { parts.last().map(|s| s.to_string()) } else { None };
    (spec, len)
}

/// One GB standard entry in the left tree: standard number + display name, mapped to a catalog
/// category when a block library exists for it. `cat: None` entries are placeholders (block not
/// generated yet) and shown greyed out.
struct StdEntry {
    std: &'static str,
    name: &'static str,
    cat: Option<&'static str>,
}

/// A top-level part group in the left tree, holding its GB standard entries.
struct StdGroup {
    title: &'static str,
    entries: &'static [StdEntry],
}

/// Bolt standards mirrored from the Zhongwang part tree. Only GB/T 5782 has generated blocks so
/// far; the rest are placeholders pending block generation.
const BOLTS: &[StdEntry] = &[
    StdEntry { std: "GB/T 27-2013", name: "六角头加强杆螺栓 A级", cat: Some("螺栓27") },
    StdEntry { std: "GB/T 27-2013", name: "六角头加强杆螺栓 B级", cat: Some("螺栓27") },
    StdEntry { std: "GB/T 28-2013", name: "六角头螺杆带孔加强杆螺栓 A级", cat: Some("螺栓28") },
    StdEntry { std: "GB/T 28-2013", name: "六角头螺杆带孔加强杆螺栓 B级", cat: Some("螺栓28") },
    StdEntry { std: "GB/T 29.1-2013", name: "六角头带槽螺栓 A级", cat: Some("螺栓291") },
    StdEntry { std: "GB/T 31.1-2013", name: "六角头螺杆带孔螺栓 A级", cat: Some("螺栓311") },
    StdEntry { std: "GB/T 31.2-1988", name: "六角头螺杆带孔螺栓 B级", cat: Some("螺栓312") },
    StdEntry { std: "GB/T 31.3-1988", name: "六角头螺杆带孔螺栓 细牙 B级", cat: Some("螺栓313") },
    StdEntry { std: "GB/T 31.3-1988", name: "六角头螺杆带孔螺栓 细牙 A级", cat: Some("螺栓313") },
    StdEntry { std: "GB/T 32.1-2020", name: "六角头头部带孔螺栓 A级", cat: Some("螺栓321") },
    StdEntry { std: "GB/T 32.1-2020", name: "六角头头部带孔螺栓 细牙 A级", cat: Some("螺栓323") },
    StdEntry { std: "GB/T 32.2-1988", name: "六角头头部带孔螺栓 B级", cat: Some("螺栓322") },
    StdEntry { std: "GB/T 32.2-1988", name: "六角头头部带孔螺栓 细牙 B级", cat: Some("螺栓323") },
    StdEntry { std: "GB/T 32.3-2020", name: "六角头头部带孔螺栓 细牙 B级", cat: Some("螺栓323") },
    StdEntry { std: "GB/T 1228-2006", name: "钢结构用高强度大六角头螺栓", cat: Some("螺栓1228") },
    StdEntry { std: "GB/T 5780-2016", name: "六角头螺栓 C级", cat: Some("螺栓5780") },
    StdEntry { std: "GB/T 5780-2016", name: "六角头螺栓 C级(垫圈面型)", cat: None },
    StdEntry { std: "GB/T 5781-2016", name: "六角头螺栓 C级 全螺纹(垫圈面型)", cat: None },
    StdEntry { std: "GB/T 5781-2016", name: "六角头螺栓 C级 全螺纹", cat: Some("螺栓5781") },
    StdEntry { std: "GB/T 5782-2016", name: "六角头螺栓 A级 B级", cat: Some("螺栓") },
    StdEntry { std: "GB/T 5783-2016", name: "六角头螺栓 全螺纹 A级 B级", cat: Some("螺栓5783") },
    StdEntry { std: "GB/T 5784-2016", name: "六角头螺栓 半六角头", cat: None },
    StdEntry { std: "GB/T 5785-2016", name: "六角头螺栓 细牙 A级 B级", cat: Some("螺栓5785") },
    StdEntry { std: "GB/T 5786-2016", name: "六角头螺栓 细牙 全螺纹 A级 B级", cat: Some("螺栓5786") },
    StdEntry { std: "GB/T 5789-1986", name: "六角法兰面螺栓 加大系列 B级", cat: None },
    StdEntry { std: "GB/T 5790-1986", name: "六角法兰面螺栓 加大系列 细牙 B级", cat: None },
    StdEntry { std: "GB/T 9125.1-2020", name: "管法兰连接用六角螺栓 PN系列", cat: Some("螺栓9125") },
    StdEntry { std: "GB/T 16674.1-2004", name: "六角法兰面螺栓 小系列 粗杆 B级", cat: None },
    StdEntry { std: "GB/T 16674.1-2004", name: "六角法兰面螺栓 小系列 细杆 B级", cat: None },
    StdEntry { std: "GB/T 16674.2-2016", name: "六角法兰面螺栓 小系列 细牙 粗杆", cat: None },
    StdEntry { std: "GB/T 16674.2-2016", name: "六角法兰面螺栓 小系列 细牙 细杆", cat: None },
    StdEntry { std: "JB/ZQ 4322-2006", name: "六角头螺栓 A型", cat: None },
    StdEntry { std: "JB/ZQ 4322-2006", name: "六角头螺栓 B型", cat: None },
];

const SCREWS: &[StdEntry] = &[StdEntry { std: "GB/T 70.1", name: "内六角圆柱头螺钉", cat: Some("螺钉") }];
const NUTS: &[StdEntry] = &[StdEntry { std: "GB/T 6170", name: "1型六角螺母", cat: Some("螺母") }];
const WASHERS: &[StdEntry] = &[StdEntry { std: "GB/T 97.1", name: "平垫圈 A级", cat: Some("垫圈") }];
const SPRING: &[StdEntry] = &[StdEntry { std: "GB/T 93", name: "标准型弹簧垫圈", cat: Some("弹垫") }];
const PINS: &[StdEntry] = &[StdEntry { std: "GB/T 119.1", name: "圆柱销", cat: Some("销") }];
const KEYS: &[StdEntry] = &[StdEntry { std: "GB/T 1096", name: "普通平键 A型", cat: Some("键") }];
const BEARINGS: &[StdEntry] = &[StdEntry { std: "GB/T 276", name: "深沟球轴承 62系列", cat: Some("轴承") }];

/// Left-tree groups in display order.
fn std_tree() -> &'static [StdGroup] {
    &[
        StdGroup { title: "六角螺栓", entries: BOLTS },
        StdGroup { title: "内六角螺钉", entries: SCREWS },
        StdGroup { title: "六角螺母", entries: NUTS },
        StdGroup { title: "平垫圈", entries: WASHERS },
        StdGroup { title: "弹簧垫圈", entries: SPRING },
        StdGroup { title: "圆柱销", entries: PINS },
        StdGroup { title: "普通平键", entries: KEYS },
        StdGroup { title: "深沟球轴承", entries: BEARINGS },
    ]
}

/// Middle-table columns per category: (header label, dim field key).
fn columns(category: &str) -> Vec<(&'static str, &'static str)> {
    match category {
        cat if cat.starts_with("螺栓") => vec![("d", "d"), ("k", "k"), ("e", "e"), ("s", "s")],
        "螺钉" => vec![("d", "d"), ("dk", "dk"), ("k", "k"), ("s", "s")],
        "螺母" => vec![("d", "d"), ("s", "s"), ("m", "m")],
        "垫圈" => vec![("d1", "d1"), ("d2", "d2"), ("h", "h")],
        "弹垫" => vec![("d1", "d1"), ("d2", "d2"), ("b", "b"), ("t", "t")],
        "销" => vec![("d", "d"), ("c", "c")],
        "键" => vec![("b", "b"), ("h", "h")],
        "轴承" => vec![("d", "d"), ("D", "D"), ("B", "B")],
        _ => Vec::new(),
    }
}

/// Format a dimension value: integers drop the trailing `.0`.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Place a label inside a fixed-width cell so table columns line up with the header.
fn alloc_cell(ui: &mut egui::Ui, w: f32, rt: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(w, 0.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(rt);
        },
    );
}

/// Cached preview for the currently selected part: block name + rendered texture (or error).
#[derive(Clone)]
struct PartPreview {
    name: String,
    tex: Result<TextureHandle, String>,
}

/// Render the chosen single-block DXF to an egui texture, in memory (no CLI subprocess).
fn preview_texture(
    ctx: &egui::Context,
    root: &Path,
    entry: &PartEntry,
) -> Result<TextureHandle, String> {
    let file = root.join(&entry.rel_path);
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    let doc = cadcraft_io::read_dxf(&bytes).map_err(|e| e.to_string())?;
    let png = cadcraft_io::png(&doc, &cadcraft_doc::Space::Model, 320, 240).map_err(|e| e.to_string())?;
    let img = cadcraft_io::raster::decode(&png).map_err(|e| e.to_string())?;
    let size = [img.width() as usize, img.height() as usize];
    let tex = ctx.load_texture(
        format!("part-preview-{}", entry.name),
        ColorImage::from_rgba_unmultiplied(size, img.rgba()),
        TextureOptions::LINEAR,
    );
    Ok(tex)
}

/// Book the chosen part into the parts list: the engine merges same-name/same-model rows and
/// increments the quantity. `handle` is the dropped block reference's handle, when known.
fn book_part(app: &mut CadApp, entry: &PartEntry, handle: Option<String>) {
    let mut params = json!({
        "block": entry.name,
        "name": entry.name,
        "kind": "标准件",
    });
    if let Some(h) = handle {
        params["ref"] = json!(h);
    }
    let _ = app.run("bom.add", params);
    // Refresh the on-sheet parts list above the title block; the balloon pick chains on after.
    let _ = app.run("bom.table", json!({}));
}

/// Insert the chosen block programmatically through the engine's `insert` command.
fn do_insert(
    app: &mut CadApp,
    root: &Path,
    entry: &PartEntry,
    x: f64,
    y: f64,
    scale: f64,
    rot: f64,
) {
    let file = root.join(&entry.rel_path);
    if !file.exists() {
        app.set_status(format!("{}: {}", crate::tl!("Part file not found"), file.display()));
        return;
    }
    app.session.cancel();
    let params = json!({
        "name": entry.name,
        "file": file.to_string_lossy(),
        "at": [x, y],
        "scale": scale,
        "rotation": rot,
    });
    match app.run("insert", params) {
        Ok(v) => {
            let handle = v.get("handle").and_then(Value::as_str).map(str::to_string);
            book_part(app, entry, handle);
            app.start("bom.balloon");
            let _ = app.session.zoom_extents();
            app.set_status(format!("{}: {}", entry.name, crate::tl!("inserted")));
        }
        Err(err) => app.set_status(err),
    }
}

/// Start the interactive INSERT command for the chosen part and feed `name=path` as its first
/// answer, so the command stops at the "Specify insertion point" prompt for a canvas pick.
fn do_pick_insert(app: &mut CadApp, root: &Path, entry: &PartEntry) {
    let file = root.join(&entry.rel_path);
    if !file.exists() {
        app.set_status(format!("{}: {}", crate::tl!("Part file not found"), file.display()));
        return;
    }
    app.session.cancel();
    app.start("insert");
    let arg = format!("{}={}", entry.name, file.to_string_lossy());
    if let Err(e) = app.session.input(cadcraft_engine::Input::Text(arg)).map_err(|e| e.to_string()) {
        app.set_status(e);
        return;
    }
    app.set_status(format!("{}: {}", entry.name, crate::tl!("Pick insertion point…")));
}

/// Palette body: three columns — category tree, spec table, preview & options.
pub fn section(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let root = repo_root();

    // Cache the parsed catalog per resolved root across frames.
    let cat_id = egui::Id::unique("parts-catalog");
    let catalog: Vec<PartEntry> = match &root {
        Some(r) => {
            let key = r.clone();
            if let Some((k, v)) = ui.data(|d| d.get_temp::<(PathBuf, Vec<PartEntry>)>(cat_id))
                && k == key
            {
                v
            } else {
                let v = load_catalog(&key);
                ui.data_mut(|d| d.insert_temp(cat_id, (key, v.clone())));
                v
            }
        }
        None => Vec::new(),
    };

    // Panel transient state, kept across frames in egui's temp store.
    let kw_id = egui::Id::unique("parts-kw");
    let cat_sel_id = egui::Id::unique("parts-cat");
    let sel_id = egui::Id::unique("parts-sel");
    let scale_id = egui::Id::unique("parts-scale");
    let rot_id = egui::Id::unique("parts-rotation");
    let pending_id = egui::Id::unique("parts-pending-book");

    let mut kw: String = ui.data(|d| d.get_temp(kw_id)).unwrap_or_default();
    let mut cat: String = ui.data(|d| d.get_temp(cat_sel_id)).unwrap_or_default();
    let mut sel: Option<String> = ui.data(|d| d.get_temp::<String>(sel_id));
    let mut scale: String = ui.data(|d| d.get_temp(scale_id)).unwrap_or_else(|| "1".into());
    let mut rotation: String = ui.data(|d| d.get_temp(rot_id)).unwrap_or_else(|| "0".into());
    let mut pending: Option<PartEntry> = ui.data(|d| d.get_temp(pending_id));

    // An interactive INSERT started from this palette books itself once the command finishes:
    // take the dropped reference handle, then merge it into the parts list. A cancelled pick
    // leaves `last_insert` empty, so nothing is booked.
    if let Some(entry) = pending.clone()
        && app.session.running.is_none()
    {
        let handle = app.session.take_last_insert().map(|h| h.hex());
        if let Some(handle) = handle {
            book_part(app, &entry, Some(handle));
            // Chain the numbered-balloon pick: the engine anchors the leader to this row's new
            // reference and prompts for the circle location.
            app.start("bom.balloon");
        }
        pending = None;
    }

    match &root {
        None => {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(crate::tl!("Standard parts catalog not found. Set CADCRAFT_PARTS_ROOT.")).color(t.warn));
            });
            return;
        }
        Some(_) => {}
    }

    let cat_list = categories(&catalog);
    if cat.is_empty() || !cat_list.contains(&cat) {
        cat = cat_list.first().cloned().unwrap_or_default();
    }

    // Filtered entries for the active category + keyword.
    let kw_l = kw.to_ascii_lowercase();
    let filtered: Vec<&PartEntry> = catalog
        .iter()
        .filter(|p| {
            p.category == cat
                && (kw_l.is_empty()
                    || p.name.to_ascii_lowercase().contains(&kw_l)
                    || p.category.to_ascii_lowercase().contains(&kw_l))
        })
        .collect();

    if !sel.as_ref().is_some_and(|s| filtered.iter().any(|p| &p.name == s)) {
        sel = filtered.first().map(|p| p.name.clone());
    }
    let chosen = filtered.iter().find(|p| Some(&p.name) == sel.as_ref()).cloned();

    // Render (and cache) the chosen part's preview texture.
    let preview_id = egui::Id::unique("parts-preview");
    let ctx = ui.ctx().clone();
    let preview: Option<PartPreview> = match chosen.as_ref().zip(root.as_ref()) {
        Some((entry, root)) => {
            let cached = ctx.data(|d| d.get_temp::<PartPreview>(preview_id));
            let fresh = match cached {
                Some(c) if c.name == entry.name => c,
                _ => match preview_texture(&ctx, root, entry) {
                    Ok(tex) => PartPreview { name: entry.name.clone(), tex: Ok(tex) },
                    Err(e) => PartPreview { name: entry.name.clone(), tex: Err(e) },
                },
            };
            ctx.data_mut(|d| d.insert_temp(preview_id, fresh.clone()));
            Some(fresh)
        }
        None => None,
    };

    // View checkboxes are fixed: main + right locked on, left disabled (no left-view block data).
    // Option checkboxes (first-version UI state, not persisted).
    let mut opt_basic = true;
    let mut opt_dim = false;
    let mut opt_block = false;
    let mut opt_group = false;

    ui.columns(3, |cols| {
        let [left, mid, right] = cols else { return };

        // ===== 左栏：多级折叠树（大类 → 标准族 → 叶子条目） =====
        left.label(RichText::new("国标列表").strong());
        egui::ScrollArea::vertical()
            .id_salt("parts-tree")
            .auto_shrink([false, true])
            .show(left, |ui| {
                for g in std_tree() {
                    egui::CollapsingHeader::new(RichText::new(g.title).strong())
                        .default_open(true)
                        .show(ui, |ui| {
                            for e in g.entries {
                                match e.cat {
                                    Some(key) => {
                                        let count = catalog.iter().filter(|p| &p.category == key).count();
                                        if count == 0 {
                                            continue;
                                        }
                                        let on = cat == key;
                                        let title = format!("{}\n{}  ({})", e.name, e.std, count);
                                        let resp = ui.selectable_label(on, RichText::new(title));
                                        if resp.clicked() {
                                            cat = key.to_string();
                                            sel = None;
                                        }
                                    }
                                    None => {
                                        let title = format!("{}\n{}", e.name, e.std);
                                        ui.add_enabled(
                                            false,
                                            egui::Label::new(RichText::new(title).color(t.text_dim)),
                                        );
                                    }
                                }
                            }
                        });
                }
            });

        // ===== 中栏：搜索 + 规格表 =====
        let mw = mid.available_width().max(60.0);
        mid.add(
            egui::TextEdit::singleline(&mut kw)
                .hint_text(crate::tl!("Search model…"))
                .desired_width(mw),
        );
        // 固定表头（不随列表滚动）
        let cols = columns(&cat);
        const NAME_W: f32 = 78.0;
        const SPEC_W: f32 = 40.0;
        const NUM_W: f32 = 30.0;
        const LEN_W: f32 = 32.0;
        mid.horizontal(|ui| {
            alloc_cell(ui, NAME_W, RichText::new("型号").strong());
            alloc_cell(ui, SPEC_W, RichText::new("规格").strong());
            for (h, _) in &cols {
                alloc_cell(ui, NUM_W, RichText::new(*h).strong());
            }
            alloc_cell(ui, LEN_W, RichText::new("长度").strong());
        });
        mid.separator();
        egui::ScrollArea::vertical()
            .id_salt("parts-table")
            .auto_shrink([false, true])
            .show(mid, |ui| {
                let mut dbl: Option<PartEntry> = None;
                for p in &filtered {
                    let on = sel.as_deref() == Some(p.name.as_str());
                    let (spec, len) = parse_spec(&p.name);
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(NAME_W, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let resp = ui.selectable_label(on, RichText::new(&p.name));
                                if resp.clicked() {
                                    sel = Some(p.name.clone());
                                }
                                if resp.double_clicked() {
                                    sel = Some(p.name.clone());
                                    dbl = Some((*p).clone());
                                }
                            },
                        );
                        alloc_cell(ui, SPEC_W, RichText::new(spec).color(t.text_dim));
                        for (_, key) in &cols {
                            let v = p.dim.get(*key).map(|n| fmt_num(*n)).unwrap_or_default();
                            alloc_cell(ui, NUM_W, RichText::new(v).color(t.text_dim));
                        }
                        alloc_cell(ui, LEN_W, RichText::new(len.unwrap_or_default()).color(t.text_dim));
                    });
                }
                if filtered.is_empty() {
                    ui.label(RichText::new(crate::tl!("No parts match.")).color(t.text_dim));
                }
                if let (Some(entry), Some(root)) = (dbl, root.as_ref()) {
                    do_pick_insert(app, root, &entry);
                    pending = Some(entry);
                }
            });

        // ===== 右栏：预览 + 选项 =====
        let preview_h = 150.0;
        let (resp, painter) =
            right.allocate_painter(egui::vec2(right.available_width(), preview_h), egui::Sense::hover());
        painter.rect_filled(resp.rect, 6.0, egui::Color32::from_rgb(0x17, 0x1a, 0x1e));
        match &preview {
            Some(PartPreview { tex: Ok(tex), .. }) => {
                let size = tex.size_vec2();
                let k = (resp.rect.width() / size.x).min(resp.rect.height() / size.y).min(1.0);
                let iw = size.x * k;
                let ih = size.y * k;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(
                        resp.rect.min.x + (resp.rect.width() - iw) / 2.0,
                        resp.rect.min.y + (resp.rect.height() - ih) / 2.0,
                    ),
                    egui::vec2(iw, ih),
                );
                painter.image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            Some(PartPreview { tex: Err(e), .. }) => {
                painter.text(resp.rect.center(), egui::Align2::CENTER_CENTER, e, crate::theme::body(), t.warn);
            }
            None => {
                let pname = chosen.as_ref().map(|e| e.name.as_str()).unwrap_or("—");
                painter.text(resp.rect.center(), egui::Align2::CENTER_CENTER, pname, crate::theme::body(), t.text_dim);
            }
        }

        // 选中件参数标注（型号 / 规格 / 各尺寸 / 长度）
        if let Some(e) = &chosen {
            let (spec, len) = parse_spec(&e.name);
            right.add_space(2.0);
            right.horizontal(|ui| {
                ui.label(RichText::new("型号").strong().color(t.text_dim));
                ui.label(&e.name);
            });
            right.horizontal(|ui| {
                ui.label(RichText::new("规格").strong().color(t.text_dim));
                ui.label(spec);
            });
            for (h, key) in &cols {
                let v = e.dim.get(*key).map(|n| fmt_num(*n)).unwrap_or_default();
                right.horizontal(|ui| {
                    ui.label(RichText::new(*h).strong().color(t.text_dim));
                    ui.label(v);
                });
            }
            if let Some(l) = len {
                right.horizontal(|ui| {
                    ui.label(RichText::new("长度").strong().color(t.text_dim));
                    ui.label(l);
                });
            }
        }

        right.add_space(6.0);
        right.label(RichText::new("视图").strong());
        right.horizontal(|ui| {
            let mut main = true;
            let mut left = false;
            let mut right = true;
            ui.add_enabled(false, egui::Checkbox::new(&mut main, "主视图"));
            ui.add_enabled(false, egui::Checkbox::new(&mut left, "左视图"));
            ui.add_enabled(false, egui::Checkbox::new(&mut right, "右视图"));
        });

        right.add_space(4.0);
        right.label(RichText::new("出库选项").strong());
        right.checkbox(&mut opt_group, "零件编组");
        right.checkbox(&mut opt_block, "零件做块");
        right.checkbox(&mut opt_basic, "基本图元");
        right.checkbox(&mut opt_dim, "尺寸标注");

        right.add_space(6.0);
        right.horizontal(|ui| {
            ui.label(crate::tl!("Scale"));
            ui.add(egui::TextEdit::singleline(&mut scale).desired_width(60.0));
            ui.label(crate::tl!("Rotation"));
            ui.add(egui::TextEdit::singleline(&mut rotation).desired_width(60.0));
        });

        let mut insert = false;
        let mut pick = false;
        right.horizontal(|ui| {
            insert = ui.button(crate::tl!("Insert")).clicked();
            pick = ui.button(crate::tl!("Pick insert")).clicked();
        });

        let x = 0.0;
        let y = 0.0;
        let s = scale.trim().parse::<f64>().unwrap_or(1.0);
        let rot = rotation.trim().parse::<f64>().unwrap_or(0.0);
        if let Some(entry) = chosen {
            if insert
                && let Some(root) = root.as_ref()
            {
                do_insert(app, root, &entry, x, y, s, rot);
            }
            if pick
                && let Some(root) = root.as_ref()
            {
                do_pick_insert(app, root, &entry);
                pending = Some(entry.clone());
            }
        }
    });

    // Persist transient state. Selection/scale are stored as plain String so the read-back type
    // matches the write type (egui temp store mixes in the Rust TypeId).
    ui.data_mut(|d| {
        d.insert_temp(kw_id, kw);
        d.insert_temp(cat_sel_id, cat);
        match &sel {
            Some(s) => {
                d.insert_temp(sel_id, s.clone());
            }
            None => {
                d.remove::<String>(sel_id);
            }
        }
        d.insert_temp(scale_id, scale);
        d.insert_temp(rot_id, rotation);
        match &pending {
            Some(p) => {
                d.insert_temp(pending_id, p.clone());
            }
            None => {
                d.remove::<PartEntry>(pending_id);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_engine::{Input, Session};
    use crate::Services;

    /// After a part is booked, the chained `bom.balloon` must start through the CadApp command
    /// dispatch (not be swallowed as a UI command), prompt for the leader start, then the circle
    /// location, and finish on the second picked point.
    #[test]
    fn chained_balloon_command_starts_and_takes_a_point() {
        let mut app = CadApp::new(Session::new(), Services::default());
        app.run("bom.add", json!({"block": "B1", "name": "螺栓", "kind": "标准件"})).unwrap();

        app.start("bom.balloon");
        let running = app.session.running.as_ref().expect("balloon command starts");
        assert_eq!(running.id, "bom.balloon");
        let prompt = app.session.current_prompt().expect("balloon prompts for a point");
        assert!(prompt.accept.point);

        // Stage 0: leader start on the part; stage 1: circle location.
        app.session.input(Input::Point(cadcraft_engine::geom::Vec2::new(10.0, 10.0))).unwrap();
        assert!(app.session.running.is_some(), "start point moves to the circle stage");
        app.session.input(Input::Point(cadcraft_engine::geom::Vec2::new(30.0, 30.0))).unwrap();
        assert!(app.session.running.is_none(), "circle point finishes the balloon");
        let d = app.session.doc().unwrap();
        assert_eq!(d.model.len(), 3, "leader + circle + number text");
    }
}
