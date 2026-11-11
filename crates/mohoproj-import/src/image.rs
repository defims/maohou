//! Image layers (ADR 0106): the picture on the stage, and its mesh warp.
//!
//! An undeformed image layer draws its file as one bitmap rectangle. Moho sizes it with the
//! layer's own `width`/`height` (inches, the artist's chosen drawn size) and, for PSD-sourced
//! layers, offsets it by `psd_layer_translation` — both are real-file evidence (the Lute
//! startup project); without them the image draws at its natural pixel size over the dpi.
//!
//! A deformed image layer carries a `mesh` (the same points/curves/shapes structure a vector
//! layer has; the corpus's deformed specimens are all vector layers, so the reader accepts
//! the shared structure and reports what it can't hold). The image then draws through the
//! mesh: each shape's outline is split into triangles, and each triangle is one shape with a
//! [`Paint::Bitmap`] fill whose matrix maps the image onto it — image space → shape space,
//! exactly fitted from the triangle's three corners ([`anicraft_doc::fit_affine`]). Points
//! with position channels ride their triangle as point tracks (ADR 0105) with the pinned
//! image coordinates as [`anicraft_doc::TextureMap`], and evaluation refits the matrix per
//! frame — the texture follows the warp with no renderer changes. Handles are not part of an
//! image cage's deformation: outlines are straight-edge polygons here.

use crate::import::Builder;
use crate::json::{self, array, get};
use crate::mesh::{MeshData, parse_mesh};
use anicraft_doc::{ColorEffect, Element, ElementId, ElementKind, Paint, PointTrack, Shape, TextureMap, fit_affine};
use anicraft_geom::{Affine, FillRule, Path, Vec2};
use serde_json::Value;
use std::sync::Arc;

/// Most outline points one image-mesh shape may have before it is reported (the corpus's
/// cages are quads; this only keeps a bad file from ballooning the element list).
const MAX_OUTLINE: usize = 256;
/// Most triangles one image layer may produce.
const MAX_TRIANGLES: usize = 2_048;

/// An image layer's elements: the mesh warp when the layer has one, else the plain picture.
pub(crate) fn image_elements(b: &mut Builder, layer: &Value, world: Affine, out: &mut Vec<Arc<Element>>) {
    let Some(file) = crate::layer::fileref(layer) else {
        b.report.skip("image layers without a file reference");
        return;
    };
    let bytes = match (b.read)(&file) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            b.report.skip("missing image files");
            return;
        }
        Err(e) => {
            b.report.notes.push(format!("image file unreadable: {e}"));
            return;
        }
    };
    let short: String = file.rsplit(['/', '\\']).next().unwrap_or(&file).chars().take(256).collect();
    let Some(id) = b.register_bitmap(short, bytes) else {
        b.report.skip("image files that are not PNG or JPEG");
        return;
    };
    let (bw, bh) = b.doc.bitmap(id).map(|m| (m.width as f32, m.height as f32)).unwrap_or((1.0, 1.0));
    let (w, h) = drawn_size(layer, bw, bh, b.dpi);
    let mut centre = get(layer, "origin").and_then(|v| json::vec2(Some(v))).unwrap_or(Vec2::ZERO);
    if let Some(t) = get(layer, "psd_layer_translation").and_then(|v| json::vec2(Some(v))) {
        centre = centre + t;
    }
    deformer_flags(b, layer);
    match get(layer, "mesh").filter(|m| !m.is_null()) {
        Some(mesh) => warp_elements(b, mesh, id, (bw, bh), w, h, centre, world, out),
        None => plain_picture(b, id, (bw, bh), w, h, centre, world, out),
    }
}

/// The plain picture: one bitmap rectangle of the drawn size about the centre.
fn plain_picture(b: &mut Builder, id: anicraft_doc::BitmapId, (bw, bh): (f32, f32), w: f32, h: f32, centre: Vec2, world: Affine, out: &mut Vec<Arc<Element>>) {
    let mut path = Path::new();
    path.move_to(about(centre, -w * 0.5, -h * 0.5))
        .line_to(about(centre, w * 0.5, -h * 0.5))
        .line_to(about(centre, w * 0.5, h * 0.5))
        .line_to(about(centre, -w * 0.5, h * 0.5))
        .close();
    out.push(Arc::new(Element {
        id: ElementId(b.doc.alloc_id()),
        name: String::new(),
        transform: b.to_px.mul(&world),
        color: ColorEffect::default(),
        kind: ElementKind::Shape(Shape {
            path: Arc::new(path),
            fill: Some(Paint::Bitmap { bitmap: id, repeat: false, matrix: rect_matrix(bw, bh, w, h, centre) }),
            stroke: None,
            fill_rule: FillRule::NonZero,
            merge: false,
            bind: Vec::new(),
            points: Vec::new(),
            trim: None,
            texture: None,
        }),
    }));
}

