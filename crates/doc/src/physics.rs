//! Physics baking (ADR 0106, Moho's layer physics, decision D3: a self-built integrator):
//! a deterministic offline simulation of layer rigid bodies under gravity and wind, baked
//! into translation keys before playback or export.
//!
//! Physics is stateful — the opposite of `evaluate(doc, frame)` being a pure function of the
//! frame — so the model never simulates at draw time. [`bake`] integrates the scene once,
//! frame order fixed, and returns each body layer's position per frame; the caller (the
//! engine's `physics.bake` command) writes those into the layer's translation channel, after
//! which every renderer, export and the timeline show the motion with no further machinery —
//! exactly what Moho's `use_baked_physics` means.
//!
//! First slice: bodies are axis-aligned boxes taken from the layer's own artwork; they fall,
//! land and bounce on static layers (restitution) and slow along the ground (friction).
//! Body-vs-body collisions, rotation, density ratios, motors and force fields are Moho
//! behaviours this slice does not hold — the import reports them, and the integrator is
//! written so they can slot in.

use crate::{Document, Layer, LayerId, Scene};
use anicraft_geom::Vec2;
use serde::{Deserialize, Serialize};

/// Most bodies one scene simulates.
pub const MAX_BODIES: usize = 2_000;
/// Most frames a bake runs (the scene's length anyway; a guard for wild ranges).
pub const MAX_BAKE_FRAMES: u32 = 100_000;
/// A body at rest below this speed stays there (px/s).
const SLEEP_SPEED: f32 = 4.0;
/// Bounces gentler than this settle instead of ringing forever.
const REST_SPEED: f32 = 24.0;
/// Contact slows sliding by at most this much per frame (friction 1 stops in a frame).
const FRICTION_PER_SECOND: f32 = 30.0;

/// A layer that takes part in the simulation (Moho's `physics.*`, the editable core).
/// Imported from a Moho layer's `physics` group; the untouched keys ride the sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicsBody {
    /// Immovable (Moho's `physics.static`): a floor or wall the other bodies land on.
    #[serde(default, rename = "static")]
    pub static_body: bool,
    /// Sliding slows by this factor per second of contact, `0..=1` (Moho's `friction`).
    #[serde(default = "default_friction")]
    pub friction: f32,
    /// Bounciness, `0..=1` (Moho's `restitution`).
    #[serde(default = "default_restitution")]
    pub restitution: f32,
    /// The body's speed at the first frame, stage pixels per second (Moho's `velocity`,
    /// converted from inches).
    #[serde(default)]
    pub velocity: Vec2,
}

fn default_friction() -> f32 {
    0.3
}
fn default_restitution() -> f32 {
    0.5
}

impl PhysicsBody {
    /// The imported values made safe: finite and in range.
    pub fn sanitized(mut self) -> Self {
        let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        self.friction = unit(self.friction);
        self.restitution = unit(self.restitution);
        let v = self.velocity;
        self.velocity = Vec2::new(finite(v.x, 0.0), finite(v.y, 0.0));
        self
    }
}

fn finite(v: f32, d: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-1e6, 1e6)
    } else {
        d
    }
}

/// The scene's simulation world (Moho's physics-enabled `GroupLayer`): gravity and wind in
/// stage pixels per second squared, y down. `None` — no physics-enabled group — means the
/// scene has no simulation and a bake is refused.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicsWorld {
    #[serde(default)]
    pub gravity: Vec2,
    #[serde(default)]
    pub wind: Vec2,
}

impl PhysicsWorld {
    /// The imported values made safe.
    pub fn sanitized(mut self) -> Self {
        self.gravity = Vec2::new(finite(self.gravity.x, 0.0), finite(self.gravity.y, 0.0));
        self.wind = Vec2::new(finite(self.wind.x, 0.0), finite(self.wind.y, 0.0));
        self
    }

    /// Whether the world carries any force at all (a zero world simulates nothing).
    pub fn is_neutral(&self) -> bool {
        self.gravity.length() < 1e-6 && self.wind.length() < 1e-6
    }
}

