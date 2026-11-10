//! Skeletons and bones (ADR 0104): Moho's rig, on the property-channel foundation (ADR 0102).
//!
//! A skeleton is a flat table of bones addressed by index — the addressing Moho's public
//! scripting API exposes (`M_Skeleton:Bone(id)`, `fParent` ints), and the addressing its
//! files serialize. Bones never enter the [`crate::DisplayList`]: evaluation computes each
//! bone's world [`Affine`] per frame (pure, no state), and skinned artwork is drawn through
//! those matrices. The chain math follows Moho's bone model: a bone extends along its local
//! +x axis by `length`; a root bone sits in skeleton space at `pos`/`angle`; a child bone's
//! `pos` is measured in its parent's frame, origin at the parent's *base* (corpus:
//! CharWalk's Body hangs at CG's base with pos (0, 0), and children drawn at the tip store
//! pos.x = the parent's length), and its `angle` relative to the parent's direction.
//!
//! Units are the stage's (ADR 0102): pixels, screen-clockwise radians, y down. Import
//! converts Moho's inches and y-up CCW radians once, at the same conjugation every other
//! channel uses, so evaluation never converts.
//!
//! Inverse kinematics (Moho's `target_bone`): a bone whose target is set swings — with its
//! parent, a two-bone chain — so its tip reaches the target bone's tip (the analytic
//! two-circle solve through the child's joint offset; lengths hold, unreachable targets
//! clamp at full stretch). Passes repeat at most `MAX_IK_PASSES` times and every walk is
//! bounded, so a target cycle just
//! settles. Chains through scaled or flipped bones don't solve (rare, and the frame
//! arithmetic stops being a rotation) — the FK pose stands.
//!
//! Smart-bone actions (`actions[]` inside a bone channel, ADR 0104) are matched by name
//! across the skeleton: the pose whose values run along an *angle* channel is the driver —
//! its values are the dial angles each pose frame answers to — and every channel carrying
//! an action of the same name blends its pose in, linearly, at the frame the driver value
//! maps to. Moho's exact dial semantics are not public; this model is the documented
//! reading, the sidecar keeps the original, and the corpus gates calibrate it.

use crate::channel::{Channel, ChannelValue};
use anicraft_geom::{Affine, Vec2};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Most bones one skeleton holds (files and commands alike).
pub const MAX_BONES: usize = 2_048;

/// How many times IK solves sweep the skeleton before settling (chain-on-chain rigs).
const MAX_IK_PASSES: usize = 8;

/// Index of a bone in its skeleton's [`Skeleton::bones`] table. Moho addresses bones by
/// this integer everywhere (parents, targets, point bindings); `-1` in a file means none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BoneId(pub u32);

/// One bone of a skeleton: Moho's `Bone` object, minimal editable subset (ADR 0104).
/// Everything else the file carries stays in [`Bone::extra`] and round-trips untouched.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    /// The bone this one hangs from (`None` = root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<BoneId>,
    /// Length along the bone's own +x axis (stage pixels).
    #[serde(default)]
    pub length: f32,
    /// How strongly bound points follow this bone relative to its neighbours (Moho's
    /// `strength`, used by weighted binding).
    #[serde(default = "one_f")]
    pub strength: f32,
    /// The bone's base position over time: a root bone sits at it in skeleton space; a
    /// child bone offsets from its parent's *base*, in the parent's rotated frame (a
    /// child drawn at the parent's tip stores pos.x = the parent's length).
    #[serde(default, skip_serializing_if = "Channel::is_empty")]
    pub pos: Channel<Vec2>,
    /// The bone's direction over time, screen-clockwise radians; a child bone's angle is
    /// relative to its parent's.
    #[serde(default, skip_serializing_if = "Channel::is_empty")]
    pub angle: Channel<f32>,
    /// Uniform scale over time (Moho's `anim_scale` — Moho bone scaling is uniform).
    #[serde(default, skip_serializing_if = "Channel::is_empty")]
    pub scale: Channel<f32>,
    /// The bone whose tip this bone's chain reaches for (IK): the channel holds bone
    /// indices, negative for none (Moho's `target_bone`).
    #[serde(default, skip_serializing_if = "Channel::is_empty")]
    pub target: Channel<f32>,
    /// Clamp [`Bone::angle`] to `[min_constraint, max_constraint]` when set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub constraints: bool,
    #[serde(default)]
    pub min_constraint: f32,
    #[serde(default)]
    pub max_constraint: f32,
    /// The bone is hidden (editing view only — bones draw nothing anyway).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// A static shift of the bone's base, in its parent's frame (Moho's `offset`).
    #[serde(default, skip_serializing_if = "is_zero2")]
    pub offset: Vec2,
    /// Mirror the bone's frame horizontally / vertically over time.
    #[serde(default, skip_serializing_if = "Channel::is_empty")]
    pub flip_h: Channel<bool>,
    #[serde(default, skip_serializing_if = "Channel::is_empty")]
    pub flip_v: Channel<bool>,
    /// Every other key the file carried (Moho's remaining Bone keys — dynamics, control
    /// parents, physics, display), kept verbatim so a save round-trips them (ADR 0104).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

