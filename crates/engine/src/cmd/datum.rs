//! Datum feature symbol (国标基准代号): a thin-line circle holding a capital datum letter, a
//! thin stem to the datum feature, and a short thick bar on the feature. Mirrors the Zhongwang
//! datum-symbol command; geometry is drawn as plain entities on its own layer so it round-trips
//! through the DXF loop without a dedicated entity kind.
//!
//! Placement is two-point: `attach` is the datum feature point (the thick bar's midpoint), `tip`
//! is the circle centre. The symbol axis follows the attach→tip direction; the thick bar runs
//! perpendicular to it, so a horizontal feature gets a horizontal bar and a vertical feature a
//! vertical one. The letter stays upright regardless of the axis.

use cadcraft_doc::{Common, Drawing, EntityKind, Handle, HAlign, Lineweight, Space, Text, VAlign};
use cadcraft_geom::Vec2;
use serde_json::{Value, json};

use super::helpers::{circle, line, v3};
use super::{CommandSpec, bad, f64_or, point_req, str_param};
use crate::{Accept, Interactive, Input, Prompt, Result, Session, Step};

/// Layer holding datum feature symbols.
const DATUM_LAYER: &str = "DATUM";
/// Circle radius (mm); 8 mm diameter, matching the parts-list balloon size.
const DATUM_RADIUS: f64 = 4.0;
/// Datum letter height (mm), matching the annotation text height.
const DATUM_TEXT_H: f64 = 3.5;
/// Thick bar length (mm), ≈ the circle diameter.
const DATUM_BAR_LEN: f64 = 8.0;
/// Thin frame / stem / letter colour: green (ACI 3), matching the other annotation text.
const DATUM_LINE_COLOR: cadcraft_color::Color = cadcraft_color::Color::Index(3);
/// Thick datum bar colour: cyan (ACI 4), as in the reference drawing.
const DATUM_BAR_COLOR: cadcraft_color::Color = cadcraft_color::Color::Index(4);
/// Fine line weight (0.18 mm) for the circle, stem and letter.
const FINE_LW: Lineweight = Lineweight::Mm100(18);
/// Thick line weight (0.50 mm) for the datum feature bar.
const THICK_LW: Lineweight = Lineweight::Mm100(50);
/// Datum letters reserved by GB: I, O, Q are skipped when auto-incrementing (easily mistaken
/// for 1, 0 or other marks). A manually typed letter is still accepted.
const FORBIDDEN: &[char] = &['I', 'O', 'Q'];

fn line_common() -> Common {
    Common { layer: DATUM_LAYER.into(), color: DATUM_LINE_COLOR, linetype: "ByLayer".into(), lineweight: FINE_LW, ..Common::default() }
}

fn bar_common() -> Common {
    Common { layer: DATUM_LAYER.into(), color: DATUM_BAR_COLOR, linetype: "ByLayer".into(), lineweight: THICK_LW, ..Common::default() }
}

fn text_common() -> Common {
    Common { layer: DATUM_LAYER.into(), color: DATUM_LINE_COLOR, linetype: "ByLayer".into(), lineweight: FINE_LW, ..Common::default() }
}

fn add(s: &mut Session, space: &Space, common: Common, kind: EntityKind) -> Result<Handle> {
    Ok(s.doc_mut()?.add(space, common, kind)?)
}

/// Normalise a typed datum letter: first character, upper-cased; must be an ASCII letter.
fn normalize_letter(raw: &str) -> Result<String> {
    let c = raw.trim().chars().next().ok_or_else(|| bad("datum", "empty datum letter"))?;
    let up = c.to_ascii_uppercase();
    if !up.is_ascii_alphabetic() {
        return Err(bad("datum", format!("`{raw}` is not a valid datum letter")));
    }
    Ok(up.to_string())
}

/// Next free datum letter on the current space: first unused A–Z, skipping I/O/Q.
fn next_letter(s: &Session) -> String {
    let used: std::collections::HashSet<char> = s
        .doc()
        .ok()
        .and_then(|d: &Drawing| d.space(&s.space()))
        .map(|store| {
            store
                .iter()
                .filter(|e| e.common.layer == DATUM_LAYER)
                .filter_map(|e| match &e.kind {
                    EntityKind::Text(t) => t.value.trim().chars().next().map(|c| c.to_ascii_uppercase()),
                    _ => None,
                })
                .filter(|c| c.is_ascii_alphabetic())
                .collect()
        })
        .unwrap_or_default();
    for c in 'A'..='Z' {
        if !FORBIDDEN.contains(&c) && !used.contains(&c) {
            return c.to_string();
        }
    }
    "Z".to_string()
}

