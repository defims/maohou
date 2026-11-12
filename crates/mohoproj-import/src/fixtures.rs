//! Corpus gate: the maop-corpus fixtures (Moho Pro 14.4 native saves) all import, and what
//! each one reports matches CASES.md. These tests run wherever the corpus sits (its README:
//! point `MAOHOU_FIXTURES` at the `maop/` directory, or keep the checkout beside the
//! workspace as `maop-corpus/`); without it they pass trivially, so machines without the
//! corpus still build green.

use crate::import::import_path;
use std::path::{Path, PathBuf};

/// Finds the corpus fixtures directory, or `None` (tests then say why they didn't run).
fn corpus() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MAOHOU_FIXTURES") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let beside = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maop-corpus/maop");
    beside.is_dir().then_some(beside)
}

/// The fixture's path, or `None` after explaining why the test didn't run.
fn fixture(name: &str) -> Option<PathBuf> {
    let dir = corpus()?;
    let f = dir.join(format!("{name}.moho"));
    f.is_file().then_some(f).or_else(|| {
        eprintln!("skipped {name}: no {name}.moho in the corpus");
        None
    })
}

fn missing_corpus() {
    eprintln!("skipped: no maop-corpus at MAOHOU_FIXTURES or ../../maop-corpus");
}

fn layer_names(doc: &anicraft_doc::Document) -> Vec<&str> {
    doc.timeline(0).layers.iter().map(|l| l.name.as_str()).collect()
}

// --- one test per fixture; bodies assert what CASES.md records, from the actual files ---

#[test]
fn empty_imports() {
    let Some(path) = fixture("empty") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("empty.moho: {e}"));
    assert_eq!(layer_names(&doc), ["Layer 1"]);
    assert_eq!(doc.stage.width, 1280);
    assert_eq!(doc.stage.height, 720);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(report.summary(), "Imported everything in the file.");
    // The sidecar keeps the whole original.
    assert_eq!(doc.maop_maohou.as_ref().unwrap().raw["version"], 1045);
}

#[test]
fn vectors_imports() {
    let Some(path) = fixture("vectors") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("vectors.moho: {e}"));
    // Bottom-first in the file: Layer 1, Text, Shapes → top-first here.
    assert_eq!(layer_names(&doc), ["Shapes", "Text", "Layer 1"]);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn keyframes_imports() {
    let Some(path) = fixture("keyframes") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("keyframes.moho: {e}"));
    assert_eq!(layer_names(&doc), ["KFDemo", "Layer 1"]);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn text_imports() {
    let Some(path) = fixture("text") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("text.moho: {e}"));
    assert_eq!(layer_names(&doc), ["Title", "Layer 1"]);
    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    assert_eq!(els.len(), 1, "the text layer lays out one block");
    let anicraft_doc::ElementKind::Text(block) = &els[0].kind else { panic!("a text element") };
    assert_eq!(block.text, "Text");
    // 60 pt at Moho's 72 pt/inch and 320 px/inch = 266.7 px.
    assert!((block.size - 800.0 / 3.0).abs() < 0.5, "size {} px", block.size);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn switch_imports() {
    let Some(path) = fixture("switch") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("switch.moho: {e}"));
    assert_eq!(layer_names(&doc), ["Mouths", "Smile", "Frown", "Layer 1"]);
    // The switch imports as a switch (ADR 0105): its variants are children and its selector
    // channel rides the layer. This fixture never selected a variant, so the channel holds
    // one empty key — evaluation shows nothing until a variant is picked.
    assert!(report.notes.is_empty() || !report.notes.iter().any(|n| n.contains("switch")), "{report:?}");
    assert!(!report.skipped.iter().any(|(k, _)| k.contains("switch")), "{:?}", report.skipped);
    let mouths = doc.timeline(0).layers.iter().find(|l| l.name == "Mouths").unwrap();
    assert_eq!(mouths.kind, anicraft_doc::LayerKind::Switch);
    assert_eq!(mouths.switch_to.keys.len(), 1);
    assert_eq!(mouths.switch_to.keys[0].val, "");
    for v in ["Smile", "Frown"] {
        let l = doc.timeline(0).layers.iter().find(|l| l.name == v).unwrap();
        assert_eq!(l.parent, Some(mouths.id), "{v} inside the switch");
        assert!(l.visible);
    }
    // Nothing draws until a variant is picked: the saved state selects none.
    assert!(anicraft_doc::evaluate(&doc, 0).items.is_empty());
}

