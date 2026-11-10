//! The bone system end to end (ADR 0104): rig commands, posing, IK, binding, the render
//! gate (a bound shape follows its bone's analytic prediction), the file format, plug-in
//! policy and robustness.

use super::*;
use anicraft_doc::{Document, LayerKind, Stage};
use anicraft_geom::Vec2;
use anicraft_render::render_frame;
use serde_json::json;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A 200 × 100 white stage, 20 frames. `rect` draws a 10×10 black rect at (x, y) and
/// returns (session, element id).
fn stage_with(x: f32, y: f32) -> (Session, u64) {
    let d = Document { stage: Stage { width: 200, height: 100, ..Stage::default() }, ..Document::default() };
    let mut s = Session::new(d);
    run(&mut s, "draw.rect", json!({"x": x, "y": y, "w": 10.0, "h": 10.0, "fill": "#000000", "stroke": null}));
    run(&mut s, "timeline.insert_frame", json!({"count": 19}));
    let el = s.selection[0].0;
    (s, el)
}

/// An empty 200 × 100 stage, 20 frames.
fn blank() -> Session {
    let d = Document { stage: Stage { width: 200, height: 100, ..Stage::default() }, ..Document::default() };
    let mut s = Session::new(d);
    run(&mut s, "timeline.insert_frame", json!({"count": 19}));
    s
}

/// A bone layer named `name` with one artwork layer inside it, both on an empty stage.
fn rig_with_art(s: &mut Session, name: &str) -> u64 {
    run(s, "bone.rig_add", json!({"name": name}));
    let rig = s.layer.0;
    // The artwork joins the rig (as an imported rig's layers do): a new layer moved down
    // lands inside the bone layer above it.
    run(s, "layer.new", json!({"name": "Art"}));
    run(s, "layer.move_down", json!({}));
    assert!(s.doc.timeline(0).layer(s.layer).unwrap().parent.is_some(), "Art sits inside the rig");
    rig
}

/// Whether pixel (x, y) of `frame` is black (else white).
fn black(s: &Session, frame: u32, x: u32, y: u32) -> bool {
    let pm = render_frame(&s.doc, frame, 1.0).unwrap();
    let p = pm.pixel(x, y).unwrap();
    assert!(p[0] < 0.01 || p[0] > 0.99, "pixel ({x}, {y}) is part-covered: {p:?}");
    p[0] < 0.5
}

/// A rig with a bone pivoted at (100, 50), its angle keyed 0→90° over frames 0→12, and a
/// 10×10 rect at (120, 45) bound to it. Returns (session, rect id).
fn rigged() -> (Session, u64) {
    let mut s = blank();
    rig_with_art(&mut s, "Rig");
    run(&mut s, "draw.rect", json!({"x": 120.0, "y": 45.0, "w": 10.0, "h": 10.0, "fill": "#000000", "stroke": null}));
    let el = s.selection[0].0;
    run(&mut s, "bone.add", json!({"name": "dial", "pos": [100.0, 50.0], "length": 60.0, "angle": 0.0}));
    run(&mut s, "bone.key", json!({"bone": 0, "channel": "angle", "frame": 12, "value": 1.5707964}));
    run(&mut s, "bone.bind", json!({"bone": 0, "element": [el]}));
    (s, el)
}

#[test]
fn a_rig_holds_bones_and_poses_them_over_time() {
    let mut s = blank();
    let rig = LayerId(rig_with_art(&mut s, "Rig"));
    let tl = s.doc.timeline(0);
    assert_eq!(tl.layers[0].kind, LayerKind::Bone, "the rig sits above the artwork");
    assert_eq!(tl.layers[0].name, "Rig");
    // Bones chain: the second hangs off the first's tip by default.
    run(&mut s, "bone.add", json!({"name": "root", "pos": [100.0, 50.0], "length": 60.0, "angle": 0.0}));
    run(&mut s, "bone.add", json!({"name": "tip", "length": 40.0}));
    let sk = s.doc.timeline(0).layer(rig).unwrap().skeleton.clone().unwrap();
    assert_eq!(sk.bones.len(), 2);
    assert_eq!(sk.bones[1].parent, Some(anicraft_doc::BoneId(0)));
    // info reads the rig back at a frame: the second bone's base sits at the first's tip.
    let v = run(&mut s, "bone.info", json!({"layer": rig.0, "frame": 0}));
    let bones = v["bones"].as_array().unwrap();
    assert_eq!(bones[1]["name"], "tip");
    let tip_x = bones[1]["x"].as_f64().unwrap();
    assert!((tip_x - 160.0).abs() < 1e-3, "child base at the root's tip: {bones:?}");
    // Posing the root carries the child: its base lands on the root's tip.
    run(&mut s, "bone.key", json!({"bone": 0, "channel": "angle", "frame": 12, "value": 1.5707964}));
    let v = run(&mut s, "bone.info", json!({"layer": rig.0, "frame": 12}));
    let bones = v["bones"].as_array().unwrap();
    let (x, y) = (bones[1]["x"].as_f64().unwrap(), bones[1]["y"].as_f64().unwrap());
    assert!((x - 100.0).abs() < 1e-3 && (y - 110.0).abs() < 1e-3, "child base at the turned root's tip: ({x}, {y})");
    // Undo takes the last key back, and everything with it.
    run(&mut s, "edit.undo", json!({}));
    let sk = s.doc.timeline(0).layer(rig).unwrap().skeleton.clone().unwrap();
    assert_eq!(sk.bones[0].angle.keys.len(), 1, "undo removes the pose key");
}

