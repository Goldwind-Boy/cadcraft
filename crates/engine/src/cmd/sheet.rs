//! GB sheet frame (图幅设置): draws the paper boundary (thin) and the inner frame (thick) for a
//! chosen GB size / orientation / margin, and reports the inner frame's geometry so the UI can
//! mount title-block / parts-list column blocks at its lower-right corner.
//!
//! Sizes follow GB/T 14689: basic formats B (short) × L (long), binding margin a = 25 on the
//! left, side margin c (10 for A0/A1/A2, 5 for A3/A4); the no-binding edge e is 20 for A0/A1,
//! 10 otherwise. The title block follows GB/T 10609.1: 180 wide, 48 high on A2/A3/A4 and 56 on
//! A0/A1.

use cadcraft_doc::{Common, Entity, EntityKind, Handle, Lineweight};
use cadcraft_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::helpers::{lwpoly, rect_vertices};
use super::modify::scale_entities;
use super::{CommandSpec, bad, bool_or, f64_or, point_param, str_param};
use crate::{Result, Session};

/// GB basic format: (B short edge, L long edge, side margin c, no-binding edge e).
const FORMATS: &[(&str, f64, f64, f64, f64)] = &[
    ("A0", 841.0, 1189.0, 10.0, 20.0),
    ("A1", 594.0, 841.0, 10.0, 20.0),
    ("A2", 420.0, 594.0, 10.0, 10.0),
    ("A3", 297.0, 420.0, 5.0, 10.0),
    ("A4", 210.0, 297.0, 5.0, 10.0),
];

/// Binding margin on the left edge.
const BIND_A: f64 = 25.0;
/// Title block height (GB/T 10609.1): taller on A0/A1.
fn title_height(code: &str) -> f64 {
    if matches!(code, "A0" | "A1") { 56.0 } else { 48.0 }
}

fn frame_common(layer: &str, lw: u16) -> Common {
    Common {
        layer: layer.into(),
        color: cadcraft_color::Color::ByLayer,
        linetype: "ByLayer".into(),
        lineweight: Lineweight::Mm100(lw),
        ..Common::default()
    }
}

/// Add a closed rectangular polyline to the active space.
fn add_rect(s: &mut Session, space: &cadcraft_doc::Space, common: Common, a: Vec2, b: Vec2) -> Result<Handle> {
    let kind: EntityKind = lwpoly(rect_vertices(a, b), true);
    Ok(s.doc_mut()?.add(space, common, kind)?)
}

/// Layers holding a previously generated frame (thin paper boundary / thick inner frame).
const FRAME_LAYERS: &[&str] = &["FRAME_THIN", "FRAME"];
/// Block names of the title block / parts-list column blocks mounted by an earlier run.
const COLUMN_BLOCKS: &[&str] = &["标题栏-1", "明细表1"];

/// Remove the frame and column blocks an earlier `sheet` run left behind, so re-running replaces
/// the old sheet instead of stacking another frame and title block on top of it.
fn clear_previous(s: &mut Session, space: &cadcraft_doc::Space) -> Result<()> {
    let Some(store) = s.doc_mut()?.space_mut(space) else { return Ok(()) };
    let stale: Vec<Handle> = store
        .iter()
        .filter(|e| {
            FRAME_LAYERS.contains(&e.common.layer.as_str())
                || matches!(&e.kind, EntityKind::Insert(i) if COLUMN_BLOCKS.contains(&i.block.as_str()))
        })
        .map(|e| e.handle)
        .collect();
    for h in stale {
        store.remove(h);
    }
    Ok(())
}

/// Layers holding annotations / sheet furnishings that keep their own size when the drawing
/// scale changes: balloons, parts table, datum symbols and the title-block column layer.
const ANNO_LAYERS: &[&str] = &["BALLOON", "BOM_TABLE", "DATUM", "PART_THIN"];

/// Layer holding an imported sheet frame block (中望图框块), which stays put when the scale changes.
const FRAME_BLOCK_LAYER: &str = "图框层";

