//! Synthetic-document tests: build small `.mohoproj` JSON by hand and check what the
//! importer maps, what it reports, and that hostile input never panics. The corpus fixtures
//! (Moho Pro 14.4 native saves) are gated on their presence in `tests::fixtures`.

use crate::import::{import_json, Report};
use crate::zip;
use anicraft_geom::{PathCmd, Vec2};
use serde_json::{json, Value};

/// A minimal legal document (31 top-level keys are Moho's; only what the reader touches is
/// listed here), one empty MeshLayer on a 1280×720 stage.
fn minimal() -> Value {
    json!({
        "mime_type": "application/x-vnd.lm_mohodoc",
        "version": 1045,
        "major_version": 1,
        "rev_version": 0,
        "comment": "",
        "doc_uuid": "00000000-0000-0000-0000-000000000000",
        "created_date": "Fri Nov 6 12:00:00 2026",
        "modified_date": "Fri Nov 6 12:00:00 2026",
        "project_data": {
            "width": 1280, "height": 720, "fps": 24.0, "start_frame": 1, "end_frame": 240,
            "back_color": {"r": 234, "g": 234, "b": 234, "a": 255}
        },
        "thumbnail": "",
        "styles": [],
        "layercomps": [],
        "action_refs": [],
        "metadata": {"what": 0},
        "onions_enabled": false, "onions_sellayer": true, "onions_filled": true, "onions_colored": true, "onions_relative": true, "onions_behind": true,
        "onions_frame0": -100000, "onions_frame1": -100000, "onions_frame2": -100000, "onions_frame3": -100000,
        "onions_frame4": -100000, "onions_frame5": -100000, "onions_frame6": -100000, "onions_frame7": -100000,
        "animated_values": {},
        "layers": [{"type": "MeshLayer", "name": "Layer 1", "mesh": {"type": "Mesh", "points": [], "curves": [], "shapes": []}}],
        "documentviewstate": {}
    })
}

fn channel(t: &str, when: Vec<i64>, val: Vec<Value>) -> Value {
    let interp = when.iter().map(|_| json!({"im": 1, "v1": 0.1, "v2": 0.5, "in": 1, "h": 0, "s": false, "t": 0})).collect::<Vec<_>>();
    json!({"type": t, "ref": false, "mute": false, "when": when, "val": val, "interp": interp})
}

fn import(root: &Value) -> (anicraft_doc::Document, Report) {
    let read = |_: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok(None) };
    import_json(root.clone(), &read, None).unwrap_or_else(|e| panic!("import failed: {e}"))
}

#[test]
fn minimal_document_maps_stage_and_one_layer() {
    let (doc, report) = import(&minimal());
    assert_eq!(doc.stage.width, 1280);
    assert_eq!(doc.stage.height, 720);
    assert!((doc.stage.fps - 24.0).abs() < 1e-6);
    let l = &doc.timeline(0).layers[0];
    assert_eq!(l.name, "Layer 1");
    assert_eq!(l.keyframes.len(), 1);
    assert_eq!(l.keyframes[0].duration, 240, "frames 1..240 become one 240-frame span");
    assert!(report.skipped.is_empty());
    assert!(report.summary().starts_with("Imported everything"));
    // The sidecar keeps the whole original.
    assert_eq!(doc.maop_maohou.as_ref().unwrap().source, "mohoproj 1045");
    assert_eq!(doc.maop_maohou.as_ref().unwrap().raw["version"], 1045);
}