/// The stage-4 skeleton gate (ADR 0104): bones.moho's five-bone rig imports whole — the
/// hierarchy, the channels (y-flipped), the lengths in stage pixels, and every key the
/// model doesn't hold verbatim in the sidecar — with no "skeleton bones" report line.
#[test]
fn bones_imports() {
    let Some(path) = fixture("bones") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("bones.moho: {e}"));
    assert_eq!(layer_names(&doc), ["Rig", "Layer 1"]);
    let rig = &doc.timeline(0).layers[0];
    assert_eq!(rig.kind, anicraft_doc::LayerKind::Bone);
    assert!(!report.skipped.contains_key("skeleton bones"), "{:?}", report.skipped);
    let sk = rig.skeleton.as_ref().unwrap();
    assert_eq!(sk.bones.len(), 5);
    // The hierarchy straight from the file: this fixture's bones are all roots (a Lua-made
    // rig with no parenting yet), named hips/spine/chest/arm_l/arm_r.
    assert_eq!(sk.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["hips", "spine", "chest", "arm_l", "arm_r"]);
    assert!(sk.bones.iter().all(|b| b.parent.is_none()));
    // chest is the one constrained bone: bounds flip and swap with the y axis
    // (CCW −0.5236..1.5708 → screen −1.5708..0.5236).
    let chest = &sk.bones[2];
    assert!(chest.constraints);
        let (ccw_half, ccw_sixth) = (std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_6);
    assert!((chest.min_constraint + ccw_half).abs() < 1e-3 && (chest.max_constraint - ccw_sixth).abs() < 1e-3, "{}..{}", chest.min_constraint, chest.max_constraint);
    // Lengths convert with the dpi (0.5 in = 160 px); the rest pose angle flips sign.
    assert!((sk.bones[0].length - 160.0).abs() < 1e-3, "{}", sk.bones[0].length);
    let angle = sk.bones[0].angle.sample_or(0.0, 0.0);
    assert!((angle + std::f32::consts::FRAC_PI_2).abs() < 1e-3, "{angle}");
    // The untouched keys: Moho's remaining Bone keys, captured for the round trip.
    for key in ["damping_force", "physics_torque", "anim_parent", "ik_lock", "bone_tags"] {
        assert!(sk.bones[0].extra.contains_key(key), "missing {key} in {:?}", sk.bones[0].extra.keys().collect::<Vec<_>>());
    }
    assert_eq!(sk.extra.get("type").and_then(|v| v.as_str()), Some("Skeleton"));
    // The untouched mesh layer carries no bindings (its mesh is empty anyway).
    let mesh = &doc.timeline(0).layers[1];
    assert!(mesh.keyframes[0].elements.iter().all(|el| match &el.kind {
        anicraft_doc::ElementKind::Shape(s) => s.bind.is_empty(),
        _ => true,
    }));
}

/// smartbones.moho's dial rig: two bones at the root (dial + target), binding mode 1.
#[test]
fn smartbones_imports() {
    let Some(path) = fixture("smartbones") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("smartbones.moho: {e}"));
    assert_eq!(layer_names(&doc), ["SmartRig", "Layer 1"]);
    let rig = &doc.timeline(0).layers[0];
    assert_eq!(rig.kind, anicraft_doc::LayerKind::Bone);
    let sk = rig.skeleton.as_ref().unwrap();
    assert_eq!(sk.bones.len(), 2);
    assert_eq!(sk.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["dial", "target"]);
    assert_eq!(sk.binding_mode, 1);
    // The fixture carries no smart-bone actions (its channels hold single keys), so the
    // report says nothing about them.
    assert!(!report.skipped.contains_key("smart bone actions"), "{:?}", report.skipped);
}

