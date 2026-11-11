//! Bone commands (ADR 0104): build a rig on a bone layer, pose its bones with channel
//! keys, aim chains with IK targets, and bind shapes to bones. A drag (posing, IK, binding)
//! sends many calls inside one gesture: one undo step, like the camera.
//!
//! Every command names the rig by a layer: the layer itself when it carries the skeleton,
//! else the bone layer above it that does — artwork commands on a bound child layer resolve
//! to the rig that moves it. `bone.info` reads the rig back (bones, sampled matrices), and
//! is how the stage overlay and tests see what evaluation will do.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{Bone, BoneAction, BoneId, LayerId, LayerKind, MAX_BONES, Skeleton};
use anicraft_geom::{Affine, Vec2};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c("bone.rig_add", "Add Bone Rig", "Insert/Timeline", None, "{name?} (a bone layer above the active one, holding an empty skeleton; artwork inside it can then bind to its bones)", true, rig_add),
        c("bone.add", "Add Bone", "Insert/Timeline", None, "{name?, parent? (bone index, default the last), length? (px, default 100), pos? [x, y] (offset from the parent's tip), angle? (rad), layer?} → {bone}", true, add),
        c("bone.delete", "Delete Bone", "Modify/Timeline", None, "{bone, layer?} (its children re-parent to its parent; shapes bound only to it unbind)", true, delete),
        c("bone.set", "Bone Settings", "", None, "{bone, layer?, name?, parent?, length?, strength?, hidden?, constraints?, min?, max? (rad, screen clockwise), offset? [x, y]}", true, set),
        c("bone.key", "Bone Key", "", None, "{bone, channel: \"angle\"|\"pos\"|\"scale\"|\"target\", value (rad | [x, y] | number | bone index), frame?, layer?} (keys the bone's channel at the frame)", true, key),
        c("bone.clear_channel", "Clear Bone Channel", "", None, "{bone, channel, layer?} (removes every key of that bone channel)", true, clear_channel),
        c("bone.bind", "Bind to Bone", "", None, "{bones: [{bone, weight}…], layer?, element?} (binds the shape to the rig's bones, weights normalised to 1; a single {bone} binds at full weight)", true, bind),
        c("bone.unbind", "Unbind Shape", "", None, "{layer?, element?} (the shape draws unbound again)", true, unbind),
        c("bone.info", "Bone Info", "", None, "{layer?, frame?} → the rig's bones (name, parent, length, sampled angle/pos/scale/target) and each bone's world matrix", false, info),
    ]
}

/// The rig layer the command targets: `layer` when it is a bone layer, else the nearest
/// enclosing bone layer (the rig that moves this artwork). The skeleton may still be empty
/// — `bone.add` bootstraps it.
fn rig_of(s: &Session, layer: Option<u64>) -> Result<LayerId> {
    let id = layer.map(LayerId).unwrap_or(s.layer);
    let tl = s.doc.timeline(s.scene);
    let mut lid = id;
    for _ in 0..=anicraft_doc::MAX_LAYER_DEPTH {
        let Some(l) = tl.layer(lid) else { break };
        if l.kind == LayerKind::Bone {
            return Ok(l.id);
        }
        match l.parent {
            Some(p) => lid = p,
            None => break,
        }
    }
    Err(EngineError::State("no bone rig here; add one first (Add Bone Rig, or bone.rig_add)".into()))
}

/// The skeleton behind `layer`, for reading.
fn skeleton(s: &Session, rig: LayerId) -> Result<Arc<Skeleton>> {
    s.doc.timeline(s.scene).layer(rig).and_then(|l| l.skeleton.clone()).filter(|sk| !sk.is_empty()).ok_or_else(|| EngineError::State("the layer carries no skeleton".into()))
}

/// The skeleton behind `layer`, for editing (copy-on-write).
fn skeleton_mut(s: &mut Session, rig: LayerId) -> Result<&mut Skeleton> {
    let tl = s.doc.timeline_mut(s.scene);
    let layer = tl.layer_mut(rig).ok_or_else(|| EngineError::State("the rig layer is gone".into()))?;
    if layer.skeleton.is_none() {
        layer.skeleton = Some(Arc::new(Skeleton::default()));
    }
    // Only this layer's skeleton is touched: make_mut clones when shared (undo snapshots).
    let sk = layer.skeleton.as_mut().ok_or_else(|| EngineError::State("the layer carries no skeleton".into()))?;
    Ok(Arc::make_mut(sk))
}

