//! Parts list (明细栏 / BOM) ledger, mirroring the Zhongwang part-list dialog.
//!
//! Rows merge same-name / same-model parts: inserting the same standard part again increments its
//! existing row's quantity instead of adding a duplicate row. Each row keeps the block-reference
//! handles and number-balloon handles so the on-sheet parts table and the numbered leader balloons
//! stay in sync.
//!
//! This round the ledger lives in [`crate::DocState`] (session memory). The on-sheet parts Table
//! geometry and balloon entities are saved with the file; the edited metadata is rebuilt on reopen.
//! File-level persistence of the ledger is a later enhancement (it needs records on both the DXF
//! write and read sides).

use cadcraft_doc::{CellStyle, Common, Drawing, EntityKind, Handle, HAlign, Lineweight, Space, TableStyle, Text, VAlign, entity_bounds};
use cadcraft_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::helpers::{circle, line, v3};
use super::table::make_table;
use super::machines::number;
use super::{CommandSpec, bad, bool_or, f64_or, point_param, point_req, str_param};
use crate::{Accept, Interactive, Input, Prompt, Result, Session, Step};

/// Part-type dropdown values (Zhongwang dialog).
#[allow(dead_code)]
pub const PART_KINDS: &[&str] = &[
    "标准件",
    "外购件",
    "借用件",
    "外协零件",
    "外委部件",
    "自制零件",
    "自制部件",
];

/// One parts-list row.
#[derive(Clone, Debug, Default)]
pub struct BomItem {
    /// Drawing ordinal, 1-based; renumbered after rows are added or removed.
    pub no: u32,
    /// Drawing number / code (图号/代号).
    pub code: String,
    /// Part name (名称).
    pub name: String,
    pub qty: i64,
    pub material: String,
    /// Unit weight (单重).
    pub weight: String,
    /// Total weight (总重).
    pub total_weight: String,
    pub note: String,
    /// Part type, one of [`PART_KINDS`].
    pub kind: String,
    /// Associated block name; the merge key when present.
    pub block: String,
    /// Handles of the block references on the sheet for this row.
    pub refs: Vec<Handle>,
    /// Handles of the number balloons for this row.
    pub balloons: Vec<Handle>,
}

/// The ledger for one open drawing.
#[derive(Clone, Debug, Default)]
pub struct BomState {
    pub items: Vec<BomItem>,
}

impl BomState {
    /// Locate an existing row by block name (preferred), else by code + name.
    fn find_index(&self, block: &str, code: &str, name: &str) -> Option<usize> {
        self.items.iter().position(|it| {
            if !block.is_empty() {
                it.block.eq_ignore_ascii_case(block)
            } else {
                it.code == code && it.name == name
            }
        })
    }

    /// Reassign 1-based ordinals in current row order after an add / remove / reorder.
    fn renumber(&mut self) {
        for (i, it) in self.items.iter_mut().enumerate() {
            it.no = i as u32 + 1;
        }
    }
}

fn handle_param(p: &Value, key: &str) -> Option<Handle> {
    p.get(key).and_then(Value::as_str).and_then(Handle::parse_hex)
}

/// Apply editable field overrides from a JSON object onto a row. Unknown keys are ignored.
fn apply_fields(it: &mut BomItem, fields: &Value) {
    let Some(o) = fields.as_object() else { return };
    let str = |v: &Value| v.as_str().unwrap_or_default().to_string();
    for (k, v) in o {
        match k.as_str() {
            "code" => it.code = str(v),
            "name" => it.name = str(v),
            "material" => it.material = str(v),
            "weight" => it.weight = str(v),
            "totalWeight" => it.total_weight = str(v),
            "note" => it.note = str(v),
            "kind" => it.kind = str(v),
            "qty" => it.qty = v.as_f64().filter(|x| *x >= 0.0).unwrap_or(1.0) as i64,
            _ => {}
        }
    }
}

fn row_json(it: &BomItem) -> Value {
    json!({
        "no": it.no,
        "code": it.code,
        "name": it.name,
        "qty": it.qty,
        "material": it.material,
        "weight": it.weight,
        "totalWeight": it.total_weight,
        "note": it.note,
        "kind": it.kind,
        "block": it.block,
        "refs": it.refs.iter().map(|h| h.hex()).collect::<Vec<_>>(),
        "balloons": it.balloons.iter().map(|h| h.hex()).collect::<Vec<_>>(),
    })
}

fn list_json(state: &BomState) -> Value {
    json!({ "count": state.items.len(), "items": state.items.iter().map(row_json).collect::<Vec<_>>() })
}

