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
use crate::import::{Builder, FollowRequest, MOHO_DPI};
use anicraft_doc::{ColorEffect, Shape};
use crate::json::{self, array, get};
use crate::{audio, image, mesh};
use crate::text;
use anicraft_doc::{Element, ElementKind, Filter, Keyframe, Layer, LayerId, LayerKind};
use anicraft_geom::{Affine, FillRule};
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
/// children name it as their parent; evaluation chains its transform and effects), whether
/// it is a mask member of a masking group (it becomes the mask layer the group's clip
/// forms from, drawing itself when the Moho member did), and whether a masking group with
/// no region to show it (it doesn't).
#[derive(Clone, Copy)]
pub(crate) struct Inherit {
    pub parent: Option<LayerId>,
    pub mask_clip: bool,
    pub drawn: bool,
    pub hidden: bool,
}

impl Inherit {
    pub(crate) fn under(parent: Option<LayerId>) -> Self {
        Self { parent, mask_clip: false, drawn: false, hidden: false }
    }
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

/// Moho's per-layer mask role (`masking`; the scripting API's MOHO.MM_* constants):
/// 0 = masked (draws only in the group's mask region), 1 = not masked (draws everywhere),
/// 2 = "+ Add to mask" (draws and defines the region), 3 = subtract from it, 4 = add but
/// keep invisible, 5 = subtract but keep invisible, 6/7 = clear the mask, then add
/// (drawn/invisible). Only a group whose `group_mask` is 2 applies it — the scripting API
/// constants name the modes "0 = none, 1 = turn on masking, initially showing all
/// sub-layers, 2 = … hiding all sub-layers"; mode 2 is the manual's "Hide all".
fn mask_role(layer: &Value) -> i64 {
    json::integer(get(layer, "masking")).unwrap_or(0).clamp(0, 7)
}

fn layer_type(layer: &Value) -> String {
    json::string(get(layer, "type")).unwrap_or_default()
}

/// The leaf types whose artwork can form a mask (a group with bones or variants cannot sit
/// under a mask in AniCraft's flat timeline: [`anicraft_doc::Timeline::repair_masks`]
/// re-parents anything but a normal layer out of one).
fn can_mask(layer: &Value) -> bool {
    matches!(layer_type(layer).as_str(), "MeshLayer" | "ImageLayer" | "TextLayer")
}

/// A masking group (`group_mask` 2): Moho hides everything in the group until the mask
/// members' fills say where the members may draw. AniCraft says exactly that with a mask
/// layer and the masked members as its children, the mask first so the run stays unbroken
/// (ADR 0108): a mask member that drew itself (roles 2/6) imports with `mask_draws`, an
/// invisible one (4/7) clips alone. What has no slot is counted: past the first mask
/// member extra ones don't reach the single clip, subtracting (3/5) cuts nothing, a mask
/// role on a container or a content-less layer does nothing, and a container cannot sit
/// under a mask. With no region at all Moho shows nothing, so the members import hidden.
fn walk_masked_group(b: &mut Builder, kids: &[Value], into: &Inherit, depth: usize, out: &mut Vec<FlatLayer>) {
    let add = |k: &Value| matches!(mask_role(k), 2 | 4 | 6 | 7);
    let mut clip: Option<LayerId> = None;
    let mut past_first = false;
    for k in kids.iter().rev().filter(|k| add(k) && can_mask(k)) {
        let as_mask = Inherit { parent: into.parent, mask_clip: true, drawn: matches!(mask_role(k), 2 | 6), hidden: false };
        if let Some(id) = build_layer(b, k, &as_mask, depth, out) {
            if clip.is_none() {
                clip = Some(id);
            } else {
                past_first = true;
            }
        }
    }
    if past_first {
        b.report.skip("mask layers past the first in a group (only the first clips)");
    }
    if kids.iter().any(|k| add(k) && !can_mask(k)) {
        b.report.skip("mask roles on layers that cannot mask");
    }
    let subtracted = kids.iter().filter(|k| matches!(mask_role(k), 3 | 5)).count();
    if subtracted > 0 {
        b.report.skip("subtract-from-mask layers (nothing is subtracted)");
    }
    if kids.iter().any(|k| matches!(mask_role(k), 6 | 7)) {
        b.report.skip("clear-mask mask members (the union acts as one mask)");
    }
    // Members: the masked ones (role 0) under the mask; the unmasked (1), the subtracting
    // (3, drawn) and the ones without a clip to fall under draw beside it — a subtract
    // that keeps invisible (5) hides itself as in Moho.
    let hidden_group = clip.is_none() && subtracted == 0;
    for k in kids.iter().rev().filter(|k| !add(k) || !can_mask(k)) {
        let member_into = if mask_role(k) == 0 && is_container(k) {
            if clip.is_some() {
                b.report.skip("groups inside a masked group (they import unmasked)");
            }
            Inherit { hidden: hidden_group, ..*into }
        } else if mask_role(k) == 5 {
            Inherit { parent: into.parent, mask_clip: false, drawn: false, hidden: true }
        } else if mask_role(k) == 0 && let Some(c) = clip {
            Inherit { parent: Some(c), mask_clip: false, drawn: false, hidden: false }
        } else {
            Inherit { hidden: hidden_group, ..*into }
        };
        build_layer(b, k, &member_into, depth, out);
    }
}

fn is_container(layer: &Value) -> bool {
    matches!(layer_type(layer).as_str(), "GroupLayer" | "BoneLayer" | "SwitchLayer")
}

/// Resolves the Follow Path relations seen during the walk (ADR 0109): the followed curve
/// flattens onto its target layer as an invisible shape (it may belong to no shape of its
/// own, and the relation needs its geometry), and the follower carries the relation — its
/// target, the curve element, the animated progress, rotate to follow and bend with curve.
pub(crate) fn resolve_follows(b: &mut Builder, flat: &mut [FlatLayer], layers: &[Value]) {
    for r in std::mem::take(&mut b.follows) {
        let Some(&target_id) = b.uuids.get(&r.target_uuid) else {
            b.report.skip("follow-path relations (their target layer is missing)");
            continue;
        };
        let Some(target_value) = find_layer(layers, &r.target_uuid) else { continue };
        let Some(target_index) = flat.iter().position(|f| f.layer.id == target_id) else { continue };
        let (ok, el_id) = build_follow(b, &mut flat[target_index].layer, target_value, r.curve);
        if !ok {
            b.report.skip("follow-path relations (their curve is unreadable)");
            continue;
        }
        let mut following = anicraft_doc::Channel::default();
        anim::read_channel_value(&mut following, &r.following, &mut b.report, 0, |ch, i| channel::sample_f64_at(ch, i).unwrap_or(0.0) as f32);
        let Some(follower) = flat.iter_mut().find(|f| f.layer.id == r.follower) else { continue };
        follower.layer.follow = Some(anicraft_doc::FollowPath { target: target_id, element: el_id, subpath: 0, following, rotate: r.rotate, bending: r.bending });
    }
}

/// Flattens the followed curve onto the target layer's first keyframe as an invisible
/// shape (a path with no fill and no stroke draws nothing); returns the element's id.
fn build_follow(b: &mut Builder, target: &mut Layer, target_value: &Value, curve: usize) -> (bool, anicraft_doc::ElementId) {
    let data = mesh::parse_mesh(b, get(target_value, "mesh").unwrap_or(&Value::Null));
    let Some(path) = data.as_ref().and_then(|d| mesh::curve_path(d, curve)) else {
        return (false, anicraft_doc::ElementId(b.doc.alloc_id()));
    };
    let to_px = b.to_px;
    let transform = to_px.mul(&layer_transform(b, target_value));
    let el_id = anicraft_doc::ElementId(b.doc.alloc_id());
    let el = Element {
        id: el_id,
        name: String::new(),
        transform,
        color: ColorEffect::default(),
        kind: ElementKind::Shape(Shape { path: Arc::new(path), fill: None, stroke: None, fill_rule: FillRule::NonZero, merge: false, bind: Vec::new(), points: Vec::new(), trim: None, texture: None }),
    };
    if let Some(kf) = target.keyframes.first_mut() {
        Arc::make_mut(kf).elements.push(Arc::new(el));
    }
    (true, el_id)
}

/// Finds a layer by its Moho uuid, anywhere in the inline tree.
fn find_layer<'v>(layers: &'v [Value], uuid: &str) -> Option<&'v Value> {
    for l in layers {
        if json::string(get(l, "uuid")).as_deref() == Some(uuid) {
            return Some(l);
        }
        if let Some(found) = array(get(l, "layers")).and_then(|kids| find_layer(kids, uuid)) {
            return Some(found);
        }
    }
    None
}