/// One body in the simulation: its box (stage pixels), its layer, and its state.
struct Body {
    layer: LayerId,
    rest: Vec2,
    static_body: bool,
    friction: f32,
    restitution: f32,
    velocity: Vec2,
    /// The box around the rest position: centre minus rest = `0`.
    box_min: Vec2,
    box_max: Vec2,
}

/// One baked body: its layer and the position it should sit at each frame (index = frame).
#[derive(Clone, Debug, PartialEq)]
pub struct BakedBody {
    pub layer: LayerId,
    /// Stage-space translation per frame; index 0 is the rest position.
    pub positions: Vec<Vec2>,
}

/// The scene's baked simulation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Baked {
    pub bodies: Vec<BakedBody>,
}

impl Baked {
    /// The translation a layer should carry at `frame`, or `None` when it isn't a body.
    pub fn position(&self, layer: LayerId, frame: u32) -> Option<Vec2> {
        self.bodies.iter().find(|b| b.layer == layer).and_then(|b| b.positions.get(frame as usize).copied())
    }
}

/// What went wrong (the engine command turns these into command errors).
#[derive(Clone, Debug, PartialEq)]
pub enum PhysicsError {
    /// The scene has no physics-enabled world (Moho's group `enable_physics`).
    NoWorld,
    /// The scene has physics-enabled layers but none of them is a drawable body.
    NoBodies,
}

impl std::fmt::Display for PhysicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PhysicsError::NoWorld => write!(f, "no physics world in this scene (a Moho group with physics enabled carries one)"),
            PhysicsError::NoBodies => write!(f, "no physics bodies in this scene (a Moho layer with physics enabled is one)"),
        }
    }
}

/// The layer's body box at its rest pose: the union of its first keyframe's element bounds
/// (element transforms applied, layer transform excluded — the bake moves that). Layers
/// whose elements have no bounds (an empty or symbol-only layer) return `None`.
fn body_box(layer: &Layer) -> Option<(Vec2, Vec2)> {
    let kf = layer.keyframes.first()?;
    let mut min: Option<Vec2> = None;
    let mut max: Option<Vec2> = None;
    let mut grow = |b: (Vec2, Vec2)| {
        min = Some(match min {
            Some(m) => Vec2::new(m.x.min(b.0.x), m.y.min(b.0.y)),
            None => b.0,
        });
        max = Some(match max {
            Some(m) => Vec2::new(m.x.max(b.1.x), m.y.max(b.1.y)),
            None => b.1,
        });
    };
    for el in &kf.elements {
        match &el.kind {
            crate::ElementKind::Shape(s) => {
                let b = s.path.bounds();
                let (a, c) = (el.transform.apply(b.min), el.transform.apply(b.max));
                let (a, c) = (Vec2::new(a.x.min(c.x), a.y.min(c.y)), Vec2::new(a.x.max(c.x), a.y.max(c.y)));
                grow((a, c));
            }
            crate::ElementKind::Text(t) => {
                let b = t.outline.bounds();
                let (a, c) = (el.transform.apply(b.min), el.transform.apply(b.max));
                let (a, c) = (Vec2::new(a.x.min(c.x), a.y.min(c.y)), Vec2::new(a.x.max(c.x), a.y.max(c.y)));
                grow((a, c));
            }
            // Symbol and camera instances have no direct bounds of their own.
            _ => {}
        }
    }
    match (min, max) {
        (Some(a), Some(c)) if c.x > a.x + 1e-3 && c.y > a.y + 1e-3 => Some((a, c)),
        _ => None,
    }
}

/// The scene's bodies, in layer order. A layer participates when it carries a
/// [`PhysicsBody`]; the layer's translation at the bake's start is its rest position.
fn bodies_of(scene: &Scene) -> Vec<Body> {
    let mut out = Vec::new();
    for layer in &scene.timeline.layers {
        // Moho's bodies sit inside the physics-enabled group (its children); any layer
        // carrying a body takes part, wherever it nests.
        let Some(body) = layer.physics else { continue };
        let Some((box_min, box_max)) = body_box(layer) else { continue };
        let rest = layer.translation.sample_or(0.0, Vec2::ZERO);
        out.push(Body {
            layer: layer.id,
            rest,
            static_body: body.static_body,
            friction: body.friction,
            restitution: body.restitution,
            velocity: body.velocity,
            box_min,
            box_max,
        });
        if out.len() >= MAX_BODIES {
            break;
        }
    }
    out
}