#[test]
fn the_layer_tree_flattens_top_first_with_folders_first() {
    let mut root = minimal();
    // Moho lists bottom-first: the switch sits above its siblings, its last variant on top.
    root["layers"] = json!([
        {"type": "MeshLayer", "name": "Ground", "mesh": {"points": [], "curves": [], "shapes": []}},
        {"type": "SwitchLayer", "name": "Mouths", "switch_keys": channel("String", vec![0], vec![json!("Smile")]), "layers": [
            {"type": "MeshLayer", "name": "Frown", "mesh": {"points": [], "curves": [], "shapes": []}},
            {"type": "MeshLayer", "name": "Smile", "mesh": {"points": [], "curves": [], "shapes": []}}
        ]},
        {"type": "GroupLayer", "name": "Head", "layers": [
            {"type": "MeshLayer", "name": "Eyes", "mesh": {"points": [], "curves": [], "shapes": []}}
        ]}
    ]);
    let (doc, report) = import(&root);
    let names: Vec<&str> = doc.timeline(0).layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Head", "Eyes", "Mouths", "Smile", "Frown", "Ground"], "top-first, folder before its run");
    let tl = doc.timeline(0);
    // Parent links: Eyes → Head, Smile/Frown → Mouths; folders draw nothing.
    let head = tl.layers.iter().find(|l| l.name == "Head").unwrap();
    assert_eq!(head.kind, anicraft_doc::LayerKind::Folder);
    let eyes = tl.layers.iter().find(|l| l.name == "Eyes").unwrap();
    assert_eq!(eyes.parent, Some(head.id));
    // The switch imports as a switch with its selector: Smile picked at frame 0, so Smile
    // draws and Frown waits (ADR 0105) — nothing is frozen or hidden.
    let mouths = tl.layers.iter().find(|l| l.name == "Mouths").unwrap();
    assert_eq!(mouths.kind, anicraft_doc::LayerKind::Switch);
    assert_eq!(mouths.switch_to.keys[0].val, "Smile");
    let smile = tl.layers.iter().find(|l| l.name == "Smile").unwrap();
    let frown = tl.layers.iter().find(|l| l.name == "Frown").unwrap();
    assert!(smile.visible && frown.visible, "the selector decides, not visibility");
    assert!(tl.shows_through_switches(3, 0), "Smile (index 3) draws");
    assert!(!tl.shows_through_switches(4, 0), "Frown (index 4) waits");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn static_layer_transforms_bake_into_elements() {
    let mut root = minimal();
    // A 0.5-inch square at layer translation (1, 0) inches, rotated 90° (π/2, Moho radians,
    // y-up), scaled 2×. The stage is 1280×720 at 320 dpi: 4 × 2.25 inches, centre (640, 360).
    root["layers"][0]["transforms"] = json!({
        "translation": channel("Vec3", vec![0], vec![json!({"x": 1.0, "y": 0.0, "z": 0.0})]),
        "scale": channel("Vec3", vec![0], vec![json!({"x": 2.0, "y": 2.0, "z": 1.0})]),
        "rotation_z": channel("Val", vec![0], vec![json!(std::f64::consts::FRAC_PI_2)])
    });
    root["layers"][0]["mesh"] = json!({
        "points": [
            {"position": channel("Vec2", vec![0], vec![json!({"x": 0.0, "y": 0.0})])},
            {"position": channel("Vec2", vec![0], vec![json!({"x": 0.5, "y": 0.0})])},
            {"position": channel("Vec2", vec![0], vec![json!({"x": 0.5, "y": -0.5})])},
            {"position": channel("Vec2", vec![0], vec![json!({"x": 0.0, "y": -0.5})])}
        ],
        "curves": [{"closed": true, "points": [
            {"point": 0, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
            {"point": 1, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
            {"point": 2, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
            {"point": 3, "smoothness": channel("Val", vec![0], vec![json!(0.0)])}
        ]}],
        "shapes": [{"has_fill": true, "has_outline": false, "edges": {"curve": [0, 0, 0, 0], "segment": [0, 1, 2, 3], "flag": [0, 0, 0, 0]},
            "style": {"fill_color": channel("Color", vec![0], vec![json!({"r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0})])}}]
    });
    let (doc, _) = import(&root);
    let layer = &doc.timeline(0).layers[0];
    let el = &layer.keyframes[0].elements[0];
    // Layer (0,0) inch lands at (640 + 320, 360) px, and the shape rotates with the layer.
    let origin = el.transform.apply(anicraft_geom::Vec2::ZERO);
    assert!((origin.x - 960.0).abs() < 1e-3 && (origin.y - 360.0).abs() < 1e-3, "origin+translation, y up → y down");
    // The shape's 0.5-inch x axis (160 px) turns upward (y flips with the rotation)…
    let along = el.transform.apply(anicraft_geom::Vec2::new(0.5, 0.0)) - origin;
    assert!(along.x.abs() < 1e-3 && (along.y - (-320.0)).abs() < 1e-3, "rotation π/2 maps +x to +y (up = -y on screen), 2× scale");
}

#[test]
fn gradients_and_strokes_become_paints() {
    let mut root = minimal();
    root["layers"][0]["mesh"] = json!({
        "points": [
            {"position": channel("Vec2", vec![0], vec![json!({"x": -1.0, "y": 1.0})])},
            {"position": channel("Vec2", vec![0], vec![json!({"x": 1.0, "y": -1.0})])}
        ],
        "curves": [
            {"closed": true, "points": [
                {"point": 0, "smoothness": channel("Val", vec![0], vec![json!(1.0)])},
                {"point": 1, "smoothness": channel("Val", vec![0], vec![json!(1.0)])}
            ]},
            {"closed": false, "points": [
                {"point": 0, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 1, "smoothness": channel("Val", vec![0], vec![json!(0.0)])}
            ]}
        ],
        "shapes": [
            {"has_fill": true, "has_outline": false, "edges": {"curve": [0, 0], "segment": [0, 1], "flag": [0, 0]},
             "style": {"fill_style_id": 9, "fill_style": {"type": "SS_Gradient2", "gradient_type": 0, "gradients": [
                {"location": channel("Val", vec![0], vec![json!(0.0)]), "color": channel("Color", vec![0], vec![json!({"r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0})])},
                {"location": channel("Val", vec![0], vec![json!(1.0)]), "color": channel("Color", vec![0], vec![json!({"r": 0.0, "g": 0.0, "b": 1.0, "a": 1.0})])}
             ]}, "fill_color": channel("Color", vec![0], vec![json!({"r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0})])}},
            {"has_fill": false, "has_outline": true, "edges": {"curve": [1], "segment": [0], "flag": [1]},
             "style": {"line_width": 0.00625, "line_color": channel("Color", vec![0], vec![json!({"r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0})])}}
        ]
    });
    let (doc, report) = import(&root);
    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    assert_eq!(els.len(), 2, "one fill element, one stroke element");
    let anicraft_doc::ElementKind::Shape(fill) = &els[0].kind else { panic!("fill element") };
    match &fill.fill {
        Some(anicraft_doc::Paint::Linear { stops, .. }) => {
            assert_eq!(stops.len(), 2);
            assert_eq!(stops[0].color, anicraft_doc::Rgba::new(1.0, 0.0, 0.0, 1.0));
        }
        other => panic!("linear gradient expected, got {other:?}"),
    }
    let anicraft_doc::ElementKind::Shape(line) = &els[1].kind else { panic!("stroke element") };
    let stroke = line.stroke.as_ref().unwrap();
    assert!((stroke.width - 0.00625).abs() < 1e-9, "line width stays in document units; the transform carries the scale");
    assert!(report.skipped.is_empty(), "all four gradient types map now: {:?}", report.skipped);
}

#[test]
fn reflected_and_angle_gradients_import_with_effect_geometry() {
    let mut root = minimal();
    root["layers"] = json!([
        {"type": "MeshLayer", "name": "L", "mesh": {
            "points": [
                {"position": channel("Vec2", vec![0], vec![json!({"x": 0.0, "y": 1.0})]), "curves": []},
                {"position": channel("Vec2", vec![0], vec![json!({"x": 1.0, "y": 1.0})]), "curves": []},
                {"position": channel("Vec2", vec![0], vec![json!({"x": 1.0, "y": 0.0})]), "curves": []},
                {"position": channel("Vec2", vec![0], vec![json!({"x": 0.0, "y": 0.0})]), "curves": []}
            ],
            "curves": [
                {"closed": true, "points": [{"point": 0, "smoothness": channel("Val", vec![0], vec![json!(0.0)]), "weight_in": channel("Val", vec![0], vec![json!(1.0)]), "weight_out": channel("Val", vec![0], vec![json!(1.0)]), "offset_in": channel("Val", vec![0], vec![json!(0.0)]), "offset_out": channel("Val", vec![0], vec![json!(0.0)])}, {"point": 1, "smoothness": channel("Val", vec![0], vec![json!(0.0)]), "weight_in": channel("Val", vec![0], vec![json!(1.0)]), "weight_out": channel("Val", vec![0], vec![json!(1.0)]), "offset_in": channel("Val", vec![0], vec![json!(0.0)]), "offset_out": channel("Val", vec![0], vec![json!(0.0)])}, {"point": 2, "smoothness": channel("Val", vec![0], vec![json!(0.0)]), "weight_in": channel("Val", vec![0], vec![json!(1.0)]), "weight_out": channel("Val", vec![0], vec![json!(1.0)]), "offset_in": channel("Val", vec![0], vec![json!(0.0)]), "offset_out": channel("Val", vec![0], vec![json!(0.0)])}, {"point": 3, "smoothness": channel("Val", vec![0], vec![json!(0.0)]), "weight_in": channel("Val", vec![0], vec![json!(1.0)]), "weight_out": channel("Val", vec![0], vec![json!(1.0)]), "offset_in": channel("Val", vec![0], vec![json!(0.0)]), "offset_out": channel("Val", vec![0], vec![json!(0.0)])}]}
            ],
            "shapes": [
                {"has_fill": true, "has_outline": false,
                 "effect_offset": channel("Vec2", vec![0], vec![json!({"x": 0.25, "y": 0.0})]),
                 "effect_rotation": channel("Val", vec![0], vec![json!(1.5707964)]),
                 "effect_scale": channel("Val", vec![0], vec![json!(2.0)]),
                 "edges": {"curve": [0, 0, 0, 0], "segment": [0, 1, 2, 3], "flag": [0, 0, 0, 0]},
                 "style": {"fill_style_id": 9, "fill_style": {"type": "SS_Gradient2", "gradient_type": 2, "through_alpha": false, "gradients": [
                    {"location": channel("Val", vec![0], vec![json!(0.0)]), "color": channel("Color", vec![0], vec![json!({"r": 1.0, "g": 0.0, "b": 0.0, "a": 0.0})])},
                    {"location": channel("Val", vec![0], vec![json!(1.0)]), "color": channel("Color", vec![0], vec![json!({"r": 0.0, "g": 0.0, "b": 1.0, "a": 1.0})])}
                 ]}}}
            ]
        }}
    ]);
    let (doc, _report) = import(&root);
    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    let anicraft_doc::ElementKind::Shape(fill) = &els[0].kind else { panic!("fill element") };
    // Reflected maps to its own paint; through_alpha=false means opaque stops.
    let Some(anicraft_doc::Paint::Reflected { start, end, stops }) = &fill.fill else { panic!("reflected gradient, got {:?}", fill.fill) };
    assert_eq!(stops[0].color.a, 1.0, "through_alpha false makes the stops opaque");
    // The axis runs from the shape's centre (the mirror point) to the left edge, and the
    // effect transform turns it 90° about the centre, scales it ×2 and shifts it by 0.25"
    // (all in shape space; the element transform carries the inch→pixel mapping).
    let centre = Vec2::new(0.5, 0.5);
    let expected = |p: Vec2| {
        let q = (p - centre) * 2.0;
        Vec2::new(q.x * 1.5707964_f32.cos() - q.y * 1.5707964_f32.sin(), q.x * 1.5707964_f32.sin() + q.y * 1.5707964_f32.cos()) + centre + Vec2::new(0.25, 0.0)
    };
    assert!((*start - expected(centre)).length() < 1e-4, "{start:?}");
    assert!((*end - expected(Vec2::new(0.0, 0.5))).length() < 1e-4, "{end:?}");
}

#[test]
fn unsupported_things_count_and_degrade() {
    let mut root = minimal();
    root["layers"] = json!([
        {"type": "BoneLayer", "name": "Rig", "skeleton": {"type": "Skeleton", "binding_mode": 1, "bones": [
            {"type": "Bone", "name": "hips", "parent": -1, "anim_angle": channel("Val", vec![0, 24], vec![json!(0.0), json!(1.57)])}
        ], "bones_groups": []}},
        {"type": "ParticleLayer", "name": "Dust"},
        {"type": "MeshLayer", "name": "L", "blend_mode": 2, "physics": {"enabled": true}, "layer_effects": {"blur": channel("Val", vec![0], vec![json!(0.25)])}, "mesh": {"points": [], "curves": [], "shapes": []}}
    ]);
    let (doc, report) = import(&root);
    // Bones import as the rig now (ADR 0104): no "skeleton bones" line any more.
    assert!(!report.skipped.contains_key("skeleton bones"), "{:?}", report.skipped);
    assert!(report.skipped.contains_key("particle layers"));
    assert!(!report.skipped.contains_key("layer blur"), "blur imports as a layer effect now");
    assert!(report.skipped.contains_key("layer blend modes"));
    // The blur sits on the layer's effect stack (0.25" at 320 px/inch = 80 px).
    let l = doc.timeline(0).layers.iter().find(|l| l.name == "L").unwrap();
    assert_eq!(l.filters, vec![anicraft_doc::Filter::Blur { blur_x: 80.0, blur_y: 80.0, quality: 3 }]);
    // The rig layer carries its bone: a bone layer kind, the angle channel y-flipped
    // (Moho CCW +1.57 → screen clockwise -1.57), the untouched keys in the sidecar.
    let rig = doc.timeline(0).layers.iter().find(|l| l.name == "Rig").unwrap();
    assert_eq!(rig.kind, anicraft_doc::LayerKind::Bone);
    let sk = rig.skeleton.as_ref().unwrap();
    assert_eq!(sk.bones.len(), 1);
    assert_eq!(sk.bones[0].name, "hips");
    assert_eq!(sk.binding_mode, 1);
    assert!((sk.bones[0].angle.sample_or(24.0, 0.0) + 1.57).abs() < 1e-4, "{:?}", sk.bones[0].angle.sample(24.0));
    assert_eq!(sk.bones[0].extra.get("type").and_then(|v| v.as_str()), Some("Bone"));
    assert!(sk.extra.contains_key("bones_groups"));
}

#[test]
fn group_layers_keep_their_transform_opacity_and_filters() {
    let mut root = minimal();
    root["layers"] = json!([
        {"type": "GroupLayer", "name": "G",
         "origin": {"x": 1.0, "y": 1.0},
         "layer_effects": {"alpha": channel("Val", vec![0, 10], vec![json!(1.0), json!(0.2)])},
         "layer_shadow": {"on": channel("Bool", vec![0], vec![json!(true)]), "angle": channel("Val", vec![0], vec![json!(0.0)]), "blur": channel("Val", vec![0], vec![json!(0.05)]), "offset": channel("Val", vec![0], vec![json!(0.1)]), "expansion": channel("Val", vec![0], vec![json!(0.02)]), "noise_amp": channel("Val", vec![0], vec![json!(0.5)]), "threshold": channel("Bool", vec![0], vec![json!(true)]), "color": channel("Color", vec![0], vec![json!({"r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0})])},
         "transforms": {"translation": channel("Vec3", vec![0], vec![json!({"x": 0.5, "y": -0.25, "z": 0.0})]), "scale": channel("Vec3", vec![0], vec![json!({"x": 2.0, "y": 1.0, "z": 1.0})])},
         "layers": [
            {"type": "MeshLayer", "name": "Kid", "mesh": {"points": [], "curves": [], "shapes": []}}
         ]}
    ]);
    let (doc, _report) = import(&root);
    let g = doc.timeline(0).layers.iter().find(|l| l.name == "G").unwrap();
    let kid = doc.timeline(0).layers.iter().find(|l| l.name == "Kid").unwrap();
    assert_eq!(kid.parent, Some(g.id), "the child points at the folder");
    // The container's static transform lives on the folder as channels (inch → px, y flip).
    assert_eq!(g.translation.keys.first().map(|k| k.val), Some(Vec2::new(160.0, 80.0)));
    assert_eq!(g.scale.keys.first().map(|k| k.val), Some(Vec2::new(2.0, 1.0)));
    assert_eq!(g.origin, Vec2::new(960.0, 40.0), "the origin maps as a point: 1 inch right of centre, 1 inch up");
    // Opacity keys arrive as a channel.
    assert_eq!(g.opacity.keys.len(), 2);
    assert!((g.opacity.sample_or(10.0, 1.0) - 0.2).abs() < 1e-6);
    // The shadow stack: blur 0.05"→16 px, offset 0.1"→32 px, expansion 0.02"→6.4 px.
    assert_eq!(g.filters.len(), 1);
    let Some(anicraft_doc::Filter::DropShadow { blur_x, distance, expansion, noise, threshold, .. }) = g.filters.first() else { panic!("a shadow") };
    assert!((blur_x - 16.0).abs() < 1e-4 && (distance - 32.0).abs() < 1e-4 && (expansion - 6.4).abs() < 1e-3);
    assert!((noise - 0.5).abs() < 1e-6 && *threshold);
    // The child carries none of it: nothing bakes.
    assert!(kid.translation.keys.is_empty());
}

#[test]
fn animated_transform_channels_import_and_sample() {
    let mut root = minimal();
    // Linear keys at frames 0/12/24, inches → pixels (320/in, y flipped) on the layer.
    root["layers"][0]["transforms"] = json!({
        "translation": channel("Vec3", vec![0, 12, 24], vec![json!({"x": 0.0, "y": 0.0, "z": 0.0}), json!({"x": 1.0, "y": -0.5, "z": 0.0}), json!({"x": 2.0, "y": 0.0, "z": 0.0})]),
        "rotation_z": channel("Val", vec![0, 24], vec![json!(0.0), json!(std::f64::consts::PI)])
    });
    // The camera zooms 2 → 3 over the range; a marker at frame 6.
    root["animated_values"] = json!({
        "camera_zoom": channel("Val", vec![0, 48], vec![json!(2.0), json!(3.0)]),
        "timeline_markers": channel("String", vec![-1000000, 6], vec![json!(""), json!("Kick")])
    });
    let (doc, report) = import(&root);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let l = &doc.timeline(0).layers[0];
    // The layer carries the channels; the elements keep only the inch→pixel mapping.
    assert_eq!(l.translation.keys.iter().map(|k| k.val.x).collect::<Vec<_>>(), vec![0.0, 320.0, 640.0]);
    assert_eq!(l.translation.keys[1].val.y, 160.0, "y flips with the coordinate system");
    assert!((l.rotation.keys[1].val - -std::f32::consts::PI).abs() < 1e-5, "rotation flips sign: {:?}", l.rotation.keys[1].val);
    // Frame-by-frame sampling (the parity gate's in-repo half): linear segments exact.
    let x = |f: u32| l.translation.sample_or(f as f32, anicraft_geom::Vec2::ZERO).x;
    for f in 0..=24u32 {
        assert!((x(f) - 26.666668 * f as f32).abs() < 1e-4, "frame {f}: {}", x(f));
    }
    assert!((l.rotation.sample_or(12.0, 0.0) - -std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    // The camera zooms 2 → 3, so the view scale runs 1 → 1.5.
    let cam = &doc.scenes[0].camera;
    assert!(!cam.is_neutral());
    assert!((cam.zoom.sample_or(0.0, 2.0) - 2.0).abs() < 1e-6);
    assert!((cam.zoom.sample_or(48.0, 2.0) - 3.0).abs() < 1e-6);
    assert!((cam.zoom.sample_or(24.0, 2.0) - 2.5).abs() < 1e-6);
    assert_eq!(doc.scenes[0].markers, vec![(6, "Kick".into())]);
}

#[test]
fn image_layers_find_their_file_or_report_it() {
    let mut root = minimal();
    root["layers"][0] = json!({"type": "ImageLayer", "name": "Photo", "fileref": {"path": "media/photo.png", "relativeTo": "MOHOProject"}});
    let read = |p: &str| -> Result<Option<Vec<u8>>, crate::MohoError> {
        if p == "media/photo.png" {
            // A 1×1 red PNG.
            let mut bytes: Vec<u8> = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().map_err(|e| crate::MohoError::NotMoho(e.to_string()))?;
                writer.write_image_data(&[255, 0, 0, 255]).map_err(|e| crate::MohoError::NotMoho(e.to_string()))?;
            }
            return Ok(Some(bytes));
        }
        Ok(None)
    };
    let (doc, report) = import_json(root.clone(), &read, None).unwrap();
    assert!(report.skipped.is_empty(), "the image is in the package");
    let bitmap = &doc.bitmaps[0];
    assert_eq!((bitmap.width, bitmap.height), (1, 1));
    let el = &doc.timeline(0).layers[0].keyframes[0].elements[0];
    let anicraft_doc::ElementKind::Shape(s) = &el.kind else { panic!("image element") };
    assert!(matches!(s.fill, Some(anicraft_doc::Paint::Bitmap { .. })));
    // Missing file: the layer stays, empty, and the report says what happened.
    root["layers"][0]["fileref"]["path"] = json!("media/gone.png");
    let read_none = |_: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok(None) };
    let (doc, report) = import_json(root, &read_none, None).unwrap();
    assert!(report.skipped.contains_key("missing image files"));
    assert!(doc.timeline(0).layers[0].keyframes[0].elements.is_empty());
}

#[test]
fn containers_and_garbage_fail_cleanly() {
    // Not a Moho document.
    let read = |_: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok(None) };
    assert!(import_json(json!({"mime_type": "application/json"}), &read, None).is_err());
    assert!(import_json(json!({"layers": []}), &read, None).is_err(), "Moho refuses empty layer lists");
    assert!(import_json(json!({"major_version": 2, "layers": [{}]}), &read, None).is_err());
    // Hostile values: NaN positions, out-of-range indices, deep nesting — reports, no panic.
    let mut hostile = minimal();
    hostile["layers"][0]["mesh"] = json!({
        "points": [{"position": channel("Vec2", vec![0], vec![json!(null)])}],
        "curves": [{"closed": false, "points": [{"point": 99}]}],
        "shapes": [{"has_fill": true, "edges": {"curve": [7], "segment": [3], "flag": [0]}}]
    });
    let (_, report) = import_json(hostile, &read, None).unwrap();
    assert!(!report.skipped.is_empty());
    // A deep layer nest stops at the depth cap.
    let mut deep = minimal();
    let mut layer = json!({"type": "MeshLayer", "name": "deepest", "mesh": {"points": [], "curves": [], "shapes": []}});
    for _ in 0..40 {
        layer = json!({"type": "GroupLayer", "name": "g", "layers": [layer]});
    }
    deep["layers"] = json!([layer]);
    let (_, report) = import_json(deep, &read, None).unwrap();
    assert!(report.skipped.contains_key("layers nested deeper than 16"));
}

#[test]
fn a_zip_package_reads_like_a_bare_file() {
    let project = serde_json::to_vec(&minimal()).unwrap();
    let zip = zip::tests::make_zip(&[("Project.mohoproj", &project, true), ("preview.jpg", b"jpeg", false)]);
    let archive = zip::Archive::open(&zip).unwrap();
    let read = |p: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok(archive.read(p)?) };
    let (doc, _) = crate::import::import_with(&read).unwrap();
    assert_eq!(doc.stage.width, 1280);
    // The same bytes through the disk path (a temp .moho package).
    let path = std::env::temp_dir().join(format!("anicraft-moho-test-{}.moho", std::process::id()));
    std::fs::write(&path, &zip).unwrap();
    let (doc2, _) = crate::import_path(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(doc, doc2);
}

fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    }
    out
}

fn import_with_media(root: &Value, name: &str, bytes: Vec<u8>) -> (anicraft_doc::Document, Report) {
    let read = move |p: &str| -> Result<Option<Vec<u8>>, crate::MohoError> { Ok((p == name).then_some(bytes.clone())) };
    import_json(root.clone(), &read, None).unwrap_or_else(|e| panic!("import failed: {e}"))
}

/// The image-layer mesh warp (ADR 0106): a 2-inch-wide image layer whose 4-point cage
/// becomes two textured triangles; a corner with an animated position channel rides both
/// triangles as point tracks with pinned image coordinates, and evaluation re-aims the
/// bitmap matrix per frame.
#[test]
fn image_meshes_warp_their_picture() {
    let mut root = minimal();
    root["layers"][0] = json!({
        "type": "ImageLayer", "name": "Warp",
        "image_fileref": {"path": "media/tex.png", "relativeTo": "MOHOProject"},
        "width": 2.0, "height": 1.0,
        "mesh": {
            "points": [
                {"position": channel("Vec2", vec![0], vec![json!({"x": -1.0, "y": 0.5})])},
                {"position": channel("Vec2", vec![0, 12], vec![json!({"x": 1.0, "y": 0.5}), json!({"x": 1.5, "y": 0.5})])},
                {"position": channel("Vec2", vec![0], vec![json!({"x": 1.0, "y": -0.5})])},
                {"position": channel("Vec2", vec![0], vec![json!({"x": -1.0, "y": -0.5})])}
            ],
            "curves": [{"closed": true, "points": [
                {"point": 0, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 1, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 2, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 3, "smoothness": channel("Val", vec![0], vec![json!(0.0)])}
            ]}],
            "shapes": [{"has_fill": false, "has_outline": false, "edges": {"curve": [0, 0, 0, 0], "segment": [0, 1, 2, 3], "flag": [0, 0, 0, 0]}}]
        }
    });
    let (doc, report) = import_with_media(&root, "media/tex.png", tiny_png());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    assert_eq!(els.len(), 2, "the quad cage splits into two triangles");
    // Both pieces are textured triangles whose one animated corner carries its channel.
    for el in els {
        let anicraft_doc::ElementKind::Shape(s) = &el.kind else { panic!("a shape") };
        assert!(s.texture.is_some(), "the image coordinates ride the piece");
        assert_eq!(s.points.len(), 1, "one animated corner per piece");
        assert_eq!(s.texture.as_ref().unwrap().uv.len(), 3);
        assert_eq!(anicraft_doc::path_point_count(&s.path), 3);
    }
    // The two pieces share the animated corner's channel, so the seam stays welded.
    let at = |f: u32| -> Vec<anicraft_geom::Vec2> {
        els.iter().map(|el| {
            let anicraft_doc::ElementKind::Shape(s) = &el.kind else { panic!("a shape") };
            s.points[0].position.sample(f as f32).unwrap()
        }).collect()
    };
    let (a0, b0) = (at(0), at(12));
    assert!((a0[0] - b0[0]).length() > 0.4, "the corner moves between the frames");
    // Evaluation: frame 12's paths land where the channels say (stage pixels).
    let dl = anicraft_doc::evaluate(&doc, 12);
    assert_eq!(dl.items.len(), 2);
    // Paths and fill matrices live in shape space (Moho inches, y up); the element
    // transform carries them to the stage.
    let inch = [(-1.0_f32, 0.5_f32), (1.5, 0.5), (1.0, -0.5), (-1.0, -0.5)];
    let uv = [(0.0_f32, 0.0_f32), (2.0, 0.0), (2.0, 1.0), (0.0, 1.0)];
    for it in &dl.items {
        let PathCmd::MoveTo(p) = it.path.cmds[0] else { panic!("move") };
        assert!(inch.iter().any(|(x, y)| (p - anicraft_geom::Vec2::new(*x, *y)).length() < 1e-3), "triangle corner at the deformed position: {p:?}");
        // The fill is a bitmap whose matrix aims the image at the deformed piece.
        let anicraft_doc::Paint::Bitmap { matrix, bitmap, .. } = it.fill.clone().unwrap() else { panic!("bitmap fill") };
        assert_eq!(bitmap.0, doc.bitmaps[0].id.0);
        let mut hit = false;
        for (u, (x, y)) in uv.iter().zip(inch) {
            if (matrix.apply(anicraft_geom::Vec2::new(u.0, u.1)) - anicraft_geom::Vec2::new(x, y)).length() < 1e-2 {
                hit = true;
            }
        }
        assert!(hit, "a pinned uv lands on its deformed corner");
    }
}

/// An undeformed image layer keeps the plain picture, sized by the layer's own width/height
/// (inches) and offset by its PSD translation.
#[test]
fn plain_image_layers_draw_their_picture_sized_and_placed() {
    let mut root = minimal();
    root["layers"][0] = json!({
        "type": "ImageLayer", "name": "Photo",
        "image_fileref": {"path": "media/tex.png", "relativeTo": "MOHOProject"},
        "width": 1.0, "height": 0.5,
        "psd_layer_translation": {"x": 0.5, "y": 0.25}
    });
    let (doc, report) = import_with_media(&root, "media/tex.png", tiny_png());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    let el = &doc.timeline(0).layers[0].keyframes[0].elements[0];
    let anicraft_doc::ElementKind::Shape(s) = &el.kind else { panic!("a shape") };
    let anicraft_doc::Paint::Bitmap { matrix, .. } = s.fill.as_ref().unwrap() else { panic!("bitmap fill") };
    // The image (2×1 px) fills a 1×0.5-inch box whose centre sits at the PSD translation.
    // Image y runs down, layer y up: the image's top edge sits at centre.y + h/2.
    let tl = matrix.apply(anicraft_geom::Vec2::new(0.0, 0.0));
    let br = matrix.apply(anicraft_geom::Vec2::new(2.0, 1.0));
    assert!((tl.x - 0.0).abs() < 1e-3 && (tl.y - 0.5).abs() < 1e-3, "top-left at (centre.x-w/2, centre.y+h/2): {tl:?}");
    assert!((br.x - 1.0).abs() < 1e-3 && (br.y - 0.0).abs() < 1e-3, "{br:?}");
}

fn tiny_wav() -> Vec<u8> {
    // A 16-bit mono PCM WAV, 8000 Hz, 8 bytes of silence.
    let data = [0u8; 8];
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&8000u32.to_le_bytes());
    out.extend_from_slice(&8000u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}


/// An audio layer becomes a library sound and a Stream sound on its keyframe (ADR 0106,
/// on the upstream audio model): the timeline shows the sound's length, and the report
/// stays clean.
#[test]
fn audio_layers_become_keyframe_sounds() {
    let mut root = minimal();
    root["layers"][0] = json!({
        "type": "AudioLayer", "name": "Boom",
        "audio_path": "media/boom.wav"
    });
    let (doc, report) = import_with_media(&root, "media/boom.wav", tiny_wav());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(doc.sounds.len(), 1, "the file decoded into the library");
    let l = &doc.timeline(0).layers[0];
    assert!(l.has_sounds());
    let k = &l.keyframes[0];
    let snd = k.sound.as_ref().unwrap();
    assert_eq!(snd.sync, anicraft_doc::SoundSync::Stream, "a Moho audio layer plays tied to the timeline");
    assert_eq!(doc.sound(snd.sound).map(|s| s.frames), Some(4));
    // The span covers the sound's frames at the project rate (8000 Hz over 24 fps < 1 frame → 1).
    assert_eq!(k.duration, 1);
    assert!(k.elements.is_empty(), "audio layers draw nothing");
}

/// An audio file that can't decode is counted, not silently dropped.
#[test]
fn undecodable_audio_files_are_counted() {
    let mut root = minimal();
    root["layers"][0] = json!({
        "type": "AudioLayer", "name": "Mystery",
        "audio_path": "media/song.aiff"
    });
    let (doc, report) = import_with_media(&root, "media/song.aiff", b"FORM????AIFF".to_vec());
    assert!(doc.sounds.is_empty());
    assert_eq!(report.skipped.get("audio files that are not WAV, Ogg Vorbis or MP3"), Some(&1), "{:?}", report.skipped);
}

/// The document soundtrack comes across as a library sound with a note.
#[test]
fn the_soundtrack_lands_in_the_library() {
    let mut root = minimal();
    root["project_data"]["soundtrack"] = json!("media/boom.wav");
    let (doc, report) = import_with_media(&root, "media/boom.wav", tiny_wav());
    assert_eq!(doc.sounds.len(), 1, "the soundtrack decoded into the library");
    assert!(report.notes.iter().any(|n| n.contains("soundtrack")), "{:?}", report.notes);
    assert!(!report.skipped.contains_key("document soundtrack"), "{:?}", report.skipped);
}

/// Physics (ADR 0106): a physics-enabled group's gravity/wind becomes the scene's world,
/// its body layers carry the body values (converted from inches), and Moho's own baked
/// `physics_nudge` channel merges into the layer's translation keys.
#[test]
fn physics_worlds_bodies_and_baked_nudges_import() {
    let mut root = minimal();
    root["layers"][0] = json!({
        "type": "GroupLayer", "name": "World",
        "enable_physics": channel("Bool", vec![0], vec![json!(true)]),
        "gravity": {"x": 0.0, "y": -10.0},
        "layers": [{
            "type": "MeshLayer", "name": "Ball",
            "physics": {"enabled": true, "static": false, "density": 1.0, "friction": 0.3, "restitution": 0.5, "velocity": {"x": 1.0, "y": 0.0}},
            "transforms": {"translation": channel("Vec3", vec![0], vec![json!({"x": 1.0, "y": 0.0, "z": 0.0})]),
                           "physics_nudge": channel("Vec2", vec![0, 6], vec![json!({"x": 0.0, "y": 0.0}), json!({"x": 2.0, "y": -1.0})])},
            "mesh": {"points": [
                {"position": channel("Vec2", vec![0], vec![json!({"x": -0.5, "y": 0.5})])},
                {"position": channel("Vec2", vec![0], vec![json!({"x": 0.5, "y": 0.5})])},
                {"position": channel("Vec2", vec![0], vec![json!({"x": 0.5, "y": -0.5})])},
                {"position": channel("Vec2", vec![0], vec![json!({"x": -0.5, "y": -0.5})])}
            ], "curves": [{"closed": true, "points": [
                {"point": 0, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 1, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 2, "smoothness": channel("Val", vec![0], vec![json!(0.0)])},
                {"point": 3, "smoothness": channel("Val", vec![0], vec![json!(0.0)])}
            ]}], "shapes": [{"has_fill": true, "has_outline": false, "edges": {"curve": [0, 0, 0, 0], "segment": [0, 1, 2, 3], "flag": [0, 0, 0, 0]},
                "style": {"fill_color": channel("Color", vec![0], vec![json!({"r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0})])}}]}
        }]
    });
    let (doc, report) = import(&root);
    let scene = &doc.scenes[0];
    let world = scene.physics.expect("the group's world became the scene's");
    // Gravity converts from Moho inches/s² y-up (0, −10) to stage px/s² y-down: (0, +3200).
    assert!(world.gravity.x.abs() < 1e-3 && (world.gravity.y - 3200.0).abs() < 1e-3, "{:?}", world.gravity);
    let ball = scene.timeline.layers.iter().find(|l| l.name == "Ball").unwrap();
    let body = ball.physics.expect("the body layer carries its body");
    assert!(!body.static_body);
    assert!((body.friction - 0.3).abs() < 1e-6 && (body.restitution - 0.5).abs() < 1e-6);
    // The start velocity converted (1 inch/s → +x 320 px/s, y up → y down).
    assert!((body.velocity.x - 320.0).abs() < 1e-3, "{:?}", body.velocity);
    // The baked nudge merged into the translation: key 0 = the translation's own (320, 0),
    // key 6 = translation + nudge (Moho's (2, −1) inches convert y-up→down to (640, +320)).
    assert_eq!(ball.translation.keys.len(), 2, "{:?}", ball.translation.keys);
    let k6 = &ball.translation.keys[1];
    assert_eq!(k6.frame, 6);
    assert!((k6.val.x - 960.0).abs() < 1e-3 && (k6.val.y - 320.0).abs() < 1e-3, "{:?}", k6.val);
    // The fixture's Smooth-interp keys re-key linearly, and the report says so.
    assert_eq!(report.skipped.get("physics nudge easing (the merged keys are linear)"), Some(&1), "{:?}", report.skipped);
    // And our own bake reproduces the drop under this world: bodies simulate.
    let baked = anicraft_doc::bake_physics(&doc, 0).unwrap();
    let y5 = baked.position(ball.id, 5).unwrap().y;
    assert!(y5 > 0.0, "the ball fell by frame 5: {y5}");
}

/// Physics off (or no enabled group) leaves no world, and the non-held body options are
/// counted.
#[test]
#[allow(clippy::approx_constant)] // Moho's own bytes carry 3.141593, its shortened π
fn physics_off_or_unheld_options_report() {
    let mut root = minimal();
    root["layers"][0] = json!({
        "type": "GroupLayer", "name": "Plain",
        "enable_physics": channel("Bool", vec![0], vec![json!(false)]),
        "layers": [{
            "type": "MeshLayer", "name": "Thing",
            "physics": {"enabled": true, "static": true, "density": 2.5, "sleeping": true, "respawn": 3, "enable_motor": true},
            "mesh": {"points": [], "curves": [], "shapes": []}
        }]
    });
    let (doc, report) = import(&root);
    assert!(doc.scenes[0].physics.is_none(), "no enabled group → no world");
    let thing = &doc.scenes[0].timeline.layers[1];
    let body = thing.physics.expect("the body imports even without a world");
    assert!(body.static_body);
    assert!(report.skipped.contains_key("physics density ratios (bodies weigh the same)"), "{:?}", report.skipped);
    assert!(report.skipped.contains_key("physics motors (the body does not spin)"), "{:?}", report.skipped);
    // Moho's constant default motor keys (the same value at two frames, motor off) are
    // not a motor: the corpus bodies carry them and import uncounted.
    let mut constant = minimal();
    constant["layers"][0] = json!({
        "type": "GroupLayer", "name": "Plain",
        "layers": [{
            "type": "MeshLayer", "name": "Coasting",
            "physics": {"enabled": true, "enable_motor": false, "motor_speed": channel("Val", vec![0, 167], vec![json!(3.141593), json!(3.141593)]), "motor_torque": 10000.0},
            "mesh": {"points": [], "curves": [], "shapes": []}
        }]
    });
    let (_, constant_report) = import(&constant);
    assert!(!constant_report.skipped.keys().any(|k| k.contains("motor")), "an off motor with constant keys is not a motor: {:?}", constant_report.skipped);
    assert!(report.skipped.contains_key("physics body options (sleep, respawn, pivot)"), "{:?}", report.skipped);
}
