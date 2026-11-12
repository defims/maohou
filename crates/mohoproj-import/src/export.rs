//! The reverse of the import (stage 7, ADR 0107): the document model back to a Moho package —
//! `Project.mohoproj` beside `preview.jpg` inside a `.moho` zip, or a bare `.mohoproj`.
//!
//! Two paths, by the document's origin:
//!
//! * **A document imported from Moho** carries the untouched source JSON in its
//!   `maop_maohou` sidecar. The export pairs the raw layers with the model's (the import's
//!   flatten order is deterministic, so pairing is positional), keeps every key the model
//!   doesn't hold — the original mesh with its true handles, unknown keys, textures — and
//!   overwrites the mapped ones with the model's current state (name, visibility, transform
//!   channels, switch selection, skeleton, physics body, text). The result re-imports to the
//!   same model.
//! * **A native document** generates the canonical template (the corpus's default layer,
//!   transforms and styles) and draws the model into it: shapes become mesh points and curves
//!   in Moho inches, with handles written in the same form the importer reconstructs them
//!   from, so the exported file draws exactly what the model draws.
//!
//! What the model holds but this writer doesn't put in the file is counted in the report;
//! nothing falls out silently.

use crate::MohoError;
use anicraft_doc::{Document, Element, ElementKind, Interp, Key, Layer, LayerKind, Paint, Rgba, Scene, Shape};
use anicraft_geom::{PathCmd, Vec2};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The canonical defaults the corpus's fresh minimal document carries: project fields, the
/// default layer, the named styles, the editor state. Extracted once from `empty.moho`
/// (Moho 14.4's own save) and kept as data.
const TEMPLATE: &str = include_str!("export_template.json");

/// Moho's project scale (see the import side).
const DPI: f32 = crate::import::MOHO_DPI;

/// One path segment drawn into the mesh: its endpoints and its Bézier handles when it curves.
type Segment = (Vec2, Option<(Vec2, Vec2)>, Vec2);

/// What the export couldn't put in the file, plus the files it wrote beside it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExportReport {
    pub skipped: BTreeMap<String, usize>,
    pub notes: Vec<String>,
    /// Media written beside the package (images, sounds), as file names.
    pub files: Vec<String>,
}

impl ExportReport {
    pub(crate) fn skip(&mut self, what: &str) {
        *self.skipped.entry(what.to_string()).or_default() += 1;
    }
    /// One line for the status bar, in the shape of the import report's.
    pub fn summary(&self) -> String {
        if self.skipped.is_empty() && self.notes.is_empty() {
            return "Exported everything in the document.".into();
        }
        let mut out = String::from("Exported.");
        let parts: Vec<String> = self.skipped.iter().map(|(k, n)| format!("{n} {k}")).collect();
        if !parts.is_empty() {
            out.push_str(&format!(" Not carried: {}.", parts.join(", ")));
        }
        if !self.notes.is_empty() {
            out.push_str(&format!(" {}", self.notes.join(" ")));
        }
        out
    }
}

/// A document point (stage pixels, y down) in Moho inches (y up, origin centre).
fn to_inch(doc: &Document, p: Vec2) -> (f64, f64) {
    let half_w = doc.stage.width as f32 * 0.5;
    let half_h = doc.stage.height as f32 * 0.5;
    ((p.x - half_w) as f64 / f64::from(DPI), (half_h - p.y) as f64 / f64::from(DPI))
}

/// A colour as Moho's project int form.
fn color255(c: Rgba) -> Value {
    let q = |v: f32| ((v * 255.0).round().clamp(0.0, 255.0)) as i64;
    json!({"r": q(c.r), "g": q(c.g), "b": q(c.b), "a": q(c.a)})
}
/// A colour as Moho's style float form.
fn color01(c: Rgba) -> Value {
    json!({"r": c.r, "g": c.g, "b": c.b, "a": c.a})
}

/// The static single-key interp of the canonical template (what Moho writes on every
/// un-animated channel).
fn static_interp() -> Value {
    json!([{"im": 1, "v1": 0.1, "v2": 0.5, "in": 1, "h": 0, "s": false, "t": 0}])
}

/// The channel object a template layer carries: `{type, ref, mute, when, val, interp}`.
fn static_channel(kind: &str, val: Value) -> Value {
    json!({"type": kind, "ref": false, "mute": false, "when": [0], "val": [val], "interp": static_interp()})
}