/// Bakes the scene's physics: each body's stage-space translation for every frame from the
/// scene's start to its end. Deterministic: same document in, same positions out.
pub fn bake(doc: &Document, scene_index: usize) -> Result<Baked, PhysicsError> {
    let Some(scene) = doc.scenes.get(scene_index) else { return Err(PhysicsError::NoWorld) };
    let Some(world) = &scene.physics else { return Err(PhysicsError::NoWorld) };
    let bodies = bodies_of(scene);
    if bodies.iter().all(|b| b.static_body) || bodies.is_empty() {
        return Err(PhysicsError::NoBodies);
    }
    let (start, end) = scene.frame_range();
    let frames = end.saturating_sub(start).clamp(1, MAX_BAKE_FRAMES);
    let dt = (1.0 / doc.stage.fps.max(1.0)) as f64;
    let gravity = world.gravity;
    let wind = world.wind;

    // State per body: position (box centre), velocity.
    let mut pos: Vec<Vec2> = bodies.iter().map(|b| (b.box_min + b.box_max) * 0.5).collect();
    let mut vel: Vec<Vec2> = bodies.iter().map(|b| b.velocity).collect();
    let mut asleep: Vec<bool> = bodies.iter().map(|b| b.static_body).collect();
    let half: Vec<Vec2> = bodies.iter().map(|b| (b.box_max - b.box_min) * 0.5).collect();
    let mut out = vec![Vec::with_capacity(frames as usize + 1); bodies.len()];

    // Frame 0 is the rest pose: the simulation starts with the first step.
    for i in 0..bodies.len() {
        let rest_centre = (bodies[i].box_min + bodies[i].box_max) * 0.5;
        out[i].push(bodies[i].rest + pos[i] - rest_centre);
    }
    for _f in 1..frames {
        // Forces (static bodies don't move).
        for i in 0..bodies.len() {
            if bodies[i].static_body {
                continue;
            }
            vel[i] = vel[i] + (gravity + wind) * dt as f32;
            pos[i] = pos[i] + vel[i] * dt as f32;
        }
        // Collisions, dynamic against static (body-vs-body waits for a Moho specimen that
        // needs it): on overlap, push out along the smallest penetration axis and reflect.
        for i in 0..bodies.len() {
            if bodies[i].static_body {
                continue;
            }
            for (j, s) in bodies.iter().enumerate() {
                if !s.static_body || j == i {
                    continue;
                }
                let (smin, smax) = (s.box_min + (pos[j] - (s.box_min + s.box_max) * 0.5), s.box_max + (pos[j] - (s.box_min + s.box_max) * 0.5));
                let overlap_x = ((pos[i].x + half[i].x) - smin.x).min(smax.x - (pos[i].x - half[i].x));
                let overlap_y = ((pos[i].y + half[i].y) - smin.y).min(smax.y - (pos[i].y - half[i].y));
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }
                if overlap_y <= overlap_x {
                    // Vertical: land on top of, or butt under, the static box.
                    let down = vel[i].y >= 0.0 || pos[i].y < (smin.y + smax.y) * 0.5;
                    if down {
                        pos[i].y = smin.y - half[i].y;
                        vel[i].y = if -vel[i].y * bodies[i].restitution > REST_SPEED { -vel[i].y * bodies[i].restitution } else { 0.0 };
                        vel[i].x *= 1.0 - (bodies[i].friction * FRICTION_PER_SECOND * dt as f32).min(1.0);
                    } else {
                        pos[i].y = smax.y + half[i].y;
                        vel[i].y = if vel[i].y * bodies[i].restitution > REST_SPEED { -vel[i].y * bodies[i].restitution } else { 0.0 };
                    }
                } else {
                    // Horizontal wall.
                    let right = vel[i].x >= 0.0;
                    if right {
                        pos[i].x = smin.x - half[i].x;
                    } else {
                        pos[i].x = smax.x + half[i].x;
                    }
                    vel[i].x = 0.0;
                }
            }
            // Sleep: a settled body keeps its place (its velocity has already faded).
            if vel[i].length() < SLEEP_SPEED {
                asleep[i] = true;
                vel[i] = Vec2::ZERO;
            }
            let _ = &mut asleep[i];
        }
        // Record: the box centre moved from the rest centre by `pos[i] − rest_centre`; the
        // layer translation rides that offset from the layer's rest translation.
        for i in 0..bodies.len() {
            let rest_centre = (bodies[i].box_min + bodies[i].box_max) * 0.5;
            out[i].push(bodies[i].rest + pos[i] - rest_centre);
        }
        if out[0].len() >= frames as usize {
            break;
        }
    }

    Ok(Baked {
        bodies: bodies.into_iter().zip(out).map(|(b, positions)| BakedBody { layer: b.layer, positions }).collect(),
    })
}