/// `bom.add`: book one part. Same-name / same-model parts merge and their quantity increments.
///
/// `{code?, name?, qty?: number (default 1), material?, weight?, totalWeight?, note?, kind?,
///   block?, ref?, balloon?}`
fn run_add(s: &mut Session, p: &Value) -> Result<Value> {
    let code = str_param(p, "code").unwrap_or_default().to_string();
    let name = str_param(p, "name").unwrap_or_default().to_string();
    let block = str_param(p, "block").unwrap_or_default().to_string();
    let qty = p.get("qty").and_then(Value::as_f64).filter(|x| *x > 0.0).unwrap_or(1.0) as i64;
    let r#ref = handle_param(p, "ref");
    let balloon = handle_param(p, "balloon");

    let bom = &mut s.state_mut()?.bom;
    let idx = bom.find_index(&block, &code, &name);

    let (index, no, qty, merged) = match idx {
        Some(i) => {
            let it = &mut bom.items[i];
            it.qty += qty;
            if let Some(h) = r#ref {
                if !it.refs.contains(&h) {
                    it.refs.push(h);
                }
            }
            if let Some(h) = balloon {
                if !it.balloons.contains(&h) {
                    it.balloons.push(h);
                }
            }
            (i, it.no, it.qty, true)
        }
        None => {
            let mut it = BomItem {
                no: 0,
                code,
                name,
                qty,
                material: str_param(p, "material").unwrap_or_default().to_string(),
                weight: str_param(p, "weight").unwrap_or_default().to_string(),
                total_weight: str_param(p, "totalWeight").unwrap_or_default().to_string(),
                note: str_param(p, "note").unwrap_or_default().to_string(),
                kind: str_param(p, "kind").unwrap_or_default().to_string(),
                block,
                refs: r#ref.into_iter().collect(),
                balloons: balloon.into_iter().collect(),
            };
            bom.items.push(it.clone());
            bom.renumber();
            let i = bom.items.len() - 1;
            it.no = bom.items[i].no;
            (i, it.no, it.qty, false)
        }
    };
    let count = bom.items.len();
    if let Ok(st) = s.state_mut() {
        st.last_bom = Some(no);
    }
    Ok(json!({ "index": index, "no": no, "qty": qty, "merged": merged, "count": count }))
}

/// `bom.list`: return every row as JSON.
fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let bom = s.state()?.bom.clone();
    Ok(list_json(&bom))
}

/// `bom.set`: edit fields, append an empty row, or remove a row.
///
/// Edit: `{index?: number, no?: number, fields: {code,name,qty,material,weight,totalWeight,note,kind}}`
/// Append: `{addRow: true, fields?}`
/// Remove: `{removeIndex?: number, removeNo?: number}`
fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let bom = &mut s.state_mut()?.bom;

    // Append an empty (or pre-filled) row.
    if bool_or(p, "addRow", false) {
        let mut it = BomItem::default();
        apply_fields(&mut it, p.get("fields").unwrap_or(&Value::Null));
        bom.items.push(it);
        bom.renumber();
        return Ok(list_json(bom));
    }

    // Remove a row by 0-based index or 1-based ordinal.
    if let Some(i) = p.get("removeIndex").and_then(Value::as_u64).map(|u| u as usize) {
        if i >= bom.items.len() {
            return Err(bad("bom.set", format!("removeIndex {i} out of range")));
        }
        bom.items.remove(i);
        bom.renumber();
        return Ok(list_json(bom));
    }
    if let Some(no) = p.get("removeNo").and_then(Value::as_u64).map(|n| n as u32) {
        let Some(i) = bom.items.iter().position(|it| it.no == no) else {
            return Err(bad("bom.set", format!("no row numbered {no}")));
        };
        bom.items.remove(i);
        bom.renumber();
        return Ok(list_json(bom));
    }

    // Edit an existing row.
    let idx = p
        .get("index")
        .and_then(Value::as_u64)
        .map(|u| u as usize)
        .or_else(|| p.get("no").and_then(Value::as_u64).map(|n| n as u32).and_then(|no| bom.items.iter().position(|it| it.no == no)));
    let Some(i) = idx else {
        return Err(bad("bom.set", "provide `index`/`no`, `addRow`, or `removeIndex`/`removeNo`"));
    };
    if i >= bom.items.len() {
        return Err(bad("bom.set", format!("index {i} out of range")));
    }
    let fields = p.get("fields").unwrap_or(&Value::Null);
    apply_fields(&mut bom.items[i], fields);
    Ok(list_json(bom))
}

/// `bom.clear`: remove every row (used before regenerating the on-sheet parts table).
fn run_clear(s: &mut Session, _p: &Value) -> Result<Value> {
    let bom = &mut s.state_mut()?.bom;
    bom.items.clear();
    Ok(json!({ "count": 0 }))
}

/// Layer holding number balloons (thin continuous leader + circle).
const BALLOON_LAYER: &str = "BALLOON";
/// Default balloon circle radius (mm) and number text height.
const BALLOON_RADIUS: f64 = 4.0;
const BALLOON_TEXT_H: f64 = 3.5;

/// Largest ordinal already drawn on a balloon in the active space, plus one; 1 when empty.
/// Balloon numbers increment off the on-sheet balloons themselves, so consecutive leaders come
/// out 1, 2, 3… without repeating, independent of the ledger rows.
fn next_balloon_no(s: &Session) -> u32 {
    let Ok(d) = s.doc() else { return 1 };
    let space = s.space();
    let mut max = 0u32;
    if let Some(store) = d.space(&space) {
        for e in store.iter() {
            if e.common.layer != BALLOON_LAYER {
                continue;
            }
            if let EntityKind::Text(t) = &e.kind
                && let Some(n) = number(&t.value)
                && n.fract() == 0.0 && n >= 1.0
            {
                let u = n as u32;
                if u > max {
                    max = u;
                }
            }
        }
    }
    max + 1
}