/// Writes a model channel into a Moho channel object. `kind` is the val type ("Val", "Vec2",
/// "Vec3", "Bool", "Color", "String"); `val` converts one model value; `default` is the
/// un-animated form (the template's). A channel without keys writes the default.
fn channel<T: Clone>(kind: &str, ch: &anicraft_doc::Channel<T>, default: Value, val: impl Fn(&T) -> Value, components: usize) -> Value {
    let keys: &[Key<T>] = &ch.keys;
    if keys.is_empty() {
        return static_channel(kind, default);
    }
    let when: Vec<Value> = keys.iter().map(|k| json!(k.frame)).collect();
    let val_out: Vec<Value> = keys.iter().map(|k| val(&k.val)).collect();
    let interp: Vec<Value> = keys
        .iter()
        .map(|k| {
            let mut entry = match &k.out {
                Interp::Linear => json!({"im": 0}),
                Interp::Smooth => json!({"im": 1}),
                Interp::Hold => json!({"im": 3}),
                Interp::Bezier(handles) => {
                    // Moho's per-component Bézier handles: angle and fraction of the span, out
                    // and in, as the importer reads them back.
                    let b: Vec<Value> = (0..components.max(1))
                        .map(|c| {
                            let h = handles.get(c).or(handles.first());
                            let (ao, po) = h.map_or((0.0, 1.0 / 3.0), |h| (h.angle as f64, h.percent as f64));
                            json!({"ao": ao, "ai": ao, "po": po, "pi": po})
                        })
                        .collect();
                    json!({"im": 9, "b": b})
                }
            };
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("v1".into(), json!(0.0));
                obj.insert("v2".into(), json!(0.5));
                obj.insert("in".into(), json!(k.interval.max(1)));
                obj.insert("h".into(), json!(k.hold));
                obj.insert("s".into(), json!(false));
                obj.insert("t".into(), json!(0));
            }
            entry
        })
        .collect();
    json!({"type": kind, "ref": false, "mute": false, "when": when, "val": val_out, "interp": interp})
}

/// The template JSON, parsed once (small: about 40 KB).
fn template() -> &'static Value {
    use std::sync::OnceLock;
    static TEMPLATE_PARSED: OnceLock<Value> = OnceLock::new();
    TEMPLATE_PARSED.get_or_init(|| serde_json::from_str(TEMPLATE).unwrap_or(Value::Null))
}

