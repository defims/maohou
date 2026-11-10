//! A `BoneLayer`'s `skeleton` → AniCraft's rig (ADR 0104).
//!
//! Moho's `Bone` serializes 61 keys; the rig model keeps the editable subset as fields —
//! name, parent, length, strength, the three anim channels, the IK target, angle
//! constraints, hidden, offset and the flips — and captures every other key verbatim in
//! [`anicraft_doc::Bone::extra`], so a save round-trips what it doesn't yet model. Bone
//! channels convert like every other channel (ADR 0102): positions map through the
//! inch→pixel transform (a point, like the camera track), angles flip sign (y up → screen
//! clockwise), lengths and offsets scale by the dpi. Constraint bounds flip and swap with
//! the y axis. Smart-bone actions (`actions[]` inside a channel, each with a pose
//! sub-channel) convert pose timelines with the same converters and ride on the channels;
//! Moho's layer-level `actions:[{name, pose}]` reference table has no slot here and stays
//! in the sidecar.

use crate::anim;
use crate::channel::{self, Channel};
use crate::import::{Builder, Report};
use crate::json::{self, array, get};
use anicraft_doc::{Bone, BoneAction, BoneId, Channel as DocChannel, ChannelValue, Skeleton};
use anicraft_geom::Vec2;
use serde_json::Value;
use std::sync::Arc;

/// The bone keys the model reads; everything else lands in `Bone::extra` untouched.
const MODELED: [&str; 15] = [
    "name",
    "parent",
    "length",
    "strength",
    "anim_pos",
    "anim_angle",
    "anim_scale",
    "target_bone",
    "constraints",
    "min_constraint",
    "max_constraint",
    "hidden",
    "offset",
    "flip_h",
    "flip_v",
];