/// Whether `e` is part geometry that follows the drawing scale. Part block references scale
/// together with the loose outline geometry so a view does not split apart; sheet frames, column
/// blocks, annotations, tables, hatches and proxy objects keep their own size. Annotation text and
/// dimension heights are sized by the annotation scale, not the sheet scale.
fn scales_on_sheet(e: &Entity) -> bool {
    let layer = e.common.layer.as_str();
    if FRAME_LAYERS.contains(&layer) || ANNO_LAYERS.contains(&layer) || layer == FRAME_BLOCK_LAYER {
        return false;
    }
    match &e.kind {
        EntityKind::Line(_)
        | EntityKind::Point(_)
        | EntityKind::Circle(_)
        | EntityKind::Arc(_)
        | EntityKind::Ellipse(_)
        | EntityKind::LwPolyline(_)
        | EntityKind::Polyline3d(_)
        | EntityKind::Spline(_)
        | EntityKind::Helix(_)
        | EntityKind::Ray(_)
        | EntityKind::XLine(_) => true,
        // Part block references scale with the outline; the title-block / parts-list column blocks
        // stay put.
        EntityKind::Insert(i) => !COLUMN_BLOCKS.contains(&i.block.as_str()),
        // Dimensions, plain leaders and tolerances follow the outline to their new position; their
        // text/arrow heights stay put (sized by the annotation scale) and the measurement snapshot
        // is unchanged.
        EntityKind::Dimension(_) | EntityKind::Leader(_) | EntityKind::Tolerance(_) => true,
        // Text/mtext, multileaders, hatches, solids, traces, wipes, tables, viewports, images,
        // regions, meshes and proxy objects keep their own size.
        _ => false,
    }
}

/// Scale denominator `n` from a `1:n` ratio parameter (positive integer, default 1).
fn ratio_denominator(p: &Value) -> Result<u32> {
    let n = match p.get("ratio") {
        Some(Value::Number(v)) => v.as_f64().unwrap_or(1.0),
        Some(Value::String(t)) => t.trim().trim_start_matches("1:").parse::<f64>().unwrap_or(1.0),
        _ => 1.0,
    };
    if !(n >= 1.0) || !n.is_finite() {
        return Err(bad("sheet", "ratio must be a positive `1:n` denominator (n ≥ 1)"));
    }
    Ok(n.round() as u32)
}