fn build_layer(b: &mut Builder, layer: &Value, into: &Inherit, depth: usize, out: &mut Vec<FlatLayer>) -> Option<LayerId> {
    let ty = layer_type(layer);
    let name = json::string(get(layer, "name")).unwrap_or_else(|| "Layer".into());
    let id = LayerId(b.doc.alloc_id());
    let mut l = Layer::new(id, name);
    l.parent = into.parent;
    // The Follow Path relations reference layers by Moho uuid (ADR 0109).
    if let Some(u) = json::string(get(layer, "uuid")).filter(|u| !u.is_empty()) {
        b.uuids.insert(u, id);
    }
    // The smart-bone action membership table: which rig actions the layer belongs to
    // (the poses themselves ride the skeleton's channels; ADR 0104).
    for a in array(get(layer, "actions")).unwrap_or(&[]) {
        let Some(name) = json::string(get(a, "name")).filter(|n| !n.is_empty()) else { continue };
        l.smart_actions.push(anicraft_doc::SmartActionRef { name, pose: json::integer(get(a, "pose")).unwrap_or(0) });
    }
    l.visible = layer_visibility(b, layer) && !into.hidden;
    // The layer's effect stack: opacity and the static filters (ADR 0103). Any layer kind
    // carries them in Moho.
    l.opacity = layer_opacity(b, layer);
    l.filters = layer_filters(b, layer);
    // The layer as a physics body (ADR 0106): Moho's `physics.enabled` decides; the keys
    // the model doesn't hold are counted and ride the sidecar. A physics-enabled group
    // also collects the scene's world.
    collect_world(b, layer);
    l.physics = read_physics_body(b, layer);
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
        "MeshLayer" | "ImageLayer" | "TextLayer" => {
            count_layer_extras(b, layer);
            // A Follow Path relation resolves after the walk (the target may come later in
            // Moho's bottom-first array than its follower): record it here (ADR 0109).
            let target_uuid = json::string(get(layer, "follow_layer_uuid")).unwrap_or_default();
            if !target_uuid.is_empty()
                && let Some(curve) = json::integer(get(layer, "follow_curve")).filter(|c| *c >= 0)
            {
                b.follows.push(FollowRequest {
                    follower: id,
                    target_uuid,
                    curve: curve as usize,
                    rotate: json::boolean(get(layer, "rotate_to_follow")).unwrap_or(false),
                    bending: json::boolean(get(layer, "follow_bending")).unwrap_or(false),
                    following: get(layer, "transforms").and_then(|t| get(t, "following")).cloned().unwrap_or(Value::Null),
                });
            }
            // A mask member of a masking group becomes the mask itself: its fills clip the
            // group's other members (walk_masked_group parents them), and draw beneath
            // them when the Moho member drew itself (ADR 0108).
            if into.mask_clip {
                l.kind = LayerKind::Mask;
                l.mask_draws = into.drawn;
            }
            match ty.as_str() {
                "MeshLayer" => {
                    let m = get(layer, "mesh").unwrap_or(&Value::Null);
                    finish_leaf(b, l, world, Content::Mesh(m), parent_bone(layer), out);
                }
                "ImageLayer" => finish_leaf(b, l, world, Content::Image(layer), parent_bone(layer), out),
                _ => finish_leaf(b, l, world, Content::Text(layer), parent_bone(layer), out),
            }
            return Some(id);
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
            let kids_into = Inherit::under(Some(id));
            // Moho's group masking ("all invisible"): members draw only inside the mask
            // members' region. Any other group mode (none, "all visible") draws everyone.
            if json::integer(get(layer, "group_mask")).unwrap_or(0) == 2 {
                walk_masked_group(b, kids, &kids_into, depth + 1, out);
            } else {
                walk(b, kids, &kids_into, depth + 1, out);
            }
            return Some(id);
        }
        "NoteLayer" => b.report.skip("note layers"),
        "ParticleLayer" => b.report.skip("particle layers"),
        "PatchLayer" => b.report.skip("patch layers"),
        "AudioLayer" => {
            count_layer_extras(b, layer);
            audio::audio_layer(b, layer, &mut l);
            out.push(FlatLayer { layer: l });
            return Some(id);
        }
        "" => b.report.skip("layers without a type"),
        other => b.report.skip(&format!("layers of unknown type ({other})")),
    }
    None
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
    // The Follow Path relation itself resolves (ADR 0109); bending has no rendering slot:
    // the vendor publishes no bend algorithm and the corpus has no bent specimen.
    if json::boolean(get(layer, "follow_bending")).unwrap_or(false) {
        b.report.skip("follow bending (the artwork rides the path, it does not bend)");
    }
    if ch("timeline_markers").is_some_and(|c| channel::sample_string(&c, frame).is_some_and(|s| !s.is_empty())) {
        b.report.skip("layer timeline markers");
    }
    if json::boolean(get(layer, "animated_layer_effects")).unwrap_or(false) {
        b.report.skip("animated layer effects");
    }
    // A timing offset on a content layer shifts its whole channel timeline; the audio
    // layer's bar resolves in audio.rs instead. No corpus specimen shifts content, so it
    // is counted until one does.
    if json::integer(get(layer, "timing_offset")).unwrap_or(0) != 0 && layer_type(layer) != "AudioLayer" {
        b.report.skip("layer timing offsets (their channels play shifted)");
    }
    if json::boolean(get(layer, "mask_expansion")).unwrap_or(false) {
        b.report.skip("mask expansion");
    }
    if json::integer(get(layer, "3d_mode")).is_some_and(|m| m != 0) {
        b.report.skip("3D layers");
    }
    if json::boolean(get(layer, "face_camera")).unwrap_or(false) {
        b.report.skip("3D face-camera layers");
    }
}