fn read_bone(p: &Value) -> Result<BoneId> {
    let b = opt_u64(p, "bone")?.ok_or_else(|| EngineError::Param("bone (index) is required".into()))?;
    Ok(BoneId(u32::try_from(b).map_err(|_| EngineError::Param("bone index out of range".into()))?))
}

/// Reads a bone's channel name.
fn read_channel(p: &Value) -> Result<BoneField> {
    let name = opt_str(p, "channel")?.ok_or_else(|| EngineError::Param("channel (\"angle\"|\"pos\"|\"scale\"|\"target\") is required".into()))?;
    match name {
        "angle" => Ok(BoneField::Angle),
        "pos" => Ok(BoneField::Pos),
        "scale" => Ok(BoneField::Scale),
        "target" => Ok(BoneField::Target),
        other => Err(EngineError::Param(format!("unknown bone channel {other:?} (angle, pos, scale, target)"))),
    }
}

enum BoneField {
    Angle,
    Pos,
    Scale,
    Target,
}

fn rig_add(s: &mut Session, p: &Value) -> Result<Value> {
    let name = opt_str(p, "name")?.unwrap_or("Rig").chars().take(256).collect::<String>();
    if s.doc.timeline(s.scene).layers.len() >= 1000 {
        return Err(EngineError::State("too many layers (limit 1000)".into()));
    }
    let id = LayerId(s.doc.alloc_id());
    let frames = s.frame_count().max(1);
    let mut layer = anicraft_doc::Layer::new(id, name);
    layer.kind = LayerKind::Bone;
    layer.keyframes = vec![Arc::new(anicraft_doc::Keyframe::new(0, frames))];
    let tl = s.doc.timeline_mut(s.scene);
    let at = tl.layers.iter().position(|l| l.kind == LayerKind::Camera).unwrap_or(0);
    tl.layers.insert(at, Arc::new(layer));
    tl.repair_masks();
    s.layer = id;
    s.selection.clear();
    Ok(json!({"layer": id.0, "added": true}))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let name = opt_str(p, "name")?.unwrap_or("").chars().take(256).collect::<String>();
    let length = opt_f32(p, "length")?.unwrap_or(100.0);
    if !(length.is_finite() && length >= 0.0) {
        return Err(EngineError::Param("length must be a finite distance ≥ 0".into()));
    }
    let angle = opt_f32(p, "angle")?.unwrap_or(0.0);
    let pos = opt_vec2(p, "pos")?.map(|(x, y)| Vec2::new(x, y));
    // A fresh rig has no skeleton yet: bone.add bootstraps it (the empty case).
    let n = s.doc.timeline(s.scene).layer(rig).and_then(|l| l.skeleton.as_ref()).map(|sk| sk.bones.len()).unwrap_or(0);
    if n >= MAX_BONES {
        return Err(EngineError::State(format!("too many bones (limit {MAX_BONES})")));
    }
    // `parent: -1` roots the bone (an IK control bone lives outside the chain); a missing
    // parent chains off the last bone, the way Moho's Add Bone does.
    let parent = match p.get("parent").and_then(|v| v.as_i64()) {
        Some(-1) => None,
        Some(v) if (0..i64::from(u32::MAX)).contains(&v) => {
            let b = BoneId(v as u32);
            if b.0 as usize >= n {
                return Err(EngineError::Param(format!("parent {} is not a bone of this rig ({n} bones)", b.0)));
            }
            Some(b)
        }
        Some(_) => return Err(EngineError::Param("parent must be a bone index, or −1 for none".into())),
        // A new bone hangs off the last one (the way Moho's Add Bone chains).
        None if n > 0 => Some(BoneId((n - 1) as u32)),
        None => None,
    };
    let mut bone = Bone { name: if name.is_empty() { format!("bone {n}") } else { name }, parent, length, ..Bone::default() };
    bone.angle.set_key(s.frame, angle);
    if let Some(pos) = pos {
        bone.pos.set_key(s.frame, pos);
    }
    let id = BoneId(n as u32);
    let sk = skeleton_mut(s, rig)?;
    sk.bones.push(bone);
    sk.sanitize();
    Ok(json!({"bone": id.0}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let bone = read_bone(p)?;
    let sk = skeleton(s, rig)?;
    let Some(removed_parent) = sk.bones.get(bone.0 as usize).map(|b| b.parent) else {
        return Err(EngineError::Param(format!("bone {} is not a bone of this rig ({})", bone.0, sk.bones.len())));
    };
    let sk = skeleton_mut(s, rig)?;
    sk.bones.remove(bone.0 as usize);
    // Children re-parent to the removed bone's parent; indices above shift down, and IK
    // targets follow them (targets on the deleted bone drop to none).
    for b in &mut sk.bones {
        b.parent = match b.parent {
            Some(x) if x == bone => removed_parent,
            Some(x) if x.0 > bone.0 => Some(BoneId(x.0 - 1)),
            x => x,
        };
        for k in &mut b.target.keys {
            if k.val == bone.0 as f32 {
                k.val = -1.0;
            } else if k.val > bone.0 as f32 && k.val >= 0.0 {
                k.val -= 1.0;
            }
        }
    }
    sk.sanitize();
    // Shapes bound (only) to it unbind: their weights name a bone that no longer exists.
    unbind_bone_everywhere(s, rig, bone);
    Ok(json!({"deleted": bone.0}))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let bone = read_bone(p)?;
    let n = skeleton(s, rig)?.bones.len();
    if bone.0 as usize >= n {
        return Err(EngineError::Param(format!("bone {} is not a bone of this rig ({n})", bone.0)));
    }
    if let Some(parent) = opt_u64(p, "parent")?
        && u32::try_from(parent).map(|v| v as usize >= n).unwrap_or(true)
    {
        return Err(EngineError::Param(format!("parent {parent} is not a bone of this rig ({n})")));
    }
    let sk = skeleton_mut(s, rig)?;
    let Some(b) = sk.bones.get_mut(bone.0 as usize) else { return Err(EngineError::State("bone gone".into())) };
    if let Some(name) = opt_str(p, "name")? {
        b.name = name.chars().take(256).collect();
    }
    if let Some(parent) = opt_u64(p, "parent")? {
        // The bounds check above ran on the same length; this assignment is safe.
        b.parent = u32::try_from(parent).ok().filter(|v| *v != bone.0).map(BoneId);
    }
    if let Some(v) = opt_f32(p, "length")? {
        if !(v.is_finite() && v >= 0.0) {
            return Err(EngineError::Param("length must be a finite distance ≥ 0".into()));
        }
        b.length = v;
    }
    if let Some(v) = opt_f32(p, "strength")? {
        if !v.is_finite() || v < 0.0 {
            return Err(EngineError::Param("strength must be a finite weight ≥ 0".into()));
        }
        b.strength = v;
    }
    if let Some(v) = opt_bool(p, "hidden")? {
        b.hidden = v;
    }
    if let Some(v) = opt_bool(p, "constraints")? {
        b.constraints = v;
    }
    if let Some(v) = opt_f32(p, "min")? {
        b.min_constraint = v;
    }
    if let Some(v) = opt_f32(p, "max")? {
        b.max_constraint = v;
    }
    if let Some((x, y)) = opt_vec2(p, "offset")? {
        b.offset = Vec2::new(x, y);
    }
    sk.sanitize();
    Ok(json!({"bone": bone.0}))
}

fn key(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let bone = read_bone(p)?;
    let field = read_channel(p)?;
    let frame = opt_u64(p, "frame")?.unwrap_or(s.frame as u64).min(anicraft_doc::MAX_CHANNEL_FRAME as u64) as u32;
    let sk = skeleton_mut(s, rig)?;
    let Some(b) = sk.bones.get_mut(bone.0 as usize) else { return Err(EngineError::Param(format!("bone {} is not a bone of this rig", bone.0))) };
    match field {
        BoneField::Angle => {
            let v = opt_f32(p, "value")?.ok_or_else(|| EngineError::Param("value (radians) is required".into()))?;
            if v.is_finite() {
                b.angle.set_key(frame, v);
            }
        }
        BoneField::Pos => {
            let (x, y) = opt_vec2(p, "value")?.ok_or_else(|| EngineError::Param("value {x, y} is required".into()))?;
            if x.is_finite() && y.is_finite() {
                b.pos.set_key(frame, Vec2::new(x, y));
            }
        }
        BoneField::Scale => {
            let v = opt_f32(p, "value")?.ok_or_else(|| EngineError::Param("value (a factor) is required".into()))?;
            if v.is_finite() && v > 0.0 {
                b.scale.set_key(frame, v);
            }
        }
        BoneField::Target => {
            let v = opt_f32(p, "value")?.ok_or_else(|| EngineError::Param("value (a bone index, or −1 for none) is required".into()))?;
            if v.is_finite() {
                b.target.set_key(frame, v);
            }
        }
    }
    sk.sanitize();
    Ok(json!({"bone": bone.0, "frame": frame}))
}

fn clear_channel(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let bone = read_bone(p)?;
    let field = read_channel(p)?;
    let sk = skeleton_mut(s, rig)?;
    let Some(b) = sk.bones.get_mut(bone.0 as usize) else { return Err(EngineError::Param(format!("bone {} is not a bone of this rig", bone.0))) };
    let cleared = match field {
        BoneField::Angle => std::mem::take(&mut b.angle).keys.is_empty(),
        BoneField::Pos => std::mem::take(&mut b.pos).keys.is_empty(),
        BoneField::Scale => std::mem::take(&mut b.scale).keys.is_empty(),
        BoneField::Target => std::mem::take(&mut b.target).keys.is_empty(),
    };
    Ok(json!({"bone": bone.0, "cleared": !cleared}))
}

fn bind(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let sk = skeleton(s, rig)?;
    let n = sk.bones.len();
    // One `bone` shorthand binds at full weight; `bones` names weighted pairs.
    let mut pairs: Vec<(BoneId, f32)> = Vec::new();
    if let Some(bone) = opt_u64(p, "bone")? {
        pairs.push((BoneId(u32::try_from(bone).map_err(|_| EngineError::Param("bone index out of range".into()))?), 1.0));
    } else if let Some(list) = p.get("bones").and_then(|v| v.as_array()) {
        for e in list {
            let bone = e.get("bone").and_then(|v| v.as_u64()).ok_or_else(|| EngineError::Param("each bones[] entry needs a bone index".into()))?;
            let bone = BoneId(u32::try_from(bone).map_err(|_| EngineError::Param("bone index out of range".into()))?);
            if bone.0 as usize >= n {
                return Err(EngineError::Param(format!("bone {} is not a bone of this rig ({n} bones)", bone.0)));
            }
            let weight = e.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0);
            if !weight.is_finite() || weight < 0.0 {
                return Err(EngineError::Param("weights must be finite and ≥ 0".into()));
            }
            pairs.push((bone, weight as f32));
        }
    } else {
        return Err(EngineError::Param("give a bone (index) or bones: [{bone, weight}…]".into()));
    }
    if pairs.is_empty() {
        return Err(EngineError::Param("bind at least one bone".into()));
    }
    let total: f32 = pairs.iter().map(|(_, w)| *w).sum();
    if total <= 1e-6 {
        return Err(EngineError::Param("the weights sum to zero".into()));
    }
    for (_, w) in &mut pairs {
        *w /= total;
    }
    let elements: Vec<anicraft_doc::ElementId> = match opt_u64_list(p, "element")? {
        Some(list) => list.into_iter().map(anicraft_doc::ElementId).collect(),
        None => s.selection.clone(),
    };
    if elements.is_empty() {
        return Err(EngineError::State("select a shape (or name element)".into()));
    }
    let mut bound = 0usize;
    let tl = s.doc.timeline_mut(s.scene);
    for lid in rig_layer_and_children(tl, rig) {
        let Some(layer) = tl.layer_mut(lid) else { continue };
        for k in &mut layer.keyframes {
            let k = Arc::make_mut(k);
            for el in &mut k.elements {
                if !elements.contains(&el.id) {
                    continue;
                }
                let el = Arc::make_mut(el);
                if let anicraft_doc::ElementKind::Shape(shape) = &mut el.kind {
                    shape.bind = pairs.clone();
                    bound += 1;
                }
            }
        }
    }
    if bound == 0 {
        return Err(EngineError::State("no shape among the selection to bind".into()));
    }
    Ok(json!({"bound": bound}))
}

fn unbind(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let elements: Vec<anicraft_doc::ElementId> = match opt_u64_list(p, "element")? {
        Some(list) => list.into_iter().map(anicraft_doc::ElementId).collect(),
        None => s.selection.clone(),
    };
    if elements.is_empty() {
        return Err(EngineError::State("select a shape (or name element)".into()));
    }
    let mut freed = 0usize;
    let tl = s.doc.timeline_mut(s.scene);
    for lid in rig_layer_and_children(tl, rig) {
        let Some(layer) = tl.layer_mut(lid) else { continue };
        for k in &mut layer.keyframes {
            let k = Arc::make_mut(k);
            for el in &mut k.elements {
                if !elements.contains(&el.id) {
                    continue;
                }
                let el = Arc::make_mut(el);
                if let anicraft_doc::ElementKind::Shape(shape) = &mut el.kind
                    && !shape.bind.is_empty()
                {
                    shape.bind.clear();
                    freed += 1;
                }
            }
        }
    }
    Ok(json!({"unbound": freed}))
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let rig = rig_of(s, opt_u64(p, "layer")?)?;
    let sk = skeleton(s, rig)?;
    let frame = opt_u64(p, "frame")?.unwrap_or(s.frame as u64) as u32;
    let mats = sk.matrices(frame);
    let bones: Vec<Value> = sk
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let m = mats.get(i).copied().unwrap_or(Affine::IDENTITY);
            let t = m.translation();
            json!({
                "bone": i,
                "name": b.name,
                "parent": b.parent.map(|x| x.0),
                "length": b.length,
                "hidden": b.hidden,
                "angle": b.angle.sample_or(frame as f32, 0.0),
                "pos": {"x": b.pos.sample_or(frame as f32, Vec2::ZERO).x, "y": b.pos.sample_or(frame as f32, Vec2::ZERO).y},
                "scale": b.scale.sample_or(frame as f32, 1.0),
                "target": b.target.sample_or(frame as f32, -1.0),
                "actions": b.angle.actions.iter().map(|a: &BoneAction<f32>| a.name.as_str()).chain(b.pos.actions.iter().map(|a| a.name.as_str())).collect::<Vec<_>>(),
                "x": t.x,
                "y": t.y,
            })
        })
        .collect();
    Ok(json!({"layer": rig.0, "bones": bones, "binding_mode": sk.binding_mode}))
}

