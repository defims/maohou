//! The stage-3 real-machine gates (parity plan › stage 3): Moho Pro 14.4 rendered the
//! corpus fixtures itself (FileRender, document resolution, transparent), and the imported
//! documents must reproduce those frames through the CPU oracle. Renderer AA and the
//! importer's handle approximation differ, so raw pixel diffs are meaningless; each
//! comparison aligns the frames by translation (4 px cells, ±300 px) on the content mask and
//! then checks the colour structure: dark (gradient ends) and light (gradient starts) cells
//! must land in the same places.
//!
//! Findings this gate pins (details in maop-corpus CASES.md):
//! - gradients.moho: its 13 filled shapes carry zero-area degenerate curves (2 points,
//!   smoothness 0), so Moho renders only the outline strokes of the four genuine rectangles —
//!   the gate verifies our strokes and that we add no spurious fills.
//! - gradients-gui.moho: a working angle gradient (the S18 pentagon, white→black, sweep from
//!   12 o'clock anticlockwise — the phase this gate verified) must come through our import
//!   with the same luminance layout. Flattening the gradient is the negative control.

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

/// The fixture's path, or `None` after explaining why the test didn't run.
fn fixture(dir: &Path, name: &str) -> Option<PathBuf> {
    let f = dir.join(format!("{name}.moho"));
    f.is_file().then_some(f).or_else(|| {
        eprintln!("skipped {name}: not in the corpus");
        None
    })
}

fn moho_render(dir: &Path, name: &str) -> Option<anicraft_doc::Pixels> {
    let p = dir.join("inputs").join(format!("{name}.png"));
    let Ok(bytes) = std::fs::read(&p) else {
        eprintln!("skipped: no Moho render (inputs/{name}.png) — run the probe scripts on the real machine");
        return None;
    };
    Some(anicraft_doc::decode_png(&bytes).unwrap())
}

/// Downsampled cells (4 px): content (alpha > 24) and, per cell, the mean straight
/// luminance of covered pixels (0 where empty).
fn cells(px: &[u8], w: u32, h: u32, step: u32) -> (u32, u32, Vec<bool>, Vec<f32>) {
    let gw = w / step;
    let gh = h / step;
    let mut on = vec![false; (gw * gh) as usize];
    let mut lum = vec![0.0f32; (gw * gh) as usize];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut lit = 0u32;
            let mut sum = 0.0f64;
            for dy in 0..step {
                for dx in 0..step {
                    let i = (((gy * step + dy) * w + gx * step + dx) * 4) as usize;
                    let a = px[i + 3];
                    if a > 24 {
                        lit += 1;
                        let af = a as f64;
                        let (r, g, b) = (px[i] as f64, px[i + 1] as f64, px[i + 2] as f64);
                        sum += (0.2126 * r + 0.7152 * g + 0.0722 * b) / af;
                    }
                }
            }
            let k = (gy * gw + gx) as usize;
            on[k] = lit > 0;
            if lit > 0 {
                lum[k] = (sum / lit as f64) as f32;
            }
        }
    }
    (gw, gh, on, lum)
}

/// Best translation of `a` onto `b` (±`range` cells) by content F1.
fn best_shift(a: &[bool], b: &[bool], gw: u32, gh: u32, range: u32) -> (i64, i64, f64) {
    let f1 = |dx: i64, dy: i64| -> f64 {
        let (mut inter, mut na, mut nb) = (0u64, 0u64, 0u64);
        for y in 0..gh as i64 {
            for x in 0..gw as i64 {
                let vb = b[(y as u32 * gw + x as u32) as usize];
                let (ax, ay) = (x - dx, y - dy);
                let va = ax >= 0 && ax < gw as i64 && ay >= 0 && ay < gh as i64 && a[(ay as u32 * gw + ax as u32) as usize];
                if va {
                    na += 1;
                }
                if vb {
                    nb += 1;
                }
                if va && vb {
                    inter += 1;
                }
            }
        }
        if na + nb == 0 {
            0.0
        } else {
            2.0 * inter as f64 / (na + nb) as f64
        }
    };
    let (mut bx, mut by, mut bf) = (0i64, 0i64, -1f64);
    for dy in -(range as i64)..=(range as i64) {
        for dx in -(range as i64)..=(range as i64) {
            let v = f1(dx, dy);
            if v > bf {
                (bx, by, bf) = (dx, dy, v);
            }
        }
    }
    (bx, by, bf)
}

fn straight_rgba(ours: &anicraft_render::Pixmap) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((ours.width * ours.height * 4) as usize);
    for p in &ours.data {
        let a = p[3];
        let un = |c: f32| if a > 0.0 { (c / a).clamp(0.0, 1.0) } else { 0.0 };
        rgba.extend_from_slice(&[(un(p[0]) * 255.0) as u8, (un(p[1]) * 255.0) as u8, (un(p[2]) * 255.0) as u8, (a * 255.0) as u8]);
    }
    rgba
}