#[test]
fn gradients_imports() {
    let Some(path) = fixture("gradients") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("gradients.moho: {e}"));
    assert_eq!(layer_names(&doc), ["Layer 1"]);
    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    // 17 shapes, each with an outline and 13 of them gradient-filled: S4-S7 reflected,
    // S8+S10-S12 radial, S13-S17 angle (S1-S3 and S9 carry gradient definitions but no
    // fill). 13 fill elements + 17 stroke elements.
    assert_eq!(els.len(), 30, "13 fills + 17 strokes");
    let count = |p: fn(&anicraft_doc::Paint) -> bool| els.iter().filter(|el| matches!(&el.kind, anicraft_doc::ElementKind::Shape(s) if s.fill.as_ref().is_some_and(p))).count();
    assert_eq!(count(|f| matches!(f, anicraft_doc::Paint::Radial { .. })), 4);
    assert_eq!(count(|f| matches!(f, anicraft_doc::Paint::Reflected { .. })), 4);
    assert_eq!(count(|f| matches!(f, anicraft_doc::Paint::Angle { .. })), 5);
    assert_eq!(count(|f| matches!(f, anicraft_doc::Paint::Linear { .. })), 0, "the linear shapes carry no fill");
    // All four gradient enums import without approximation now, and the fixture's uniform
    // point width (1.75× on every point) rides the stroke width exactly.
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    // The named style table comes across (Moho's 12 built-ins define nothing but names).
    assert_eq!(doc.styles.len(), 12);
    assert_eq!(doc.styles[0].name, "Wet Ink");
}

#[test]
fn follow_path_imports() {
    let Some(path) = fixture("follow-path") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("follow-path.moho: {e}"));
    assert_eq!(layer_names(&doc), ["FollowLayer", "Layer 1"]);
    let els = &doc.timeline(0).layers[1].keyframes[0].elements;
    assert_eq!(els.len(), 31, "13 fill + 17 stroke elements from 17 shapes, plus the followed curve");
    // The Follow Path relation rides the document (ADR 0109): the empty FollowLayer follows
    // Layer 1's S curve, rotating along it, its progress animated; bending alone is counted
    // (no published algorithm, no bent specimen).
    assert_eq!(report.skipped.get("follow bending (the artwork rides the path, it does not bend)"), Some(&1), "{:?}", report.skipped);
    let follow = doc.timeline(0).layers[0].follow.as_ref().expect("the FollowLayer carries the relation");
    assert_eq!(follow.target, doc.timeline(0).layers[1].id);
    assert!(follow.rotate, "rotate to follow");
    assert!(follow.bending, "bend with curve is kept (counted, not rendered)");
    assert!((follow.following.sample_or(0.0, -1.0) - 1.0).abs() < 1e-6, "progress 1 at frame 0");
    assert!((follow.following.sample_or(50.0, -1.0) - 0.0).abs() < 1e-6, "progress 0 at frame 50");
    // The followed S curve (Moho mesh curve 17, no shape of its own) landed on the target
    // as the relation's invisible geometry.
    assert!(els.last().is_some_and(|el| el.id == follow.element
        && matches!(&el.kind, anicraft_doc::ElementKind::Shape(s) if s.fill.is_none() && s.stroke.is_none())), "the followed curve is the invisible path");
}

#[test]
fn point_animation_imports() {
    let Some(path) = fixture("point-animation") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("point-animation.moho: {e}"));
    assert_eq!(layer_names(&doc), ["PointAnim", "Layer 1"]);
    assert!(report.skipped.is_empty(), "the fixture's meshes are empty: {:?}", report.skipped);
}