/// The layer's physics body (Moho's `physics.*`; ADR 0106): enabled layers carry the
/// editable core — static, friction, restitution, velocity (inches/s, converted). Density
/// ratios, respawning, sleeping, pivots, motors and force fields have no slot in the first
/// slice and are counted when they carry values; the group's world (its `enable_physics`,
/// `gravity`, `wind`) collects on the builder.
fn read_physics_body(b: &mut Builder, layer: &Value) -> Option<anicraft_doc::PhysicsBody> {
    let ph = get(layer, "physics")?;
    let frame = b.frame;
    let on = json::boolean(get(ph, "enabled")).unwrap_or(false);
    if !on {
        return None;
    }
    let ch = |k: &str| get(ph, k).and_then(Channel::parse);
    // Moho writes constant default motor keys on every body (the corpus's two keys carry
    // the same π with the motor off): a motor matters only when it is on or its speed
    // genuinely varies over time.
    let motor_varies = ch("motor_speed").is_some_and(|c| {
        c.is_animated()
            && c.val().first().and_then(|v| json::num(Some(v))).is_some_and(|first| {
                !c.val().iter().all(|v| json::num(Some(v)).is_some_and(|v| (v - first).abs() < 1e-9))
            })
    });
    if json::boolean(get(ph, "enable_motor")).unwrap_or(false) || motor_varies {
        b.report.skip("physics motors (the body does not spin)");
    }
    if ch("force_field_vector").is_some_and(|c| c.is_animated()) || json::boolean(get(ph, "force_field")).unwrap_or(false) {
        b.report.skip("physics force fields");
    }
    if json::boolean(get(ph, "sleeping")).unwrap_or(false) || json::integer(get(ph, "respawn")).is_some_and(|r| r != 0) || json::boolean(get(ph, "pivot")).unwrap_or(false) {
        b.report.skip("physics body options (sleep, respawn, pivot)");
    }
    // Moho stores density/friction/restitution as plain numbers (fresh-minimal evidence);
    // a channelled form reads too.
    let num = |k: &str, d: f64| json::num(get(ph, k)).or_else(|| ch(k).and_then(|c| channel::sample_f64(&c, frame))).unwrap_or(d);
    if (num("density", 1.0) - 1.0).abs() > 1e-6 {
        b.report.skip("physics density ratios (bodies weigh the same)");
    }
    let velocity = get(ph, "velocity").and_then(|v| json::vec2(Some(v))).unwrap_or(anicraft_geom::Vec2::ZERO);
    Some(anicraft_doc::PhysicsBody {
        static_body: json::boolean(get(ph, "static")).unwrap_or(false),
        friction: num("friction", 0.3).clamp(0.0, 1.0) as f32,
        restitution: num("restitution", 0.5).clamp(0.0, 1.0) as f32,
        velocity: anicraft_geom::Vec2::new(velocity.x * b.dpi, -velocity.y * b.dpi),
    })
}