/// A stable UUID for the document: a UUID-shaped hash of its layers' ids and names, so the
/// same document exports to the same `doc_uuid` (deterministic bytes, as everywhere).
fn doc_uuid(doc: &Document) -> String {
    let mut seed: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            seed ^= u64::from(*b);
            seed = seed.wrapping_mul(0x100_0000_01b3);
        }
    };
    for s in &doc.scenes {
        mix(s.name.as_bytes());
        for l in &s.timeline.layers {
            mix(l.id.0.to_le_bytes().as_slice());
            mix(l.name.as_bytes());
        }
    }
    let hex = format!("{seed:016X}{:016X}", seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    format!("{}-{}-4{}-8{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]).to_uppercase()
}

/// The whole `Project.mohoproj` JSON for `doc`, plus the report.
pub fn project_json(doc: &Document) -> (Value, ExportReport) {
    let t = template();
    let mut report = ExportReport::default();
    let scene = doc.scenes.first().cloned().unwrap_or_else(|| Arc::new(Scene::new("Scene 1")));

    let mut root = Map::new();
    root.insert("version".into(), t.get("version").cloned().unwrap_or(json!(1045)));
    root.insert("mime_type".into(), t.get("mime_type").cloned().unwrap_or_else(|| json!("application/x-vnd.lm_mohodoc")));
    root.insert("major_version".into(), t.get("major_version").cloned().unwrap_or(json!(1)));
    root.insert("rev_version".into(), t.get("rev_version").cloned().unwrap_or(json!(0)));
    root.insert("doc_uuid".into(), json!(doc_uuid(doc)));
    root.insert("created_date".into(), t.get("created_date").cloned().unwrap_or_else(|| json!("")));
    root.insert("modified_date".into(), t.get("modified_date").cloned().unwrap_or_else(|| json!("")));
    root.insert("metadata".into(), t.get("metadata").cloned().unwrap_or_else(|| json!({})));
    root.insert("comment".into(), t.get("comment").cloned().unwrap_or_else(|| json!("")));
    if let Some(Value::Object(onions)) = t.get("onions") {
        for (k, v) in onions {
            root.insert(k.clone(), v.clone());
        }
    }
    root.insert("layercomps".into(), t.get("layercomps").cloned().unwrap_or_else(|| json!([])));
    root.insert("action_refs".into(), t.get("action_refs").cloned().unwrap_or_else(|| json!([])));
    root.insert("styles".into(), t.get("styles").cloned().unwrap_or_else(|| json!([])));
    root.insert("documentviewstate".into(), t.get("documentviewstate").cloned().unwrap_or_else(|| json!({})));
    root.insert("project_data".into(), project_data(doc, &scene));
    root.insert("animated_values".into(), animated_values(doc, &scene));
    root.insert("thumbnail".into(), json!(""));
    root.insert("layers".into(), layers(doc, &scene, &mut report));
    (Value::Object(root), report)
}

/// `project_data`: the template's defaults, overridden by the document's stage and range.
fn project_data(doc: &Document, scene: &Scene) -> Value {
    let mut pd = template().get("project_data").and_then(Value::as_object).cloned().unwrap_or_default();
    pd.insert("width".into(), json!(doc.stage.width));
    pd.insert("height".into(), json!(doc.stage.height));
    pd.insert("fps".into(), json!(doc.stage.fps));
    pd.insert("back_color".into(), color255(doc.stage.background));
    let (start, end) = scene.frames.unwrap_or((0, scene.timeline.frame_count()));
    pd.insert("start_frame".into(), json!(start + 1));
    pd.insert("end_frame".into(), json!(end.max(start + 1)));
    Value::Object(pd)
}

/// `animated_values`: the camera and the timeline markers, the scene's own channels.
fn animated_values(doc: &Document, scene: &Scene) -> Value {
    let half_w = doc.stage.width as f32 * 0.5;
    let half_h = doc.stage.height as f32 * 0.5;
    let point = |p: Vec2| {
        let (x, y) = to_inch(doc, p);
        json!({"x": x, "y": y, "z": crate::anim::CAMERA_Z_DEFAULT})
    };
    let camera = &scene.camera;
    let mut out = json!({
        "camera_track": channel("Vec3", &camera.track, point(Vec2::new(half_w, half_h)), |v| point(*v), 2),
        "camera_zoom": channel("Val", &camera.zoom, json!(2.0), |v| json!(v), 1),
        "camera_roll": channel("Val", &camera.roll, json!(0.0), |v| json!(-f64::from(*v)), 1),
        "camera_pan_tilt": static_channel("Vec2", json!({"x": 0.0, "y": 0.0})),
        "timeline_markers": static_channel("String", json!("")),
    });
    if !scene.markers.is_empty() {
        let when: Vec<Value> = scene.markers.iter().map(|(f, _)| json!(f)).collect();
        let val: Vec<Value> = scene.markers.iter().map(|(_, s)| json!(s)).collect();
        let interp: Vec<Value> = scene.markers.iter().map(|_| json!({"im": 3, "v1": 0.0, "v2": 0.5, "in": 1, "h": 0, "s": false, "t": 0})).collect();
        out["timeline_markers"] = json!({"type": "String", "ref": false, "mute": false, "when": when, "val": val, "interp": interp});
    }
    out
}

/// The inline layer tree: the model's flat list with parent links, back into Moho's
/// bottom-first nesting (a container before the children it flattens, siblings in reverse).
fn layers(doc: &Document, scene: &Scene, report: &mut ExportReport) -> Value {
    let order = &scene.timeline.layers;
    let mut children: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (i, l) in order.iter().enumerate() {
        match l.parent {
            Some(p) => children.entry(p.0).or_default().push(i),
            None => roots.push(i),
        }
    }
    // The raw sidecar layers, in the import's pairing order.
    let mut raw: Vec<Value> = Vec::new();
    if let Some(sidecar) = &doc.maop_maohou {
        enumerate_sidecar(sidecar.raw.get("layers"), &mut raw);
    }
    let mut out: Vec<Value> = Vec::new();
    for i in roots.into_iter().rev() {
        out.push(write_layer(doc, order, &children, &raw, i, report));
    }
    Value::Array(out)
}

/// One layer node: the paired raw layer with the model's state over it, or a generated one.
fn write_layer(doc: &Document, order: &[Arc<Layer>], children: &BTreeMap<u64, Vec<usize>>, raw: &[Value], index: usize, report: &mut ExportReport) -> Value {
    let l = &order[index];
    let mut node = match raw.get(index).and_then(Value::as_object) {
        Some(r) => {
            let mut node = r.clone();
            merge_layer(doc, l, &mut node, report);
            node
        }
        None => generate_layer(doc, l, report),
    };
    let kids = children.get(&l.id.0).cloned().unwrap_or_default();
    if !kids.is_empty() {
        let mut list: Vec<Value> = Vec::new();
        for k in kids.into_iter().rev() {
            list.push(write_layer(doc, order, children, raw, k, report));
        }
        node.insert("layers".into(), Value::Array(list));
    }
    Value::Object(node)
}

/// The sidecar's raw layers, in the import's flatten order: siblings visited back-to-front,
/// a container before the children it flattens.
fn enumerate_sidecar(layers: Option<&Value>, out: &mut Vec<Value>) {
    let Some(list) = crate::json::array(layers) else { return };
    for layer in list.iter().rev() {
        out.push(layer.clone());
        if let Some(kids) = layer.get("layers") {
            enumerate_sidecar(Some(kids), out);
        }
    }
}

/// Overwrites the mapped fields of one raw layer with the model's current state.
fn merge_layer(doc: &Document, l: &Layer, node: &mut Map<String, Value>, _report: &mut ExportReport) {
    node.insert("name".into(), json!(l.name));
    node.insert("visible".into(), json!(l.visible));
    // The transform channels: written when the model carries them (an animated leaf or a
    // compositing container); a static-baked leaf keeps the file's baked transform.
    let animated = !l.translation.is_empty() || !l.scale.is_empty() || !l.rotation.is_empty() || !l.shear.is_empty();
    if animated {
        let tr = node.entry("transforms".to_string()).or_insert_with(|| json!({}));
        if let Some(obj) = tr.as_object_mut() {
            write_transforms(&transforms_of(l), obj);
        }
        let (x, y) = to_inch(doc, l.origin);
        node.insert("origin".into(), json!({"x": x, "y": y}));
    }
    if !l.opacity.is_empty() {
        let fx = node.entry("layer_effects".to_string()).or_insert_with(|| json!({}));
        if let Some(obj) = fx.as_object_mut() {
            obj.insert("alpha".into(), channel("Val", &l.opacity, json!(1.0), |v| json!(v), 1));
        }
    }
    if l.kind == LayerKind::Switch {
        node.insert("switch_keys".into(), channel("String", &l.switch_to, json!(""), |s| json!(s), 0));
    }
    if let Some(skeleton) = &l.skeleton {
        node.insert("skeleton".into(), write_skeleton(doc, skeleton));
    }
    if let Some(body) = &l.physics {
        let ph = node.entry("physics".to_string()).or_insert_with(|| json!({}));
        if let Some(obj) = ph.as_object_mut() {
            obj.insert("enabled".into(), json!(true));
            obj.insert("static".into(), json!(body.static_body));
            obj.insert("friction".into(), json!(body.friction));
            obj.insert("restitution".into(), json!(body.restitution));
            obj.insert("velocity".into(), json!({"x": body.velocity.x / DPI, "y": -body.velocity.y / DPI}));
        }
    }
    if let Some(block) = l.keyframes.iter().flat_map(|k| k.elements.iter()).find_map(|e| match &e.kind {
        ElementKind::Text(b) => Some(b),
        _ => None,
    }) {
        write_text_into(block, node);
    }
}

/// The layer's channels, as the transform writer takes them.
fn transforms_of(l: &Layer) -> crate::anim::LayerTransform {
    crate::anim::LayerTransform {
        translation: l.translation.clone(),
        scale: l.scale.clone(),
        rotation: l.rotation.clone(),
        shear: l.shear.clone(),
        flip_h: l.flip_h.clone(),
        flip_v: l.flip_v.clone(),
    }
}

/// The text fields the model holds, over a layer node's.
fn write_text_into(block: &anicraft_doc::TextBlock, node: &mut Map<String, Value>) {
    node.insert("type".into(), json!("TextLayer"));
    node.insert("text".into(), json!(block.text));
    // Moho stores the size in points; the model keeps pixels (dpi/72 per point).
    node.insert("textsize".into(), json!(block.size * 72.0 / DPI));
    node.insert("fillcolor".into(), color255(block.color));
    node.insert("fill".into(), json!(block.stroke.is_none()));
    node.insert("stroke".into(), json!(block.stroke.is_some()));
    if let Some(stroke) = &block.stroke {
        node.insert("linecolor".into(), color255(stroke.color));
        node.insert("linewidth".into(), json!(stroke.width / DPI));
    }
}

/// The ten transform channels of a layer, from the model's.
fn write_transforms(t: &crate::anim::LayerTransform, out: &mut Map<String, Value>) {
    let vec3 = |x: f64, y: f64, z: f64| json!({"x": x, "y": y, "z": z});
    out.insert("translation".into(), channel("Vec3", &t.translation, vec3(0.0, 0.0, 0.0), |v| vec3(v.x as f64 / f64::from(DPI), -v.y as f64 / f64::from(DPI), 0.0), 2));
    out.insert("scale".into(), channel("Vec3", &t.scale, vec3(1.0, 1.0, 1.0), |v| vec3(v.x as f64, v.y as f64, 1.0), 2));
    out.insert("rotation_x".into(), static_channel("Val", json!(0.0)));
    out.insert("rotation_y".into(), static_channel("Val", json!(0.0)));
    out.insert("rotation_z".into(), channel("Val", &t.rotation, json!(0.0), |v| json!(-f64::from(*v)), 1));
    out.insert("flip_h".into(), channel("Bool", &t.flip_h, json!(false), |v| json!(v), 0));
    out.insert("flip_v".into(), channel("Bool", &t.flip_v, json!(false), |v| json!(v), 0));
    out.insert("shear".into(), channel("Vec3", &t.shear, vec3(0.0, 0.0, 0.0), |v| vec3(-v.x as f64, -v.y as f64, 0.0), 2));
    out.insert("following".into(), static_channel("Val", json!(0.0)));
    out.insert("physics_nudge".into(), static_channel("Vec2", json!({"x": 0.0, "y": 0.0})));
}

/// The skeleton JSON from the model's rig: the mapped keys plus each bone's extra keys,
/// verbatim (that's what the model kept them for).
fn write_skeleton(doc: &Document, skeleton: &anicraft_doc::Skeleton) -> Value {
    let bones: Vec<Value> = skeleton
        .bones
        .iter()
        .map(|bone| {
            let mut node: Map<String, Value> = bone.extra.clone().into_iter().collect();
            node.insert("name".into(), json!(bone.name));
            node.insert("parent".into(), json!(bone.parent.map_or(-1, |p| p.0 as i64)));
            node.insert("length".into(), json!(bone.length / DPI));
            node.insert("strength".into(), json!(bone.strength));
            node.insert("hidden".into(), json!(bone.hidden));
            node.insert("constraints".into(), json!(bone.constraints));
            // The bounds flip and swap back with the y axis.
            node.insert("min_constraint".into(), json!(-f64::from(bone.max_constraint)));
            node.insert("max_constraint".into(), json!(-f64::from(bone.min_constraint)));
            let base = |p: Vec2| {
                let (x, y) = to_inch(doc, p);
                json!({"x": x, "y": y, "z": 0.0})
            };
            let delta = |p: Vec2| json!({"x": p.x as f64 / f64::from(DPI), "y": -p.y as f64 / f64::from(DPI), "z": 0.0});
            let is_root = bone.parent.is_none();
            node.insert("anim_pos".into(), channel("Vec3", &bone.pos, base(Vec2::ZERO), |v| if is_root { base(*v) } else { delta(*v) }, 2));
            node.insert("anim_angle".into(), channel("Val", &bone.angle, json!(0.0), |v| json!(-f64::from(*v)), 1));
            node.insert("anim_scale".into(), channel("Val", &bone.scale, json!(1.0), |v| json!(v), 1));
            node.insert("target_bone".into(), channel("Val", &bone.target, json!(-1.0), |v| json!(v), 1));
            node.insert("flip_h".into(), channel("Bool", &bone.flip_h, json!(false), |v| json!(v), 0));
            node.insert("flip_v".into(), channel("Bool", &bone.flip_v, json!(false), |v| json!(v), 0));
            let with_actions = |node: &mut Map<String, Value>, name: &str, acts: Vec<Value>| {
                if !acts.is_empty()
                    && let Some(obj) = node.get_mut(name).and_then(Value::as_object_mut)
                {
                    obj.insert("actions".into(), Value::Array(acts));
                }
            };
            with_actions(&mut node, "anim_angle", actions_of(&bone.angle, "Val", json!(0.0), |v| json!(-f64::from(*v)), 1));
            with_actions(&mut node, "anim_pos", actions_of(&bone.pos, "Vec3", delta(Vec2::ZERO), |v| if is_root { base(*v) } else { delta(*v) }, 2));
            with_actions(&mut node, "anim_scale", actions_of(&bone.scale, "Val", json!(1.0), |v| json!(v), 1));
            Value::Object(node)
        })
        .collect();
    let mut node: Map<String, Value> = skeleton.extra.clone().into_iter().collect();
    node.entry("type".to_string()).or_insert_with(|| json!("BoneGroup"));
    node.insert("binding_mode".into(), json!(skeleton.binding_mode));
    node.insert("bones".into(), Value::Array(bones));
    Value::Object(node)
}

/// The `actions[]` of one bone channel: each pose as a channel object of the same kind,
/// its keys converted like the channel's own (angles back to Moho's radians, positions to
/// inches).
fn actions_of<T: Clone>(ch: &anicraft_doc::Channel<T>, kind: &str, default: Value, val: impl Fn(&T) -> Value + Copy, components: usize) -> Vec<Value> {
    ch.actions.iter().map(|a| json!({"name": a.name, "pose": channel(kind, &a.pose, default.clone(), val, components)})).collect()
}

/// A layer the model generates from nothing (a native document): the canonical template plus
/// the model's mapped values.
fn generate_layer(doc: &Document, l: &Layer, report: &mut ExportReport) -> Map<String, Value> {
    let mut node = template().get("layer_defaults").and_then(Value::as_object).cloned().unwrap_or_default();
    node.insert("type".into(), json!(moho_type(l)));
    node.insert("name".into(), json!(l.name));
    node.insert("visible".into(), json!(l.visible));
    node.insert("uuid".into(), json!(layer_uuid(l.id)));
    node.insert("layer_color".into(), color255(l.color));
    write_transforms(&transforms_of(l), &mut node);
    let animated = !l.translation.is_empty() || !l.scale.is_empty() || !l.rotation.is_empty() || !l.shear.is_empty();
    if animated {
        let (x, y) = to_inch(doc, l.origin);
        node.insert("origin".into(), json!({"x": x, "y": y}));
    }
    if !l.opacity.is_empty() {
        node.insert("layer_effects".into(), json!({"alpha": channel("Val", &l.opacity, json!(1.0), |v| json!(v), 1)}));
    }
    if let Some(body) = &l.physics {
        node.insert("physics".into(), json!({
            "enabled": true, "static": body.static_body,
            "friction": body.friction, "restitution": body.restitution,
            "velocity": {"x": body.velocity.x / DPI, "y": -body.velocity.y / DPI},
        }));
    }
    match l.kind {
        LayerKind::Switch => {
            node.insert("switch_keys".into(), channel("String", &l.switch_to, json!(""), |s| json!(s), 0));
            node.insert("switch_interpolation".into(), json!(false));
            node.insert("frame_by_frame".into(), json!(false));
        }
        LayerKind::Bone | LayerKind::Armature => {
            if let Some(skeleton) = &l.skeleton {
                node.insert("skeleton".into(), write_skeleton(doc, skeleton));
            }
        }
        LayerKind::Mask => {
            node.insert("masking".into(), json!(true));
        }
        _ => {}
    }
    let elements: Vec<&Element> = l.keyframes.iter().flat_map(|k| k.elements.iter().map(|e| &**e)).collect();
    if let Some(block) = elements.iter().find_map(|e| match &e.kind {
        ElementKind::Text(b) => Some(&**b),
        _ => None,
    }) {
        write_text_into(block, &mut node);
    }
    let shapes: Vec<&Shape> = elements.iter().filter_map(|e| match &e.kind {
        ElementKind::Shape(s) => Some(s),
        _ => None,
    }).collect();
    if !shapes.is_empty() {
        node.insert("mesh".into(), write_mesh(doc, &shapes, report));
    }
    node
}

/// A stable UUID for a layer id (deterministic, UUID-shaped).
fn layer_uuid(id: anicraft_doc::LayerId) -> String {
    let a = id.0.wrapping_mul(0xD1B5_4A32_D192_ED03);
    let b = id.0.wrapping_mul(0xABCD_9725_35E7_1D6D) ^ 0x8000_0000_0000_0000;
    format!("{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}", (a >> 32) as u32, (a >> 16) as u16, (a >> 4) as u16 & 0xFFF, (b >> 20) as u16 & 0xFFF, b & 0xFFFF_FFFF_FFFF).to_uppercase()
}

/// Moho's type string for a model layer.
fn moho_type(l: &Layer) -> &'static str {
    match l.kind {
        LayerKind::Folder | LayerKind::Guide | LayerKind::Mask | LayerKind::Camera | LayerKind::Armature => "GroupLayer",
        LayerKind::Bone => "BoneLayer",
        LayerKind::Switch => "SwitchLayer",
        LayerKind::Normal => {
            let has_text = l.keyframes.iter().any(|k| k.elements.iter().any(|e| matches!(e.kind, ElementKind::Text(_))));
            if has_text {
                "TextLayer"
            } else {
                "MeshLayer"
            }
        }
    }
}