fn one_f() -> f32 {
    1.0
}

fn is_zero2(v: &Vec2) -> bool {
    *v == Vec2::ZERO
}

/// A skeleton on a bone layer: the bone table plus Moho's `binding_mode`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    #[serde(default)]
    pub bones: Vec<Bone>,
    /// Moho's `skeleton.binding_mode` (how Moho auto-binds artwork; recorded, not applied).
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub binding_mode: i32,
    /// Moho's `skeleton.type` and `bones_groups[]`, verbatim (bone groups ride in the
    /// sidecar until the live fixture says otherwise).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

fn is_zero_i(v: &i32) -> bool {
    *v == 0
}

/// One bone's sampled pose (channels and actions resolved, IK not yet applied).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    pos: Vec2,
    angle: f32,
    scale: f32,
    flip_h: bool,
    flip_v: bool,
}

impl Default for Pose {
    fn default() -> Self {
        Pose { pos: Vec2::ZERO, angle: 0.0, scale: 1.0, flip_h: false, flip_v: false }
    }
}

/// One solved IK chain: the parent and child bones whose angles a target overrides.
struct IkChain {
    parent: usize,
    child: usize,
    target: usize,
}

impl Skeleton {
    /// The bone at `id`, or `None` past the table.
    pub fn bone(&self, id: BoneId) -> Option<&Bone> {
        self.bones.get(id.0 as usize)
    }

    /// Whether anything here holds bones (a plain folder with no skeleton is no rig).
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// Repairs a skeleton read from a file or command: bounds, cycles, finite values.
    /// Parents pointing outside the table (or closing a cycle) drop to `None`; targets
    /// stay as channel data (the solver checks them each frame).
    pub fn sanitize(&mut self) {
        self.bones.truncate(MAX_BONES);
        let n = self.bones.len() as u32;
        for b in &mut self.bones {
            b.name = b.name.chars().take(256).collect();
            if let Some(p) = b.parent
                && p.0 >= n
            {
                b.parent = None;
            }
            b.length = finite_or(b.length, 0.0).max(0.0);
            b.strength = finite_or(b.strength, 1.0).max(0.0);
            b.pos.sanitize();
            b.angle.sanitize();
            b.scale.sanitize();
            b.target.sanitize();
            b.flip_h.sanitize();
            b.flip_v.sanitize();
            if !b.min_constraint.is_finite() {
                b.min_constraint = 0.0;
            }
            if !b.max_constraint.is_finite() {
                b.max_constraint = 0.0;
            }
            b.offset = Vec2::new(finite_or(b.offset.x, 0.0), finite_or(b.offset.y, 0.0));
        }
        for i in 0..self.bones.len() {
            if self.chain_cycles(BoneId(i as u32))
                && let Some(b) = self.bones.get_mut(i)
            {
                b.parent = None;
            }
        }
    }

    /// Whether following `id`'s parent chain returns to `id` (sanitize breaks these).
    fn chain_cycles(&self, id: BoneId) -> bool {
        let mut cur = id;
        for _ in 0..=self.bones.len() {
            let Some(b) = self.bone(cur) else { return false };
            let Some(p) = b.parent else { return false };
            if p == id {
                return true;
            }
            cur = p;
        }
        true
    }

    /// Each bone's world matrix at `frame`, in skeleton space (the bone layer's own
    /// space), table order, IK and smart-bone actions applied.
    pub fn matrices(&self, frame: u32) -> Vec<Affine> {
        let mut pose = self.poses(frame, true);
        self.solve_ik(frame, &mut pose);
        self.forward(&pose)
    }