/// The layer's drawn size in inches: its `width`/`height` when both are positive and finite,
/// else the natural pixel size over the dpi.
fn drawn_size(layer: &Value, bw: f32, bh: f32, dpi: f32) -> (f32, f32) {
    let w = json::num(get(layer, "width")).map(|v| v as f32).filter(|v| v.is_finite() && *v > 1e-4);
    let h = json::num(get(layer, "height")).map(|v| v as f32).filter(|v| v.is_finite() && *v > 1e-4);
    match (w, h) {
        (Some(w), Some(h)) => (w, h),
        _ => (bw / dpi, bh / dpi),
    }
}

/// The deformer switches and picture options with no slot yet, counted when set.
fn deformer_flags(b: &mut Builder, layer: &Value) {
    if json::boolean(get(layer, "frame_zero_deformer")).unwrap_or(false) {
        b.report.skip("frame-zero deformers (the cage is drawn as authored)");
    }
    if json::boolean(get(layer, "squashable_deformer")).unwrap_or(false) {
        b.report.skip("squashable deformers (the cage is drawn as authored)");
    }
    if json::boolean(get(layer, "image_cropped")).unwrap_or(false) {
        b.report.skip("cropped image layers (drawn whole)");
    }
    if json::num(get(layer, "interpreted_fps")).is_some_and(|v| v > 0.0) {
        b.report.skip("movie image layers (their still forms are drawn)");
    }
}

/// A point at `(dx, dy)` about the image centre.
fn about(centre: Vec2, dx: f32, dy: f32) -> Vec2 {
    Vec2::new(centre.x + dx, centre.y + dy)
}

/// The static fill matrix for a plain rectangle: image pixels → layer space (image top-left
/// onto the frame's top-left; image y down, layer y up).
fn rect_matrix(bw: f32, bh: f32, w: f32, h: f32, centre: Vec2) -> Affine {
    let sx = w / bw.max(1.0);
    let sy = -h / bh.max(1.0);
    Affine { m: [sx, 0.0, 0.0, sy, centre.x - w * 0.5, centre.y + h * 0.5] }
}

/// The mesh warp: the layer's mesh shapes become textured triangles. `centre`, `w`, `h`
/// place the image in layer space; `(bw, bh)` is the image's pixel size.
fn warp_elements(b: &mut Builder, mesh: &Value, id: anicraft_doc::BitmapId, (bw, bh): (f32, f32), w: f32, h: f32, centre: Vec2, world: Affine, out: &mut Vec<Arc<Element>>) {
    let Some(data) = parse_mesh(b, mesh) else { return };
    let Some(shapes) = array(get(mesh, "shapes")) else {
        b.report.skip("image meshes without shapes (drawn as the plain picture)");
        return;
    };
    let transform = b.to_px.mul(&world);
    // Texture coordinates put the image on the mesh: the mapping is the plain rectangle's,
    // whatever the mesh covers (points outside the frame extrapolate; the fill draws them
    // transparent).
    let uv_of = |p: Vec2| -> Vec2 { Vec2::new(((p.x - centre.x) / w + 0.5) * bw, (0.5 - (p.y - centre.y) / h) * bh) };
    let mut triangles = 0usize;
    let mut open_pieces = 0usize;
    for shape in shapes {
        for ring in outlines(b, shape, &data) {
            let tris = ear_clip(&ring);
            if tris.is_empty() {
                open_pieces += 1;
                continue;
            }
            for tri in tris {
                if triangles >= MAX_TRIANGLES {
                    b.report.skip("image meshes past 2,048 triangles");
                    return;
                }
                triangles += 1;
                textured_triangle(b, &data, &tri, id, &uv_of, transform, out);
            }
        }
    }
    if open_pieces > 0 {
        b.report.skip("open or self-overlapping image-mesh outlines (drawn in drawable pieces)");
    }
    if triangles == 0 && open_pieces == 0 {
        b.report.skip("image meshes without drawable shapes (drawn as the plain picture)");
    }
}

