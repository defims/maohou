//! Texture-pinned fills (ADR 0106, Moho's image-layer mesh warp): a shape whose bitmap fill
//! is pinned to its path points, so deforming the path deforms the texture.
//!
//! Moho warps an image layer through its mesh: the mesh's points carry (animated) positions
//! and the image is drawn through the deformed mesh. AniCraft draws a [`Paint::Bitmap`]
//! through its fill matrix — an affine, image space → shape space — so the bridge is: keep
//! the image-space coordinate ([`TextureMap::uv`]) of every path point on the shape, and
//! evaluation derives, each frame, the affine that best maps the deformed points onto those
//! coordinates. Three points fit exactly, so a triangulated piece (the Moho importer splits
//! outlines into triangles) warps exactly at every corner; the fill stays a plain
//! [`Paint::Bitmap`], which the renderer, the GPU stage and every export already draw. No
//! renderer changes; the whole warp lives in evaluation.

use crate::{BitmapId, Paint};
use anicraft_geom::{Affine, Path, PathCmd, Vec2};
use serde::{Deserialize, Serialize};

/// Most points a texture map pins (an image mesh is far smaller; a cap keeps bad files
/// from growing evaluation).
pub const MAX_TEXTURE_POINTS: usize = 20_000;

/// A shape's bitmap fill pinned to its path points (ADR 0106): `uv` holds the image pixel
/// coordinate of each path point, in command order (`path_point_count` entries). With the
/// shape's points static the fill's own matrix is already exact; when they animate,
/// evaluation refits the matrix from the deformed points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextureMap {
    pub bitmap: BitmapId,
    #[serde(default)]
    pub uv: Vec<Vec2>,
}

/// Every path point in command order (endpoints and control points), as [`crate::points`]
/// numbers them.
pub fn path_points(path: &Path) -> Vec<Vec2> {
    let mut out = Vec::new();
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => out.push(p),
            PathCmd::QuadTo(c, p) => {
                out.push(c);
                out.push(p);
            }
            PathCmd::CubicTo(c1, c2, p) => {
                out.push(c1);
                out.push(c2);
                out.push(p);
            }
            PathCmd::Close => {}
        }
    }
    out
}

/// The affine that best maps `from` onto `to` (least squares over point pairs; three pairs
/// fit exactly, more give the best fit). `None` when the pairs are too few, the counts
/// disagree, or the system is degenerate (collinear points, non-finite values).
pub fn fit_affine(from: &[Vec2], to: &[Vec2]) -> Option<Affine> {
    if from.len() != to.len() || from.len() < 3 {
        return None;
    }
    // Normal equations of min |A·(x, y, 1) − (u, v)|: rows [x, y, 1], unknowns per
    // component. The 3×3 moment matrix is shared by both components.
    let mut sxx = 0.0f64;
    let mut sxy = 0.0f64;
    let mut syy = 0.0f64;
    let mut sx = 0.0f64;
    let mut sy = 0.0f64;
    let mut bx = [0.0f64; 3];
    let mut by = [0.0f64; 3];
    for (p, q) in from.iter().zip(to) {
        let (x, y) = (f64::from(p.x), f64::from(p.y));
        let (u, v) = (f64::from(q.x), f64::from(q.y));
        if !(x.is_finite() && y.is_finite() && u.is_finite() && v.is_finite()) {
            return None;
        }
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        sx += x;
        sy += y;
        bx[0] += x * u;
        bx[1] += y * u;
        bx[2] += u;
        by[0] += x * v;
        by[1] += y * v;
        by[2] += v;
    }
    let n = from.len() as f64;
    let m = [sxx, sxy, sx, sxy, syy, sy, sx, sy, n];
    let sol_x = solve3(&m, &bx)?;
    let sol_y = solve3(&m, &by)?;
    let affine = Affine { m: [sol_x[0] as f32, sol_y[0] as f32, sol_x[1] as f32, sol_y[1] as f32, sol_x[2] as f32, sol_y[2] as f32] };
    affine.is_finite().then_some(affine)
}