    /// The skinning matrices: each bone's pose at `frame` relative to its rest pose at
    /// frame 0 (rest is FK only — artwork binds to the unposed rig, and IK targets that
    /// already reach at rest must not bend it). Identity at the rest frame, so a rig at
    /// rest renders exactly as if unbound.
    pub fn skin_matrices(&self, frame: u32) -> Vec<Affine> {
        let rest = self.forward(&self.poses(0, false));
        let mut pose = self.poses(frame, true);
        self.solve_ik(frame, &mut pose);
        let pose = self.forward(&pose);
        pose.iter()
            .zip(rest.iter())
            .map(|(w, r)| r.inverse().map_or(Affine::IDENTITY, |ri| w.mul(&ri)))
            .collect()
    }

    /// Samples every bone's channels (smart-bone actions blended in when `actions`).
    fn poses(&self, frame: u32, actions: bool) -> Vec<Pose> {
        let f = frame as f32;
        let drivers = if actions { self.drivers() } else { Vec::new() };
        let mut out: Vec<Pose> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let mut p = Pose {
                pos: b.pos.sample_or(f, Vec2::ZERO) + b.offset,
                angle: b.angle.sample_or(f, 0.0),
                scale: finite_or(b.scale.sample_or(f, 1.0), 1.0),
                flip_h: b.flip_h.sample_or(f, false),
                flip_v: b.flip_v.sample_or(f, false),
            };
            if b.constraints && b.min_constraint <= b.max_constraint {
                p.angle = p.angle.clamp(b.min_constraint, b.max_constraint);
            }
            if actions {
                if let Some(v) = blend_channel(&b.angle, &drivers, &self.bones, f, p.angle) {
                    p.angle = v;
                }
                if let Some(v) = blend_channel(&b.pos, &drivers, &self.bones, f, p.pos) {
                    p.pos = v;
                }
                if let Some(v) = blend_channel(&b.scale, &drivers, &self.bones, f, p.scale) {
                    p.scale = finite_or(v, 1.0);
                }
            }
            out.push(p);
        }
        out
    }

    /// The driver mapping per action name: the first action of that name carried on an
    /// *angle* channel (its pose values are the dial angles each pose frame answers to),
    /// with the index of the bone that owns it.
    fn drivers(&self) -> Vec<Driver<'_>> {
        let mut out: Vec<Driver> = Vec::new();
        for (i, b) in self.bones.iter().enumerate() {
            for a in &b.angle.actions {
                if !out.iter().any(|d| d.name == a.name) {
                    out.push(Driver { name: a.name.clone(), action: a, bone: i });
                }
            }
        }
        out
    }

    /// The IK chains: every bone with a positive in-table target and an in-table parent
    /// (neither being the target itself).
    fn chains(&self, frame: u32) -> Vec<IkChain> {
        let f = frame as f32;
        let mut out = Vec::new();
        for (i, b) in self.bones.iter().enumerate() {
            let t = b.target.sample_or(f, -1.0).round();
            if !t.is_finite() || t < 0.0 || t as usize >= self.bones.len() || t as usize == i {
                continue;
            }
            let Some(p) = b.parent else { continue };
            let p = p.0 as usize;
            if p >= self.bones.len() || p == t as usize {
                continue;
            }
            out.push(IkChain { parent: p, child: i, target: t as usize });
        }
        out
    }

    /// Solves every chain's two-bone angles in place, repeating at most `MAX_IK_PASSES`
    /// times so chains aimed at other chains settle.
    ///
    /// In the parent's base frame both links share the parent's base as their pivot: the
    /// child's joint sits at its stored `pos` (CharWalk-style rigs hang children past the
    /// tip), swinging around that base with the parent's angle, and the child extends from
    /// the joint. Solving for the child's tip on the goal tip is then a two-circle
    /// intersection: the joint traces a circle of radius |pos| around the shoulder, and the
    /// tip must land a child-length from the goal.
    fn solve_ik(&self, frame: u32, pose: &mut [Pose]) {
        let chains = self.chains(frame);
        if chains.is_empty() {
            return;
        }
        for _ in 0..MAX_IK_PASSES {
            let mats = self.forward(pose);
            let mut changed = false;
            for c in &chains {
                let (Some(a), Some(b)) = (self.bones.get(c.parent), self.bones.get(c.child)) else { continue };
                if b.length <= 1e-6 {
                    continue;
                }
                // The goal tip, in the parent bone's base frame (its world matrix with its
                // own rotation stripped; scaled or flipped chains don't solve).
                let (Some(parent_world), Some(pose_p)) = (mats.get(c.parent).copied(), pose.get(c.parent).copied()) else { continue };
                if (pose_p.scale - 1.0).abs() > 1e-6 || pose_p.flip_h || pose_p.flip_v {
                    continue;
                }
                let Some(pose_c) = pose.get(c.child).copied() else { continue };
                let parent_base = parent_world.mul(&Affine::rotate(-pose_p.angle));
                let goal = mats.get(c.target).copied().zip(self.bones.get(c.target)).map(|(m, tb)| m.apply(Vec2::new(tb.length, 0.0)));
                let Some(goal) = goal.filter(|g| g.is_finite()) else { continue };
                let local = parent_base.inverse().map_or(goal, |inv| inv.apply(goal));
                let d = local.length();
                if d <= 1e-6 {
                    continue;
                }
                // The joint circle: the stored joint (child base) sits at angle `phi0` in
                // the parent's frame at radius d1; the parent's swing ψ aims it. Radius 0
                // degenerates — only the child aims.
                let d1 = pose_c.pos.length();
                let phi0 = pose_c.pos.y.atan2(pose_c.pos.x);
                let beta = local.y.atan2(local.x);
                let clamp = |bone: &Bone, v: f32| {
                    if bone.constraints && bone.min_constraint <= bone.max_constraint {
                        v.clamp(bone.min_constraint, bone.max_constraint)
                    } else {
                        v
                    }
                };
                let (want, child_angle) = if d1 <= 1e-6 {
                    // The child pivots on the parent's base itself: aim it straight at the
                    // goal (the parent's swing is unconstrained — leave it).
                    (pose_p.angle, beta - pose_p.angle)
                } else {
                    // Two-circle intersection: the joint's foot x along the shoulder→goal
                    // line, its height ±y off the line.
                    let bl = b.length;
                    let x = ((d1 * d1 + d * d - bl * bl) / (2.0 * d)).clamp(-d1, d1);
                    let y2 = d1 * d1 - x * x;
                    let (psi, joint) = if y2 > 1e-6 {
                        // Bend toward the current pose: the swing side that moves it least.
                        let y = y2.sqrt();
                        let cur = pose_p.angle + phi0 - beta;
                        let pick = if (cur - y.atan2(x)).abs() <= (cur - (-y).atan2(x)).abs() { y } else { -y };
                        let psi = beta + pick.atan2(x);
                        (psi, Vec2::new(d1 * psi.cos(), d1 * psi.sin()))
                    } else {
                        // Out of reach: full stretch along the goal's direction.
                        (beta, Vec2::new(d1 * beta.cos(), d1 * beta.sin()))
                    };
                    // The parent's angle aims the stored joint at ψ; the child aims from
                    // its solved joint at the goal.
                    let want = psi - phi0;
                    let child_world = (local.y - joint.y).atan2(local.x - joint.x);
                    (want, child_world - want)
                };
                let want = clamp(a, want);
                let child_angle = clamp(b, child_angle);
                let cur = pose_p.angle;
                let cur_child = pose_c.angle;
                if (cur - want).abs() > 1e-4 || (cur_child - child_angle).abs() > 1e-4 {
                    if let Some(p) = pose.get_mut(c.parent) {
                        p.angle = want;
                    }
                    if let Some(p) = pose.get_mut(c.child) {
                        p.angle = child_angle;
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// World matrices from sampled poses: a root bone sits at its local matrix in skeleton
    /// space; a child hangs off its parent's tip. Bones resolve in table order, with a
    /// parent named later in the table computed on demand (sanitize removed cycles).
    fn forward(&self, pose: &[Pose]) -> Vec<Affine> {
        let mut mats: Vec<Option<Affine>> = vec![None; pose.len()];
        for i in 0..pose.len() {
            self.world_at(i, pose, &mut mats, 0);
        }
        mats.into_iter().map(|m| m.unwrap_or(Affine::IDENTITY)).collect()
    }

    fn world_at(&self, i: usize, pose: &[Pose], mats: &mut [Option<Affine>], depth: usize) -> Option<Affine> {
        if depth > MAX_BONES {
            return None;
        }
        if let Some(m) = mats.get(i).and_then(|m| *m) {
            return Some(m);
        }
        let b = self.bones.get(i)?;
        let local = local_matrix(pose.get(i)?);
        let world = match b.parent {
            Some(p) if (p.0 as usize) < pose.len() => {
                // A child's pose is in its parent's frame, origin at the parent's *base*
                // (corpus: CharWalk's Body hangs at CG's base pos=(0,0); a child at the
                // parent's tip stores pos.x = parent length; sketch_bones.lua maps the
                // drawn base point by the parent rest matrix inverse) — so the parent's
                // world matrix composes straight onto the child's local matrix.
                let pw = self.world_at(p.0 as usize, pose, mats, depth + 1)?;
                pw.mul(&local)
            }
            // A parent outside the table (a leftover link commands may build): root.
            _ => local,
        };
        let world = if world.is_finite() { world } else { Affine::IDENTITY };
        if let Some(slot) = mats.get_mut(i) {
            *slot = Some(world);
        }
        Some(world)
    }
}

/// A driver mapping found on an angle channel: the action and the bone that owns it.
struct Driver<'a> {
    name: String,
    action: &'a Action<f32>,
    bone: usize,
}

/// One bone's local matrix from its pose: position (offset included), angle, uniform
/// scale, flips. Identity when everything sits at rest.
fn local_matrix(p: &Pose) -> Affine {
    let (sx, sy) = if p.flip_h {
        (-p.scale, p.scale)
    } else if p.flip_v {
        (p.scale, -p.scale)
    } else {
        (p.scale, p.scale)
    };
    if sx == 1.0 && sy == 1.0 && p.angle == 0.0 && p.pos == Vec2::ZERO {
        return Affine::IDENTITY;
    }
    Affine::translate(p.pos.x, p.pos.y).mul(&Affine::rotate(p.angle)).mul(&Affine::scale(sx, sy))
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        d
    }
}

/// Blends the smart-bone actions on `ch` over the base value: each action of a driven name
/// weighs by the driver bone's base angle at this frame, samples its pose at the mapped
/// pose frame, and the poses average; the average then blends over the base linearly.
fn blend_channel<T: ChannelValue>(ch: &Channel<T>, drivers: &[Driver], bones: &[Bone], f: f32, base: T) -> Option<T> {
    if ch.actions.is_empty() {
        return None;
    }
    let mut acc: Option<(T, f32)> = None;
    let mut wsum = 0.0_f32;
    for a in &ch.actions {
        let Some(d) = drivers.iter().find(|d| d.name == a.name) else { continue };
        let driver_angle = bones.get(d.bone)?.angle.sample_or(f, 0.0);
        let w = d.action.weight(driver_angle);
        if w <= 0.0 {
            continue;
        }
        let Some(pf) = d.action.pose_frame(driver_angle) else { continue };
        let Some(v) = a.pose.sample(pf) else { continue };
        acc = match acc {
            None => Some((v, w)),
            Some((old, ow)) => {
                let f = w / (ow + w).max(1e-9);
                Some((T::from_components(&old, &v, |_| f), ow + w))
            }
        };
        wsum += w;
    }
    let (pose_v, _) = acc?;
    let w = wsum.clamp(0.0, 1.0);
    Some(T::from_components(&base, &pose_v, |_| w))
}

/// A smart-bone action a [`Channel`] may carry: a named alternate timeline (`pose`) for
/// the same property (ADR 0104). Actions match by name across a skeleton; the one riding
/// an *angle* channel drives the rest (see the module header).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action<T> {
    pub name: String,
    /// The pose timeline, keyed by pose frame (`pose.when` straight from the file).
    #[serde(default)]
    pub pose: Channel<T>,
}

impl<T: ChannelValue> Action<T> {
    /// The pose frame the driver value `driver` sits at, inverted linearly through the
    /// pose's own key values (`None` when the pose has no usable value span).
    pub fn pose_frame(&self, driver: f32) -> Option<f32> {
        let keys = &self.pose.keys;
        let first = keys.first()?;
        let last = keys.last()?;
        let (v0, v1) = (first.val.component(0), last.val.component(0));
        if !v0.is_finite() || !v1.is_finite() || (v1 - v0).abs() < 1e-9 {
            return None;
        }
        let t = ((driver - v0) / (v1 - v0)).clamp(0.0, 1.0);
        let (f0, f1) = (first.frame as f32, last.frame as f32);
        Some(f0 + (f1 - f0) * t)
    }

    /// The blend weight of this action at driver value `driver`: 1 inside the pose's value
    /// span, feathered linearly to 0 across 10% of the span outside either end.
    pub fn weight(&self, driver: f32) -> f32 {
        let keys = &self.pose.keys;
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 0.0 };
        let (v0, v1) = (first.val.component(0), last.val.component(0));
        if !driver.is_finite() || !v0.is_finite() || !v1.is_finite() || (v1 - v0).abs() < 1e-9 {
            return 0.0;
        }
        let (lo, hi) = if v0 <= v1 { (v0, v1) } else { (v1, v0) };
        if driver >= lo && driver <= hi {
            return 1.0;
        }
        let feather = (hi - lo) * 0.1;
        if feather <= 0.0 {
            return 0.0;
        }
        if driver < lo {
            ((driver - (lo - feather)) / feather).clamp(0.0, 1.0)
        } else {
            (1.0 - (driver - hi) / feather).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn bone(name: &str, parent: Option<BoneId>, length: f32) -> Bone {
        Bone { name: name.into(), parent, length, strength: 1.0, ..Bone::default() }
    }

    #[test]
    fn root_sits_at_its_channels() {
        let mut b = bone("root", None, 100.0);
        b.pos.set_key(0, Vec2::new(10.0, 20.0));
        b.angle.set_key(0, FRAC_PI_2);
        let sk = Skeleton { bones: vec![b], ..Skeleton::default() };
        let m = &sk.matrices(0)[0];
        // Rotated a quarter turn (screen y down): the tip points down.
        let tip = m.apply(Vec2::new(100.0, 0.0));
        assert!((tip.x - 10.0).abs() < 1e-4 && (tip.y - 120.0).abs() < 1e-4, "{tip:?}");
    }

    #[test]
    fn child_pos_is_in_the_parents_base_frame() {
        // Moho's convention (CharWalk): a child drawn at the parent's tip stores
        // pos.x = the parent's length; a child at pos (0, 0) shares the parent's base.
        let mut parent = bone("a", None, 100.0);
        parent.angle.set_key(0, 0.0);
        let mut child = bone("b", Some(BoneId(0)), 50.0);
        child.pos.set_key(0, Vec2::new(100.0, 0.0));
        child.angle.set_key(0, FRAC_PI_2);
        let sk = Skeleton { bones: vec![parent.clone(), child], ..Skeleton::default() };
        let m = sk.matrices(0);
        // Child base at the parent's tip (100, 0), turning down from there.
        let child_base = m[1].apply(Vec2::ZERO);
        assert!((child_base.x - 100.0).abs() < 1e-4 && child_base.y.abs() < 1e-4, "{child_base:?}");
        let child_tip = m[1].apply(Vec2::new(50.0, 0.0));
        assert!((child_tip.x - 100.0).abs() < 1e-4 && (child_tip.y - 50.0).abs() < 1e-4, "{child_tip:?}");
        // A zero-pos child sits on the parent's base (CharWalk's Body on CG).
        let mut at_base = bone("c", Some(BoneId(0)), 50.0);
        at_base.angle.set_key(0, FRAC_PI_2);
        let sk = Skeleton { bones: vec![parent.clone(), at_base], ..Skeleton::default() };
        let base = sk.matrices(0)[1].apply(Vec2::ZERO);
        assert!(base.length() < 1e-4, "{base:?}");
    }

    #[test]
    fn parent_rotation_carries_children() {
        let mut parent = bone("a", None, 100.0);
        parent.angle.set_key(0, FRAC_PI_2);
        let mut child = bone("b", Some(BoneId(0)), 50.0);
        child.pos.set_key(0, Vec2::new(100.0, 0.0));
        let sk = Skeleton { bones: vec![parent, child], ..Skeleton::default() };
        let m = sk.matrices(0);
        let tip = m[1].apply(Vec2::new(50.0, 0.0));
        // Whole chain turned: the child's tip hangs off the (0,100) tip, further down.
        assert!(tip.x.abs() < 1e-4 && (tip.y - 150.0).abs() < 1e-4, "{tip:?}");
    }

    #[test]
    fn scale_and_length_compose() {
        let mut parent = bone("a", None, 100.0);
        parent.scale.set_key(0, 2.0);
        let mut child = bone("b", Some(BoneId(0)), 50.0);
        child.pos.set_key(0, Vec2::new(100.0, 0.0));
        let sk = Skeleton { bones: vec![parent, child], ..Skeleton::default() };
        let m = sk.matrices(0);
        // The parent's doubled scale carries the child's joint twice as far, and the
        // child's own frame scales with the chain (its 50 renders 100 long).
        let base = m[1].apply(Vec2::ZERO);
        assert!((base.x - 200.0).abs() < 1e-4, "{base:?}");
        let tip = m[1].apply(Vec2::new(50.0, 0.0));
        assert!((tip.x - 300.0).abs() < 1e-4 && tip.y.abs() < 1e-4, "{tip:?}");
    }

    #[test]
    fn constraints_clamp_the_angle() {
        let mut b = bone("c", None, 10.0);
        b.constraints = true;
        b.min_constraint = -0.5;
        b.max_constraint = 0.5;
        b.angle.set_key(0, 3.0);
        let sk = Skeleton { bones: vec![b], ..Skeleton::default() };
        let m = &sk.matrices(0)[0];
        let tip = m.apply(Vec2::new(10.0, 0.0));
        assert!((tip.x - 10.0 * 0.87758).abs() < 1e-3 && (tip.y - 10.0 * 0.47943).abs() < 1e-3, "{tip:?}");
    }

    #[test]
    fn skin_is_identity_at_rest_and_follows_the_pose() {
        let mut b = bone("a", None, 100.0);
        b.angle.set_key(0, 0.0);
        b.angle.set_key(12, FRAC_PI_2);
        let sk = Skeleton { bones: vec![b], ..Skeleton::default() };
        assert_eq!(sk.skin_matrices(0), vec![Affine::IDENTITY]);
        let f12 = sk.skin_matrices(12);
        let p = f12[0].apply(Vec2::new(100.0, 0.0));
        assert!(p.x.abs() < 1e-4 && (p.y - 100.0).abs() < 1e-4, "{p:?}");
    }

    #[test]
    fn skin_at_rest_survives_a_target_defined_at_rest() {
        // The IK target sits where the chain already reaches at frame 0: the rest skin
        // must stay identity (the rig was bound in this pose).
        let mut upper = bone("upper", None, 80.0);
        upper.angle.set_key(0, 0.0);
        let mut fore = bone("fore", Some(BoneId(0)), 60.0);
        fore.pos.set_key(0, Vec2::new(80.0, 0.0));
        fore.target.set_key(0, 2.0);
        let mut goal = bone("goal", None, 10.0);
        goal.pos.set_key(0, Vec2::new(130.0, 0.0));
        let sk = Skeleton { bones: vec![upper, fore, goal], ..Skeleton::default() };
        assert_eq!(sk.skin_matrices(0), vec![Affine::IDENTITY; 3]);
    }

    #[test]
    fn two_bone_ik_reaches_a_close_target() {
        let mut upper = bone("upper", None, 80.0);
        upper.angle.set_key(0, 0.0);
        let mut fore = bone("fore", Some(BoneId(0)), 60.0);
        fore.pos.set_key(0, Vec2::new(80.0, 0.0));
        fore.target.set_key(0, 2.0);
        let mut goal = bone("goal", None, 20.0);
        goal.pos.set_key(0, Vec2::new(100.0, -40.0));
        let sk = Skeleton { bones: vec![upper, fore, goal], ..Skeleton::default() };
        let m = sk.matrices(0);
        let tip = m[1].apply(Vec2::new(60.0, 0.0));
        // The chain reaches the goal bone's TIP: its base (100, −40) plus 20 along it.
        let want = Vec2::new(120.0, -40.0);
        assert!((tip - want).length() < 0.5, "tip {tip:?} vs goal {want:?}");
        // The shoulder stays planted and lengths hold.
        let base = m[1].apply(Vec2::ZERO);
        assert!(base.length() > 79.0 && base.length() < 81.0, "upper stretched? {base:?}");
    }

    #[test]
    fn two_bone_ik_clamps_an_unreachable_target() {
        let mut upper = bone("upper", None, 50.0);
        upper.angle.set_key(0, 0.0);
        let mut fore = bone("fore", Some(BoneId(0)), 40.0);
        fore.pos.set_key(0, Vec2::new(50.0, 0.0));
        fore.target.set_key(0, 2.0);
        let mut goal = bone("goal", None, 10.0);
        goal.pos.set_key(0, Vec2::new(1000.0, 0.0));
        let sk = Skeleton { bones: vec![upper, fore, goal], ..Skeleton::default() };
        let m = sk.matrices(0);
        // Full stretch along +x: the tip lands at 90.
        let tip = m[1].apply(Vec2::new(40.0, 0.0));
        assert!((tip.x - 90.0).abs() < 0.5 && tip.y.abs() < 0.5, "{tip:?}");
    }

    #[test]
    fn ik_moves_descendants_with_the_chain() {
        let mut upper = bone("upper", None, 80.0);
        upper.angle.set_key(0, 0.0);
        let mut fore = bone("fore", Some(BoneId(0)), 60.0);
        fore.pos.set_key(0, Vec2::new(80.0, 0.0));
        fore.target.set_key(0, 2.0);
        let mut hand = bone("hand", Some(BoneId(1)), 20.0);
        hand.pos.set_key(0, Vec2::new(60.0, 0.0));
        hand.angle.set_key(0, FRAC_PI_2);
        let mut goal = bone("goal", None, 10.0);
        goal.pos.set_key(0, Vec2::new(100.0, -40.0));
        let sk = Skeleton { bones: vec![upper, fore, hand, goal], ..Skeleton::default() };
        let m = sk.matrices(0);
        // The solved forearm carries the hand: its tip sits 20 px off the forearm tip,
        // turned a quarter turn relative to it.
        let fore_tip = m[1].apply(Vec2::new(60.0, 0.0));
        let hand_tip = m[2].apply(Vec2::new(20.0, 0.0));
        let off = hand_tip - fore_tip;
        assert!((off.length() - 20.0).abs() < 0.5, "{off:?}");
    }

    #[test]
    fn sanitize_drops_cycles_and_bad_parents() {
        let mut a = bone("a", Some(BoneId(1)), 10.0);
        a.target.set_key(0, 1.0);
        let mut b = bone("b", Some(BoneId(0)), 10.0);
        b.target.set_key(0, 99.0);
        let mut sk = Skeleton { bones: vec![a, b], ..Skeleton::default() };
        sk.sanitize();
        assert_eq!(sk.bones[0].parent, None, "the cycle breaks at the first link");
        assert_eq!(sk.bones[1].parent, Some(BoneId(0)));
        // Out-of-range targets stay as data; the solver just ignores them.
        assert_eq!(sk.bones[1].target.sample_or(0.0, -1.0), 99.0);
        // And the matrices stay finite (no panic, no hang).
        assert!(sk.matrices(0).iter().all(|m| m.is_finite()));
    }

    #[test]
    fn actions_blend_by_the_driver_angle() {
        // A dial (root, angle channel carrying the action) and a follower whose angle pose
        // is named the same: turning the dial to 90° drives the follower's pose key at
        // frame 12 (its angle 90°).
        let quarter = std::f32::consts::FRAC_PI_2;
        let mut dial = bone("dial", None, 40.0);
        let mut dial_angle = Channel::default();
        let mut dial_pose = Channel::default();
        dial_pose.set_key(0, 0.0);
        dial_pose.set_key(24, quarter);
        dial_angle.actions.push(Action { name: "bend".into(), pose: dial_pose });
        dial_angle.set_key(0, 0.0);
        dial_angle.set_key(24, quarter);
        dial.angle = dial_angle;
        let mut follower = bone("f", Some(BoneId(0)), 30.0);
        let mut f_angle = Channel::default();
        let mut f_pose = Channel::default();
        f_pose.set_key(0, 0.0);
        f_pose.set_key(12, quarter);
        f_angle.actions.push(Action { name: "bend".into(), pose: f_pose });
        follower.angle = f_angle;
        let sk = Skeleton { bones: vec![dial, follower], ..Skeleton::default() };
        // Dial at rest: no blend; the follower hangs at the dial's base pointing +x.
        let m0 = sk.matrices(0);
        let t0 = m0[1].apply(Vec2::new(30.0, 0.0));
        assert!((t0.x - 30.0).abs() < 1e-3 && t0.y.abs() < 1e-3, "follower straight at rest: {t0:?}");
        // Dial turned 90° (frame 24): pose frame 12 on the follower's pose (its 90° key):
        // the follower (still at the dial's base) now points back along −x.
        let m24 = sk.matrices(24);
        let t24 = m24[1].apply(Vec2::new(30.0, 0.0));
        assert!((t24.x + 30.0).abs() < 1e-3 && t24.y.abs() < 1e-3, "follower bent: {t24:?}");
    }

    #[test]
    fn pose_frame_inverts_through_the_pose_keys() {
        let mut pose = Channel::default();
        pose.set_key(0, 0.0);
        pose.set_key(24, 90.0);
        let a: Action<f32> = Action { name: "bend".into(), pose };
        assert_eq!(a.pose_frame(0.0), Some(0.0));
        assert_eq!(a.pose_frame(90.0), Some(24.0));
        assert_eq!(a.pose_frame(45.0), Some(12.0));
        assert_eq!(a.pose_frame(1000.0), Some(24.0), "clamped outside the span");
        assert_eq!(a.weight(45.0), 1.0);
        assert_eq!(a.weight(0.0), 1.0);
        assert_eq!(a.weight(500.0), 0.0, "feathered out beyond the span");
    }
}