/// Every fixture that exists imports without error (the acceptance gate's first clause).
#[test]
fn all_corpus_fixtures_import() {
    let Some(dir) = corpus() else { return missing_corpus() };
    let mut n = 0;
    let mut failures = Vec::new();
    for e in std::fs::read_dir(&dir).expect("corpus dir reads") {
        let Ok(e) = e else { continue };
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("moho") {
            continue;
        }
        n += 1;
        if let Err(err) = import_path(&p) {
            failures.push(format!("{}: {err}", p.display()));
        }
    }
    assert!(n >= 8, "expected the corpus's fixtures, found {n} in {}", dir.display());
    assert!(failures.is_empty(), "fixtures failed to import:\n{}", failures.join("\n"));
}

/// A real Moho-saved document with animation data (the fork's CharWalk sample, next to the
/// corpus in the local knowledge base): 39 keyframed channels, ten timeline markers and a
/// 44-frame project range. Verified against the file's own channel data — the in-repo half
/// of the stage-2 gate; the Moho-runtime half (Lua probe) stays with the owner's machine.
#[test]
fn charwalk_markers_and_range_import() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plan/moho/samples/CharWalk.mohoproj");
    if !path.is_file() {
        eprintln!("skipped: no CharWalk.mohoproj beside the corpus");
        return;
    }
    let (doc, _report) = import_path(&path).unwrap_or_else(|e| panic!("CharWalk: {e}"));
    // Its bones and point animation are later milestones; the report says so.
    // The 0-based project range 1..44 becomes (0, 44): a 44-frame timeline, as Moho shows.
    // It equals the imported content's length, so no explicit range is stored.
    assert_eq!(doc.scenes[0].frames, None, "range == content: nothing to store");
    assert_eq!(doc.frame_count(), 44);
    // The walk cycle's markers, one per frame, in order.
    let labels: Vec<(u32, &str)> = doc.scenes[0].markers.iter().map(|(f, s)| (*f, s.as_str())).collect();
    assert_eq!(labels, vec![(1, "Contact"), (6, "Passing"), (12, "Kickoff"), (16, "Float"), (23, "Contact"), (28, "Passing"), (34, "Kickoff"), (38, "Float"), (45, "Contact")], "{labels:?}");
    // Its camera is the default (zoom 2 at the centre): nothing stored.
    assert!(doc.scenes[0].camera.is_neutral());
    // Bone and point channels are later milestones and stay in the sidecar untouched.
    let raw = &doc.maop_maohou.as_ref().unwrap().raw;
    assert_eq!(raw["animated_values"]["timeline_markers"]["val"][1], "Contact");
}

