//! Physics end to end (ADR 0106): the bake command keys the bodies' translations, undo
//! restores, evaluation shows the fall, and a scene without a world refuses.

use super::*;
use crate::Session;
use anicraft_doc::{Document, LayerId, PhysicsBody, PhysicsWorld, Scene, Stage};
use anicraft_geom::Vec2;
use serde_json::json;
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A 400 × 300 stage: a 100 px box at (150, 50) over a static floor slab whose top is at
/// 250, under Moho's default group gravity (0, −10 inches/s² y-up = 3,200 px/s² here).
fn physics_session() -> Session {
    let d = Document { stage: Stage { width: 400, height: 300, ..Stage::default() }, ..Document::default() };
    let mut s = Session::new(d);
    let mut k = anicraft_doc::Keyframe::new(0, 60);
    k.elements.push(Arc::new(anicraft_doc::Element {
        id: anicraft_doc::ElementId(s.doc.alloc_id()),
        name: String::new(),
        transform: anicraft_geom::Affine::translate(150.0, 50.0),
        color: Default::default(),
        kind: anicraft_doc::ElementKind::Shape(anicraft_doc::Shape::filled(anicraft_geom::Path::rect(0.0, 0.0, 100.0, 100.0), anicraft_doc::Paint::Solid(anicraft_doc::Rgba::BLACK))),
    }));
    let mut falling = anicraft_doc::Layer::new(LayerId(s.doc.alloc_id()), "Box");
    falling.keyframes = vec![Arc::new(k)];
    falling.physics = Some(PhysicsBody { static_body: false, friction: 0.3, restitution: 0.5, velocity: Vec2::ZERO });
    let mut kf = anicraft_doc::Keyframe::new(0, 60);
    kf.elements.push(Arc::new(anicraft_doc::Element {
        id: anicraft_doc::ElementId(s.doc.alloc_id()),
        name: String::new(),
        transform: anicraft_geom::Affine::translate(0.0, 250.0),
        color: Default::default(),
        kind: anicraft_doc::ElementKind::Shape(anicraft_doc::Shape::filled(anicraft_geom::Path::rect(0.0, 0.0, 400.0, 30.0), anicraft_doc::Paint::Solid(anicraft_doc::Rgba::BLACK))),
    }));
    let mut floor = anicraft_doc::Layer::new(LayerId(s.doc.alloc_id()), "Floor");
    floor.keyframes = vec![Arc::new(kf)];
    floor.physics = Some(PhysicsBody { static_body: true, friction: 0.0, restitution: 0.0, velocity: Vec2::ZERO });
    let mut scene = Scene::new("Scene 1");
    scene.timeline.layers = vec![Arc::new(falling), Arc::new(floor)];
    scene.physics = Some(PhysicsWorld { gravity: Vec2::new(0.0, 3200.0), wind: Vec2::ZERO }.sanitized());
    s.doc.scenes = vec![Arc::new(scene)];
    s.layer = s.doc.timeline(0).layers[0].id;
    s
}

#[test]
fn baking_keys_the_fall_and_undo_restores() {
    let mut s = physics_session();
    let out = run(&mut s, "physics.bake", json!({}));
    assert_eq!(out["baked"], json!([s.doc.timeline(0).layers[0].id.0]), "the dynamic body bakes");
    // The box is 100 tall, the floor 30: pick the box by its bounds.
    let y = |f: u32| {
        let dl = anicraft_doc::evaluate(&s.doc, f);
        let it = dl.items.iter().find(|i| {
            let h = i.path.bounds().max.y - i.path.bounds().min.y;
            (h - 100.0).abs() < 5.0
        }).unwrap();
        it.transform.translation().y
    };
    // The box ends on the floor (its top at 250): element rest 50 + translation 150.
    let last = y(59);
    assert!((last - 150.0).abs() < 1.5, "resting on the floor: {last}");
    assert!(y(3) < last && y(3) > 50.0, "partway down at frame 3: {}", y(3));
    // Undo restores the unbaked channels.
    s.undo().unwrap();
    let l = s.doc.timeline(0).layer(s.doc.timeline(0).layers[0].id).unwrap();
    assert!(l.translation.keys.is_empty(), "the bake undid");
    let info = run(&mut s, "physics.info", json!({}));
    assert_eq!(info["world"]["gravity"]["y"], json!(3200.0));
    assert_eq!(info["bodies"].as_array().map(|a| a.len()), Some(2));
}

#[test]
fn a_scene_without_a_world_refuses_and_an_animated_body_refuses() {
    let mut s = physics_session();
    {
        let scene = Arc::make_mut(&mut s.doc.scenes[0]);
        scene.physics = None;
    }
    let err = s.execute("physics.bake", &json!({})).unwrap_err();
    assert!(err.to_string().contains("no physics world"), "{err}");
    let mut s = physics_session();
    let id = s.doc.timeline(0).layers[0].id;
    s.doc.timeline_mut(0).layer_mut(id).unwrap().translation.set_key(0, Vec2::new(10.0, 0.0));
    s.doc.timeline_mut(0).layer_mut(id).unwrap().translation.set_key(5, Vec2::new(20.0, 0.0));
    let err = s.execute("physics.bake", &json!({})).unwrap_err();
    assert!(err.to_string().contains("animates its translation"), "{err}");
}

#[test]
fn baking_a_still_world_leaves_bodies_in_place() {
    let mut s = physics_session();
    {
        let scene = Arc::make_mut(&mut s.doc.scenes[0]);
        scene.physics = Some(PhysicsWorld { gravity: Vec2::ZERO, wind: Vec2::ZERO });
    }
    run(&mut s, "physics.bake", json!({}));
    let l = s.doc.timeline(0).layer(s.doc.timeline(0).layers[0].id).unwrap();
    for k in &l.translation.keys {
        assert!(k.val.length() < 1e-4, "no forces, no motion: {:?}", k.val);
    }
}