/// JSON form:
/// `{size: "A0".."A4"|"custom", width?, height?, orientation?: "landscape"|"portrait",
///   bind?: bool (default true, binding margin), origin?: [x,y], ratio?: n (1:n denominator)}`
pub(crate) fn run_sheet(s: &mut Session, p: &Value) -> Result<Value> {
    let code = str_param(p, "size").unwrap_or("A3").to_ascii_uppercase();
    let bind = bool_or(p, "bind", true);
    let portrait = str_param(p, "orientation").is_some_and(|o| o.eq_ignore_ascii_case("portrait"));
    let origin = point_param(p, "origin").unwrap_or(Vec2::ZERO);

    let (b, l, c, e) = match FORMATS.iter().find(|(k, _, _, _, _)| *k == code) {
        Some(row) => (row.1, row.2, row.3, row.4),
        None if code == "CUSTOM" => {
            let w = f64_or(p, "width", 0.0);
            let h = f64_or(p, "height", 0.0);
            if w <= 0.0 || h <= 0.0 {
                return Err(bad("sheet", "custom size needs positive `width` and `height`"));
            }
            // For a custom sheet treat the longer side as L and the shorter as B, then honour orientation.
            let (bb, ll) = if w >= h { (h, w) } else { (w, h) };
            (bb, ll, 10.0, 10.0)
        }
        None => return Err(bad("sheet", format!("unknown size `{code}` (A0..A4 or custom)"))),
    };

    // Landscape: long edge along X; portrait: short edge along X.
    let (w, h) = if portrait { (b, l) } else { (l, b) };
    if w <= 1e-6 || h <= 1e-6 {
        return Err(bad("sheet", "the sheet has no area"));
    }

    // Margins: binding keeps a 25 mm left edge; otherwise an even edge e on all sides.
    let (left, right, bottom, top) = if bind {
        (BIND_A, w - c, c, h - c)
    } else {
        (e, w - e, e, h - e)
    };
    if left >= right || bottom >= top {
        return Err(bad("sheet", "margins are larger than the sheet"));
    }

    let space = s.space();
    clear_previous(s, &space)?;

    // Scale bookkeeping: scale the existing bare geometry by the ratio between the previously
    // applied scale and the requested one, so toggling the scale back and forth does not stack
    // error. Furnishings / annotations keep their own size.
    let new_n = ratio_denominator(p)?;
    let old_n = s.state()?.sheet_ratio.max(1);

    // Collect the scalable geometry after the old frame / column blocks are cleared and before
    // the new frame is drawn, so neither is scaled.
    let targets: Vec<Handle> = s
        .doc()?
        .space(&space)
        .map(|store| store.iter().filter(|e| scales_on_sheet(e)).map(|e| e.handle).collect())
        .unwrap_or_default();

    let outer_a = origin;
    let outer_b = origin + Vec2::new(w, h);
    let inner_a = origin + Vec2::new(left, bottom);
    let inner_b = origin + Vec2::new(right, top);

    // Paper boundary: thin continuous line. Inner frame: thick continuous line.
    add_rect(s, &space, frame_common("FRAME_THIN", 18), outer_a, outer_b)?;
    add_rect(s, &space, frame_common("FRAME", 70), inner_a, inner_b)?;

    // Scale the bare part geometry about the inner frame's lower-left corner.
    let mut scaled = 0usize;
    let mut factor = 1.0_f64;
    if new_n != old_n && !targets.is_empty() {
        factor = old_n as f64 / new_n as f64;
        let base = Vec3::new(inner_a.x, inner_a.y, 0.0);
        scaled = scale_entities(s, &targets, base, factor, false)?.len();
    }
    s.state_mut()?.sheet_ratio = new_n;

    let inner_right = origin + Vec2::new(right, bottom);
    Ok(json!({
        "size": code,
        "width": w,
        "height": h,
        "orientation": if portrait { "portrait" } else { "landscape" },
        "bind": bind,
        "origin": [origin.x, origin.y],
        "inner": { "left": left, "right": right, "bottom": bottom, "top": top },
        "innerRight": [inner_right.x, inner_right.y],
        "titleHeight": title_height(&code),
        "partsHeight": 35.0,
        "ratio": new_n,
        "scaleFactor": factor,
        "scaled": scaled,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("sheet", "Sheet Setup", run_sheet)
        .menu(&["Format", "Sheet Setup"])
        .params("{size: \"A0\"..\"A4\"|\"custom\", width?, height?, orientation?: \"landscape\"|\"portrait\", bind?: bool, origin?: [x,y], ratio?: n (1:n denominator)}")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::{DimKind, Dimension, HAlign, Insert, Text, VAlign};
    use super::super::helpers::{line, v3};
    use serde_json::json;

    fn run(s: &mut Session, params: Value) -> Value {
        s.execute("sheet", &params).unwrap()
    }

    #[test]
    fn a3_landscape_binding() {
        let mut s = Session::new();
        let v = run(&mut s, json!({"size": "A3", "orientation": "landscape", "bind": true}));
        assert_eq!(v["width"], 420.0);
        assert_eq!(v["height"], 297.0);
        assert_eq!(v["inner"], json!({"left": 25.0, "right": 415.0, "bottom": 5.0, "top": 292.0}));
        assert_eq!(v["innerRight"], json!([415.0, 5.0]));
        assert_eq!(v["titleHeight"], 48.0);

        let d = s.doc().unwrap();
        assert_eq!(d.model.len(), 2, "one outer + one inner frame");
        let thin = d.model.iter().find(|e| e.common.layer == "FRAME_THIN").unwrap();
        let thick = d.model.iter().find(|e| e.common.layer == "FRAME").unwrap();
        assert_eq!(thin.common.lineweight, Lineweight::Mm100(18));
        assert_eq!(thick.common.lineweight, Lineweight::Mm100(70));
    }

    #[test]
    fn a3_portrait_swaps_edges() {
        let mut s = Session::new();
        let v = run(&mut s, json!({"size": "A3", "orientation": "portrait", "bind": true}));
        assert_eq!(v["width"], 297.0);
        assert_eq!(v["height"], 420.0);
        assert_eq!(v["innerRight"], json!([292.0, 5.0]));
    }

    #[test]
    fn a4_and_a0_margins() {
        let mut s = Session::new();
        let v = run(&mut s, json!({"size": "A4", "orientation": "landscape", "bind": true}));
        assert_eq!(v["width"], 297.0);
        assert_eq!(v["height"], 210.0);
        assert_eq!(v["innerRight"], json!([292.0, 5.0]));

        let mut s2 = Session::new();
        let v2 = run(&mut s2, json!({"size": "A0", "orientation": "landscape", "bind": true}));
        // A0: B 841 x L 1189, side margin c = 10, title block 56 high.
        assert_eq!(v2["width"], 1189.0);
        assert_eq!(v2["height"], 841.0);
        assert_eq!(v2["innerRight"], json!([1179.0, 10.0]));
        assert_eq!(v2["titleHeight"], 56.0);
    }

    #[test]
    fn no_binding_even_edge() {
        let mut s = Session::new();
        let v = run(&mut s, json!({"size": "A3", "orientation": "landscape", "bind": false}));
        // A3 no-binding edge e = 10 on all sides.
        assert_eq!(v["inner"], json!({"left": 10.0, "right": 410.0, "bottom": 10.0, "top": 287.0}));
    }

    #[test]
    fn custom_size_requires_positive() {
        let mut s = Session::new();
        assert!(s
            .execute("sheet", &json!({"size": "custom", "orientation": "landscape"}))
            .is_err());
        let v = run(&mut s, json!({"size": "custom", "width": 300.0, "height": 200.0, "orientation": "landscape"}));
        assert_eq!(v["width"], 300.0);
        assert_eq!(v["height"], 200.0);
    }

    #[test]
    fn rerun_replaces_previous_frame_and_columns() {
        let mut s = Session::new();
        run(&mut s, json!({"size": "A3", "orientation": "landscape", "bind": true}));

        // Simulate a title block mounted by the earlier dialog run.
        let space = s.space();
        let ins = Insert { block: "标题栏-1".into(), ..Default::default() };
        s.doc_mut().unwrap().add(&space, frame_common("0", 18), EntityKind::Insert(ins)).unwrap();
        assert_eq!(s.doc().unwrap().model.len(), 3);

        run(&mut s, json!({"size": "A4", "orientation": "landscape", "bind": true}));
        let d = s.doc().unwrap();
        assert_eq!(d.model.len(), 2, "previous frame and title block are replaced, not stacked");
        assert!(d.model.iter().all(|e| !matches!(e.kind, EntityKind::Insert(_))));
        assert!(d.model.iter().all(|e| FRAME_LAYERS.contains(&e.common.layer.as_str())));
    }

    fn add_line(s: &mut Session, a: Vec2, b: Vec2) {
        let space = s.space();
        s.doc_mut().unwrap().add(&space, Common::default(), line(a, b)).unwrap();
    }

    #[test]
    fn scale_shrinks_bare_geometry_about_frame_corner() {
        let mut s = Session::new();
        add_line(&mut s, Vec2::new(10.0, 10.0), Vec2::new(30.0, 10.0));

        // 1:2 → factor 1/2, scaled about the A3 inner-frame lower-left corner (25, 5).
        let v = run(&mut s, json!({"size": "A3", "ratio": 2}));
        assert_eq!(v["ratio"], 2);
        assert!((v["scaleFactor"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(v["scaled"], 1);

        let d = s.doc().unwrap();
        let l = d.model.iter().find_map(|e| match &e.kind { EntityKind::Line(x) => Some(x.clone()), _ => None }).unwrap();
        // (10,10) -> (25 + (10-25)/2, 5 + (10-5)/2) = (17.5, 7.5); (30,10) -> (27.5, 7.5).
        assert!((l.a.x - 17.5).abs() < 1e-9 && (l.a.y - 7.5).abs() < 1e-9);
        assert!((l.b.x - 27.5).abs() < 1e-9 && (l.b.y - 7.5).abs() < 1e-9);
    }

    #[test]
    fn scale_back_to_one_restores_the_geometry() {
        let mut s = Session::new();
        add_line(&mut s, Vec2::new(10.0, 10.0), Vec2::new(30.0, 10.0));
        run(&mut s, json!({"size": "A3", "ratio": 2}));

        // Back to 1:1: factor old/new = 2/1 = 2, restoring the original coordinates.
        let v = run(&mut s, json!({"size": "A3", "ratio": 1}));
        assert!((v["scaleFactor"].as_f64().unwrap() - 2.0).abs() < 1e-9);

        let d = s.doc().unwrap();
        let l = d.model.iter().find_map(|e| match &e.kind { EntityKind::Line(x) => Some(x.clone()), _ => None }).unwrap();
        assert!((l.a.x - 10.0).abs() < 1e-9 && (l.a.y - 10.0).abs() < 1e-9);
        assert!((l.b.x - 30.0).abs() < 1e-9 && (l.b.y - 10.0).abs() < 1e-9);
    }

    #[test]
    fn annotations_keep_their_size_on_scale_change() {
        let mut s = Session::new();
        let space = s.space();
        let txt = EntityKind::Text(Text {
            insert: v3(Vec2::new(10.0, 10.0)),
            align_pt: None,
            height: 3.5,
            value: "A".into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        });
        s.doc_mut().unwrap().add(&space, Common::default(), txt).unwrap();

        let v = run(&mut s, json!({"size": "A3", "ratio": 2}));
        assert_eq!(v["scaled"], 0, "text is annotation, not part geometry");

        let d = s.doc().unwrap();
        let t = d.model.iter().find_map(|e| match &e.kind { EntityKind::Text(x) => Some(x.clone()), _ => None }).unwrap();
        assert!((t.insert.x - 10.0).abs() < 1e-9 && (t.insert.y - 10.0).abs() < 1e-9);
        assert!((t.height - 3.5).abs() < 1e-9);
    }

    #[test]
    fn part_block_reference_scales_with_the_outline() {
        let mut s = Session::new();
        let space = s.space();
        let ins = Insert {
            block: "转子磁极1".into(),
            insert: v3(Vec2::new(10.0, 10.0)),
            scale: Vec3::new(1.0, 1.0, 1.0),
            ..Default::default()
        };
        s.doc_mut().unwrap().add(&space, Common::default(), EntityKind::Insert(ins)).unwrap();

        // 1:2 → factor 1/2 about (25, 5): insert point (10,10) -> (17.5, 7.5), block scale 1 -> 0.5.
        let v = run(&mut s, json!({"size": "A3", "ratio": 2}));
        assert_eq!(v["scaled"], 1, "part block reference follows the outline scale");

        let d = s.doc().unwrap();
        let i = d.model.iter().find_map(|e| match &e.kind { EntityKind::Insert(x) => Some(x.clone()), _ => None }).unwrap();
        assert!((i.insert.x - 17.5).abs() < 1e-9 && (i.insert.y - 7.5).abs() < 1e-9);
        assert!((i.scale.x - 0.5).abs() < 1e-9 && (i.scale.y - 0.5).abs() < 1e-9);
    }

    #[test]
    fn frame_layer_block_keeps_its_size() {
        let mut s = Session::new();
        let space = s.space();
        let ins = Insert {
            block: "ZwmFrameMain_A3".into(),
            insert: v3(Vec2::new(10.0, 10.0)),
            scale: Vec3::new(2.0, 2.0, 2.0),
            ..Default::default()
        };
        s.doc_mut().unwrap().add(&space, frame_common(FRAME_BLOCK_LAYER, 18), EntityKind::Insert(ins)).unwrap();

        let v = run(&mut s, json!({"size": "A3", "ratio": 2}));
        assert_eq!(v["scaled"], 0, "imported frame block stays put");

        let d = s.doc().unwrap();
        let i = d.model.iter().find_map(|e| match &e.kind { EntityKind::Insert(x) => Some(x.clone()), _ => None }).unwrap();
        assert!((i.insert.x - 10.0).abs() < 1e-9 && (i.insert.y - 10.0).abs() < 1e-9);
        assert!((i.scale.x - 2.0).abs() < 1e-9 && (i.scale.y - 2.0).abs() < 1e-9);
    }

    #[test]
    fn dimension_follows_outline_position_but_keeps_measurement() {
        let mut s = Session::new();
        let space = s.space();
        let dm = EntityKind::Dimension(Dimension {
            kind: DimKind::Linear { rotation: 0.0 },
            defpt: v3(Vec2::new(10.0, 20.0)),
            text_mid: Vec3::ZERO,
            p13: v3(Vec2::new(10.0, 10.0)),
            p14: v3(Vec2::new(30.0, 10.0)),
            p15: v3(Vec2::ZERO),
            p16: v3(Vec2::ZERO),
            text: String::new(),
            style: "ISO-25".into(),
            measurement: 50.0,
            text_rotation: 0.0,
            user_text_pos: false,
            block: None,
            overrides: Default::default(),
            assoc: Vec::new(),
            marks: Default::default(),
            insertion_point: Vec3::ZERO,
        });
        s.doc_mut().unwrap().add(&space, Common::default(), dm).unwrap();

        // 1:2 → factor 1/2 about (25, 5): definition points follow the outline, the measurement
        // snapshot and the text/arrow heights stay unchanged.
        let v = run(&mut s, json!({"size": "A3", "ratio": 2}));
        assert_eq!(v["scaled"], 1, "dimension follows the outline position");

        let d = s.doc().unwrap();
        let dm = d.model.iter().find_map(|e| match &e.kind { EntityKind::Dimension(x) => Some(x.clone()), _ => None }).unwrap();
        assert!((dm.p13.x - 17.5).abs() < 1e-9 && (dm.p13.y - 7.5).abs() < 1e-9);
        assert!((dm.p14.x - 27.5).abs() < 1e-9 && (dm.p14.y - 7.5).abs() < 1e-9);
        assert!((dm.defpt.x - 17.5).abs() < 1e-9 && (dm.defpt.y - 12.5).abs() < 1e-9);
        assert!((dm.measurement - 50.0).abs() < 1e-9, "measurement snapshot unchanged");
    }
}