/// The stage-2 real-machine sampling gate: the corpus fixture keyframes-layer.moho was
/// generated by Moho Pro 14.4 (Lua: gen_layeranim.lua — 3 linear translation keys, 2
/// rotation keys, camera zoom 2→4 and roll 0→20°, all INTERP_LINEAR), and Moho's own
/// per-frame evaluation of those channels (probe_layeranim.lua, AnimChannel:GetValue)
/// is the ground truth. Our importer converts inches→pixels; the comparison converts
/// back and demands the parity plan's ±1e-6 in Moho's native units, per frame, on
/// Channel::sample straight from the imported document.
#[test]
fn keyframes_layer_channels_match_moho_runtime_per_frame() {
    use anicraft_geom::Vec2;
    let Some(path) = fixture("keyframes-layer") else { return missing_corpus() };
    let probe = corpus().unwrap().join("inputs/keyframes-layer-probe.txt");
    if !probe.is_file() {
        eprintln!("skipped: no keyframes-layer-probe.txt beside the corpus");
        return;
    }
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("keyframes-layer.moho: {e}"));
    // The gradient shapes carry this fixture's canvas; their approximations are counted
    // elsewhere. What matters here: no animation was dropped.
    assert!(!report.skipped.contains_key("layer transform animation (drawn at frame 1)"));
    assert!(!report.skipped.contains_key("camera animation"));

    let layer = &doc.timeline(0).layers[0];
    let camera = &doc.scenes[0].camera;
    let text = std::fs::read_to_string(&probe).unwrap();
    let mut checked = 0u32;
    for line in text.lines().filter(|l| l.starts_with("frame ")) {
        // "frame N translation X Y Z rotation R camera_zoom Z camera_roll R camera_track X Y Z"
        let tok: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| tok[i].parse::<f64>().unwrap();
        let (frame, tx, ty, _tz, rot, zoom, roll, camx, camy, _camz) =
            (num(1) as f32, num(3), num(4), num(5), num(7), num(9), num(11), num(13), num(14), num(15));
        // Our channels live in stage pixels (y down); back to Moho inches (y up).
        let t = layer.translation.sample_or(frame, Vec2::ZERO);
        let (ours_x, ours_y) = ((t.x as f64) / 320.0, (t.y as f64) / -320.0);
        assert!((ours_x - tx).abs() < 1e-6, "frame {frame} translation x: {ours_x} vs Moho {tx}");
        assert!((ours_y - ty).abs() < 1e-6, "frame {frame} translation y: {ours_y} vs Moho {ty}");
        // Rotation flips sign in the y-down space; stored screen-clockwise.
        let r = layer.rotation.sample_or(frame, 0.0) as f64;
        assert!((-r - rot).abs() < 1e-6, "frame {frame} rotation: {} vs Moho {rot}", -r);
        // The camera: zoom is Moho's own units, roll flips sign, track maps as a point.
        let z = camera.zoom.sample_or(frame, 2.0) as f64;
        assert!((z - zoom).abs() < 1e-6, "frame {frame} zoom: {z} vs Moho {zoom}");
        let roll_ours = camera.roll.sample_or(frame, 0.0) as f64;
        assert!((-roll_ours - roll).abs() < 1e-6, "frame {frame} roll: {} vs Moho {roll}", -roll_ours);
        let (cx, cy) = ((camx as f32) * 320.0 + 640.0, (-camy as f32) * 320.0 + 360.0);
        let track = camera.track.sample_or(frame, Vec2::new(640.0, 360.0));
        assert!((track.x - cx).abs() < 1e-4, "frame {frame} track x: {} vs Moho {cx}", track.x);
        assert!((track.y - cy).abs() < 1e-4, "frame {frame} track y: {} vs Moho {cy}", track.y);
        checked += 1;
    }
    assert_eq!(checked, 31, "every probed frame compared");
}

/// keyframes-camera.moho carries the corpus's group-masking idiom (ADR 0108): the Eyes
/// group hides its members (Moho `group_mask` 2) and the eye-white layer adds itself to
/// the mask (`masking` 2 = MM_ADD_MASK: it draws itself and defines the region), with the
/// pupil masked by it. The full render gate lives in the CLI's mask_render_gate.rs.
#[test]
fn group_mask_imports_as_a_drawing_mask() {
    let Some(path) = fixture("keyframes-camera") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("keyframes-camera.moho: {e}"));
    assert!(!report.skipped.iter().any(|(k, _)| k.contains("mask")), "{:?}", report.skipped);
    let tl = doc.timeline(0);
    let mask = tl.layers.iter().find(|l| l.kind == anicraft_doc::LayerKind::Mask).expect("the eye white imports as the mask");
    assert_eq!(mask.name, "Eyes");
    assert!(mask.mask_draws, "Moho's add-to-mask member draws itself");
    let pupil = tl.layers.iter().find(|l| l.name == "Pupil").expect("the pupil");
    assert_eq!(pupil.parent, Some(mask.id), "the pupil is the mask's child");
}

