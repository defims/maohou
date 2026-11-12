//! The switch layer end to end (ADR 0105): the switch commands, evaluation through the
//! command-built document, the file format round trip, plug-in policy and robustness.

use super::*;
use anicraft_doc::{Document, LayerKind, PointTrack, Stage, Stroke, TextAlign, Trim};
use anicraft_geom::Vec2;
use serde_json::json;
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A 200 × 100 stage, 20 frames, with two variant layers ("Frown" and "Smile") and a
/// switch ("Mouths") above them, built through the layer commands where they exist.
fn switch_session() -> Session {
    let d = Document { stage: Stage { width: 200, height: 100, ..Stage::default() }, ..Document::default() };
    let mut s = Session::new(d);
    run(&mut s, "timeline.insert_frame", json!({"count": 19}));
    for (name, x) in [("Frown", 10.0), ("Smile", 120.0)] {
        run(&mut s, "layer.new", json!({"name": name}));
        run(&mut s, "draw.rect", json!({"x": x, "y": 10.0, "w": 10.0, "h": 10.0, "fill": "#000000", "stroke": null}));
    }
    // The switch itself: a container the two variants live in, retyped as a switch. The
    // document model has no "new switch" command yet — a Moho import or the engine test
    // types the layer, exactly what the importer does.
    let mut sw = anicraft_doc::Layer::new(anicraft_doc::LayerId(s.doc.alloc_id()), "Mouths");
    sw.kind = LayerKind::Switch;
    let sw_id = sw.id;
    let ids: Vec<anicraft_doc::LayerId> = s.doc.timeline(0).layers.iter().filter(|l| l.name == "Frown" || l.name == "Smile").map(|l| l.id).collect();
    let tl = s.doc.timeline_mut(0);
    for id in ids {
        tl.layer_mut(id).unwrap().parent = Some(sw_id);
    }
    tl.layers.insert(0, Arc::new(sw));
    s.layer = sw_id;
    s
}

#[test]
fn switching_keys_and_shows_one_variant_per_frame() {
    let mut s = switch_session();
    run(&mut s, "switch.set_variant", json!({"name": "Frown", "frame": 0}));
    run(&mut s, "switch.set_variant", json!({"name": "Smile", "frame": 10}));
    let seen = |s: &Session, f: u32| {
        let dl = anicraft_doc::evaluate(&s.doc, f);
        dl.items.iter().map(|i| (i.transform.translation().x.round() as u32, i.transform.translation().y.round() as u32)).collect::<Vec<_>>()
    };
    assert_eq!(seen(&s, 0).len(), 1, "one variant at frame 0");
    assert_eq!(seen(&s, 10).len(), 1, "one variant at frame 10");
    assert_eq!(seen(&s, 5), seen(&s, 0), "the earlier key holds between keys");
    // The variants sit at different spots, so the pick is visible.
    assert_ne!(seen(&s, 0), seen(&s, 10));
    let info = run(&mut s, "switch.info", json!({}));
    let mut variants = info["variants"].clone();
    variants.as_array_mut().unwrap().sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    assert_eq!(variants, json!(["Frown", "Smile"]));
    assert_eq!(info["selection"], json!("Frown"), "the session's frame is 0");
    assert_eq!(info["keys"].as_array().unwrap().len(), 2);
    let later = run(&mut s, "switch.info", json!({"frame": 10}));
    assert_eq!(later["selection"], json!("Smile"), "frame 10's key");
}

#[test]
fn deleting_a_switch_key_lets_the_earlier_key_hold() {
    let mut s = switch_session();
    run(&mut s, "switch.set_variant", json!({"name": "Frown", "frame": 0}));
    run(&mut s, "switch.set_variant", json!({"name": "Smile", "frame": 10}));
    run(&mut s, "switch.delete_key", json!({"frame": 10}));
    let info = run(&mut s, "switch.info", json!({"frame": 10}));
    assert_eq!(info["selection"], json!("Frown"), "frame 0's key holds again");
    let err = s.execute("switch.delete_key", &json!({"frame": 10}));
    assert!(err.is_err(), "a second delete has nothing to delete");
}

#[test]
fn a_variant_that_is_not_a_child_is_refused() {
    let mut s = switch_session();
    let err = s.execute("switch.set_variant", &json!({"name": "Pucker"}));
    assert!(err.is_err(), "Pucker is not a variant");
    // Unknown layers refuse too.
    let err = s.execute("switch.set_variant", &json!({"name": "Frown", "layer": 987654}));
    assert!(err.is_err());
}

#[test]
fn commands_outside_a_switch_refuse() {
    let mut s = switch_session();
    s.layer = s.doc.timeline(0).layers.iter().find(|l| l.name == "Frown").map(|l| l.id).unwrap();
    // Frown's parent IS the switch, so this resolves; move to the root instead.
    let root = s.doc.timeline(0).layers.iter().find(|l| l.name == "Frown").map(|l| l.parent.unwrap()).unwrap();
    let _ = root;
    let mut t = switch_session();
    let no_switch = t.doc.timeline_mut(0).layers.pop();
    let _ = no_switch;
    let mut plain = Session::new(Document::default());
    let err = plain.execute("switch.info", &json!({}));
    assert!(err.is_err(), "no switch on a fresh session");
    let _ = (s, root);
}

