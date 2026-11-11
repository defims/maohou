//! The inline layer tree → AniCraft's flat layer list with parent links.
//!
//! Moho stores `layers[]` bottom-first (corpus: CharWalk lists a rig's right limbs before
//! Body/Head and its left limbs last, and a head group lists the eye white before the pupil);
//! AniCraft's index 0 is the top. So the walk visits siblings back-to-front and emits a
//! container before the children it flattens, which keeps every folder's children in one
//! unbroken run under it (what `Timeline::repair_masks` and `group_range` expect).
//!
//! Type mapping: Mesh/Image/Text layers carry content; Group/Bone/Switch layers flatten to
//! folders that composite (ADR 0103): the container's transform channels, its `layer_effects`
//! alpha and its effect stack stay on the folder layer itself, and evaluation renders its
//! whole run as one group — so a transformed, faded or filtered group behaves as in Moho, and
//! its children keep their own channels instead of baking their ancestors'. A container with
//! none of those stays a pure organisational folder. A switch imports as a switch (ADR
//! 0105): its `switch_keys` channel rides the layer and evaluation shows one variant per
//! frame. Bones and physics are counted in the report.

use crate::anim;
use crate::channel::{self, Channel};
use crate::import::{Builder, MOHO_DPI};
use crate::json::{self, array, get};
use crate::{image, mesh};
use crate::text;
use anicraft_doc::{Element, ElementKind, Filter, Keyframe, Layer, LayerId, LayerKind};
use anicraft_geom::Affine;
use serde_json::Value;
use std::sync::Arc;

/// How deep Moho nesting may go before the importer gives up on a subtree (AniCraft's own
/// timeline cap is 16).
const MAX_TREE_DEPTH: usize = 16;
/// Most layers one document may produce.
const MAX_LAYERS: usize = 5_000;

/// One flattened layer, ready for the timeline.
pub(crate) struct FlatLayer {
    pub layer: Layer,
}