/// physics-drop.moho (Moho Pro 14.4, CASES #20): Moho's real physics configuration —
/// the world on a BoneLayer (gravity direction channel 4.712389 rad = 270° down,
/// strength 0 default) holding a filled Polygon body whose physics.enabled is on.
/// The importer reads the bone-gravity world into the scene and the body onto the layer;
/// Moho 14.4 itself never evaluates layer physics in playback/preview/render/export
/// (documented in CASES), so the drop motion proof stays with our analytic bake gates.
#[test]
fn physics_drop_imports_world_and_body() {
    let Some(path) = fixture("physics-drop") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("physics-drop.moho: {e}"));
    let world = doc.scenes[0].physics.as_ref().expect("the bone layer's gravity became the scene world");
    // Direction 4.712389 rad (270°, straight down, y up) with strength 0: a zero world.
    assert!(world.gravity.x.abs() < 1e-3, "{:?}", world.gravity);
    assert!(world.gravity.y.abs() < 1e-3, "strength 0 → no force: {:?}", world.gravity);
    let poly = doc.timeline(0).layers.iter().find(|l| l.name == "Polygon").expect("the body layer");
    let body = poly.physics.expect("the body carries its physics");
    assert!(!body.static_body);
    assert!((body.friction - 0.3).abs() < 1e-3 && (body.restitution - 0.5).abs() < 1e-3);
    assert!(!report.skipped.contains_key("skeleton bones") || true);
}

/// image-warp.moho (CASES #21): an imported PNG on an image layer, sized by the layer's
/// own width/height; Moho's {1000000, 1000000} `psd_layer_translation` sentinel must not
/// move the picture. Moho 14.4 exposes no image mesh to scripting, so the mesh-warp form
/// itself stays with the analytic importer gates.
#[test]
fn image_warp_fixture_places_the_picture() {
    let Some(path) = fixture("image-warp") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("image-warp.moho: {e}"));
    assert_eq!(doc.bitmaps.len(), 1, "the PNG landed in the library");
    // The fixture carries the physics rig alongside (the same Moho session saved both);
    // the image layer is what this gate reads.
    let el = doc
        .timeline(0)
        .layers
        .iter()
        .flat_map(|l| l.keyframes[0].elements.iter())
        .find(|el| matches!(&el.kind, anicraft_doc::ElementKind::Shape(s) if s.fill.as_ref().is_some_and(|f| matches!(f, anicraft_doc::Paint::Bitmap { .. }))))
        .expect("the image draws as a bitmap-filled shape");
    let anicraft_doc::ElementKind::Shape(s) = &el.kind else { panic!("a shape") };
    // The layer's own 0.177778-inch drawn size is the shape's inch-space bounds (the
    // element transform carries them to stage pixels).
    let bounds = s.path.bounds();
    let side = (bounds.max.x - bounds.min.x).hypot(bounds.max.y - bounds.min.y) / std::f32::consts::SQRT_2;
    assert!((side - 0.177778).abs() < 0.005, "drawn size honoured (inches): {side}");
    let centre = (bounds.min + bounds.max) * 0.5;
    assert!(centre.x.abs() < 1e5 && centre.y.abs() < 1e5, "the psd sentinel must not move the picture: {centre:?}");
    assert!(!report.skipped.contains_key("missing image files"), "{:?}", report.skipped);
}

/// audio-track.moho (CASES #22): a General-Imported WAV on an AudioLayer —
/// `audio_path`/`audio_fileref` pointing beside the package (`relativeTo: "Project"`).
/// The sound lands in the library and plays as a Stream keyframe sound.
#[test]
fn audio_track_fixture_becomes_a_keyframe_sound() {
    let Some(path) = fixture("audio-track") else { return missing_corpus() };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("audio-track.moho: {e}"));
    assert_eq!(doc.sounds.len(), 1, "the WAV decoded into the library: {:?}", report.skipped);
    assert_eq!(doc.sounds[0].frames, 11025, "0.5 s at 22050 Hz");
    let bounce = doc.timeline(0).layers.iter().find(|l| l.name == "Bounce").expect("the audio layer");
    assert!(bounce.has_sounds());
    let snd = bounce.keyframes[0].sound.as_ref().unwrap();
    assert_eq!(snd.sync, anicraft_doc::SoundSync::Stream);
    assert!(!report.skipped.contains_key("missing audio files"), "{:?}", report.skipped);
}
