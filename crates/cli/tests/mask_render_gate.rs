//! The Moho group-masking gate (ADR 0108): keyframes-camera.moho was saved by Moho Pro
//! 14.4 with the rig's eyes masked the way real rigs do it — the "Eyes" group hides its
//! members (`group_mask` 2, the scripting API's "turn on masking, initially hiding all
//! sub-layers") and the eye-white layer adds itself to the mask (its `masking` is 2, the
//! scripting API's MM_ADD_MASK: the layer draws itself *and* defines the region its group
//! draws in). The pupil must therefore show only inside the white, and the white must show.
//!
//! This fixture has no Moho FileRender PNG in `inputs/` (the camera probe scripts rendered
//! frames, not this check), so the truth is the file's own geometry, rendered through the
//! CPU oracle — plus two negative controls that prove the checks can fail: unparent the
//! pupil and the black draws well past the white; take the mask's self-drawing away (Moho's
//! other mask form, MM_ADD_MASK_INVIS) and the white vanishes. Moho's own render of this
//! fixture stays with the corpus's real-machine probes, as for the camera gates.

use anicraft_doc::{ElementKind, LayerId, LayerKind};
use anicraft_geom::{Affine, FillRule, Vec2};
use anicraft_mohoproj_import::import_path;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// The stage-space bounds of a layer's shape fills (the importer bakes the layer's world
/// into the element transforms).
fn fill_bounds(doc: &anicraft_doc::Document, id: LayerId) -> anicraft_geom::Rect {
    let tl = doc.timeline(0);
    let l = tl.layer(id).unwrap();
    let mut acc: Option<anicraft_geom::Rect> = None;
    for kf in &l.keyframes {
        for el in &kf.elements {
            if let ElementKind::Shape(s) = &el.kind {
                let b = s.path.transformed(&el.transform).bounds();
                acc = Some(match acc {
                    Some(a) => a.union(&b),
                    None => b,
                });
            }
        }
    }
    acc.expect("the layer has shape fills")
}

/// The mask's fill area in stage pixels: inside it a clipped pupil may draw.
fn in_fill(doc: &anicraft_doc::Document, id: LayerId, p: Vec2) -> bool {
    let tl = doc.timeline(0);
    let l = tl.layer(id).unwrap();
    for kf in &l.keyframes {
        for el in &kf.elements {
            if let ElementKind::Shape(s) = &el.kind
                && s.path.contains(p, &el.transform, FillRule::NonZero)
            {
                return true;
            }
        }
    }
    false
}

/// The shortest distance from `p` to the mask's fill edge (its outline stroke draws this
/// far outside, so a dark pixel that close is still the eye white's own outline).
fn fill_distance(doc: &anicraft_doc::Document, id: LayerId, p: Vec2) -> f32 {
    let tl = doc.timeline(0);
    let l = tl.layer(id).unwrap();
    let mut best = f32::INFINITY;
    for kf in &l.keyframes {
        for el in &kf.elements {
            if let ElementKind::Shape(s) = &el.kind {
                best = best.min(s.path.distance_to_edge(p, &el.transform));
            }
        }
    }
    best
}

fn render(doc: &anicraft_doc::Document) -> (u32, Vec<u8>) {
    let (w, h) = (doc.stage.width, doc.stage.height);
    let dl = anicraft_doc::evaluate(doc, 0);
    let view = anicraft_render::View { transform: Affine::IDENTITY, width: w, height: h, background: None };
    let pm = anicraft_render::render(&dl, &view).unwrap().0;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in &pm.data {
        let a = p[3];
        let un = |c: f32| if a > 0.0 { (c / a).clamp(0.0, 1.0) } else { 0.0 };
        rgba.extend_from_slice(&[(un(p[0]) * 255.0) as u8, (un(p[1]) * 255.0) as u8, (un(p[2]) * 255.0) as u8, (a * 255.0) as u8]);
    }
    (w, rgba)
}

/// Dark (near-black) 2 px cells — the pupil's fill — within the mask's bounds, with how
/// far each sits from the mask's fill area (0 = inside it).
fn dark_cells(doc: &anicraft_doc::Document, id: LayerId, w: u32, rgba: &[u8]) -> Vec<(u32, u32, f32)> {
    let bounds = fill_bounds(doc, id);
    let (x0, y0) = (bounds.min.x.max(0.0) as u32, bounds.min.y.max(0.0) as u32);
    let x1 = (bounds.max.x.ceil() as u32).min(w);
    let (y1, step) = (bounds.max.y.ceil() as u32, 2u32);
    let mut out = Vec::new();
    let mut y = y0;
    while y + step <= y1 {
        let mut x = x0;
        while x + step <= x1 {
            let mut nb = 0u32;
            for dy in 0..step {
                for dx in 0..step {
                    let i = (((y + dy) * w + x + dx) * 4) as usize;
                    if rgba[i + 3] > 128 && rgba[i].max(rgba[i + 1]).max(rgba[i + 2]) < 70 {
                        nb += 1;
                    }
                }
            }
            if nb == step * step {
                let at = Vec2::new(x as f32 + step as f32 / 2.0, y as f32 + step as f32 / 2.0);
                let d = if in_fill(doc, id, at) { 0.0 } else { fill_distance(doc, id, at) };
                out.push((x, y, d));
            }
            x += step;
        }
        y += step;
    }
    out
}