/// The stage-4 render gate (ADR 0104): the bound rect rides the bone's analytic matrix. At
/// rest the rig deforms nothing; at frame 12 the bone has turned 90° about (100, 50), and
/// the rect sits exactly where the rotation puts it.
#[test]
fn a_bound_shape_follows_its_bone_render_gate() {
    let (s, _) = rigged();
    // Frame 0: the rest pose is the identity skin — the rect is exactly where it was drawn.
    assert!(black(&s, 0, 121, 46) && !black(&s, 0, 118, 46));
    // Frame 12: the rect spans (95..105, 70..80); its old spot is white.
    assert!(black(&s, 12, 96, 71) && black(&s, 12, 104, 79) && !black(&s, 12, 121, 46), "the rect rides the bone");
}

#[test]
fn an_unbound_shape_stays_put_while_the_rig_moves() {
    let mut s = blank();
    rig_with_art(&mut s, "Rig");
    run(&mut s, "draw.rect", json!({"x": 120.0, "y": 45.0, "w": 10.0, "h": 10.0, "fill": "#000000", "stroke": null}));
    run(&mut s, "bone.add", json!({"name": "dial", "pos": [100.0, 50.0], "length": 60.0, "angle": 0.0}));
    run(&mut s, "bone.key", json!({"bone": 0, "channel": "angle", "frame": 12, "value": 1.5707964}));
    // No bind: the rect stays where it was drawn at every frame.
    assert!(black(&s, 0, 121, 46) && black(&s, 12, 121, 46));
}

#[test]
fn ik_aims_the_chain_at_its_target() {
    let mut s = blank();
    let rig = LayerId(rig_with_art(&mut s, "Rig"));
    run(&mut s, "bone.add", json!({"name": "upper", "pos": [100.0, 50.0], "length": 60.0, "angle": 0.0}));
    run(&mut s, "bone.add", json!({"name": "fore", "length": 40.0}));
    // A goal bone OUTSIDE the chain — the way Moho IK targets work: a rooted control bone
    // (parent −1) parked at (170, 20), so its tip sits at (190, 20), inside the reach disc.
    run(&mut s, "bone.add", json!({"name": "goal", "parent": -1, "pos": [170.0, 20.0], "length": 20.0}));
    run(&mut s, "bone.key", json!({"bone": 1, "channel": "target", "value": 2.0}));
    // Through the document's own rig math: the forearm tip lands on the goal's tip.
    let sk = s.doc.timeline(0).layer(rig).unwrap().skeleton.clone().unwrap();
    let mats = sk.matrices(0);
    let tip = mats[1].apply(Vec2::new(40.0, 0.0));
    let want = Vec2::new(190.0, 20.0);
    assert!((tip - want).length() < 0.5, "forearm tip {tip:?} vs goal tip {want:?}");
    // Lengths hold: the upper's base sits exactly 60 px from its shoulder.
    let base = mats[1].apply(Vec2::ZERO);
    assert!(((base - Vec2::new(100.0, 50.0)).length() - 60.0).abs() < 1e-2, "{base:?}");
    // info reports the solved pose too.
    let v = run(&mut s, "bone.info", json!({"layer": rig.0, "frame": 0}));
    let bones = v["bones"].as_array().unwrap();
    let fore_base = (bones[1]["x"].as_f64().unwrap(), bones[1]["y"].as_f64().unwrap());
    assert!(((fore_base.0 - 100.0).powi(2) + (fore_base.1 - 50.0).powi(2)).sqrt() - 60.0 < 1e-2);
}