/// Reads `skeleton` into the rig, or `None` when it holds no bones (a bone layer with an
/// empty skeleton is just a folder). `type` and `bones_groups` ride in [`Skeleton::extra`].
pub(crate) fn read_skeleton(b: &mut Builder, skeleton: Option<&Value>) -> Option<Arc<Skeleton>> {
    let skeleton = skeleton?;
    let bones = array(get(skeleton, "bones"))?;
    if bones.is_empty() {
        return None;
    }
    let mut out = Skeleton {
        binding_mode: json::integer(get(skeleton, "binding_mode")).unwrap_or(0).clamp(0, 100) as i32,
        ..Skeleton::default()
    };
    // Bone bases are document POINTS (the canvas-centre origin included), like the camera
    // track — they map through the full inch→pixel transform, not the delta scaling.
    let to_px = b.to_px;
    for raw in bones {
        let mut bone = Bone { name: json::string(get(raw, "name")).unwrap_or_else(|| "Bone".into()), ..Bone::default() };
        // The parent is a flat-table index; −1 (or anything past the table) reads as none —
        // sanitize drops the broken links after the table is complete.
        if let Some(p) = json::integer(get(raw, "parent"))
            && p >= 0
            && let Ok(p) = u32::try_from(p)
        {
            bone.parent = Some(BoneId(p));
        }
        // Lengths and offsets are inch distances; positions are document points.
        bone.length = (json::num(get(raw, "length")).unwrap_or(0.0) as f32 * b.dpi).max(0.0);
        bone.strength = json::num(get(raw, "strength")).unwrap_or(1.0) as f32;
        bone.hidden = json::boolean(get(raw, "hidden")).unwrap_or(false);
        bone.constraints = json::boolean(get(raw, "constraints")).unwrap_or(false);
        // The constraint bounds flip and swap with the y axis (CCW radians → screen CW).
        bone.min_constraint = -(json::num(get(raw, "max_constraint")).unwrap_or(0.0)) as f32;
        bone.max_constraint = -(json::num(get(raw, "min_constraint")).unwrap_or(0.0)) as f32;
        bone.offset = json::vec2(get(raw, "offset")).map_or(Vec2::ZERO, |p| Vec2::new(p.x * b.dpi, -p.y * b.dpi));
        anim::read_channel(&mut bone.pos, raw, "anim_pos", &mut b.report, 2, |ch, i| {
            let (x, y, _z) = channel::sample_vec3_at(ch, i).unwrap_or((0.0, 0.0, 0.0));
            to_px.apply(Vec2::new(x as f32, y as f32))
        });
        anim::read_channel(&mut bone.angle, raw, "anim_angle", &mut b.report, 1, |ch, i| -(channel::sample_f64_at(ch, i).unwrap_or(0.0)) as f32);
        anim::read_channel(&mut bone.scale, raw, "anim_scale", &mut b.report, 1, |ch, i| channel::sample_f64_at(ch, i).unwrap_or(1.0) as f32);
        anim::read_channel(&mut bone.target, raw, "target_bone", &mut b.report, 1, |ch, i| channel::sample_f64_at(ch, i).unwrap_or(-1.0) as f32);
        anim::read_channel(&mut bone.flip_h, raw, "flip_h", &mut b.report, 0, |ch, i| channel::sample_bool_at(ch, i).unwrap_or(false));
        anim::read_channel(&mut bone.flip_v, raw, "flip_v", &mut b.report, 0, |ch, i| channel::sample_bool_at(ch, i).unwrap_or(false));
        // Smart-bone actions ride on the channels (ADR 0104); each pose timeline converts
        // with its channel's own converter.
        let angle_val = |ch: &Channel, i: usize| -(channel::sample_f64_at(ch, i).unwrap_or(0.0)) as f32;
        let pos_val = |ch: &Channel, i: usize| {
            let (x, y, _z) = channel::sample_vec3_at(ch, i).unwrap_or((0.0, 0.0, 0.0));
            Vec2::new(x * b.dpi, -y * b.dpi)
        };
        let scale_val = |ch: &Channel, i: usize| channel::sample_f64_at(ch, i).unwrap_or(1.0) as f32;
        bone.angle.actions = actions(raw, "anim_angle", &mut b.report, 1, angle_val);
        bone.pos.actions = actions(raw, "anim_pos", &mut b.report, 2, pos_val);
        bone.scale.actions = actions(raw, "anim_scale", &mut b.report, 1, scale_val);
        // Everything else the file carried: verbatim, for the round trip.
        if let Some(obj) = raw.as_object() {
            for (k, v) in obj {
                if !MODELED.contains(&k.as_str()) {
                    bone.extra.insert(k.clone(), v.clone());
                }
            }
        }
        out.bones.push(bone);
    }
    out.sanitize();
    if let Some(t) = get(skeleton, "type") {
        out.extra.insert("type".into(), t.clone());
    }
    if let Some(g) = get(skeleton, "bones_groups") {
        out.extra.insert("bones_groups".into(), g.clone());
    }
    Some(Arc::new(out))
}

/// The smart-bone actions of one bone channel: each entry of the channel's `actions[]`
/// carries a pose sub-channel, converted with `value` like the parent channel's keys.
fn actions<T: ChannelValue>(
    raw: &Value,
    name: &str,
    report: &mut Report,
    components: usize,
    value: impl FnMut(&Channel, usize) -> T + Copy,
) -> Vec<BoneAction<T>> {
    let mut out = Vec::new();
    for a in array(get(raw, name))
        .unwrap_or(&[])
        .iter()
        .flat_map(|ch| array(get(ch, "actions")).unwrap_or(&[]))
    {
        let Some(label) = json::string(get(a, "name")) else { continue };
        let mut pose = DocChannel::<T>::default();
        if let Some(p) = get(a, "pose") {
            anim::read_channel_value(&mut pose, p, report, components, value);
        }
        out.push(BoneAction { name: label.chars().take(256).collect(), pose });
    }
    out
}
