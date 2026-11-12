//! `TextLayer` → a text element laid out with this computer's fonts (ADR 0014), as the XFL
//! importer does for Animate fields.
//!
//! Moho regenerates a text layer's geometry from its text, font and size at render time (the
//! saved `mesh_layer` is a derived view), so re-laying the text out here matches its intent.
//! Units: `textsize` is points at Moho's 72 pt/inch → pixels at the document's dpi. The
//! justification code maps 1/2/3 → left/centre/right (Moho's default 1 is left-aligned
//! text). The text stroke imports (ADR 0105): `stroke`, `linecolor` and `linewidth` (an
//! inch distance, like a shape's) become the block's stroke in pixels. Kerning, leading,
//! inherited styles and speech balloons have no slot in a text block yet and are counted
//! when non-default. The embedded `mesh_layer` — Moho's derived geometry of the text —
//! imports as real, bone-bindable shapes only when bones need it: a text riding a rig
//! (its `parent_bone` or point bindings set) draws as that geometry, because a text block
//! cannot follow bones; a plain text layer keeps re-laid, editable text and ignores the
//! derived mesh, exactly as Moho regenerates it at render time.

use crate::import::Builder;
use crate::json::{self, array, get};
use anicraft_doc::{ColorEffect, Element, ElementId, ElementKind, Rgba, TextAlign, TextBlock};
use anicraft_geom::{Affine, Vec2};
use anicraft_text::{Align, FaceMatch, Fonts, TextError};
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn text_elements(b: &mut Builder, layer: &Value, world: Affine, out: &mut Vec<Arc<Element>>) {
    let Some(txt) = json::string(get(layer, "text")).filter(|t| !t.trim().is_empty()) else { return };
    // A text riding a rig (its whole layer bound, or its saved points bound) draws as the
    // saved `mesh_layer` geometry: real shapes the bones can carry (ADR 0105). A plain text
    // layer keeps the re-laid text block — Moho regenerates that mesh from the text anyway.
    if let Some(ml) = get(layer, "mesh_layer")
        && is_bound(layer, ml)
        && has_content(get(ml, "mesh").unwrap_or(&Value::Null))
    {
        crate::mesh::mesh_elements(b, get(ml, "mesh").unwrap_or(&Value::Null), world, out);
        b.report.notes.push(format!("text \"{txt}\" drawn as its saved mesh geometry (it rides bones)"));
        return;
    }
    if !json::boolean(get(layer, "fill")).unwrap_or(true) {
        b.report.skip("text drawn as stroke only");
        return;
    }
    extras(b, layer);
    let size_pt = json::num(get(layer, "textsize")).unwrap_or(60.0);
    let font = json::string(get(layer, "font")).unwrap_or_default();
    let color = json::color255(get(layer, "fillcolor")).unwrap_or(Rgba::BLACK);
    let align = match json::integer(get(layer, "justification")).unwrap_or(1) {
        2 => TextAlign::Center,
        3 => TextAlign::Right,
        _ => TextAlign::Left,
    };
    // Points at Moho's 72 pt/inch, shown at the document's pixels per inch.
    let size = ((size_pt / 72.0) * b.dpi as f64).clamp(1.0, 2000.0) as f32;
    let fonts: &Fonts = match b.fonts {
        Some(f) => f,
        None => Fonts::get(),
    };
    b.fonts = Some(fonts);
    let face: FaceMatch = b.faces.entry(font.clone()).or_insert_with(|| fonts.find_face(&font)).clone();
    let style = anicraft_text::Style {
        family: face.family.clone().unwrap_or_default(),
        size,
        bold: face.bold,
        italic: face.italic,
        align: match align {
            TextAlign::Left => Align::Left,
            TextAlign::Center => Align::Center,
            TextAlign::Right => Align::Right,
        },
        letter_spacing: 0.0,
        line_spacing: 0.0,
        width: None,
    };
    let layout = match fonts.layout(&txt, &style) {
        Ok(l) => l,
        Err(TextError::NoFonts) => {
            b.report.skip("text (no fonts on this computer)");
            return;
        }
        Err(TextError::TooLong) => {
            b.report.skip("text cut to 100,000 characters");
            return;
        }
    };
    // A family that isn't installed falls back to the default, as the text engine does
    // (ADR 0012), and the report names it.
    if face.family.is_none() && !face.wanted.is_empty() {
        b.report.missing_fonts.insert(face.wanted.clone());
        b.report.fallback_font.get_or_insert_with(|| layout.family.clone());
    }
    let block = TextBlock {
        text: txt,
        font: layout.family,
        size: style.size,
        bold: style.bold,
        italic: style.italic,
        color,
        align,
        letter_spacing: 0.0,
        line_spacing: 0.0,
        width: None,
        outline: Arc::new(layout.outline),
        field: None, stroke: None,
        box_width: layout.width,
        box_height: layout.height,
    };
    // The block's top-left sits at the layer origin, carried by the px-space form of the
    // layer's world transform (so a rotated parent layer rotates the text with it).
    let at = b.to_px.apply(world.apply(Vec2::ZERO));
    let linear = b.to_px.mul(&world).mul(&b.from_px);
    let transform = Affine { m: [linear.m[0], linear.m[1], linear.m[2], linear.m[3], at.x, at.y] };
    out.push(Arc::new(Element {
        id: ElementId(b.doc.alloc_id()),
        name: String::new(),
        transform,
        color: ColorEffect::default(),
        kind: ElementKind::Text(Arc::new(block)),
    }));
}

/// Whether the text layer rides bones: a whole-layer binding or any bound saved point.
fn is_bound(layer: &Value, mesh_layer: &Value) -> bool {
    if json::integer(get(layer, "parent_bone")).is_some_and(|b| b >= 0) {
        return true;
    }
    let Some(mesh) = get(mesh_layer, "mesh") else { return false };
    array(get(mesh, "points")).is_some_and(|pts| pts.iter().any(|p| json::integer(get(p, "parent")).is_some_and(|b| b >= 0)))
}

/// Whether the saved mesh would draw anything at all.
fn has_content(mesh: &Value) -> bool {
    array(get(mesh, "points")).is_some_and(|p| !p.is_empty()) && array(get(mesh, "shapes")).is_some_and(|s| !s.is_empty())
}

/// Text features without a slot yet, counted when non-default.
fn extras(b: &mut Builder, layer: &Value) {
    if json::integer(get(layer, "kerning")).is_some_and(|k| k != 0) {
        b.report.skip("text kerning (drawn as 0)");
    }
    if json::integer(get(layer, "leading")).is_some_and(|l| l != 0) {
        b.report.skip("text leading (drawn as 0)");
    }
    if !json::string(get(layer, "textinheritedstyle1")).is_none_or(|s| s.is_empty())
        || !json::string(get(layer, "textinheritedstyle2")).is_none_or(|s| s.is_empty())
    {
        b.report.skip("inherited text styles");
    }
    if !json::string(get(layer, "balloonstyle")).unwrap_or_default().is_empty() {
        b.report.skip("speech balloons");
    }
}