/// The mesh of a native vector layer: the layer's shapes as Moho points, curves and shapes,
/// geometry back in inches. Gradient and bitmap fills are counted in the report (the
/// generated shape style carries a solid colour); Moho-origin documents keep their own
/// meshes through the sidecar, so this path only serves native drawings.
fn write_mesh(doc: &Document, shapes: &[&Shape], report: &mut ExportReport) -> Value {
    let mut points: Vec<Value> = Vec::new();
    let mut curves: Vec<Value> = Vec::new();
    let mut out_shapes: Vec<Value> = Vec::new();
    let mut next_id = 1u64;
    for shape in shapes {
        if shape.fill.as_ref().is_some_and(|p| !matches!(p, Paint::Solid(_))) {
            report.skip("gradient and bitmap fills (generated meshes draw their solid colour)");
        }
        let mut anchors: Vec<Vec2> = Vec::new();
        let mut segments: Vec<Segment> = Vec::new();
        for cmd in &shape.path.cmds {
            match cmd {
                PathCmd::MoveTo(to) => anchors.push(*to),
                PathCmd::LineTo(to) => {
                    let from = *anchors.last().unwrap_or(to);
                    segments.push((from, None, *to));
                    anchors.push(*to);
                }
                PathCmd::QuadTo(_, to) => {
                    let from = *anchors.last().unwrap_or(to);
                    segments.push((from, None, *to));
                    anchors.push(*to);
                }
                PathCmd::CubicTo(c1, c2, to) => {
                    let from = *anchors.last().unwrap_or(to);
                    segments.push((from, Some((*c1, *c2)), *to));
                    anchors.push(*to);
                }
                PathCmd::Close => {}
            }
        }
        if segments.is_empty() {
            continue;
        }
        // One mesh point per endpoint, shared by the segments that touch it.
        let mut index_of: Vec<(Vec2, i64)> = Vec::new();
        let mut point_of = |p: Vec2, points: &mut Vec<Value>| -> i64 {
            for (at, id) in &index_of {
                if (*at - p).dot(*at - p) < 1e-9 {
                    return *id;
                }
            }
            let id = points.len() as i64;
            index_of.push((p, id));
            let (x, y) = to_inch(doc, p);
            points.push(json!({
                "type": "Point",
                "parent": -2,
                "colored": false,
                "position": static_channel("Vec2", json!({"x": x, "y": y})),
                "width": static_channel("Val", json!(1.75)),
                "opacity": static_channel("Val", json!(1.0)),
                "color": static_channel("Color", json!({"r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0})),
                "color_strength": static_channel("Val", json!(0.0)),
            }));
            id
        };
        // An animated point track (ADR 0105) drives its point's position channel; the model's
        // tracks are path-command ordered, and this mesh's points follow the same order.
        let first_curve = curves.len();
        for (i, (from, handles, to)) in segments.iter().enumerate() {
            let from_id = point_of(*from, &mut points);
            let to_id = point_of(*to, &mut points);
            let dist = (*from - *to).length();
            let (s_out, w_out) = handles.map_or((0.0, 1.0), |(c1, _)| handle_form(c1 - *from, dist));
            let (s_in, w_in) = handles.map_or((0.0, 1.0), |(_c1, c2)| handle_form(*to - c2, dist));
            curves.push(json!({
                "type": "Curve",
                "num_points": 2,
                "closed": false,
                "profile_layer_uuid": "",
                "profile_curve_id": -1,
                "profile_repeat": 16,
                "points": [
                    curve_point(from_id, s_out, w_in, w_out),
                    curve_point(to_id, s_in, w_in, w_out),
                ],
            }));
            let _ = i;
        }
        // The shape: its curves run forward, once each (a filled loop's edges).
        let count = curves.len() - first_curve;
        let mut edge_curves = Vec::new();
        let mut edge_segments = Vec::new();
        let mut edge_flags = Vec::new();
        for c in 0..count {
            edge_curves.push(json!(first_curve + c));
            edge_segments.push(json!(0));
            edge_flags.push(json!(1));
        }
        let fill = match &shape.fill {
            Some(Paint::Solid(c)) => Some(color01(*c)),
            _ => None,
        };
        let stroke = shape.stroke.as_ref();
        out_shapes.push(json!({
            "type": "Shape",
            "name": format!("S{next_id}"),
            "id": next_id,
            "selected": false,
            "has_fill": fill.is_some(),
            "has_outline": stroke.is_some(),
            "fill_allowed": fill.is_some(),
            "combo_mode": 0,
            "edges": {"curve": edge_curves, "segment": edge_segments, "flag": edge_flags},
            "style": {
                "type": "Style",
                "name": "",
                "uuid": layer_uuid(anicraft_doc::LayerId(next_id * 7919)),
                "define_fill_color": fill.is_some(),
                "fill_color": static_channel("Color", fill.unwrap_or_else(|| color01(Rgba::WHITE))),
                "define_line_width": stroke.is_some(),
                "line_width": stroke.map_or(0.005556, |s| s.width / DPI),
                "define_line_col": stroke.is_some(),
                "line_color": static_channel("Color", stroke.map_or_else(|| color01(Rgba::BLACK), |s| color01(s.color))),
                "line_caps": 1,
                "fill_style": {"type": "SS_Gradient2", "gradient_type": 0, "through_alpha": false, "gradients": []},
                "brush_randomize": true, "brush_sat_drift": 0.0, "brush_size_amp": 0.0,
                "brush_size_scale": 0.3, "brush_spacing": 0.25, "brush_tint": true, "brush_val_drift": 0.0,
            },
            "effect_scale": static_channel("Val", json!(1.0)),
            "effect_rotation": static_channel("Val", json!(0.0)),
            "effect_offset": static_channel("Vec2", json!({"x": 0.0, "y": 0.0})),
            "inherited_style_name": "",
            "inherited_style2_name": "",
            "3d_thickness": 0.0,
        }));
        next_id += 1;
    }
    let ids: Vec<String> = out_shapes.iter().filter_map(|s| s.get("id").and_then(Value::as_u64).map(|i| i.to_string())).collect();
    json!({
        "type": "Mesh",
        "curve_interpretation": 1,
        "points": points,
        "curves": curves,
        "shapes": out_shapes,
        "groups": [],
        "next_shape_id": next_id,
        "anim_shape_order": false,
        "shape_order": static_channel("String", json!(ids.join("|"))),
        "skia_scaling": -1.0,
    })
}

/// The smoothness and weight a curve entry writes so the importer's reconstruction
/// (`distance/3 · smoothness · weight` at the tangent direction) reproduces the model's
/// handle length: smoothness 1 and the weight that carries it, or smoothness 0 for a
/// straight segment.
fn handle_form(handle: Vec2, dist: f32) -> (f64, f64) {
    if handle.length() > 1e-6 && dist > 1e-6 {
        (1.0, f64::from(handle.length() * 3.0 / dist))
    } else {
        (0.0, 1.0)
    }
}

fn curve_point(point: i64, smoothness: f64, weight_in: f64, weight_out: f64) -> Value {
    json!({
        "point": point,
        "segments_on": true,
        "smoothness": static_channel("Val", json!(smoothness)),
        "weight_in": static_channel("Val", json!(weight_in)),
        "weight_out": static_channel("Val", json!(weight_out)),
        "offset_in": static_channel("Val", json!(0.0)),
        "offset_out": static_channel("Val", json!(0.0)),
        "fill_shapes": static_channel("Bool", json!(true)),
    })
}

/// Packages the document as a `.moho` zip: `Project.mohoproj` beside `preview.jpg` (an
/// already-encoded JPEG, stored as it is).
pub fn package_bytes(doc: &Document, preview_jpeg: Option<&[u8]>) -> Result<Vec<u8>, MohoError> {
    let (project, _report) = project_json(doc);
    let json_bytes = serde_json::to_vec(&project).map_err(|e| MohoError::Json(e.to_string()))?;
    let mut zip = anicraft_media::zipw::ZipWriter::new();
    zip.write(crate::import::PROJECT_ENTRY, &json_bytes);
    if let Some(preview) = preview_jpeg {
        zip.store("preview.jpg", preview);
    }
    Ok(zip.finish())
}