/// Solves a 3×3 system by Gaussian elimination with partial pivoting and back-substitution.
/// `None` when it is singular (degenerate point sets).
fn solve3(m: &[f64; 9], b: &[f64; 3]) -> Option<[f64; 3]> {
    let mut a = [
        [m[0], m[1], m[2], b[0]],
        [m[1], m[4], m[5], b[1]],
        [m[2], m[5], m[8], b[2]],
    ];
    for col in 0..3 {
        // Pivot: the row at or below `col` with the largest |value| in this column.
        let (mut best, mut big) = (0.0f64, col);
        for r in col..3 {
            let v = a[r][col].abs();
            if v > best {
                best = v;
                big = r;
            }
        }
        if !(best > 1e-12) {
            return None;
        }
        a.swap(col, big);
        let d = a[col][col];
        for r in col + 1..3 {
            let f = a[r][col] / d;
            for c in col..4 {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    // Back-substitution (the matrix is now upper triangular).
    let mut out = [0.0; 3];
    for r in (0..3).rev() {
        let d = a[r][r];
        if !(d.abs() > 1e-12) {
            return None;
        }
        let mut s = a[r][3];
        for c in r + 1..3 {
            s -= a[r][c] * out[c];
        }
        out[r] = s / d;
    }
    Some(out)
}

/// The bitmap fill that pins `tex` to `path_now` (the shape's path, deformed or not):
/// the affine fitted from the path's points onto the stored image coordinates, in the
/// [`Paint::Bitmap`] matrix convention (image space → shape space). `None` when the map
/// doesn't match the path or the fit is degenerate (the caller keeps the static fill).
pub fn paint_for(path_now: &Path, tex: &TextureMap) -> Option<Paint> {
    if tex.uv.len() != path_point_len(path_now) || tex.uv.len() > MAX_TEXTURE_POINTS {
        return None;
    }
    let pts = path_points(path_now);
    fit_affine(&tex.uv, &pts).map(|matrix| Paint::Bitmap { bitmap: tex.bitmap, repeat: false, matrix })
}

fn path_point_len(path: &Path) -> usize {
    path.cmds.iter().map(|c| match c {
        PathCmd::MoveTo(_) | PathCmd::LineTo(_) => 1,
        PathCmd::QuadTo(..) => 2,
        PathCmd::CubicTo(..) => 3,
        PathCmd::Close => 0,
    }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(w: f32, h: f32) -> Vec<Vec2> {
        vec![Vec2::new(-w / 2.0, h / 2.0), Vec2::new(w / 2.0, h / 2.0), Vec2::new(w / 2.0, -h / 2.0), Vec2::new(-w / 2.0, -h / 2.0)]
    }

    #[test]
    fn three_points_fit_exactly() {
        let a = fit_affine(&[Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(0.0, 50.0)], &[Vec2::new(10.0, 10.0), Vec2::new(110.0, 14.0), Vec2::new(6.0, 58.0)]).unwrap();
        for (p, q) in [(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)), (Vec2::new(100.0, 0.0), Vec2::new(110.0, 14.0)), (Vec2::new(0.0, 50.0), Vec2::new(6.0, 58.0))] {
            let r = a.apply(p);
            assert!((r.x - q.x).abs() < 1e-3 && (r.y - q.y).abs() < 1e-3, "{r:?} vs {q:?}");
        }
    }

    #[test]
    fn the_image_rect_maps_to_the_frame() {
        // The importer's own convention: the image's pixel corners onto the frame corners.
        // 200×100 px image, 100×50 inch frame (y up): pixel y flips.
        let uv = vec![Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0), Vec2::new(200.0, 100.0), Vec2::new(0.0, 100.0)];
        let m = fit_affine(&uv, &quad(100.0, 50.0)).unwrap();
        // The image centre lands on the frame centre; the image's top edge (y=0) maps to
        // the frame's top (y +25 in y-up space).
        let c = m.apply(Vec2::new(100.0, 50.0));
        assert!(c.x.abs() < 1e-3 && c.y.abs() < 1e-3, "{c:?}");
        let t = m.apply(Vec2::new(100.0, 0.0));
        assert!(t.y - 25.0 < 1e-3 && t.y > 24.9, "{t:?}");
    }

    #[test]
    fn a_non_affine_quad_gives_the_best_fit() {
        // Four points that no affine maps exactly: the fit spreads the error.
        let m = fit_affine(&quad(10.0, 10.0), &quad(10.0, 10.0)).unwrap();
        let e: f32 = quad(10.0, 10.0).iter().map(|p| { let r = m.apply(*p); (r - *p).length() }).sum();
        assert!(e < 1e-2, "identity stays exact: {e}");
        let mut sheared = quad(10.0, 10.0);
        sheared[2].x += 3.0; // pull one corner out
        let m = fit_affine(&quad(10.0, 10.0), &sheared).unwrap();
        let total: f32 = quad(10.0, 10.0).iter().zip(&sheared).map(|(p, q)| { let r = m.apply(*p); (r - *q).length() }).sum();
        assert!(total > 0.1 && total < 6.0, "an impossible mapping is approximate, not exact: {total}");
    }

    #[test]
    fn degenerate_sets_refuse() {
        assert!(fit_affine(&[Vec2::ZERO; 3], &[Vec2::ZERO; 3]).is_none(), "collinear points");
        assert!(fit_affine(&[Vec2::ZERO; 2], &[Vec2::ZERO; 2]).is_none(), "too few points");
        assert!(fit_affine(&[Vec2::ZERO; 3], &[Vec2::ZERO; 2]).is_none(), "count mismatch");
        let nan = [Vec2::new(f32::NAN, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)];
        assert!(fit_affine(&nan, &nan).is_none(), "non-finite points");
    }

    #[test]
    fn paint_for_pins_the_texture() {
        let path = anicraft_geom::Path::rect(0.0, 0.0, 100.0, 50.0);
        let uv = vec![Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0), Vec2::new(200.0, 100.0), Vec2::new(0.0, 100.0)];
        let tex = TextureMap { bitmap: BitmapId(7), uv };
        let Paint::Bitmap { matrix, bitmap, repeat } = paint_for(&path, &tex).unwrap() else { panic!("a bitmap paint") };
        assert_eq!(bitmap, BitmapId(7));
        assert!(!repeat);
        // The image's corners land on the rect's corners (both lists top-left first; pixel
        // y down maps onto the rect's y down).
        let c = matrix.apply(Vec2::new(0.0, 100.0));
        assert!((c.x - 0.0).abs() < 1e-3 && (c.y - 50.0).abs() < 1e-3, "{c:?} vs (0, 50)");
        let c = matrix.apply(Vec2::new(200.0, 0.0));
        assert!((c.x - 100.0).abs() < 1e-3 && c.y.abs() < 1e-3, "{c:?} vs (100, 0)");
        // A uv list that doesn't match the path refuses.
        let short = TextureMap { bitmap: BitmapId(7), uv: vec![Vec2::ZERO; 2] };
        assert!(paint_for(&path, &short).is_none());
    }
}