/// Balloon leader and circle: blue thin continuous line (GB annotation convention, ACI 5).
const BALLOON_LINE_COLOR: cadcraft_color::Color = cadcraft_color::Color::Index(5);
/// Balloon ordinal number: green text (ACI 3), matching the parts-list text.
const BALLOON_TEXT_COLOR: cadcraft_color::Color = cadcraft_color::Color::Index(3);

fn balloon_line_common() -> Common {
    Common {
        layer: BALLOON_LAYER.into(),
        color: BALLOON_LINE_COLOR,
        linetype: "ByLayer".into(),
        lineweight: Lineweight::Mm100(18),
        ..Common::default()
    }
}

fn balloon_text_common() -> Common {
    Common {
        layer: BALLOON_LAYER.into(),
        color: BALLOON_TEXT_COLOR,
        linetype: "ByLayer".into(),
        lineweight: Lineweight::Mm100(18),
        ..Common::default()
    }
}

fn add(s: &mut Session, space: &Space, common: Common, kind: EntityKind) -> Result<Handle> {
    Ok(s.doc_mut()?.add(space, common, kind)?)
}

/// Draw one numbered leader balloon (thin leader to the picked part point, a circle at the picked
/// tip, the ordinal centred in the circle) and attach its handles to the matching ledger row.
/// `no` of 0 means "one past the largest balloon already on the sheet".
fn draw_balloon(s: &mut Session, no: u32, attach: Vec2, tip: Vec2, radius: f64, height: f64) -> Result<Value> {
    let r = radius.abs();
    if height <= 0.0 {
        return Err(bad("bom.balloon", "height must be positive"));
    }

    // Ordinal: given, else one past the largest balloon already on the sheet.
    let no = if no > 0 { no } else { next_balloon_no(s) };

    let space = s.space();
    let _ = s.doc_mut().map(|d| d.ensure_layer(BALLOON_LAYER));

    // Leader runs from the part point to the circle edge facing it, not into the circle centre.
    let dir = tip - attach;
    let edge = if dir.len() < 1e-9 { tip } else { tip - dir.normalized() * r };
    let line_h = if edge.near(attach, 1e-9) {
        None
    } else {
        Some(add(s, &space, balloon_line_common(), line(attach, edge))?)
    };
    let circle_h = add(s, &space, balloon_line_common(), circle(tip, r))?;

    let style = s.doc().map(|d| d.header.str("TEXTSTYLE", "Standard")).unwrap_or_else(|_| "Standard".into());
    let text_kind = EntityKind::Text(Text {
        insert: v3(tip),
        align_pt: Some(v3(tip)),
        height,
        value: no.to_string(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style,
        halign: HAlign::Center,
        valign: VAlign::Middle,
    });
    let text_h = add(s, &space, balloon_text_common(), text_kind)?;

    // Record the balloon handles on the matching row (by ordinal).
    let handles: Vec<Handle> = [Some(circle_h), line_h, Some(text_h)].into_iter().flatten().collect();
    let bom = &mut s.state_mut()?.bom;
    if let Some(it) = bom.items.iter_mut().find(|i| i.no == no) {
        for h in &handles {
            if !it.balloons.contains(h) {
                it.balloons.push(*h);
            }
        }
    }

    Ok(json!({
        "no": no,
        "circle": circle_h.hex(),
        "line": line_h.map(|h| h.hex()),
        "text": text_h.hex(),
        "handles": handles.iter().map(|h| h.hex()).collect::<Vec<_>>(),
    }))
}

/// `bom.balloon`: programmatic entry. `{no?, attach: [x,y], tip: [x,y], radius?, height?}`.
fn run_balloon(s: &mut Session, p: &Value) -> Result<Value> {
    let attach = point_req("bom.balloon", p, "attach")?;
    let tip = point_req("bom.balloon", p, "tip")?;
    let r = f64_or(p, "radius", BALLOON_RADIUS);
    let th = f64_or(p, "height", BALLOON_TEXT_H);
    let no = p.get("no").and_then(Value::as_u64).map(|n| n as u32).filter(|n| *n > 0).unwrap_or(0);
    draw_balloon(s, no, attach, tip, r, th)
}

/// Interactive balloon: two picked points — the leader start on the part, then the numbered
/// circle location. The ordinal defaults to the next free number already on the sheet, but can be
/// overridden by typing a number at the circle prompt; 0 means "one past the largest balloon".
#[derive(Default)]
struct BalloonM {
    /// 0 = picking the leader start; 1 = picking the circle location.
    stage: u8,
    /// Part point the leader starts from.
    attach: Option<Vec2>,
    /// Candidate start from the booked reference, kept as the Enter default.
    attach_default: Option<Vec2>,
    /// Manually typed ordinal override; 0 = next free number on the sheet.
    no: u32,
}

impl BalloonM {
    /// Candidate leader start from the most recently booked reference insertion point.
    fn anchor(s: &Session) -> Option<Vec2> {
        let Ok(st) = s.state() else { return None };
        let target = st
            .bom
            .items
            .iter()
            .find(|i| Some(i.no) == st.last_bom)
            .or_else(|| st.bom.items.last())?;
        target
            .refs
            .last()
            .and_then(|h| s.doc().ok()?.entity(*h))
            .and_then(|e| match &e.kind { EntityKind::Insert(i) => Some(i.insert.xy()), _ => None })
    }
}

impl Interactive for BalloonM {
    fn name(&self) -> &'static str {
        "BOM BALLOON"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let attach = Self::anchor(s);
        // Snapshot the candidate start here: by the time a point answer arrives, `last_point` has
        // been overwritten by the picked point. The user can re-pick the start; Enter keeps this.
        self.attach_default = attach;
        self.attach = attach;
        self.no = 0;
        self.stage = 0;
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            0 => Prompt::new("Specify leader start point", Accept::POINT).base_opt(self.attach_default),
            _ => Prompt::new("Specify balloon circle location or <number>", Accept::POINT_OR_NUMBER).base_opt(self.attach),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.stage {
            0 => match i {
                Input::Point(p) => {
                    self.attach = Some(p);
                    self.stage = 1;
                    Ok(Step::Continue)
                }
                // Enter keeps the booked-reference candidate as the leader start.
                Input::Enter => {
                    if self.attach_default.is_some() {
                        self.stage = 1;
                    }
                    Ok(Step::Continue)
                }
                Input::Cancel => Ok(Step::Cancel),
                _ => Ok(Step::Continue),
            },
            _ => match i {
                Input::Point(tip) => {
                    let attach = self.attach.or(self.attach_default).unwrap_or(Vec2::ZERO);
                    draw_balloon(s, self.no, attach, tip, BALLOON_RADIUS, BALLOON_TEXT_H)?;
                    Ok(Step::Done)
                }
                // Type a positive whole number to override the balloon ordinal, then pick the circle.
                Input::Text(t) | Input::Keyword(t) => {
                    if let Some(n) = number(&t)
                        && n.is_finite()
                        && n >= 1.0
                        && n.fract() == 0.0
                    {
                        self.no = n as u32;
                    }
                    Ok(Step::Continue)
                }
                Input::Cancel => Ok(Step::Cancel),
                _ => Ok(Step::Continue),
            },
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        if self.stage != 1 {
            return Vec::new();
        }
        let Some(a) = self.attach.or(self.attach_default) else { return Vec::new() };
        vec![line(a, cursor)]
    }
}

/// Layer holding the regenerated on-sheet parts table.
const TABLE_LAYER: &str = "BOM_TABLE";
/// Parts-table header (图面六列，对齐国标栏目块：序号/代号/名称/数量/材料/备注).
const TABLE_HEADERS: &[&str] = &["序号", "代号", "名称", "数量", "材料", "备注"];
/// Column widths (mm), summing to the 180 mm GB parts-list width.
/// 代号优先：图号/标准号较长，代号列加宽；名称/材料/备注相应收窄。
const TABLE_WIDTHS: &[f64] = &[12.0, 50.0, 40.0, 14.0, 26.0, 38.0];
const TABLE_ROW_H: f64 = 7.0;
const TABLE_TEXT_H: f64 = 3.5;
/// Table style holding the green cell styles for the on-sheet parts list.
const BOM_TABLE_STYLE: &str = "BOM_PartsList";
/// Parts-list annotation text: green (ACI 3), matching the title-block text.
const TABLE_TEXT_COLOR: cadcraft_color::Color = cadcraft_color::Color::Index(3);
/// Title-block column block mounted by the sheet setup, used to anchor the parts list above it.
const TITLE_BLOCK: &str = "标题栏-1";

/// Default parts-table anchor: the title block's top-right corner (fallback: inner frame
/// top-right). The table's bottom edge sits on this line and grows **upward** above the title
/// block instead of dropping down over it; `None` when neither a title block nor a frame exists.
fn default_table_at(d: &Drawing, space: &Space) -> Option<Vec2> {
    let store = d.space(space)?;
    let mut title: Option<Bounds2> = None;
    let mut frame: Option<Bounds2> = None;
    for e in store.iter() {
        match &e.kind {
            EntityKind::Insert(i) if i.block == TITLE_BLOCK => {
                let b = entity_bounds(d, e, 0);
                if !b.is_empty() {
                    title = Some(b);
                }
            }
            _ if e.common.layer == "FRAME" => {
                let b = entity_bounds(d, e, 0);
                if !b.is_empty() {
                    frame = Some(b);
                }
            }
            _ => {}
        }
    }
    if let Some(b) = title {
        return Some(Vec2::new(b.max.x, b.max.y));
    }
    if let Some(b) = frame {
        return Some(Vec2::new(b.max.x, b.max.y - 2.0));
    }
    None
}

/// `bom.table`: regenerate the on-sheet parts table from the ledger. An earlier BOM table on the
/// same space is removed first, so re-running replaces it instead of stacking another. When `at`
/// is omitted the previous table's insertion point is reused; failing that the table anchors above
/// the title block (or inner frame). With neither a previous table, title block nor frame the
/// ledger is updated but no table is drawn yet.
///
/// `{at?: [x,y], rowHeight?}`
fn run_table(s: &mut Session, p: &Value) -> Result<Value> {
    let row_h = f64_or(p, "rowHeight", TABLE_ROW_H).max(2.0);

    // Snapshot the ledger before touching the drawing.
    let items = s.state()?.bom.items.clone();
    let space = s.space();

    // Snapshot the previous table anchor, and a default anchor above the title block / frame.
    let (prev_at, default_at): (Option<Vec2>, Option<Vec2>) = {
        let d = s.doc()?;
        let Some(store) = d.space(&space) else {
            return Ok(json!({ "handle": Value::Null, "rows": 0, "cols": TABLE_WIDTHS.len(), "placed": false }));
        };
        let prev = store
            .iter()
            .find(|e| e.common.layer == TABLE_LAYER && matches!(&e.kind, EntityKind::Table(_)))
            .map(|e| match &e.kind { EntityKind::Table(t) => t.insert.xy(), _ => unreachable!() });
        (prev, default_table_at(d, &space))
    };

    // Remove the previous BOM table(s) on this space.
    let stale: Vec<Handle> = match s.doc()?.space(&space) {
        Some(store) => store
            .iter()
            .filter(|e| e.common.layer == TABLE_LAYER && matches!(&e.kind, EntityKind::Table(_)))
            .map(|e| e.handle)
            .collect(),
        None => Vec::new(),
    };
    if let Some(store) = s.doc_mut()?.space_mut(&space) {
        for h in stale {
            store.remove(h);
        }
    }

    // Anchor is the table's bottom-right corner on the title block top edge; the table's
    // top-left insertion point is one table-height above it, so rows grow upward, not down
    // into the title block.
    let table_w: f64 = TABLE_WIDTHS.iter().sum();
    let total_h = (items.len() + 1) as f64 * row_h;
    // A title block / frame always re-anchors the table on top of it (GB parts-list sits above
    // the title block). The previous table's insertion point is reused only as a fallback when
    // neither a title block nor a frame exists, so a stale table left by an older build cannot
    // drag the regenerated parts list back down over the title block.
    let Some(at) = point_param(p, "at")
        .or_else(|| default_at.map(|a| Vec2::new(a.x - table_w, a.y + total_h)))
        .or(prev_at)
    else {
        return Ok(json!({ "handle": Value::Null, "rows": items.len(), "cols": TABLE_WIDTHS.len(), "placed": false }));
    };

    // GB/T parts list: the header row sits at the very bottom against the title block, and
    // ordinals run bottom-to-top (the first data row above the header is #1, the top row is
    // #n). Rows passed top-to-bottom are therefore data n … data 1, then the header row.
    let mut rows: Vec<Vec<String>> = items
        .iter()
        .rev()
        .map(|it| {
            vec![
                it.no.to_string(),
                it.code.clone(),
                it.name.clone(),
                it.qty.to_string(),
                it.material.clone(),
                it.note.clone(),
            ]
        })
        .collect();
    rows.push(TABLE_HEADERS.iter().map(|s| s.to_string()).collect());

    let mut style = TableStyle::default();
    style.name = BOM_TABLE_STYLE.into();
    style.text_height = TABLE_TEXT_H;
    style.margin = 0.5;
    // GB parts-list text renders green (ACI 3), matching the title-block annotation.
    // Cell styles inherit the style's 0.18 mm default height unless set here; without an
    // explicit height the ledger and header text would render at 0.18 mm inside a 7 mm row,
    // effectively invisible. Pin both to the parts-list text height.
    let mut data = CellStyle::default();
    data.name = CellStyle::DATA.into();
    data.text_height = TABLE_TEXT_H;
    data.margin_h = 0.5;
    data.margin_v = 0.3;
    data.text_color = TABLE_TEXT_COLOR;
    data.align = 4;
    style.set_cell_style(data);
    let mut header = CellStyle::default();
    header.name = CellStyle::HEADER.into();
    header.text_height = TABLE_TEXT_H;
    header.margin_h = 0.5;
    header.margin_v = 0.3;
    header.text_color = TABLE_TEXT_COLOR;
    header.align = 5;
    style.set_cell_style(header);
    // Persist the style so the renderer resolves its green cell styles by name.
    {
        let d = s.doc_mut()?;
        match d.table_styles.iter_mut().find(|x| x.name.eq_ignore_ascii_case(BOM_TABLE_STYLE)) {
            Some(existing) => *existing = style.clone(),
            None => d.table_styles.push(style.clone()),
        }
    }
    let mut t = make_table(&style, at, rows.len(), TABLE_WIDTHS.len(), Some(row_h), None, None, None, &rows);
    t.col_widths = TABLE_WIDTHS.to_vec();
    for h in t.row_heights.iter_mut() {
        *h = row_h;
    }
    t.text_height = TABLE_TEXT_H;
    // The header is the last row, not the positional top row: assign the row styles explicitly.
    t.header = false;
    t.title = false;
    t.row_styles = (0..rows.len())
        .map(|i| if i == rows.len() - 1 { "Header".into() } else { "Data".into() })
        .collect();

    let common = Common {
        layer: TABLE_LAYER.into(),
        color: cadcraft_color::Color::ByLayer,
        linetype: "ByLayer".into(),
        lineweight: Lineweight::Mm100(18),
        ..Common::default()
    };
    s.doc_mut()?.ensure_layer(TABLE_LAYER);
    let h = s.doc_mut()?.add(&space, common, EntityKind::Table(t))?;

    Ok(json!({
        "handle": h.hex(),
        "rows": rows.len().saturating_sub(1),
        "cols": TABLE_WIDTHS.len(),
        "width": TABLE_WIDTHS.iter().sum::<f64>(),
        "placed": true,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bom.add", "BOM Add", run_add).params(
            "{code?, name?, qty?, material?, weight?, totalWeight?, note?, kind?, block?, ref?, balloon?}",
        ),
        CommandSpec::new("bom.list", "BOM List", run_list).params("{}"),
        CommandSpec::new("bom.set", "BOM Set", run_set).params(
            "{index?|no?, fields?: {code,name,qty,material,weight,totalWeight,note,kind}, addRow?, removeIndex?, removeNo?}",
        ),
        CommandSpec::new("bom.clear", "BOM Clear", run_clear).params("{}"),
        CommandSpec::new("bom.balloon", "BOM Balloon", run_balloon).params(
            "{no?, attach: [x,y], tip: [x,y], radius?, height?}",
        ).menu(&["Dimension", "Balloon"]).interactive(|_| Ok(Box::new(BalloonM::default()))),
        CommandSpec::new("bom.table", "BOM Table", run_table).params("{at: [x,y], rowHeight?}")
            .menu(&["Insert", "Parts Table"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(s: &mut Session, id: &str, params: Value) -> Value {
        s.execute(id, &params).unwrap()
    }

    #[test]
    fn first_insert_creates_a_row() {
        let mut s = Session::new();
        let v = run(&mut s, "bom.add", json!({"block": "GB5782-M10x40", "name": "六角头螺栓", "code": "GB/T 5782", "kind": "标准件"}));
        assert_eq!(v["merged"], false);
        assert_eq!(v["no"], 1);
        assert_eq!(v["qty"], 1);

        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["count"], 1);
        assert_eq!(list["items"][0]["name"], "六角头螺栓");
        assert_eq!(list["items"][0]["qty"], 1);
    }

    #[test]
    fn same_block_merges_and_increments_qty() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "GB5782-M10x40", "name": "六角头螺栓"}));
        let v = run(&mut s, "bom.add", json!({"block": "GB5782-M10x40", "name": "六角头螺栓", "qty": 2}));
        assert_eq!(v["merged"], true);
        assert_eq!(v["qty"], 3);
        assert_eq!(v["no"], 1);

        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["count"], 1, "same model stays one row");
        assert_eq!(list["items"][0]["qty"], 3);
    }

    #[test]
    fn different_models_keep_separate_rows_and_renumber() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        run(&mut s, "bom.add", json!({"block": "B2", "name": "螺母"}));
        run(&mut s, "bom.add", json!({"block": "B3", "name": "垫圈"}));

        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["count"], 3);
        assert_eq!(list["items"][0]["no"], 1);
        assert_eq!(list["items"][2]["no"], 3);

        // Remove the middle row; ordinals renumber.
        run(&mut s, "bom.set", json!({"removeIndex": 1}));
        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["count"], 2);
        assert_eq!(list["items"][1]["no"], 2);
        assert_eq!(list["items"][1]["name"], "垫圈");
    }

    #[test]
    fn edit_fields_and_append_row() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓", "qty": 1}));
        run(&mut s, "bom.set", json!({"index": 0, "fields": {"material": "Q235", "qty": 4, "note": "外购"}}));
        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["items"][0]["material"], "Q235");
        assert_eq!(list["items"][0]["qty"], 4);
        assert_eq!(list["items"][0]["note"], "外购");

        run(&mut s, "bom.set", json!({"addRow": true, "fields": {"name": "自制件", "kind": "自制零件"}}));
        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["count"], 2);
        assert_eq!(list["items"][1]["no"], 2);
        assert_eq!(list["items"][1]["kind"], "自制零件");
    }

    #[test]
    fn clear_emits_zero_rows() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        let v = run(&mut s, "bom.clear", json!({}));
        assert_eq!(v["count"], 0);
        assert_eq!(run(&mut s, "bom.list", json!({}))["count"], 0);
    }

    #[test]
    fn balloon_draws_circle_leader_text_and_links_row() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        let v = run(&mut s, "bom.balloon", json!({"no": 1, "attach": [10.0, 10.0], "tip": [30.0, 30.0]}));
        assert_eq!(v["no"], 1);

        let d = s.doc().unwrap();
        assert_eq!(d.model.len(), 3, "leader + circle + number text");
        assert!(d.model.iter().all(|e| e.common.layer == BALLOON_LAYER));
        for e in d.model.iter() {
            match &e.kind {
                EntityKind::Text(_) => assert_eq!(e.common.color, cadcraft_color::Color::Index(3), "ordinal text green"),
                EntityKind::Line(_) | EntityKind::Circle(_) => assert_eq!(e.common.color, cadcraft_color::Color::Index(5), "leader/circle blue"),
                _ => {}
            }
        }
        let txt = d
            .model
            .iter()
            .find_map(|e| match &e.kind { EntityKind::Text(t) => Some(t.value.clone()), _ => None })
            .unwrap();
        assert_eq!(txt, "1");

        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["items"][0]["balloons"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn balloon_without_row_still_draws_and_keeps_ordinal() {
        let mut s = Session::new();
        let v = run(&mut s, "bom.balloon", json!({"attach": [0.0, 0.0], "tip": [20.0, 20.0]}));
        assert_eq!(v["no"], 1);
        assert_eq!(s.doc().unwrap().model.len(), 3);
    }

    #[test]
    fn table_renders_ledger_and_replaces_previous() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "六角头螺栓", "code": "GB/T 5782", "qty": 2, "material": "Q235"}));
        run(&mut s, "bom.add", json!({"block": "B2", "name": "螺母", "code": "GB/T 6170"}));

        let v = run(&mut s, "bom.table", json!({"at": [0.0, 0.0]}));
        assert_eq!(v["rows"], 2);

        let d = s.doc().unwrap();
        let tables: Vec<_> = d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Table(_))).collect();
        assert_eq!(tables.len(), 1);
        let t = match &tables[0].kind { EntityKind::Table(t) => t, _ => unreachable!() };
        assert_eq!(t.cells.len(), 3, "two data rows + header row");
        // GB order top-to-bottom: data #2, data #1, then the header at the very bottom.
        assert_eq!(t.cells[0][2].text, "螺母");
        assert_eq!(t.cells[1][2].text, "六角头螺栓");
        assert_eq!(t.cells[1][3].text, "2");
        assert_eq!(t.cells[2][0].text, "序号");
        assert_eq!(t.row_styles.last().map(|s| s.as_str()), Some("Header"));
        assert!(t.row_styles[..2].iter().all(|s| s == "Data"));
        assert_eq!(t.col_widths.len(), 6);
        assert_eq!(t.style, BOM_TABLE_STYLE);
        let st = d
            .table_styles
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(BOM_TABLE_STYLE))
            .unwrap();
        for cs in &st.cell_styles {
            assert_eq!(cs.text_color, cadcraft_color::Color::Index(3), "{} text green", cs.name);
        }

        // Re-running replaces the old table instead of stacking another.
        run(&mut s, "bom.table", json!({"at": [0.0, 0.0]}));
        let d = s.doc().unwrap();
        assert_eq!(d.model.iter().filter(|e| matches!(&e.kind, EntityKind::Table(_))).count(), 1);
    }

    #[test]
    fn table_without_anchor_and_no_previous_is_not_placed() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        let v = run(&mut s, "bom.table", json!({}));
        assert_eq!(v["placed"], false);
        assert!(s.doc().unwrap().model.iter().all(|e| !matches!(&e.kind, EntityKind::Table(_))));
    }

    #[test]
    fn interactive_balloon_chains_off_the_booked_row() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));

        // Stage 0 picks the leader start; stage 1 picks the circle location.
        s.start("bom.balloon").unwrap();
        assert!(s.running.is_some());
        s.input(Input::Point(Vec2::new(10.0, 10.0))).unwrap();
        assert!(s.running.is_some(), "start point moves to the circle stage");
        s.input(Input::Point(Vec2::new(30.0, 30.0))).unwrap();
        assert!(s.running.is_none(), "circle point finishes the command");

        let d = s.doc().unwrap();
        assert_eq!(d.model.len(), 3, "leader + circle + number text");
        let txt = d
            .model
            .iter()
            .find_map(|e| match &e.kind { EntityKind::Text(t) => Some(t.value.clone()), _ => None })
            .unwrap();
        assert_eq!(txt, "1");

        let list = run(&mut s, "bom.list", json!({}));
        assert_eq!(list["items"][0]["balloons"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn consecutive_balloons_number_increasing() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        run(&mut s, "bom.add", json!({"block": "B2", "name": "螺母"}));

        // First leader: next free number on an empty sheet is 1.
        s.start("bom.balloon").unwrap();
        s.input(Input::Point(Vec2::new(10.0, 10.0))).unwrap();
        s.input(Input::Point(Vec2::new(30.0, 30.0))).unwrap();
        assert!(s.running.is_none());

        // Second leader: one past the balloon already on the sheet, i.e. 2.
        s.start("bom.balloon").unwrap();
        s.input(Input::Point(Vec2::new(12.0, 12.0))).unwrap();
        s.input(Input::Point(Vec2::new(32.0, 32.0))).unwrap();
        assert!(s.running.is_none());

        let mut texts: Vec<String> = s
            .doc()
            .unwrap()
            .model
            .iter()
            .filter_map(|e| match &e.kind { EntityKind::Text(t) => Some(t.value.clone()), _ => None })
            .collect();
        texts.sort();
        assert_eq!(texts, vec!["1", "2"], "consecutive balloons increment off the sheet");
    }

    #[test]
    fn interactive_balloon_accepts_typed_number_override() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));

        s.start("bom.balloon").unwrap();
        s.input(Input::Point(Vec2::new(10.0, 10.0))).unwrap();
        s.input(Input::Text("5".into())).unwrap();
        assert!(s.running.is_some(), "typed number keeps waiting for the circle point");
        s.input(Input::Point(Vec2::new(30.0, 30.0))).unwrap();
        assert!(s.running.is_none());

        let txt = s
            .doc()
            .unwrap()
            .model
            .iter()
            .find_map(|e| match &e.kind { EntityKind::Text(t) => Some(t.value.clone()), _ => None })
            .unwrap();
        assert_eq!(txt, "5", "typed ordinal overrides the row number");
    }

    #[test]
    fn cancelled_balloon_draws_nothing() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        s.start("bom.balloon").unwrap();
        s.input(Input::Cancel).unwrap();
        assert!(s.running.is_none());
        assert_eq!(s.doc().unwrap().model.len(), 0);
        let list = run(&mut s, "bom.list", json!({}));
        assert!(list["items"][0]["balloons"].as_array().unwrap().is_empty());
    }

    #[test]
    fn table_anchors_above_frame_and_grows_upward() {
        let mut s = Session::new();
        run(&mut s, "sheet", json!({"size": "A3"}));
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        run(&mut s, "bom.add", json!({"block": "B2", "name": "螺母"}));

        let v = run(&mut s, "bom.table", json!({}));
        assert_eq!(v["placed"], true);

        let d = s.doc().unwrap();
        let t = d
            .model
            .iter()
            .find_map(|e| match &e.kind { EntityKind::Table(t) => Some(t), _ => None })
            .expect("table placed");

        // A3 inner frame: right 415, top 292; the frame fallback anchor is (right, top - 2).
        let anchor = Vec2::new(415.0, 292.0 - 2.0);
        let table_w: f64 = TABLE_WIDTHS.iter().sum();
        let total_h = (2 + 1) as f64 * TABLE_ROW_H;

        assert!((t.insert.x - (anchor.x - table_w)).abs() < 1e-6, "right-aligned above the frame");
        let bottom = t.insert.y - t.row_heights.iter().sum::<f64>();
        assert!((bottom - anchor.y).abs() < 1e-6, "table bottom sits on the anchor line, rows grow upward");
        assert!((t.insert.y - (anchor.y + total_h)).abs() < 1e-6);
    }

    #[test]
    fn table_reanchors_to_frame_instead_of_stale_previous_point() {
        let mut s = Session::new();
        run(&mut s, "sheet", json!({"size": "A3"}));
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        // First table placed at an explicit, off-frame point; its insertion point becomes prev_at.
        run(&mut s, "bom.table", json!({"at": [0.0, 0.0]}));
        let stale = s
            .doc()
            .unwrap()
            .model
            .iter()
            .find_map(|e| match &e.kind { EntityKind::Table(t) => Some(t.insert.xy()), _ => None })
            .expect("first table");
        assert_eq!(stale, Vec2::ZERO);

        // Regenerating without an explicit point re-anchors to the frame, not the stale (0,0).
        run(&mut s, "bom.table", json!({}));
        let t = s
            .doc()
            .unwrap()
            .model
            .iter()
            .find_map(|e| match &e.kind { EntityKind::Table(t) => Some(t), _ => None })
            .expect("regenerated table");
        assert_ne!(t.insert.xy(), Vec2::ZERO, "does not reuse the stale previous point");
        let anchor = Vec2::new(415.0, 292.0 - 2.0);
        let table_w: f64 = TABLE_WIDTHS.iter().sum();
        let total_h = (1 + 1) as f64 * TABLE_ROW_H;
        assert!((t.insert.x - (anchor.x - table_w)).abs() < 1e-6);
        assert!((t.insert.y - (anchor.y + total_h)).abs() < 1e-6);
    }

    #[test]
    fn table_cell_styles_pin_the_parts_list_text_height() {
        let mut s = Session::new();
        run(&mut s, "bom.add", json!({"block": "B1", "name": "螺栓"}));
        run(&mut s, "bom.table", json!({"at": [0.0, 0.0]}));

        let d = s.doc().unwrap();
        let st = d
            .table_styles
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(BOM_TABLE_STYLE))
            .expect("parts-list style persisted");
        for cs in &st.cell_styles {
            assert_eq!(cs.text_height, TABLE_TEXT_H, "{} text must use the parts-list height, not the 0.18 mm default", cs.name);
            assert_eq!(cs.text_color, cadcraft_color::Color::Index(3), "{} text green", cs.name);
        }
    }
}