#[test]
fn binding_commands_keep_their_promises() {
    let mut s = blank();
    let rig = LayerId(rig_with_art(&mut s, "Rig"));
    run(&mut s, "draw.rect", json!({"x": 120.0, "y": 45.0, "w": 10.0, "h": 10.0, "fill": "#000000", "stroke": null}));
    let el = s.selection[0].0;
    // No rig: bone commands say so instead of guessing.
    let (mut plain, _) = stage_with(20.0, 45.0);
    let err = plain.execute("bone.add", &json!({})).unwrap_err().to_string();
    assert!(err.contains("no bone rig"), "{err}");
    run(&mut s, "bone.add", json!({"name": "a", "pos": [100.0, 50.0], "length": 60.0}));
    run(&mut s, "bone.add", json!({"name": "b", "length": 40.0}));
    // Weighted binds normalise: 2:2 becomes 0.5/0.5.
    run(&mut s, "bone.bind", json!({"bones": [{"bone": 0, "weight": 2.0}, {"bone": 1, "weight": 2.0}], "element": [el], "layer": rig.0}));
    let shape_of = |s: &Session| {
        let tl = s.doc.timeline(0);
        for l in &tl.layers {
            for k in &l.keyframes {
                for e in &k.elements {
                    if let anicraft_doc::ElementKind::Shape(sh) = &e.kind {
                        return sh.clone();
                    }
                }
            }
        }
        panic!("a shape");
    };
    let sh = shape_of(&s);
    assert!((sh.bind[0].1 - 0.5).abs() < 1e-4 && (sh.bind[1].1 - 0.5).abs() < 1e-4, "{:?}", sh.bind);
    // Deleting bone 0 shifts the second down to index 0, and the bind follows.
    run(&mut s, "bone.delete", json!({"bone": 0, "layer": rig.0}));
    let sh = shape_of(&s);
    assert_eq!(sh.bind, vec![(anicraft_doc::BoneId(0), 1.0)]);
    // unbind frees the shape.
    run(&mut s, "bone.unbind", json!({"element": [el], "layer": rig.0}));
    let sh = shape_of(&s);
    assert!(sh.bind.is_empty());
}

#[test]
fn bones_round_trip_through_the_file_format() {
    let (mut s, _) = rigged();
    run(&mut s, "bone.add", json!({"name": "extra", "parent": 0, "length": 20.0, "constraints": true, "min": -0.5, "max": 0.5}));
    let text = crate::format::to_json(&s.doc).unwrap();
    let doc = crate::format::from_json(&text).unwrap();
    assert_eq!(doc, s.doc, "the saved file reads back identical");
    let rig = doc.timeline(0).layers.iter().find(|l| l.kind == LayerKind::Bone).unwrap();
    assert_eq!(rig.skeleton.as_ref().unwrap().bones.len(), 2);
}

#[test]
fn bone_commands_are_shell_visible_and_plugin_safe() {
    // The registry lists them (menus are generated from it), and a plug-in may call them.
    for id in ["bone.rig_add", "bone.add", "bone.delete", "bone.set", "bone.key", "bone.clear_channel", "bone.bind", "bone.unbind", "bone.info"] {
        assert!(crate::commands().iter().any(|c| c.id == id), "{id} missing from the registry");
        assert!(crate::cmds_plugin::plugin_may_call(id).is_ok(), "{id} refused to plug-ins");
    }
}

#[test]
fn bone_commands_refuse_broken_input_without_panicking() {
    let (mut s, el) = stage_with(120.0, 45.0);
    let try_all = |s: &mut Session, p: &Value| {
        for id in ["bone.add", "bone.set", "bone.key", "bone.delete", "bone.bind", "bone.info", "bone.clear_channel", "bone.unbind", "bone.rig_add"] {
            if let Err(e) = s.execute(id, p) {
                let msg = e.to_string();
                assert!(!msg.contains("panic") && !msg.contains("internal"), "{id} {p}: {msg}");
            }
        }
    };
    for p in [
        json!({"bone": 999}),
        json!({"bone": 0, "channel": "angle"}),
        json!({"bone": 0, "channel": "weird", "value": 1.0}),
        json!({"bone": 0, "channel": "angle", "value": "no"}),
        json!({"bone": 0, "channel": "angle", "value": 1.0, "frame": u64::MAX}),
        json!({"bone": 0, "channel": "pos", "value": [1.0]}),
        json!({"bone": -5, "channel": "angle", "value": 1.0}),
        json!({"bones": [], "element": [el]}),
        json!({"bones": [{"bone": 0, "weight": -3.0}], "element": [el]}),
        json!({"bones": [{"bone": 999}], "element": [el]}),
        json!({"bones": [{"bone": 0, "weight": 1e300}], "element": [el]}),
        json!({"length": -50.0}),
        json!({"length": f64::NEG_INFINITY}),
        json!({"bone": 0, "parent": 0}),
        json!({"bone": 0, "parent": 99}),
        json!({"bone": 0, "offset": [f64::NAN, 1.0]}),
        json!({"bone": 0, "min": 5.0, "max": -5.0}),
        json!({"layer": u64::MAX}),
    ] {
        try_all(&mut s, &p);
    }
    // And the session still works after all that.
    run(&mut s, "bone.rig_add", json!({}));
    run(&mut s, "bone.add", json!({"length": 10.0}));
}