/// Near-white 2 px cells — the drawn eye white's fill — within the mask's bounds.
fn white_cells(doc: &anicraft_doc::Document, id: LayerId, w: u32, rgba: &[u8]) -> usize {
    let bounds = fill_bounds(doc, id);
    let (x0, y0) = (bounds.min.x.max(0.0) as u32, bounds.min.y.max(0.0) as u32);
    let x1 = (bounds.max.x.ceil() as u32).min(w);
    let (y1, step) = (bounds.max.y.ceil() as u32, 2u32);
    let mut white = 0usize;
    let mut y = y0;
    while y + step <= y1 {
        let mut x = x0;
        while x + step <= x1 {
            let mut nw = 0u32;
            for dy in 0..step {
                for dx in 0..step {
                    let i = (((y + dy) * w + x + dx) * 4) as usize;
                    if rgba[i + 3] > 128 && rgba[i].max(rgba[i + 1]).max(rgba[i + 2]) > 230 {
                        nw += 1;
                    }
                }
            }
            if nw == step * step {
                white += 1;
            }
            x += step;
        }
        y += step;
    }
    white
}

/// A dark pixel this far outside the mask's fill is past the eye white's own outline —
/// past its stroke and the anti-aliased edge — so only an unclipped pupil draws there.
const CLIP_REACH: f32 = 3.0;

#[test]
fn imported_group_mask_draws_and_clips() {
    let Some(dir) = corpus() else {
        eprintln!("skipped: no maop-corpus beside the workspace");
        return;
    };
    let path = dir.join("keyframes-camera.moho");
    if !path.is_file() {
        eprintln!("skipped: no keyframes-camera.moho in the corpus");
        return;
    }
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("keyframes-camera.moho: {e}"));
    assert!(!report.skipped.contains_key("mask layers (drawn unmasked)"), "{:?}", report.skipped);
    // The structure: a mask layer that draws itself, with the pupil as its child.
    let tl = doc.timeline(0);
    let mask = tl.layers.iter().find(|l| l.kind == LayerKind::Mask && l.name == "Eyes").expect("the eye-white mask layer");
    assert!(mask.mask_draws, "Moho's MM_ADD_MASK member draws itself");
    let pupil = tl.layers.iter().find(|l| l.name == "Pupil").expect("the pupil layer");
    assert_eq!(pupil.parent, Some(mask.id), "the pupil is masked by the whites");
    assert!(pupil.visible, "the pupil shows");
    // The native format keeps the flag (the document round-trips through save/load).
    let back = anicraft_engine::format::from_json(&anicraft_engine::format::to_json(&doc).unwrap()).unwrap();
    assert!(back.timeline(0).layers.iter().any(|l| l.kind == LayerKind::Mask && l.mask_draws), "mask_draws survives .anicraft");

    // The render: the white draws, the pupil draws only inside it.
    let (w, rgba) = render(&doc);
    let white = white_cells(&doc, mask.id, w, &rgba);
    assert!(white > 4, "the eye white draws (mask_draws): {white} white cells");
    let dark = dark_cells(&doc, mask.id, w, &rgba);
    assert!(dark.len() > 2, "the pupil draws: {} black cells", dark.len());
    let strays = dark.iter().filter(|(_, _, d)| *d > CLIP_REACH).count();
    assert_eq!(strays, 0, "{strays} pupil cells draw past the eye white's reach");

    // Negative control 1 — the clip rides the mask link. This fixture's pupil fits inside
    // the white (the rig draws it that way), so the link can't be seen geometrically; the
    // display list is where a dropped link would show: the pupil's items carry the mask's
    // clip, and unparenting them takes it away.
    {
        let dl = anicraft_doc::evaluate(&doc, 0);
        assert!(dl.items.iter().filter(|it| it.layer == pupil.id).all(|it| it.clip.is_some()), "the pupil's items carry the mask's clip");
    }
    let mut loose = doc.clone();
    let loose_tl = loose.timeline_mut(0);
    if let Some(p) = loose_tl.layers.iter_mut().find(|l| l.name == "Pupil") {
        Arc::make_mut(p).parent = None;
    }
    {
        let dl = anicraft_doc::evaluate(&loose, 0);
        assert!(dl.items.iter().filter(|it| it.layer == pupil.id).all(|it| it.clip.is_none()), "unparented, the pupil carries no clip");
    }

    // Negative control 2 — an invisible mask (Moho's MM_ADD_MASK_INVIS form): no white.
    let mut plain = doc.clone();
    let plain_tl = plain.timeline_mut(0);
    if let Some(m) = plain_tl.layers.iter_mut().find(|l| l.id == mask.id) {
        Arc::make_mut(m).mask_draws = false;
    }
    let (w, rgba) = render(&plain);
    let white = white_cells(&plain, mask.id, w, &rgba);
    assert_eq!(white, 0, "without mask_draws the white's fills never draw ({white} cells)");
    let dark = dark_cells(&plain, mask.id, w, &rgba);
    assert!(dark.iter().filter(|(_, _, d)| *d == 0.0).count() > 2, "the pupil still shows, clipped by the invisible white");
}