/// The stage-5 fields survive the native format: a switch with keys, point tracks, a trim
/// and a stroked text block come back equal (the file gate for the round trip).
#[test]
fn stage5_fields_round_trip_through_the_native_format() {
    let d = Document { stage: Stage { width: 200, height: 100, ..Stage::default() }, ..Document::default() };
    let mut s = Session::new(d);
    run(&mut s, "timeline.insert_frame", json!({"count": 19}));
    // A switch with selector keys.
    let mut sw = anicraft_doc::Layer::new(anicraft_doc::LayerId(s.doc.alloc_id()), "Mouths");
    sw.kind = LayerKind::Switch;
    sw.switch_to.set_key(0, "Frown".into());
    sw.switch_to.set_key(10, "Smile".into());
    s.doc.timeline_mut(0).layers.insert(0, Arc::new(sw));
    // A shape with an animated point and a trim.
    let mut shape = anicraft_doc::Shape::filled(anicraft_geom::Path::rect(0.0, 0.0, 10.0, 10.0), anicraft_doc::Paint::Solid(anicraft_doc::Rgba::BLACK));
    shape.stroke = Some(Stroke::new(anicraft_doc::Rgba::BLACK, 2.0));
    let mut ch = anicraft_doc::Channel::default();
    ch.set_key(0, Vec2::new(10.0, 10.0));
    ch.set_key(10, Vec2::new(40.0, 10.0));
    shape.points.push(PointTrack { index: 2, position: ch });
    let mut t = Trim::default();
    t.start.set_key(0, -0.1);
    t.end.set_key(5, 0.5);
    shape.trim = Some(Box::new(t));
    let el = anicraft_doc::Element { id: anicraft_doc::ElementId(s.doc.alloc_id()), name: String::new(), transform: anicraft_geom::Affine::IDENTITY, color: anicraft_doc::ColorEffect::default(), kind: anicraft_doc::ElementKind::Shape(shape) };
    let l = s.doc.timeline_mut(0).layers.last().unwrap();
    let mut k = (*l.keyframes[0]).clone();
    k.elements.push(Arc::new(el));
    let lid = l.id;
    let tl = s.doc.timeline_mut(0);
    let l = tl.layer_mut(lid).unwrap();
    l.keyframes = vec![Arc::new(k)];
    // A stroked text block.
    let block = anicraft_doc::TextBlock {
        text: "Hi".into(),
        font: "sans".into(),
        size: 20.0,
        bold: false,
        italic: false,
        color: anicraft_doc::Rgba::BLACK,
        align: TextAlign::Left,
        letter_spacing: 0.0,
        line_spacing: 0.0,
        width: None,
        field: None,
        stroke: Some(Stroke::new(anicraft_doc::Rgba::WHITE, 3.0)),
        outline: Arc::new(anicraft_geom::Path::rect(0.0, 0.0, 20.0, 20.0)),
        box_width: 20.0,
        box_height: 20.0,
    };
    let tel = anicraft_doc::Element { id: anicraft_doc::ElementId(s.doc.alloc_id()), name: String::new(), transform: anicraft_geom::Affine::IDENTITY, color: anicraft_doc::ColorEffect::default(), kind: anicraft_doc::ElementKind::Text(Arc::new(block)) };
    let l = s.doc.timeline_mut(0).layers.last().unwrap();
    let mut k = (*l.keyframes[0]).clone();
    k.elements.push(Arc::new(tel));
    let lid = l.id;
    let tl = s.doc.timeline_mut(0);
    let l = tl.layer_mut(lid).unwrap();
    l.keyframes = vec![Arc::new(k)];

    let back = format::from_json(&format::to_json(&s.doc).unwrap()).unwrap();
    assert_eq!(s.doc, back, "switch, points, trim and text stroke survive save/load");
    // v8 is the writer's version and the reader's ceiling.
    let env: serde_json::Value = serde_json::from_str(&format::to_json(&s.doc).unwrap()).unwrap();
    assert_eq!(env["version"], format::FORMAT_VERSION);
}

/// Every switch command survives hostile parameters (the panic hunter covers the registry;
/// this spells out the switch ones).
#[test]
fn switch_commands_never_panic_on_hostile_params() {
    let mut s = switch_session();
    for p in [Value::Null, json!({}), json!({"name": null}), json!({"name": 3}), json!({"name": "Frown", "frame": -1}), json!({"name": "Frown", "frame": u64::MAX}), json!({"layer": f64::MAX}), json!([])] {
        for id in ["switch.set_variant", "switch.delete_key", "switch.info"] {
            let _ = s.execute(id, &p);
        }
    }
}

#[test]
fn switch_namespace_has_a_plugin_policy() {
    for c in crate::commands() {
        if c.id.starts_with("switch.") {
            assert!(crate::cmds_plugin::plugin_may_call(c.id).is_ok(), "{} has no plug-in policy", c.id);
        }
    }
    assert!(crate::cmds_plugin::plugin_may_call("switch.set_variant").is_ok());
}