/// What a leaf layer draws, read from its type.
enum Content<'doc> {
    /// A vector layer's mesh (or `Null` when it has none).
    Mesh(&'doc Value),
    /// An image layer.
    Image(&'doc Value),
    /// A text layer.
    Text(&'doc Value),
}

/// What a layer inherits from its ancestors: where it sits in the tree (a container's
/// children name it as their parent; evaluation chains its transform and effects).
pub(crate) struct Inherit {
    pub parent: Option<LayerId>,
}

/// Walks Moho's layer array (bottom-first) into `out` (top-first, containers first).
pub(crate) fn walk(b: &mut Builder, layers: &[Value], into: &Inherit, depth: usize, out: &mut Vec<FlatLayer>) {
    if depth > MAX_TREE_DEPTH {
        b.report.skip("layers nested deeper than 16");
        return;
    }
    for layer in layers.iter().rev() {
        if out.len() >= MAX_LAYERS {
            b.report.skip("layers past 5,000");
            return;
        }
        build_layer(b, layer, into, depth, out);
    }
}

fn build_layer(b: &mut Builder, layer: &Value, into: &Inherit, depth: usize, out: &mut Vec<FlatLayer>) {
    let ty = json::string(get(layer, "type")).unwrap_or_default();
    let name = json::string(get(layer, "name")).unwrap_or_else(|| "Layer".into());
    let id = LayerId(b.doc.alloc_id());
    let mut l = Layer::new(id, name);
    l.parent = into.parent;
    l.visible = layer_visibility(b, layer);
    // The layer's effect stack: opacity and the static filters (ADR 0103). Any layer kind
    // carries them in Moho.
    l.opacity = layer_opacity(b, layer);
    l.filters = layer_filters(b, layer);
    // The layer's transform: animated channels on a leaf layer (stage 2), channels on a
    // container (stage 3's group semantics), or the static bake into the drawn elements.
    let channels = anim::layer_transform_channels(get(layer, "transforms").unwrap_or(&Value::Null), &mut b.report);
    let is_container = matches!(ty.as_str(), "GroupLayer" | "BoneLayer" | "SwitchLayer");
    let animated_leaf = channels.is_animated() && !is_container;
    let mut world = Affine::IDENTITY;
    if animated_leaf {
        l.translation = channels.translation;
        l.scale = channels.scale;
        l.rotation = channels.rotation;
        l.shear = channels.shear;
        l.flip_h = channels.flip_h;
        l.flip_v = channels.flip_v;
        if let Some(origin) = get(layer, "origin").and_then(|v| json::vec2(Some(v))) {
            l.origin = b.to_px.apply(origin);
        }
    } else if is_container {
        // The container keeps its transform as channels (a static one is a single key) and
        // composites its run; children reference it by parent, so nothing bakes. Moho writes
        // static default keys on every layer: an identity transform stays off the folder.
        if channels.has_values() && !channels.is_neutral() {
            l.translation = channels.translation;
            l.scale = channels.scale;
            l.rotation = channels.rotation;
            l.shear = channels.shear;
            l.flip_h = channels.flip_h;
            l.flip_v = channels.flip_v;
            if let Some(origin) = get(layer, "origin").and_then(|v| json::vec2(Some(v))) {
                l.origin = b.to_px.apply(origin);
            }
        }
    } else {
        world = layer_transform(b, layer);
    }
    match ty.as_str() {
        "MeshLayer" => {
            count_layer_extras(b, layer);
            let m = get(layer, "mesh").unwrap_or(&Value::Null);
            finish_leaf(b, l, world, Content::Mesh(m), parent_bone(layer), out);
        }
        "ImageLayer" => {
            count_layer_extras(b, layer);
            finish_leaf(b, l, world, Content::Image(layer), parent_bone(layer), out);
        }
        "TextLayer" => {
            count_layer_extras(b, layer);
            finish_leaf(b, l, world, Content::Text(layer), parent_bone(layer), out);
        }
        "GroupLayer" | "BoneLayer" | "SwitchLayer" => {
            l.kind = match ty.as_str() {
                // The rig (ADR 0104): bones import now; a layer without bones stays a plain
                // folder. A switch always imports as one (ADR 0105).
                "BoneLayer" => {
                    l.skeleton = crate::bone::read_skeleton(b, get(layer, "skeleton"));
                    if l.skeleton.is_none() { LayerKind::Folder } else { LayerKind::Bone }
                }
                "SwitchLayer" => {
                    count_skeleton(b, get(layer, "skeleton"));
                    read_switch(b, layer, &mut l);
                    LayerKind::Switch
                }
                _ => LayerKind::Folder,
            };
            count_layer_extras(b, layer);
            out.push(FlatLayer { layer: l });
            let kids = array(get(layer, "layers")).unwrap_or(&[]);
            let kids_into = Inherit { parent: Some(id) };
            walk(b, kids, &kids_into, depth + 1, out);
        }
        "NoteLayer" => b.report.skip("note layers"),
        "ParticleLayer" => b.report.skip("particle layers"),
        "PatchLayer" => b.report.skip("patch layers"),
        "AudioLayer" => b.report.skip("audio layers"),
        "" => b.report.skip("layers without a type"),
        other => b.report.skip(&format!("layers of unknown type ({other})")),
    }
}

/// The plain `visible` bool ANDs with the (channelled) visibility switch of the effect stack.
fn layer_visibility(b: &Builder, layer: &Value) -> bool {
    let plain = json::boolean(get(layer, "visible")).unwrap_or(true);
    let channel = get(layer, "layer_effects").and_then(|fx| get(fx, "visibility")).and_then(Channel::parse);
    plain && channel.is_none_or(|c| channel::sample_bool(&c, b.frame).unwrap_or(true))
}

/// A switch's selector (ADR 0105): Moho's `switch_keys` String channel moves over key for
/// key — its frames are already 0-based, and a String channel always steps. The variants
/// themselves stay ordinary child layers; evaluation shows the named one per frame. A
/// cross-fade switch (`switch_interpolation`) or a frame-by-frame one has no slot yet and
/// is counted; the sidecar keeps everything.
fn read_switch(b: &mut Builder, layer: &Value, l: &mut Layer) {
    if let Some(sk) = get(layer, "switch_keys") {
        let mut sel = anicraft_doc::Channel::<String>::default();
        anim::read_channel_value(&mut sel, sk, &mut b.report, 0, |ch, i| channel::sample_string_at(ch, i).unwrap_or_default());
        l.switch_to = sel;
    }
    if json::boolean(get(layer, "switch_interpolation")).unwrap_or(false) {
        b.report.skip("switch cross-fades (the switch steps between variants)");
    }
    if json::boolean(get(layer, "frame_by_frame")).unwrap_or(false) {
        b.report.skip("frame-by-frame switches (variants shown by selection)");
    }
}

/// Skeletons (and the smart-bone actions riding on them) arrive in a later milestone; the
/// sidecar keeps every number.
fn count_skeleton(b: &mut Builder, skeleton: Option<&Value>) {
    let Some(bones) = skeleton.and_then(|s| get(s, "bones")).and_then(|a| json::array(Some(a))) else { return };
    if bones.is_empty() {
        return;
    }
    *b.report.skipped.entry("skeleton bones".into()).or_default() += bones.len();
    // Smart-bone actions: `actions` arrays inside the bones' anim channels, plus per-bone
    // references. Each one is a dial-driven pose the document model can't hold yet.
    let mut actions = 0usize;
    for bone in bones {
        for ch in ["anim_angle", "anim_pos", "anim_scale"] {
            actions += get(bone, ch).and_then(|c| get(c, "actions")).and_then(|a| json::array(Some(a))).map(|a| a.len()).unwrap_or(0);
        }
    }
    if actions > 0 {
        *b.report.skipped.entry("smart bone actions".into()).or_default() += actions;
    }
}

/// Per-layer features beyond the imported transform and effect stack, counted when
/// non-default.
fn count_layer_extras(b: &mut Builder, layer: &Value) {
    let frame = b.frame;
    let ch = |k: &str| get(layer, k).and_then(Channel::parse);
    // Effect stack parts with no slot yet (blur/noise/pixelation/threshold/shadow imported).
    let fx = get(layer, "layer_effects");
    if fx.and_then(|f| get(f, "ambient_occlusion")).and_then(Channel::parse).is_some_and(|c| channel::sample_f64(&c, frame).is_some_and(|v| v.abs() > 1e-6)) {
        b.report.skip("layer ambient occlusion");
    }
    // The `on` switches are Bool channels, not plain booleans.
    let switch_on = |k: &str| get(layer, k).and_then(|g| get(g, "on")).and_then(Channel::parse).is_some_and(|c| channel::sample_bool(&c, frame).unwrap_or(false));
    if switch_on("layer_outline") {
        b.report.skip("layer outlines");
    }
    if switch_on("layer_shading") {
        b.report.skip("layer shading");
    }
    if switch_on("motion_blur") {
        b.report.skip("motion blur");
    }
    if switch_on("perspective_shadow") {
        b.report.skip("perspective shadows");
    }
    if json::integer(get(layer, "blend_mode")).is_some_and(|m| m != 0) {
        b.report.skip("layer blend modes");
    }
    if !json::string(get(layer, "follow_layer_uuid")).is_none_or(|u| u.is_empty()) {
        b.report.skip("follow-path relations (positions are already baked)");
    }
    if json::boolean(get(layer, "rotate_to_follow")).unwrap_or(false) {
        b.report.skip("rotate to follow");
    }
    if ch("timeline_markers").is_some_and(|c| channel::sample_string(&c, frame).is_some_and(|s| !s.is_empty())) {
        b.report.skip("layer timeline markers");
    }
    if array(get(layer, "actions")).is_some_and(|a| !a.is_empty()) {
        *b.report.skipped.entry("smart bone action references".into()).or_default() += 1;
    }
    if json::boolean(get(layer, "animated_layer_effects")).unwrap_or(false) {
        b.report.skip("animated layer effects");
    }
    if json::integer(get(layer, "timing_offset")).is_some_and(|i| i != 0) {
        b.report.skip("layer timing offsets");
    }
    if json::integer(get(layer, "masking")).is_some_and(|m| m != 0) {
        b.report.skip("mask layers (drawn unmasked)");
    }
    if json::integer(get(layer, "3d_mode")).is_some_and(|m| m != 0) {
        b.report.skip("3D layers");
    }
    if json::boolean(get(layer, "face_camera")).unwrap_or(false) {
        b.report.skip("3D face-camera layers");
    }
}

/// The layer's opacity channel: Moho's `layer_effects.alpha` (0..=1), key for key. A single
/// key at the default is no channel at all.
fn layer_opacity(b: &mut Builder, layer: &Value) -> anicraft_doc::Channel<f32> {
    let mut out = anicraft_doc::Channel::default();
    if let Some(fx) = get(layer, "layer_effects") {
        anim::read_channel(&mut out, fx, "alpha", &mut b.report, 1, |ch, i| {
            (channel::sample_f64_at(ch, i).unwrap_or(1.0).clamp(0.0, 1.0)) as f32
        });
    }
    if out.keys.len() == 1 && (out.keys[0].val - 1.0).abs() < 1e-6 {
        out.keys.clear();
    }
    out
}

/// The layer's effect stack (ADR 0103), read at the import frame: Moho's `layer_effects`
/// scalars plus its `layer_shadow` group. Channels become static values here — animated
/// effect parameters are a later milestone and ride in the sidecar; `animated_layer_effects`
/// is counted when the file says so.
fn layer_filters(b: &mut Builder, layer: &Value) -> Vec<Filter> {
    let frame = b.frame;
    let dpi = MOHO_DPI as f64;
    let mut out = Vec::new();
    let fx = get(layer, "layer_effects");
    let val = |root: Option<&Value>, k: &str| root.and_then(|f| get(f, k)).and_then(Channel::parse).and_then(|c| channel::sample_f64(&c, frame));
    // Blur: Moho's radius is a document distance (inches); a three-pass box blur reaches
    // Moho's soft look.
    if let Some(v) = val(fx, "blur").filter(|v| *v > 1e-6) {
        let px = ((v * dpi) as f32).clamp(0.0, anicraft_doc::MAX_BLUR);
        out.push(Filter::Blur { blur_x: px, blur_y: px, quality: 3 });
    }
    if let Some(v) = val(fx, "noise").filter(|v| *v > 1e-6) {
        out.push(Filter::Noise { amount: (v as f32).clamp(0.0, 1.0) });
    }
    if let Some(v) = val(fx, "pixelation").filter(|v| *v > 1e-6) {
        out.push(Filter::Pixelate { size: ((v * dpi) as f32).clamp(1.0, 256.0) });
    }
    if let Some(v) = val(fx, "threshold").filter(|v| *v > 1e-6) {
        out.push(Filter::Threshold { level: (v as f32).clamp(0.0, 1.0) });
    }
    // The shadow: a Bool-channel switch and its parameter group.
    let shadow = get(layer, "layer_shadow");
    let on = shadow.and_then(|s| get(s, "on")).and_then(Channel::parse).is_some_and(|c| channel::sample_bool(&c, frame).unwrap_or(false));
    if on {
        let angle = (-(val(shadow, "angle").unwrap_or(0.0))).to_degrees() as f32;
        let distance = ((val(shadow, "offset").unwrap_or(0.0) * dpi) as f32).clamp(-1000.0, 1000.0);
        let blur_px = ((val(shadow, "blur").unwrap_or(0.0) * dpi) as f32).clamp(0.0, anicraft_doc::MAX_BLUR);
        let expansion = ((val(shadow, "expansion").unwrap_or(0.0) * dpi) as f32).clamp(-100.0, 100.0);
        let color = shadow
            .and_then(|s| get(s, "color"))
            .and_then(Channel::parse)
            .and_then(|c| channel::sample_color01(&c, frame))
            .unwrap_or(anicraft_doc::Rgba::BLACK);
        if shadow.and_then(|s| json::boolean(get(s, "clip_to_group"))).unwrap_or(false) {
            b.report.skip("shadow clipped to its group (drawn unclipped)");
        }
        if val(shadow, "noise_scale").is_some_and(|v| (v - 1.0).abs() > 1e-6) {
            b.report.skip("shadow noise scale (grain drawn uniform)");
        }
        out.push(Filter::DropShadow {
            blur_x: blur_px,
            blur_y: blur_px,
            strength: 1.0,
            quality: 3,
            angle: angle.rem_euclid(360.0),
            distance,
            color,
            inner: false,
            knockout: false,
            hide_object: false,
            expansion,
            noise: (val(shadow, "noise_amp").unwrap_or(0.0) as f32).clamp(0.0, 1.0),
            threshold: shadow.and_then(|s| get(s, "threshold")).and_then(Channel::parse).is_some_and(|c| channel::sample_bool(&c, frame).unwrap_or(false)),
        });
    }
    out
}

/// Reads a layer's static transform channels (inch space, y up) around its origin pivot.
/// 3D-only channels are counted when they carry values. Called for layers whose transform
/// channels don't animate and aren't kept on the layer — animated leaves and containers go
/// through `anim::layer_transform_channels`.
fn layer_transform(b: &mut Builder, layer: &Value) -> Affine {
    let frame = b.frame;
    let empty = Value::Null;
    let tr = get(layer, "transforms").unwrap_or(&empty);
    let ch = |k: &str| get(tr, k).and_then(Channel::parse);
    let (tx, ty, tz) = ch("translation").and_then(|c| channel::sample_vec3(&c, frame)).unwrap_or((0.0, 0.0, 0.0));
    if tz.abs() > 1e-6 {
        b.report.skip("3D layer translation");
    }
    let rot = ch("rotation_z").and_then(|c| channel::sample_f64(&c, frame)).unwrap_or(0.0) as f32;
    if ch("rotation_x").and_then(|c| channel::sample_f64(&c, frame)).is_some_and(|v| v.abs() > 1e-6)
        || ch("rotation_y").and_then(|c| channel::sample_f64(&c, frame)).is_some_and(|v| v.abs() > 1e-6)
    {
        b.report.skip("3D layer rotation");
    }
    let (sx, sy, sz) = ch("scale").and_then(|c| channel::sample_vec3(&c, frame)).unwrap_or((1.0, 1.0, 1.0));
    if (sz - 1.0).abs() > 1e-6 {
        b.report.skip("3D layer scale");
    }
    let (mut sx, mut sy) = (sx.max(1e-6), sy.max(1e-6));
    if ch("flip_h").and_then(|c| channel::sample_bool(&c, frame)).unwrap_or(false) {
        sx = -sx;
    }
    if ch("flip_v").and_then(|c| channel::sample_bool(&c, frame)).unwrap_or(false) {
        sy = -sy;
    }
    let (shx, shy, _) = ch("shear").and_then(|c| channel::sample_vec3(&c, frame)).unwrap_or((0.0, 0.0, 0.0));
    if ch("following").is_some_and(|c| c.is_animated()) {
        b.report.skip("follow progress animation");
    }
    if ch("physics_nudge").is_some_and(|c| c.is_animated()) {
        b.report.skip("physics nudge animation (drawn without it)");
    }
    let origin = get(layer, "origin").and_then(|v| json::vec2(Some(v))).unwrap_or(anicraft_geom::Vec2::ZERO);
    let shear = Affine { m: [1.0, shy, shx, 1.0, 0.0, 0.0] };
    // Moho pivots layer rotation, shear and scale at the layer origin:
    // p ↦ origin + translation + M · (p − origin).
    let m = Affine::translate(origin.x + tx, origin.y + ty).mul(&Affine::rotate(rot)).mul(&shear).mul(&Affine::scale(sx, sy)).mul(&Affine::translate(-origin.x, -origin.y));
    if m.is_finite() {
        m
    } else {
        b.report.skip("unreadable layer transforms");
        Affine::IDENTITY
    }
}

/// A leaf layer's content: one keyframe spanning the imported frame range. `bind` is the
/// layer's whole-body bone (Moho's `parent_bone` — how real rigs bind, CharWalk's limbs):
/// every shape on the layer rides that bone of the enclosing rig (ADR 0104).
fn finish_leaf(b: &mut Builder, mut l: Layer, world: Affine, content: Content, bind: Option<anicraft_doc::BoneId>, out: &mut Vec<FlatLayer>) {
    let mut k = Keyframe::new(0, b.duration);
    let mut els: Vec<Arc<Element>> = Vec::new();
    match content {
        Content::Mesh(m) => mesh::mesh_elements(b, m, world, &mut els),
        Content::Image(src) => image::image_elements(b, src, world, &mut els),
        Content::Text(src) => text::text_elements(b, src, world, &mut els),
    }
    if let Some(bone) = bind {
        for el in &mut els {
            let el = Arc::make_mut(el);
            if let ElementKind::Shape(shape) = &mut el.kind {
                shape.bind = vec![(bone, 1.0)];
            }
        }
    }
    k.elements = els;
    l.keyframes = vec![Arc::new(k)];
    out.push(FlatLayer { layer: l });
}

/// The layer's whole-body bone: Moho's `parent_bone` (`-1` = none).
fn parent_bone(layer: &Value) -> Option<anicraft_doc::BoneId> {
    json::integer(get(layer, "parent_bone"))
        .filter(|i| *i >= 0)
        .and_then(|i| u32::try_from(i).ok())
        .map(anicraft_doc::BoneId)
}

/// An image (or texture) file reference: `{path, relativeTo}`. Moho 14 names the image
/// layer's own file `image_fileref` (the corpus's only populated form; `relativeTo` values
/// seen so far: the project, absolute paths, and Moho's internal support folder — that last
/// one is Moho's own content, which this reader never reaches into, so those images land in
/// the report).
pub(crate) fn fileref(layer: &Value) -> Option<String> {
    let f = get(layer, "image_fileref").or_else(|| get(layer, "fileref")).or_else(|| get(layer, "layer_ref_fileref"))?;
    json::string(get(f, "path")).filter(|p| !p.is_empty())
}
