//! Physics commands (ADR 0106): bake the scene's physics simulation into the body layers'
//! translation channels — the offline pass that turns the stateful simulation into plain
//! data every renderer, export and the timeline already draw — and read a scene's physics
//! back. Moho's `use_baked_physics` is this same idea: the simulation runs once, frame
//! order fixed, and the movie then plays from the baked keys.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::PhysicsError;
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    vec![
        c("physics.bake", "Bake Physics", "Modify/Physics", None, "{scene?} (simulates the scene's bodies once — gravity, wind, bounces off static layers — and keys each body layer's translation per frame; refuses when a body layer's translation already animates or the scene has no physics world)", true, bake),
        c("physics.info", "Physics Info", "", None, "{scene?} → the world's gravity and wind, and each body layer's static flag, friction, restitution and start velocity", false, info),
    ]
}

fn scene_of(s: &Session, scene: Option<u64>) -> Result<usize> {
    match scene {
        Some(i) => usize::try_from(i).map_err(|_| EngineError::Param("scene is past the document's scenes".into())).and_then(|i| (i < s.doc.scenes.len()).then_some(i).ok_or_else(|| EngineError::Param("scene is past the document's scenes".into()))),
        None => Ok(s.scene),
    }
}

/// The bake: simulate once, then write translation keys. One undo step.
fn bake(s: &mut Session, p: &Value) -> Result<Value> {
    let scene = scene_of(s, opt_u64(p, "scene")?)?;
    let world = s.doc.scenes.get(scene).and_then(|sc| sc.physics);
    let Some(world) = world else { return Err(EngineError::State("no physics world in this scene (a Moho group with physics enabled carries one)".into())) };
    let baked = anicraft_doc::bake_physics(&s.doc, scene).map_err(|e: PhysicsError| EngineError::State(e.to_string()))?;
    // Bodies with an already-animated translation would fight their own keys: refuse before
    // touching anything.
    for body in &baked.bodies {
        let tl = s.doc.timeline(scene);
        let Some(layer) = tl.layer(body.layer) else { continue };
        if layer.translation.is_animated() {
            return Err(EngineError::State(format!("layer {} ({}) animates its translation already; clear or bake that animation first", layer.name, body.layer.0)));
        }
    }
    // Static bodies don't move: only the dynamic ones take keys.
    let mut keyed = Vec::new();
    for body in &baked.bodies {
        let static_body = s.doc.timeline(scene).layer(body.layer).and_then(|l| l.physics).is_some_and(|b| b.static_body);
        if static_body {
            continue;
        }
        let tl = s.doc.timeline_mut(scene);
        let Some(layer) = tl.layer_mut(body.layer) else { continue };
        for (f, pos) in body.positions.iter().enumerate() {
            layer.translation.set_key(f as u32, *pos);
        }
        keyed.push(body.layer.0);
    }
    if keyed.is_empty() {
        return Err(EngineError::State("no physics bodies to bake in this scene".into()));
    }
    Ok(json!({"scene": scene, "gravity": {"x": world.gravity.x, "y": world.gravity.y}, "wind": {"x": world.wind.x, "y": world.wind.y}, "baked": keyed, "frames": baked.bodies[0].positions.len()}))
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let scene = scene_of(s, opt_u64(p, "scene")?)?;
    let sc = s.doc.scenes.get(scene).ok_or_else(|| EngineError::State("no such scene".into()))?;
    let world = sc.physics.map(|w| json!({"gravity": {"x": w.gravity.x, "y": w.gravity.y}, "wind": {"x": w.wind.x, "y": w.wind.y}}));
    let bodies: Vec<Value> = sc
        .timeline
        .layers
        .iter()
        .filter_map(|l| {
            l.physics.map(|b| {
                json!({"layer": l.id.0, "name": l.name, "static": b.static_body, "friction": b.friction, "restitution": b.restitution,
                       "velocity": {"x": b.velocity.x, "y": b.velocity.y}})
            })
        })
        .collect();
    Ok(json!({"scene": scene, "world": world, "bodies": bodies}))
}