/// One textured triangle: the deformed piece as its path, the exact image→shape matrix as
/// its fill, and — when any point of the mesh animates — the point tracks and the pinned
/// image coordinates that let evaluation follow the warp (ADR 0106).
fn textured_triangle(b: &mut Builder, data: &MeshData, tri: &[(usize, Vec2)], id: anicraft_doc::BitmapId, uv_of: &impl Fn(Vec2) -> Vec2, transform: Affine, out: &mut Vec<Arc<Element>>) {
    let pts: Vec<Vec2> = tri.iter().map(|(_, p)| *p).collect();
    let uv: Vec<Vec2> = pts.iter().map(|p| uv_of(*p)).collect();
    let Some(matrix) = fit_affine(&uv, &pts) else {
        b.report.skip("degenerate image-mesh triangles");
        return;
    };
    let mut path = Path::new();
    path.move_to(pts[0]).line_to(pts[1]).line_to(pts[2]).close();
    // Point tracks for the corners whose mesh points animate (a corner shared with the next
    // triangle carries the same channel, so the seam stays welded).
    let mut tracks = Vec::new();
    for (i, (mesh_point, _)) in tri.iter().enumerate() {
        if let Some(ch) = data.animated.get(*mesh_point).cloned().flatten() {
            tracks.push(PointTrack { index: i as u32, position: ch });
        }
    }
    let animated = !tracks.is_empty();
    let (fill, texture) = if animated {
        (None, Some(Box::new(TextureMap { bitmap: id, uv })))
    } else {
        (Some(Paint::Bitmap { bitmap: id, repeat: false, matrix }), None)
    };
    out.push(Arc::new(Element {
        id: ElementId(b.doc.alloc_id()),
        name: String::new(),
        transform,
        color: ColorEffect::default(),
        kind: ElementKind::Shape(Shape { path: Arc::new(path), fill, stroke: None, fill_rule: FillRule::NonZero, merge: false, bind: Vec::new(), points: tracks, trim: None, texture }),
    }));
}

/// A mesh shape's outline rings as `(mesh point, position)` chains (straight edges: the
/// handles of a vector mesh have no place in an image cage). Chained edge runs join; a gap
/// starts a new ring.
fn outlines(b: &mut Builder, shape: &Value, data: &MeshData) -> Vec<Vec<(usize, Vec2)>> {
    let Some(edges) = get(shape, "edges") else { return Vec::new() };
    let cs = array(get(edges, "curve")).unwrap_or(&[]);
    let ss = array(get(edges, "segment")).unwrap_or(&[]);
    let fs = array(get(edges, "flag")).unwrap_or(&[]);
    if cs.is_empty() {
        return Vec::new();
    }
    if cs.len() != ss.len() || cs.len() != fs.len() || cs.len() > MAX_OUTLINE {
        b.report.skip("image-mesh shapes with unusable edge lists");
        return Vec::new();
    }
    let mut rings: Vec<Vec<(usize, Vec2)>> = Vec::new();
    let mut cur: Vec<(usize, Vec2)> = Vec::new();
    for i in 0..cs.len() {
        let (Some(ci), Some(si), Some(fi)) = (json::integer(cs.get(i)), json::integer(ss.get(i)), json::integer(fs.get(i))) else { continue };
        let (Ok(ci), Ok(si)) = (usize::try_from(ci), usize::try_from(si)) else { continue };
        let Some(curve) = data.curves.get(ci) else {
            b.report.skip("image-mesh edges pointing past the curve list");
            continue;
        };
        let n = curve.pts.len();
        let segments = if curve.closed { n } else { n - 1 };
        if si >= segments {
            b.report.skip("image-mesh edges with out-of-range segments");
            continue;
        }
        let after = if si + 1 < n { si + 1 } else { 0 };
        let (ai, di) = if fi == 1 { (&curve.pts[after], &curve.pts[si]) } else { (&curve.pts[si], &curve.pts[after]) };
        let (Some(p0), Some(p1)) = (data.positions.get(ai.point), data.positions.get(di.point)) else { continue };
        if !cur.last().is_some_and(|(_, p)| (*p - *p0).length() < 1e-4) {
            if !cur.is_empty() {
                rings.push(std::mem::take(&mut cur));
            }
            cur.push((ai.point, *p0));
        }
        cur.push((di.point, *p1));
    }
    if !cur.is_empty() {
        rings.push(cur);
    }
    // A ring that came home drops the duplicated end.
    for ring in &mut rings {
        if ring.len() > 2 && (ring[0].1 - ring[ring.len() - 1].1).length() < 1e-4 {
            ring.pop();
        }
    }
    rings.retain(|r| r.len() >= 3);
    rings
}