/// The enclosing physics-enabled group's world (Moho's GroupLayer `enable_physics` with its
/// `gravity`/`wind`, inches/s² y-up): the first one becomes the scene's world, later ones
/// are counted. Gravity's default in the corpus is (0, −10) — down, stage pixels +y.
fn collect_world(b: &mut Builder, layer: &Value) {
    let frame = b.frame;
    // Two real-world forms (ADR 0106):
    // • the GroupLayer form — `enable_physics` ⟨ch:Bool⟩ with plain `gravity`/`wind`
    //   {x,y} inches/s² (Moho writes these on every group; most are template defaults);
    // • the BoneLayer form — Moho's own working rigs (the Boar startup file) carry the
    //   world on the bone layer: `gravity` = {direction: ⟨ch:Val⟩ radians (270° = down),
    //   strength: ⟨ch:Val⟩} (the format spec's `gravity {x,y}` guess corrected).
    let ty = json::string(get(layer, "type")).unwrap_or_default();
    let world = match ty.as_str() {
        "GroupLayer" => {
            let on = get(layer, "enable_physics").and_then(Channel::parse).is_some_and(|c| channel::sample_bool(&c, frame).unwrap_or(false));
            if !on {
                return;
            }
            let conv = |v: Option<anicraft_geom::Vec2>| -> Option<anicraft_geom::Vec2> {
                v.map(|g| anicraft_geom::Vec2::new(g.x * b.dpi, -g.y * b.dpi))
            };
            let gravity = conv(get(layer, "gravity").and_then(|v| json::vec2(Some(v)))).unwrap_or(anicraft_geom::Vec2::new(0.0, 10.0 * b.dpi));
            let wind = conv(get(layer, "wind").and_then(|v| json::vec2(Some(v)))).unwrap_or(anicraft_geom::Vec2::ZERO);
            Some(anicraft_doc::PhysicsWorld { gravity, wind })
        }
        "BoneLayer" => {
            // Direction in radians (y up): a layer-space vector, converted like every
            // inch-space direction — ×dpi, y-flipped.
            let Some(g) = get(layer, "gravity") else { return };
            let Some(dir) = get(g, "direction").and_then(Channel::parse).and_then(|c| channel::sample_f64(&c, frame)) else {
                return;
            };
            let Some(strength) = get(g, "strength").and_then(Channel::parse).and_then(|c| channel::sample_f64(&c, frame)) else {
                return;
            };
            let (sin, cos) = (dir as f32).sin_cos();
            let gravity = anicraft_geom::Vec2::new(cos * strength as f32 * b.dpi, -sin * strength as f32 * b.dpi);
            Some(anicraft_doc::PhysicsWorld { gravity, wind: anicraft_geom::Vec2::ZERO })
        }
        _ => return,
    };
    let Some(world) = world else { return };
    let world = world.sanitized();
    match &b.world {
        Some(_) => b.report.skip("physics worlds past the first (the first one simulates)"),
        None => b.world = Some(world),
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
