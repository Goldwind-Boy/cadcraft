//! Title block (标题栏) editable fields, mirroring the Zhongwang "标题栏编辑" dialog.
//!
//! The GB title block imported from `sheets/标题栏-1.dxf` is a static block: its labels are
//! fixed TEXT and nothing on the sheet can be filled in. To make its cells editable the engine
//! injects an attribute definition ([`EntityKind::AttDef`]) per fillable field into the block
//! when it is defined; INSERT instantiates each as an ATTRIB on the reference, and the
//! `title.props` / `title.set` commands back the two-column dialog (prompt | value).
//!
//! Field coordinates are block-local: the block base point is its upper-right corner, x runs
//! negative leftwards and y positive upwards (the imported frame spans x -180..0, y 0..48).

use std::collections::HashMap;

use cadcraft_color::Color;
use cadcraft_doc::{Attrib, AttribProps, Common, Entity, EntityKind, Handle, HAlign, Text, VAlign};
use cadcraft_geom::{Mat3, Vec3};
use serde_json::{Map, Value, json};

use super::{CommandSpec, bad};
use crate::{Result, Session};

/// GB title-block column block mounted by the sheet setup.
pub const TITLE_BLOCK: &str = "标题栏-1";

/// One fillable title-block field.
pub struct TitleField {
    /// DXF attribute tag (upper-case identifier).
    pub tag: &'static str,
    /// Chinese prompt shown in the dialog's left column.
    pub prompt: &'static str,
    /// Block-local insertion point (left-baseline start) and text height.
    pub x: f64,
    pub y: f64,
    pub height: f64,
}

/// The fillable cells of the GB title block, in the Zhongwang dialog order. The signature rows
/// (设计/审核/标准化/工艺) take the name to the right of the fixed label; the information rows
/// sit in their own cell beside the fixed label.
pub const TITLE_FIELDS: &[TitleField] = &[
    TitleField { tag: "ENTERPRISE", prompt: "企业名称", x: -58.0, y: 28.0, height: 3.0 },
    TitleField { tag: "TITLE", prompt: "图样名称", x: -58.0, y: 36.0, height: 4.0 },
    TitleField { tag: "DRAWNO", prompt: "图样代号", x: -50.0, y: 6.0, height: 3.0 },
    TitleField { tag: "MATERIAL", prompt: "产品名称或材料标记", x: -50.0, y: 18.0, height: 3.0 },
    TitleField { tag: "WEIGHT", prompt: "重量", x: -20.0, y: 6.0, height: 3.0 },
    TitleField { tag: "DESIGN", prompt: "设计", x: -120.0, y: 42.0, height: 3.0 },
    TitleField { tag: "CHECK", prompt: "审核", x: -120.0, y: 30.0, height: 3.0 },
    TitleField { tag: "STD", prompt: "标准化", x: -120.0, y: 6.0, height: 3.0 },
    TitleField { tag: "PROCESS", prompt: "工艺", x: -120.0, y: 18.0, height: 3.0 },
    TitleField { tag: "SCALE", prompt: "比例", x: -20.0, y: 18.0, height: 3.0 },
];

/// Layer the title-block frame and annotation live on in the source library drawing.
const TITLE_LAYER: &str = "PART_THIN";

