//! Bones tool: see the rig the active layer belongs to, and pose it (ADR 0104).
//!
//! The overlay draws each bone as a line from its base to its tip at the playhead's pose,
//! ringed where it drives IK. Dragging a ringed tip keys the TARGET bone's base so its tip
//! follows the cursor — the two-bone solver in the document model swings the chain with it
//! (Moho's hand-drag). Only view state lives here; every change is a `bone.key` command, so
//! agents and the CLI pose rigs exactly the same way, and one drag is one undo step.

use crate::app::{App, Drag, Tool};

use anicraft_doc::{LayerKind, Skeleton};
use anicraft_geom::Vec2;
use egui::{Color32, CursorIcon, Pos2, Stroke as Line};
use serde_json::json;

/// Pick radius for a bone tip, in screen points.
const PICK: f32 = 9.0;

/// The rig the active layer belongs to: the nearest enclosing bone layer's skeleton.
/// (`None` when the active layer sits outside any rig — the tool then draws nothing.)
pub fn rig(app: &App) -> Option<(anicraft_doc::LayerId, std::sync::Arc<Skeleton>)> {
    let tl = app.s.doc.timeline(app.s.scene);
    let mut lid = app.s.layer;
    for _ in 0..=anicraft_doc::MAX_LAYER_DEPTH {
        let layer = tl.layer(lid)?;
        if layer.kind == LayerKind::Bone {
            let sk = layer.skeleton.clone()?;
            if !sk.is_empty() {
                return Some((layer.id, sk));
            }
            return None;
        }
        lid = layer.parent?;
    }
    None
}

/// One drawable bone: base and tip in stage space, and whether it drives IK.
struct Bone {
    id: usize,
    base: Pos2,
    tip: Pos2,
    /// The bone this one reaches for (its target's index), if any.
    target: Option<usize>,
    /// The bone's own index as somebody else's target (its tip is the dragged thing).
    is_target: bool,
}

/// The rig's bones at the playhead, in stage space.
fn bones(app: &App) -> Vec<Bone> {
    let Some((_rig_id, sk)) = rig(app) else { return Vec::new() };
    let f = app.s.frame;
    let mats = sk.matrices(f);
    let mut out = Vec::new();
    for (i, b) in sk.bones.iter().enumerate() {
        let Some(m) = mats.get(i).copied() else { continue };
        let base = m.apply(Vec2::ZERO);
        let tip = m.apply(Vec2::new(b.length, 0.0));
        let t = b.target.sample_or(f as f32, -1.0).round();
        let target = (t.is_finite() && t >= 0.0 && (t as usize) < sk.bones.len()).then_some(t as usize);
        // Somebody aims at me: I am the moving end of their chain.
        let is_target = sk.bones.iter().any(|other| {
            let v = other.target.sample_or(f as f32, -1.0).round();
            v.is_finite() && v as usize == i && v >= 0.0
        });
        out.push(Bone { id: i, base: Pos2::new(base.x, base.y), tip: Pos2::new(tip.x, tip.y), target, is_target });
    }
    out
}

/// Whether a bone tip takes an IK drag, and which bone owns the drag.
fn grabbable(bones: &[Bone], i: usize) -> Option<usize> {
    let b = bones.get(i)?;
    // Dragging a bone with a target moves its target; dragging the target itself moves it.
    b.target.or(b.is_target.then_some(b.id))
}

/// The closest IK-grabbable tip to the pointer, within the pick radius.
fn hot_tip(app: &App, pointer: Pos2, to_screen: impl Fn(Vec2) -> Pos2) -> Option<(usize, Pos2)> {
    let bones = bones(app);
    let mut best: Option<(f32, usize, Pos2)> = None;
    for b in &bones {
        if grabbable(&bones, b.id).is_none() {
            continue;
        }
        let sp = to_screen(Vec2::new(b.tip.x, b.tip.y));
        let d = (sp - pointer).length();
        if d < PICK && best.is_none_or(|(bd, _, _)| d < bd) {
            best = Some((d, b.id, sp));
        }
    }
    best.map(|(_, id, sp)| (id, sp))
}

/// Cursor over a grabbable tip.
/// Whether the active layer is a Moho bone rig (ADR 0104): the layer itself is a rig, or it
/// sits in one (its artwork binds to that rig). Upstream's armature tool shares the tool slot.
pub(crate) fn on_rig_layer(app: &App) -> bool {
    let tl = app.s.doc.timeline(app.s.scene);
    let mut cur = Some(app.s.layer);
    for _ in 0..=anicraft_doc::MAX_LAYER_DEPTH {
        let Some(id) = cur else { break };
        let Some(l) = tl.layer(id) else { break };
        if l.kind == LayerKind::Bone {
            return true;
        }
        cur = l.parent;
    }
    false
}