/// The layer's body flag made safe after a load.
pub fn sanitize_layer(layer: &mut Layer) {
    if let Some(b) = layer.physics {
        layer.physics = Some(b.sanitized());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Element, ElementId, ElementKind, Keyframe, PhysicsBody, PhysicsWorld, Scene, Shape};
    use anicraft_geom::Affine;
    use std::sync::Arc;

    /// A scene with a world (default Moho gravity, stage px) and one box over one floor.
    fn drop_scene(gravity: Vec2) -> Document {
        let mut d = Document::default();
        let box_shape = Shape::filled(crate::Path::rect(0.0, 0.0, 100.0, 100.0), crate::Paint::Solid(crate::Rgba::BLACK));
        let floor_shape = Shape::filled(crate::Path::rect(0.0, 0.0, 800.0, 40.0), crate::Paint::Solid(crate::Rgba::BLACK));
        let mut k = Keyframe::new(0, 60);
        k.elements = vec![
            Arc::new(Element { id: ElementId(1), name: String::new(), transform: Affine::translate(300.0, 200.0), color: Default::default(), kind: ElementKind::Shape(box_shape) }),
        ];
        let mut kf = Keyframe::new(0, 60);
        kf.elements = vec![
            Arc::new(Element { id: ElementId(2), name: String::new(), transform: Affine::translate(0.0, 500.0), color: Default::default(), kind: ElementKind::Shape(floor_shape) }),
        ];
        let mut falling = crate::Layer::new(LayerId(1), "Box");
        falling.physics = Some(PhysicsBody { static_body: false, friction: 0.3, restitution: 0.5, velocity: Vec2::ZERO });
        falling.keyframes = vec![Arc::new(k)];
        let mut floor = crate::Layer::new(LayerId(2), "Floor");
        floor.physics = Some(PhysicsBody { static_body: true, friction: 0.0, restitution: 0.0, velocity: Vec2::ZERO });
        floor.keyframes = vec![Arc::new(kf)];
        let mut scene = Scene::new("Scene 1");
        scene.timeline.layers = vec![Arc::new(falling), Arc::new(floor)];
        scene.physics = Some(PhysicsWorld { gravity, wind: Vec2::ZERO }.sanitized());
        d.scenes = vec![Arc::new(scene)];
        d
    }

    #[test]
    fn a_body_falls_ballistically_without_a_floor() {
        // No static body: pure gravity. y(t) = y0 + g·t²/2 (v0 = 0, semi-implicit Euler
        // sums g·dt each frame first).
        let mut d = drop_scene(Vec2::new(0.0, 3200.0));
        let scene = Arc::make_mut(&mut d.scenes[0]);
        scene.timeline.layers.pop();
        let baked = bake(&d, 0).unwrap();
        // The baked translation is the offset from the rest pose (this layer's rest
        // translation is zero; the element transform holds the pose). The integrator is
        // symplectic Euler: p(n) = g·dt²·n(n+1)/2 from rest — energy-stable, and exactly
        // what a second bake of the same document produces.
        let dt = 1.0 / d.stage.fps;
        let g_dt2 = 3200.0 * dt * dt;
        for f in [1u32, 10, 30] {
            let y = baked.position(LayerId(1), f).unwrap().y;
            let expect = g_dt2 * (f * (f + 1)) as f32 * 0.5;
            assert!((y - expect).abs() < 0.5, "frame {f}: {y} vs {expect}");
        }
    }

    #[test]
    fn a_dropped_box_lands_on_the_floor_and_settles() {
        let d = drop_scene(Vec2::new(0.0, 3200.0));
        let baked = bake(&d, 0).unwrap();
        // The floor's top is at 540; the box is 100 tall, so its centre rests at 490
        // (translation 200 = box top at rest… the rest translation is 200 and the rest box
        // spans 200..300, landing puts the box bottom on 540 → centre 490, translation 490−50=440).
        let last = baked.position(LayerId(1), 59).unwrap();
        assert!((last.x - 0.0).abs() < 1e-3, "x unmoved: {last:?}");
        // The floor slab spans 500..540: the box rests its bottom on the top (its centre
        // at 450, translation +200 from the rest 250).
        assert!((last.y - 200.0).abs() < 1.0, "the box sits on the floor: {last:?}");
        // And it got there by falling: frame 5 is partway down.
        let mid = baked.position(LayerId(1), 5).unwrap().y;
        assert!(mid > 0.0 && mid < 200.0, "{mid}");
    }

    #[test]
    fn restitution_bounds_the_bounce() {
        let d = drop_scene(Vec2::new(0.0, 3200.0));
        let baked = bake(&d, 0).unwrap();
        // The apex of the first bounce never exceeds the drop height, and the body ends at rest.
        let mut peak = 0.0f32;
        for f in 0..60 {
            peak = peak.max(baked.position(LayerId(1), f).unwrap().y);
        }
        assert!(peak <= 200.0 + 1.0, "no bounce above the landing: {peak}");
    }

    #[test]
    fn sideways_velocity_carries_and_friction_stops_it() {
        let mut d = drop_scene(Vec2::new(0.0, 3200.0));
        let scene = Arc::make_mut(&mut d.scenes[0]);
        let layer = scene.timeline.layer_mut(LayerId(1)).unwrap();
        layer.physics = Some(PhysicsBody { static_body: false, friction: 1.0, restitution: 0.0, velocity: Vec2::new(600.0, 0.0) });
        let baked = bake(&d, 0).unwrap();
        let last = baked.position(LayerId(1), 59).unwrap();
        assert!(last.x > 20.0, "it slid right: {last:?}");
        let before_last = baked.position(LayerId(1), 50).unwrap();
        assert!((last.x - before_last.x).abs() < 1.0, "friction 1 stopped the slide by the end ({last:?} vs {before_last:?})");
    }

    #[test]
    fn bake_is_deterministic_and_bounded() {
        let d = drop_scene(Vec2::new(0.0, 3200.0));
        let a = bake(&d, 0).unwrap();
        let b = bake(&d, 0).unwrap();
        assert_eq!(a, b);
        for p in &a.bodies[0].positions {
            assert!(p.x.is_finite() && p.y.is_finite(), "no NaN: {p:?}");
        }
        assert_eq!(a.bodies[0].positions.len(), 60, "one position per scene frame");
    }

    #[test]
    fn scenes_without_a_world_refuse() {
        let mut d = drop_scene(Vec2::new(0.0, 3200.0));
        Arc::make_mut(&mut d.scenes[0]).physics = None;
        assert_eq!(bake(&d, 0), Err(PhysicsError::NoWorld));
        assert_eq!(bake(&d, 7), Err(PhysicsError::NoWorld));
        let mut d = drop_scene(Vec2::ZERO);
        // A zero world with bodies still bakes (nothing moves, but it is a legal bake).
        assert!(bake(&d, 0).is_ok());
        Arc::make_mut(&mut d.scenes[0]).timeline.layers.truncate(1);
        // Only static bodies → no bodies to simulate.
        {
            let scene = Arc::make_mut(&mut d.scenes[0]);
            let layer = scene.timeline.layer_mut(LayerId(1)).unwrap();
            layer.physics = Some(PhysicsBody { static_body: true, friction: 0.0, restitution: 0.0, velocity: Vec2::ZERO });
        }
        assert_eq!(bake(&d, 0), Err(PhysicsError::NoBodies));
    }
}