/// Structure agreement of two aligned cell grids: F1 over the dark cells (luminance below
/// the cut) and F1 over the bright cells (above it).
fn structure(la: &[f32], ca: &[bool], lb: &[f32], cb: &[bool], cut: f32) -> (f64, f64) {
    let dark = |l: &[f32], c: &[bool]| -> Vec<bool> { c.iter().zip(l).map(|(&on, &v)| on && v < cut).collect() };
    let light = |l: &[f32], c: &[bool]| -> Vec<bool> { c.iter().zip(l).map(|(&on, &v)| on && v >= cut).collect() };
    let (da, db, la_, lb_) = (dark(la, ca), dark(lb, cb), light(la, ca), light(lb, cb));
    let overlap = |a: &[bool], b: &[bool]| -> f64 {
        let (mut inter, mut na, mut nb) = (0u64, 0u64, 0u64);
        for k in 0..a.len() {
            na += a[k] as u64;
            nb += b[k] as u64;
            inter += (a[k] && b[k]) as u64;
        }
        if na + nb == 0 {
            1.0
        } else {
            2.0 * inter as f64 / (na + nb) as f64
        }
    };
    (overlap(&da, &db), overlap(&la_, &lb_))
}

fn render_ours(doc: &anicraft_doc::Document) -> anicraft_render::Pixmap {
    let (w, h) = (doc.stage.width, doc.stage.height);
    let dl = anicraft_doc::evaluate(doc, 0);
    let view = anicraft_render::View { transform: anicraft_geom::Affine::IDENTITY, width: w, height: h, background: None };
    anicraft_render::render(&dl, &view).unwrap().0
}

/// Aligns our cells onto Moho's by the best content shift, then measures the dark/light
/// structure overlaps. Returns (dx, dy, content F1, dark F1, light F1).
fn compare(moho: &anicraft_doc::Pixels, ours: &anicraft_render::Pixmap, what: &str) -> (i64, i64, f64, f64, f64) {
    assert_eq!((moho.width, moho.height), (ours.width, ours.height), "FileRender uses the document resolution");
    let (w, h) = (ours.width, ours.height);
    let step = 4u32;
    let (gw, gh, m_on, m_lum) = cells(&moho.rgba, w, h, step);
    let ours_rgba = straight_rgba(ours);
    let (_, _, o_on, o_lum) = cells(&ours_rgba, w, h, step);
    let (dx, dy, content_f1) = best_shift(&o_on, &m_on, gw, gh, 75);
    println!("{what}: best shift ({dx}, {dy}) cells = ({}px, {}px), content F1 {content_f1:.3}", dx * 4, dy * 4);
    let pick = |on: &[bool], lum: &[f32]| -> (Vec<bool>, Vec<f32>) {
        let mut on2 = vec![false; on.len()];
        let mut l2 = vec![0.0f32; lum.len()];
        for y in 0..gh as i64 {
            for x in 0..gw as i64 {
                let (sx, sy) = (x - dx, y - dy);
                let k = (y as u32 * gw + x as u32) as usize;
                if sx >= 0 && sy >= 0 && sx < gw as i64 && sy < gh as i64 {
                    let s = (sy as u32 * gw + sx as u32) as usize;
                    on2[k] = on[s];
                    l2[k] = lum[s];
                }
            }
        }
        (on2, l2)
    };
    let (o_on2, o_lum2) = pick(&o_on, &o_lum);
    let (dark, light) = structure(&o_lum2, &o_on2, &m_lum, &m_on, 0.5);
    println!("{what}: aligned dark F1 {dark:.3}, light F1 {light:.3}");
    (dx, dy, content_f1, dark, light)
}

/// gradients.moho: Moho renders the four genuine rectangles' outline strokes only (its filled
/// shapes are zero-area degenerate curves — see CASES.md); ours must agree, strokes for
/// strokes and no spurious fills.
#[test]
fn gradients_outlines_match_mohos_frame() {
    let Some(dir) = corpus() else { return eprintln!("skipped: no maop-corpus beside the workspace") };
    let Some(path) = fixture(&dir, "gradients") else { return };
    let Some(moho) = moho_render(&dir, "gradients-f1") else { return };
    let (doc, _report) = import_path(&path).unwrap_or_else(|e| panic!("gradients.moho: {e}"));
    let ours = render_ours(&doc);
    let (dx, dy, content_f1, dark, light) = compare(&moho, &ours, "gradients f1");
    // Thresholds calibrated on this fixture (numbers in CASES.md): the strokes sit within
    // the handle approximation's bias (shift 24,−24 px, content F1 0.257 — the stage-2
    // stroke-only baseline measured 0.23–0.58 at up to 36 px), and the light set being
    // empty on both sides (F1 1.0 by convention) proves we added no spurious bright fills.
    assert!(dx.abs() <= 15 && dy.abs() <= 15, "content shift ({dx}, {dy}) cells — placement wrong");
    assert!(content_f1 > 0.18, "content F1 {content_f1:.3} — the outlines don't align");
    assert!(dark > 0.18, "dark F1 {dark:.3} — strokes misplaced");
    assert!(light > 0.9, "light F1 {light:.3} — spurious fills where Moho draws none");
}