/// Append the attribute definitions for the fillable cells to a freshly imported title block.
/// `style` is the text style the imported labels use (falls back to the Standard style).
pub fn inject_defs(d: &mut cadcraft_doc::Drawing, b: &mut cadcraft_doc::Block, style: &str) {
    d.ensure_layer(TITLE_LAYER);
    for f in TITLE_FIELDS {
        let mut common = Common::default();
        common.layer = TITLE_LAYER.into();
        common.color = Color::Index(3);
        let text = Text {
            insert: Vec3::new(f.x, f.y, 0.0),
            align_pt: None,
            height: f.height,
            value: String::new(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: style.to_string(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        };
        let def = Attrib {
            tag: f.tag.into(),
            text,
            invisible: false,
            constant: false,
            prompt: f.prompt.into(),
            props: AttribProps::default(),
            multiline: None,
            modes: Default::default(),
        };
        b.entities.push(Entity { handle: d.new_handle(), common, kind: EntityKind::AttDef(def) });
    }
}

/// Locate the title-block reference in the active space (the first one, in draw order).
fn find_title(s: &Session) -> Result<Handle> {
    let d = s.doc()?;
    let store = d.space(&s.space()).ok_or_else(|| bad("title", "no active space"))?;
    for e in store.iter() {
        if let EntityKind::Insert(i) = &e.kind
            && i.block.eq_ignore_ascii_case(TITLE_BLOCK)
        {
            return Ok(e.handle);
        }
    }
    Err(bad("title", "未找到标题栏，请先在图幅设置中插入标题栏"))
}

/// The value `values` gives for `tag`, matching the tag in any case. Numbers and booleans count
/// as their text.
fn value_for(values: &Map<String, Value>, tag: &str) -> Option<String> {
    let v = values
        .get(tag)
        .or_else(|| values.iter().find(|(k, _)| k.eq_ignore_ascii_case(tag)).map(|(_, v)| v))?;
    match v {
        Value::String(t) => Some(t.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `title.props`: return every fillable field as `{tag, prompt, value}`, plus the reference
/// handle (null when no title block is on the sheet yet). The dialog loads this into its local
/// copy before editing.
fn run_props(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let title = find_title(s).ok();

    let current: HashMap<String, String> = match title.and_then(|h| d.entity(h)) {
        Some(e) => match &e.kind {
            EntityKind::Insert(i) => {
                i.attribs.iter().map(|a| (a.tag.to_ascii_uppercase(), a.text.value.clone())).collect()
            }
            _ => HashMap::new(),
        },
        None => HashMap::new(),
    };

    let items: Vec<Value> = TITLE_FIELDS
        .iter()
        .map(|f| {
            let value = current.get(&f.tag.to_ascii_uppercase()).cloned().unwrap_or_default();
            json!({ "tag": f.tag, "prompt": f.prompt, "value": value })
        })
        .collect();

    Ok(json!({ "handle": title.map(|h| h.hex()), "items": items }))
}

/// Instantiate a reference attribute from its definition, transformed into reference space,
/// mirroring the INSERT instantiation in `blocks::insert_scaled`.
fn attrib_from_def(def: &Attrib, m: &Mat3, value: String) -> Attrib {
    let mut t = def.text.clone();
    let mut kind = EntityKind::Text(t.clone());
    kind.transform(m);
    if let EntityKind::Text(tt) = kind {
        t = tt;
    }
    t.value = value;
    Attrib {
        tag: def.tag.clone(),
        text: t,
        invisible: def.invisible,
        constant: false,
        prompt: String::new(),
        props: Default::default(),
        multiline: def.multiline.as_ref().map(|ml| ml.transformed(m)),
        modes: def.modes.for_reference(),
    }
}

/// `title.set`: write the edited values back onto the title-block reference. Existing attributes
/// are updated; a field the reference is missing (e.g. an older static block) is instantiated
/// from the block definition. `{handle?, values: {TAG: value}}` → `{changed, added, handle}`.
fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let handle = p
        .get("handle")
        .and_then(Value::as_str)
        .and_then(Handle::parse_hex)
        .or_else(|| find_title(s).ok());
    let Some(h) = handle else {
        return Err(bad("title", "未找到标题栏，请先在图幅设置中插入标题栏"));
    };
    let vals = p.get("values").and_then(Value::as_object).cloned().unwrap_or_default();

    // Snapshot the definitions and the block-to-world transform before mutating the reference.
    let d = s.doc()?;
    let defs: Vec<(String, Attrib)> = d
        .block(TITLE_BLOCK)
        .map(|b| {
            b.entities
                .iter()
                .filter_map(|e| match &e.kind {
                    EntityKind::AttDef(ad) => Some((ad.tag.clone(), ad.clone())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let base = d.block(TITLE_BLOCK).map(|b| b.base.xy()).unwrap_or_default();
    let ins = match d.entity(h) {
        Some(e) => match &e.kind {
            EntityKind::Insert(i) => Some(i.clone()),
            _ => None,
        },
        None => None,
    };
    let Some(ins) = ins else {
        return Err(bad("title", "标题栏引用不存在"));
    };
    let m = ins.transform(base);

    let mut changed = 0usize;
    let mut added = 0usize;
    s.doc_mut()?.modify_entity(h, |e| {
        let EntityKind::Insert(i) = &mut e.kind else { return };
        for a in &mut i.attribs {
            if let Some(v) = value_for(&vals, &a.tag)
                && v != a.text.value
            {
                a.text.value = v;
                changed += 1;
            }
        }
        for (tag, def) in &defs {
            if i.attribs.iter().any(|a| a.tag.eq_ignore_ascii_case(tag)) {
                continue;
            }
            let Some(v) = value_for(&vals, tag) else { continue };
            i.attribs.push(attrib_from_def(def, &m, v));
            added += 1;
        }
    })?;

    Ok(json!({ "changed": changed, "added": added, "handle": h.hex() }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("title.props", "Title Props", run_props).params("{}"),
        CommandSpec::new("title.set", "Title Set", run_set)
            .params("{handle?, values: {TAG: value}}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::{Drawing, EntityKind};
    use serde_json::json;

    fn run(s: &mut Session, id: &str, params: Value) -> Value {
        s.execute(id, &params).unwrap()
    }

    /// A title block defined from an empty source carries one AttDef per fillable field.
    #[test]
    fn imported_title_block_carries_attribute_definitions() {
        let mut d = Drawing::new_metric();
        let mut b = cadcraft_doc::Block::new(TITLE_BLOCK);
        inject_defs(&mut d, &mut b, "Standard");
        let defs: Vec<&str> = b
            .entities
            .iter()
            .filter_map(|e| match &e.kind { EntityKind::AttDef(ad) => Some(ad.tag.as_str()), _ => None })
            .collect();
        assert_eq!(defs.len(), TITLE_FIELDS.len());
        for f in TITLE_FIELDS {
            assert!(defs.contains(&f.tag), "missing {}", f.tag);
        }
    }

    /// Props lists every field with an empty value when no title block is inserted; set fills the
    /// reference attributes and props reads them back.
    #[test]
    fn props_reads_back_values_written_by_set() {
        let mut s = Session::new();
        // Build the title block definition with its injected attribute defs.
        {
            let d = s.doc_mut().unwrap();
            let mut b = cadcraft_doc::Block::new(TITLE_BLOCK);
            inject_defs(d, &mut b, "Standard");
            d.blocks.insert(TITLE_BLOCK.into(), std::sync::Arc::new(b));
        }
        // Insert the reference at the origin.
        run(&mut s, "insert", json!({ "name": TITLE_BLOCK, "at": [0.0, 0.0] }));

        let props = run(&mut s, "title.props", json!({}));
        assert!(props["handle"].is_string());
        assert_eq!(props["items"].as_array().unwrap().len(), TITLE_FIELDS.len());

        let set = run(
            &mut s,
            "title.set",
            json!({ "values": { "DESIGN": "gejun", "TITLE": "支架", "SCALE": "1:2" } }),
        );
        assert_eq!(set["changed"], json!(3));

        let props = run(&mut s, "title.props", json!({}));
        let items = props["items"].as_array().unwrap();
        let get = |tag: &str| items.iter().find(|i| i["tag"] == json!(tag)).unwrap()["value"].as_str().unwrap().to_string();
        assert_eq!(get("DESIGN"), "gejun");
        assert_eq!(get("TITLE"), "支架");
        assert_eq!(get("SCALE"), "1:2");
        assert_eq!(get("MATERIAL"), "");
    }
}