/// The rig layer itself plus its whole run of children (what `group_range` names).
fn rig_layer_and_children(tl: &mut anicraft_doc::Timeline, rig: LayerId) -> Vec<LayerId> {
    let Some(i) = tl.layers.iter().position(|l| l.id == rig) else { return Vec::new() };
    tl.group_range(i).filter_map(|j| tl.layers.get(j).map(|l| l.id)).collect()
}

/// Renumbers every binding after `bone` left the table (a lone bind unbinds; a weighted
/// bind drops the pair, shifts the rest down and renormalises).
fn unbind_bone_everywhere(s: &mut Session, rig: LayerId, bone: BoneId) {
    let tl = s.doc.timeline_mut(s.scene);
    for lid in rig_layer_and_children(tl, rig) {
        let Some(layer) = tl.layer_mut(lid) else { continue };
        for k in &mut layer.keyframes {
            let k = Arc::make_mut(k);
            for el in &mut k.elements {
                let el = Arc::make_mut(el);
                if let anicraft_doc::ElementKind::Shape(shape) = &mut el.kind
                    && shape.bind.iter().any(|(b, _)| *b == bone)
                {
                    shape.bind.retain(|(b, _)| *b != bone);
                    for (b, _) in &mut shape.bind {
                        if b.0 > bone.0 {
                            b.0 -= 1;
                        }
                    }
                    let total: f32 = shape.bind.iter().map(|(_, w)| *w).sum();
                    if total <= 1e-6 {
                        shape.bind.clear();
                    } else {
                        for (_, w) in &mut shape.bind {
                            *w /= total;
                        }
                    }
                }
            }
        }
    }
}