/// gradients-gui.moho: the pentagon's working angle gradient must come through with the same
/// layout — white at 12 o'clock, dark towards three o'clock — and the flattening control
/// must collapse, so the gate can tell a gradient from a solid.
#[test]
fn gradients_gui_angle_gradient_matches_mohos_frame() {
    let Some(dir) = corpus() else { return eprintln!("skipped: no maop-corpus beside the workspace") };
    let Some(path) = fixture(&dir, "gradients-gui") else { return };
    let Some(moho) = moho_render(&dir, "gradients-gui-f1") else { return };
    let (doc, _report) = import_path(&path).unwrap_or_else(|e| panic!("gradients-gui.moho: {e}"));
    let ours = render_ours(&doc);
    let (dx, dy, content_f1, dark, light) = compare(&moho, &ours, "gradients-gui f1");
    // Calibrated (CASES.md): measured dark 0.678 / light 0.754 against the control's
    // 0.144 / 0.523 — the gate sits clearly between.
    assert!(dx.abs() <= 15 && dy.abs() <= 15, "content shift ({dx}, {dy}) cells");
    assert!(content_f1 > 0.5, "content F1 {content_f1:.3}");
    assert!(dark > 0.5, "dark F1 {dark:.3} — the angle gradient's dark sweep disagrees");
    assert!(light > 0.65, "light F1 {light:.3} — the gradient's bright side disagrees");
}

/// The negative control on the same fixture: with every gradient flattened to a solid, the
/// gradient's dark/light structure collapses.
#[test]
fn flattening_the_gradient_collapses_the_structure() {
    let Some(dir) = corpus() else { return eprintln!("skipped: no maop-corpus beside the workspace") };
    let Some(path) = fixture(&dir, "gradients-gui") else { return };
    let Some(moho) = moho_render(&dir, "gradients-gui-f1") else { return };
    let (mut doc, _) = import_path(&path).unwrap_or_else(|e| panic!("{e}"));
    for l in &mut doc.timeline_mut(0).layers {
        for kf in &mut Arc::make_mut(l).keyframes {
            for el in &mut Arc::make_mut(kf).elements {
                if let anicraft_doc::ElementKind::Shape(s) = &mut Arc::make_mut(el).kind
                    && let Some(p) = &mut s.fill
                {
                    *p = anicraft_doc::Paint::Solid(p.primary_color());
                }
            }
        }
    }
    let flat = render_ours(&doc);
    let (_, _, _, dark, light) = compare(&moho, &flat, "flattened control");
    println!("flattened control: dark F1 {dark:.3}, light F1 {light:.3}");
    assert!(dark < 0.4 && light < 0.6, "a flattened fill should lose the gradient structure (dark {dark:.3}, light {light:.3})");
}

/// smartbones.moho: the non-skeleton part is an empty mesh, and Moho renders an empty frame;
/// our import must agree. The skeleton imports since stage 4 (ADR 0104) — a rig draws
/// nothing, so the frame stays empty on both sides.
#[test]
fn smartbones_non_skeleton_matches_mohos_frame() {
    let Some(dir) = corpus() else { return eprintln!("skipped: no maop-corpus beside the workspace") };
    let Some(path) = fixture(&dir, "smartbones") else { return };
    let Some(moho) = moho_render(&dir, "smartbones-f1") else { return };
    let (doc, report) = import_path(&path).unwrap_or_else(|e| panic!("smartbones.moho: {e}"));
    // The styles table comes across; the two smart-rig bones import as the rig.
    assert_eq!(doc.styles.len(), 12);
    assert!(!report.skipped.contains_key("skeleton bones"), "{:?}", report.skipped);
    let rig = doc.timeline(0).layers.iter().find(|l| l.kind == anicraft_doc::LayerKind::Bone).unwrap();
    assert_eq!(rig.skeleton.as_ref().unwrap().bones.len(), 2);
    let ours = render_ours(&doc);
    let (w, h) = (ours.width, ours.height);
    let ours_content = ours.data.iter().filter(|p| p[3] > 0.05).count();
    let moho_content = moho.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 24).count();
    println!("smartbones f1: moho content px {moho_content}, ours {ours_content}");
    // Moho's frame is empty (the layer's mesh is empty); ours must be too.
    assert_eq!(moho_content, 0, "the fixture gained artwork — update this gate");
    assert_eq!(ours_content, 0, "our import draws artwork Moho does not: {ours_content} px");
    let _ = (w, h);
}