pub fn cursor(app: &App, pointer: Option<Pos2>, to_screen: impl Fn(Vec2) -> Pos2 + Copy) -> Option<CursorIcon> {
    if app.tool != Tool::Bone || app.drag != Drag::None {
        return None;
    }
    let p = pointer?;
    hot_tip(app, p, to_screen).map(|_| CursorIcon::Grab)
}

/// Press on a rig tip: start the IK drag (one gesture).
pub fn drag_start(app: &mut App, sp: Pos2, to_screen: impl Fn(Vec2) -> Pos2 + Copy) -> Option<Drag> {
    if app.tool != Tool::Bone {
        return None;
    }
    let (bone, _) = hot_tip(app, sp, to_screen)?;
    app.s.begin_gesture(tr!("Pose Bones"));
    Some(Drag::BoneIk { bone })
}

/// Drag tick: move the dragged bone's target base so its tip rides the cursor, keyed at
/// the playhead (the document's two-bone solver swings the chain with it).
pub fn drag_to(app: &mut App, bone: usize, cursor: Pos2) {
    let cursor = Vec2::new(cursor.x, cursor.y);
    let Some((_rig_id, sk)) = rig(app) else { return };
    let f = app.s.frame;
    let mats = sk.matrices(f);
    let Some(b) = sk.bones.get(bone) else { return };
    let t = b.target.sample_or(f as f32, -1.0).round();
    if !t.is_finite() || t < 0.0 {
        return;
    }
    let t = t as usize;
    let (Some(tm), Some(tb)) = (mats.get(t).copied(), sk.bones.get(t)) else { return };
    // The target tip now, and where the drag wants it.
    let tip = tm.apply(Vec2::new(tb.length, 0.0));
    let delta = cursor - tip;
    // The base moves in the target's parent frame: undo that frame's linear part.
    let parent_lin = match tb.parent {
        Some(p) => mats.get(p.0 as usize).copied().map(|m| anicraft_geom::Affine { m: [m.m[0], m.m[1], m.m[2], m.m[3], 0.0, 0.0] }),
        None => Some(anicraft_geom::Affine::IDENTITY),
    };
    let Some(lin) = parent_lin.and_then(|m| m.inverse()) else { return };
    let d_local = lin.apply_vector(delta);
    let cur = tb.pos.sample_or(f as f32, Vec2::ZERO);
    let next = cur + d_local;
    if !next.x.is_finite() || !next.y.is_finite() {
        return;
    }
    let _ = app.s.execute("bone.key", &json!({"bone": t, "channel": "pos", "value": [next.x, next.y], "frame": f}));
}

/// Release: close the gesture (one undo step for the whole drag).
pub fn drag_end(app: &mut App) {
    if app.s.in_gesture() {
        app.s.end_gesture();
    }
}

/// The bones overlay: lines from base to tip at the playhead's pose, rings on IK tips.
pub fn draw_overlay(app: &mut App, painter: &egui::Painter, to_screen: impl Fn(Vec2) -> Pos2 + Copy, pointer: Option<Pos2>) {
    if app.tool != Tool::Bone || crate::camera_tool::previewing(app) {
        return;
    }
    let Some((_rig_id, _sk)) = rig(app) else { return };
    let bones = bones(app);
    if bones.is_empty() {
        return;
    }
    let hot = pointer.and_then(|p| hot_tip(app, p, to_screen).map(|(id, _)| id));
    for b in &bones {
        let a = to_screen(Vec2::new(b.base.x, b.base.y));
        let z = to_screen(Vec2::new(b.tip.x, b.tip.y));
        let drag = grabbable(&bones, b.id).is_some();
        let colour = if Some(b.id) == hot { Color32::YELLOW } else if drag { Color32::from_rgb(120, 200, 255) } else { Color32::from_rgb(160, 160, 170) };
        painter.line_segment([a, z], Line::new(if Some(b.id) == hot { 3.0 } else { 2.0 }, colour));
        painter.circle_filled(a, 2.5, colour);
        // A ring on the tip that takes an IK drag.
        if drag {
            painter.circle_stroke(z, 5.0, Line::new(1.5, colour));
        }
    }

}