/// Draw one datum symbol and return its handles.
fn draw_datum(s: &mut Session, letter: String, attach: Vec2, tip: Vec2, radius: f64, height: f64) -> Result<Value> {
    let r = radius.abs();
    if height <= 0.0 {
        return Err(bad("datum", "height must be positive"));
    }
    let letter = normalize_letter(&letter)?;

    let space = s.space();
    let _ = s.doc_mut().map(|d| d.ensure_layer(DATUM_LAYER));

    // Symbol axis from the feature point to the circle centre; default straight up when the two
    // picks coincide. The bar runs along the perpendicular.
    let u = {
        let a = (tip - attach).normalized();
        if a.near(Vec2::ZERO, 1e-9) { Vec2::new(0.0, 1.0) } else { a }
    };
    let perp = u.perp();

    let circle_h = add(s, &space, line_common(), circle(tip, r))?;

    // Stem from the circle edge facing the feature, not into the circle centre.
    let edge = tip - u * r;
    let stem_h = if edge.near(attach, 1e-9) {
        None
    } else {
        Some(add(s, &space, line_common(), line(edge, attach))?)
    };

    // Thick datum bar centred on the attach point, perpendicular to the axis.
    let half = DATUM_BAR_LEN / 2.0;
    let bar_h = add(s, &space, bar_common(), line(attach - perp * half, attach + perp * half))?;

    let style = s.doc().map(|d| d.header.str("TEXTSTYLE", "Standard")).unwrap_or_else(|_| "Standard".into());
    let text_kind = EntityKind::Text(Text {
        insert: v3(tip),
        align_pt: Some(v3(tip)),
        height,
        value: letter.clone(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style,
        halign: HAlign::Center,
        valign: VAlign::Middle,
    });
    let text_h = add(s, &space, text_common(), text_kind)?;

    Ok(json!({
        "letter": letter,
        "circle": circle_h.hex(),
        "stem": stem_h.map(|h| h.hex()),
        "bar": bar_h.hex(),
        "text": text_h.hex(),
    }))
}

/// `datum`: programmatic entry. `{letter?, attach: [x,y], tip: [x,y], radius?, height?}`.
fn run_datum(s: &mut Session, p: &Value) -> Result<Value> {
    let attach = point_req("datum", p, "attach")?;
    let tip = point_req("datum", p, "tip")?;
    let letter = str_param(p, "letter").map(str::to_string).unwrap_or_else(|| next_letter(s));
    let r = f64_or(p, "radius", DATUM_RADIUS);
    let th = f64_or(p, "height", DATUM_TEXT_H);
    draw_datum(s, letter, attach, tip, r, th)
}

/// Interactive datum symbol: type (or accept) the letter, pick the feature point, then the circle
/// location.
#[derive(Default)]
struct DatumM {
    /// 0 = letter, 1 = feature point, 2 = circle location.
    stage: u8,
    letter: String,
    attach: Option<Vec2>,
}

impl DatumM {
    fn accept_letter(&mut self, s: &mut Session, raw: &str) -> Result<Step> {
        match normalize_letter(raw) {
            Ok(l) => {
                self.letter = l;
                self.stage = 1;
                Ok(Step::Continue)
            }
            Err(e) => {
                s.echo(e.to_string());
                Ok(Step::Continue)
            }
        }
    }
}

impl Interactive for DatumM {
    fn name(&self) -> &'static str {
        "DATUM"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.letter = next_letter(s);
        self.attach = None;
        self.stage = 0;
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            0 => Prompt::new("Enter datum letter", Accept::TEXT).default(self.letter.clone()),
            1 => Prompt::new("Specify datum attachment point", Accept::POINT),
            _ => Prompt::new("Specify datum symbol location", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match self.stage {
            0 => match i {
                Input::Enter => {
                    self.stage = 1;
                    Ok(Step::Continue)
                }
                Input::Text(t) | Input::Keyword(t) => self.accept_letter(s, &t),
                Input::Cancel => Ok(Step::Cancel),
                _ => Ok(Step::Continue),
            },
            1 => match i {
                Input::Point(p) => {
                    self.attach = Some(p);
                    self.stage = 2;
                    Ok(Step::Continue)
                }
                Input::Cancel => Ok(Step::Cancel),
                _ => Ok(Step::Continue),
            },
            _ => match i {
                Input::Point(tip) => {
                    let attach = self.attach.unwrap_or(Vec2::ZERO);
                    draw_datum(s, self.letter.clone(), attach, tip, DATUM_RADIUS, DATUM_TEXT_H)?;
                    Ok(Step::Done)
                }
                Input::Cancel => Ok(Step::Cancel),
                _ => Ok(Step::Continue),
            },
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        if self.stage == 2 {
            if let Some(a) = self.attach {
                return vec![line(a, cursor)];
            }
        }
        Vec::new()
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("datum", "Datum", run_datum)
            .params("{letter?, attach: [x,y], tip: [x,y], radius?, height?}")
            .menu(&["Dimension", "Datum"])
            .alias(&["datum"])
            .interactive(|_| Ok(Box::new(DatumM::default()))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::EntityKind as E;
    use serde_json::json;

    fn run(s: &mut Session, id: &str, params: Value) -> Value {
        s.execute(id, &params).unwrap()
    }

    #[test]
    fn datum_draws_circle_stem_bar_and_letter() {
        let mut s = Session::new();
        let v = run(&mut s, "datum", json!({"letter": "A", "attach": [0.0, 0.0], "tip": [0.0, 15.0]}));
        assert_eq!(v["letter"], "A");

        let d = s.doc().unwrap();
        assert_eq!(d.model.len(), 4, "circle + stem + bar + letter");
        assert!(d.model.iter().all(|e| e.common.layer == DATUM_LAYER));

        let mut circle = None;
        let mut bar = None;
        let mut stem = None;
        let mut text = None;
        for e in d.model.iter() {
            match &e.kind {
                E::Circle(c) => circle = Some((c.center.xy(), c.radius, e.common.color)),
                E::Line(l) => {
                    let (a, b) = (l.a.xy(), l.b.xy());
                    if e.common.lineweight == THICK_LW { bar = Some((a, b, e.common.color)); } else { stem = Some((a, b, e.common.color)); }
                }
                E::Text(t) => text = Some((t.value.clone(), t.height, t.halign, t.valign, e.common.color)),
                _ => {}
            }
        }

        let (center, r, cc) = circle.unwrap();
        assert_eq!(center, Vec2::new(0.0, 15.0));
        assert_eq!(r, DATUM_RADIUS);
        assert_eq!(cc, DATUM_LINE_COLOR);

        // Vertical axis: the thick bar is horizontal, centred on the attach point.
        let (ba, bb, bc) = bar.unwrap();
        assert!((ba.y - 0.0).abs() < 1e-9 && (bb.y - 0.0).abs() < 1e-9, "bar horizontal");
        let (xmin, xmax) = (ba.x.min(bb.x), ba.x.max(bb.x));
        assert!((xmin + DATUM_BAR_LEN / 2.0).abs() < 1e-9 && (xmax - DATUM_BAR_LEN / 2.0).abs() < 1e-9);
        assert_eq!(bc, DATUM_BAR_COLOR);

        // Stem runs from the circle bottom edge to the attach point.
        let (sa, sb, sc) = stem.unwrap();
        let edge = Vec2::new(0.0, 15.0 - DATUM_RADIUS);
        assert!(sa.near(edge, 1e-9) && sb.near(Vec2::ZERO, 1e-9) || sb.near(edge, 1e-9) && sa.near(Vec2::ZERO, 1e-9));
        assert_eq!(sc, DATUM_LINE_COLOR);

        let (val, h, hal, val_, tc) = text.unwrap();
        assert_eq!(val, "A");
        assert_eq!(h, DATUM_TEXT_H);
        assert_eq!(hal, HAlign::Center);
        assert_eq!(val_, VAlign::Middle);
        assert_eq!(tc, DATUM_LINE_COLOR);
    }

    #[test]
    fn datum_auto_increments_and_skips_reserved_letters() {
        let mut s = Session::new();
        let a = run(&mut s, "datum", json!({"attach": [0.0, 0.0], "tip": [0.0, 15.0]}));
        assert_eq!(a["letter"], "A");
        let b = run(&mut s, "datum", json!({"attach": [20.0, 0.0], "tip": [20.0, 15.0]}));
        assert_eq!(b["letter"], "B");
    }

    #[test]
    fn datum_rejects_non_letter() {
        let mut s = Session::new();
        let r = s.execute("datum", &json!({"letter": "1", "attach": [0.0, 0.0], "tip": [0.0, 15.0]}));
        assert!(r.is_err());
    }

    #[test]
    fn datum_sideways_axis_rotates_the_bar() {
        let mut s = Session::new();
        // Feature on the left, circle to the right: the bar must be vertical.
        run(&mut s, "datum", json!({"letter": "B", "attach": [0.0, 0.0], "tip": [15.0, 0.0]}));
        let d = s.doc().unwrap();
        let bar = d
            .model
            .iter()
            .find(|e| matches!(&e.kind, E::Line(_)) && e.common.lineweight == THICK_LW)
            .unwrap();
        let E::Line(l) = &bar.kind else { unreachable!() };
        let (a, b) = (l.a.xy(), l.b.xy());
        assert!((a.x - 0.0).abs() < 1e-9 && (b.x - 0.0).abs() < 1e-9, "bar vertical");
    }
}