/// Triangulates a simple polygon by ear clipping (`ring` in either winding; output keeps
/// the ring's order). Empty for degenerate rings (fewer than 3 points, collinear, or
/// self-overlapping so no ear clips).
fn ear_clip(ring: &[(usize, Vec2)]) -> Vec<Vec<(usize, Vec2)>> {
    let n = ring.len();
    let mut tris = Vec::new();
    if n < 3 {
        return tris;
    }
    let con = |p0: Vec2, p1: Vec2, p2: Vec2| -> f32 { (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x) };
    // The ring's winding by the shoelace sum: positive when counter-clockwise here.
    let mut shoelace = 0.0f32;
    for i in 0..n {
        let (a, b) = (ring[i].1, ring[(i + 1) % n].1);
        shoelace += a.x * b.y - b.x * a.y;
    }
    let sign = if shoelace >= 0.0 { 1.0 } else { -1.0 };
    let in_triangle = |p: Vec2, a: Vec2, o: Vec2, c: Vec2| -> bool {
        let (d1, d2, d3) = (con(a, o, p), con(o, c, p), con(c, a, p));
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    };
    // Work on ring indices; clipped ears drop out. The guard keeps a hostile ring from
    // spinning (each pass that clips nothing ends the loop).
    let mut idx: Vec<usize> = (0..n).collect();
    let mut guard = 0usize;
    while idx.len() > 3 && guard <= 4 * n + 16 {
        guard += 1;
        let mut clipped = false;
        for k in 0..idx.len() {
            let i0 = idx[(k + idx.len() - 1) % idx.len()];
            let i1 = idx[k];
            let i2 = idx[(k + 1) % idx.len()];
            let (a, o, c) = (ring[i0].1, ring[i1].1, ring[i2].1);
            // A reflex vertex is not an ear: a convex corner turns the ring's way.
            if con(a, o, c) * sign <= 1e-9 {
                continue;
            }
            let inside = idx.iter().any(|&j| j != i0 && j != i1 && j != i2 && in_triangle(ring[j].1, a, o, c));
            if inside {
                continue;
            }
            tris.push(vec![ring[i0], ring[i1], ring[i2]]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        let (a, o, c) = (ring[idx[0]].1, ring[idx[1]].1, ring[idx[2]].1);
        if con(a, o, c).abs() > 1e-9 {
            tris.push(vec![ring[idx[0]], ring[idx[1]], ring[idx[2]]]);
        }
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(pts: &[(usize, (f32, f32))]) -> Vec<(usize, Vec2)> {
        pts.iter().map(|(i, (x, y))| (*i, Vec2::new(*x, *y))).collect()
    }

    #[test]
    fn a_quad_splits_into_two_triangles() {
        let r = ring(&[(0, (0.0, 0.0)), (1, (10.0, 0.0)), (2, (10.0, 10.0)), (3, (0.0, 10.0))]);
        let tris = ear_clip(&r);
        assert_eq!(tris.len(), 2);
        let mut uses = std::collections::BTreeMap::new();
        for t in &tris {
            for (i, _) in t {
                *uses.entry(*i).or_default() += 1;
            }
        }
        assert_eq!(uses.len(), 4, "every corner is used");
        assert_eq!(uses.values().sum::<i32>(), 6, "two triangles, three corners each");
    }

    #[test]
    fn a_concave_ring_splits_along_its_ears() {
        // An L shape: 6 vertices, splits into n−2 triangles.
        let r = ring(&[(0, (0.0, 0.0)), (1, (10.0, 0.0)), (2, (10.0, 4.0)), (3, (4.0, 4.0)), (4, (4.0, 10.0)), (5, (0.0, 10.0))]);
        assert_eq!(ear_clip(&r).len(), 4);
    }

    #[test]
    fn a_degenerate_ring_clips_nothing() {
        assert!(ear_clip(&ring(&[(0, (0.0, 0.0)), (1, (1.0, 1.0))])).is_empty());
        assert!(ear_clip(&ring(&[(0, (0.0, 0.0)), (1, (1.0, 1.0)), (2, (2.0, 2.0))])).is_empty(), "collinear points clip nothing");
    }

    #[test]
    fn triangles_cover_the_ring_without_gaps() {
        let r = ring(&[(0, (0.0, 0.0)), (1, (8.0, 2.0)), (2, (10.0, 8.0)), (3, (5.0, 12.0)), (4, (-2.0, 6.0))]);
        let tris = ear_clip(&r);
        assert_eq!(tris.len(), 3, "5 vertices → 3 triangles");
        let area = |t: &Vec<(usize, Vec2)>| -> f32 {
            let (a, o, c) = (t[0].1, t[1].1, t[2].1);
            ((a.x - o.x) * (c.y - o.y) - (a.y - o.y) * (c.x - o.x)).abs() * 0.5
        };
        let poly = 0.5 * ((0.0f32 * 2.0 - 8.0 * 0.0) + (8.0 * 8.0 - 10.0 * 2.0) + (10.0 * 12.0 - 5.0 * 8.0) + (5.0 * 6.0 - (-2.0) * 12.0) + ((-2.0) * 0.0 - 0.0 * 6.0));
        let sum: f32 = tris.iter().map(area).sum();
        assert!((sum - poly.abs()).abs() < 1e-3, "{sum} vs {poly}");
    }

    #[test]
    fn a_reversed_ring_clips_the_same() {
        let r = ring(&[(0, (0.0, 0.0)), (1, (0.0, 10.0)), (2, (10.0, 10.0)), (3, (10.0, 0.0))]);
        assert_eq!(ear_clip(&r).len(), 2);
    }
}
