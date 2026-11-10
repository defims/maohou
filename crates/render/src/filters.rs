//! Filters on a group's composited pixels: premultiplied RGBA, `w × h`, row by row.
//!
//! Blur is repeated box blurs (one per quality step; three approach a Gaussian). Drop Shadow and
//! Glow blur the artwork's opacity, offset it and colour it, then put it under the artwork
//! (outer), or inside it over the artwork (inner); Knockout keeps only the shadow or glow;
//! Expansion grows or shrinks the silhouette before the blur, Noise speckles the shadow, and a
//! Threshold shadow snaps its blurred coverage to hard on/off.
//! Adjust Color is a colour matrix built from the standard matrix operations for brightness,
//! contrast, saturation and hue rotation (Haeberli, "Matrix Operations for Image Processing",
//! 1993), applied to unpremultiplied colour. Noise mixes deterministic grain over the artwork's
//! colour (the same pixel always speckles the same way); Pixelate averages blocks; Threshold
//! snaps colours to black or white by luminance.

use crate::paint::Premul;
use anicraft_doc::{Filter, Rgba};

/// Applies `filters` in order. Sizes are stage pixels, multiplied by `scale` (the view's).
pub(crate) fn apply(buf: &mut [Premul], w: usize, h: usize, filters: &[Filter], scale: f32) {
    if buf.len() != w * h || w == 0 || h == 0 {
        return;
    }
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    for f in filters {
        match f.sanitized() {
            Filter::Blur { blur_x, blur_y, quality } => blur(buf, w, h, blur_x * scale, blur_y * scale, quality),
            Filter::DropShadow { blur_x, blur_y, strength, quality, angle, distance, color, inner, knockout, hide_object, expansion, noise, threshold } => {
                let a = angle.to_radians();
                let (dx, dy) = ((distance * scale * a.cos()).round() as i64, (distance * scale * a.sin()).round() as i64);
                shadow(buf, w, h, Shadow {
                    blur_x: blur_x * scale,
                    blur_y: blur_y * scale,
                    strength,
                    quality,
                    dx,
                    dy,
                    color,
                    inner,
                    knockout: knockout || (hide_object && inner),
                    hide_object,
                    expansion: (expansion * scale).round(),
                    noise,
                    threshold,
                });
            }
            Filter::Glow { blur_x, blur_y, strength, quality, color, inner, knockout } => {
                shadow(buf, w, h, Shadow { blur_x: blur_x * scale, blur_y: blur_y * scale, strength, quality, dx: 0, dy: 0, color, inner, knockout, hide_object: false, expansion: 0.0, noise: 0.0, threshold: false });
            }
            Filter::AdjustColor { brightness, contrast, saturation, hue } => adjust(buf, &color_matrix(brightness, contrast, saturation, hue)),
            Filter::Noise { amount } => noise(buf, w, h, amount),
            Filter::Pixelate { size } => pixelate(buf, w, h, (size * scale).round().max(1.0) as usize),
            Filter::Threshold { level } => threshold(buf, level),
        }
    }
}

/// Box blur of width `n` along rows (`stride` 1) or columns, `passes` times, of any number of
/// channels per pixel. Outside the buffer is transparent.
fn box_pass<const C: usize>(buf: &mut [[f32; C]], w: usize, h: usize, n: usize, horizontal: bool, tmp: &mut Vec<[f32; C]>) {
    if n <= 1 {
        return;
    }
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let lo = n / 2;
    let hi = n - 1 - lo;
    let inv = 1.0 / n as f32;
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    tmp.clear();
    tmp.resize(len, [0.0; C]);
    for line in 0..lines {
        for (i, t) in tmp.iter_mut().enumerate() {
            if let Some(v) = buf.get(idx(line, i)) {
                *t = *v;
            }
        }
        let mut sum = [0.0f32; C];
        // Window [i - lo, i + hi]; start with [-lo, hi] for i = 0.
        for v in tmp.iter().take(hi.min(len.saturating_sub(1)) + 1) {
            for c in 0..C {
                sum[c] += v[c];
            }
        }
        for i in 0..len {
            if let Some(out) = buf.get_mut(idx(line, i)) {
                for c in 0..C {
                    out[c] = sum[c] * inv;
                }
            }
            if let Some(add) = tmp.get(i + hi + 1) {
                for c in 0..C {
                    sum[c] += add[c];
                }
            }
            if i >= lo
                && let Some(sub) = tmp.get(i - lo)
            {
                for c in 0..C {
                    sum[c] -= sub[c];
                }
            }
        }
    }
}

fn blur_channels<const C: usize>(buf: &mut [[f32; C]], w: usize, h: usize, bx: f32, by: f32, passes: u8) {
    let (nx, ny) = (bx.round().max(0.0) as usize, by.round().max(0.0) as usize);
    let mut tmp = Vec::new();
    for _ in 0..passes.max(1) {
        box_pass(buf, w, h, nx, true, &mut tmp);
        box_pass(buf, w, h, ny, false, &mut tmp);
    }
}

fn blur(buf: &mut [Premul], w: usize, h: usize, bx: f32, by: f32, passes: u8) {
    blur_channels(buf, w, h, bx, by, passes);
}

struct Shadow {
    blur_x: f32,
    blur_y: f32,
    strength: f32,
    quality: u8,
    dx: i64,
    dy: i64,
    color: Rgba,
    inner: bool,
    knockout: bool,
    hide_object: bool,
    /// Pixels the silhouette grows (positive) or shrinks (negative) before the blur.
    expansion: f32,
    /// Grain mixed into the shadow, `0..=1`.
    noise: f32,
    /// Snap the blurred coverage to hard on/off.
    threshold: bool,
}

fn shadow(buf: &mut [Premul], w: usize, h: usize, s: Shadow) {
    // The shadow's coverage: the artwork's opacity (or, inside, its transparency), offset.
    let mut cov: Vec<[f32; 1]> = vec![[if s.inner { 1.0 } else { 0.0 }]; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (sx, sy) = (x - s.dx, y - s.dy);
            let a = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h { buf.get(sy as usize * w + sx as usize).map_or(0.0, |p| p[3]) } else { 0.0 };
            if let Some(c) = cov.get_mut(y as usize * w + x as usize) {
                c[0] = if s.inner { 1.0 - a } else { a };
            }
        }
    }
    // Expansion dilates (a running box maximum) or erodes (the mirrored minimum) the
    // silhouette before the blur; the window is ±e pixels wide.
    let e = s.expansion.abs().round() as usize;
    if e > 0 {
        let n = e * 2 + 1;
        if s.expansion > 0.0 {
            box_extreme(&mut cov, w, h, n, f32::max);
        } else {
            box_extreme(&mut cov, w, h, n, f32::min);
        }
    }
    blur_channels(&mut cov, w, h, s.blur_x, s.blur_y, s.quality);
    if s.threshold {
        for k in cov.iter_mut() {
            k[0] = if k[0] >= 0.5 { 1.0 } else { 0.0 };
        }
    }
    let c = s.color.sanitized();
    for (i, (p, k)) in buf.iter_mut().zip(&cov).enumerate() {
        let sa = (k[0] * s.strength).clamp(0.0, 1.0) * c.a;
        // Shadow grain: deterministic per pixel, on the shadow colour before compositing.
        let mut col = c;
        if s.noise > 0.0 && sa > 0.0 {
            let d = (speckle(i, w) - 0.5) * s.noise;
            col = Rgba::new((c.r + d).clamp(0.0, 1.0), (c.g + d).clamp(0.0, 1.0), (c.b + d).clamp(0.0, 1.0), c.a);
        }
        let src = *p;
        let ba = src[3];
        *p = if s.inner {
            // Inside the artwork, over it.
            let a = sa * ba;
            if s.knockout { [col.r * a, col.g * a, col.b * a, a] } else { [col.r * a + src[0] * (1.0 - sa), col.g * a + src[1] * (1.0 - sa), col.b * a + src[2] * (1.0 - sa), ba] }
        } else if s.hide_object {
            [col.r * sa, col.g * sa, col.b * sa, sa]
        } else if s.knockout {
            let a = sa * (1.0 - ba);
            [col.r * a, col.g * a, col.b * a, a]
        } else {
            // Under the artwork.
            let k = 1.0 - ba;
            [src[0] + col.r * sa * k, src[1] + col.g * sa * k, src[2] + col.b * sa * k, ba + sa * k]
        };
    }
}

/// A separable running box maximum (or, with `f`, any monotone fold) over `n`-pixel windows.
fn box_extreme(buf: &mut [[f32; 1]], w: usize, h: usize, n: usize, f: fn(f32, f32) -> f32) {
    if n <= 1 {
        return;
    }
    let (lo, hi) = (n / 2, n - 1 - n / 2);
    let mut tmp = vec![[0.0f32]; 0];
    let pass = |buf: &mut [[f32; 1]], w: usize, h: usize, horizontal: bool, tmp: &mut Vec<[f32; 1]>| {
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
        tmp.clear();
        tmp.resize(len, [0.0; 1]);
        for line in 0..lines {
            for (i, t) in tmp.iter_mut().enumerate() {
                if let Some(v) = buf.get(idx(line, i)) {
                    *t = *v;
                }
            }
            for i in 0..len {
                let lo = i.saturating_sub(lo);
                let hi = (i + hi).min(len - 1);
                let mut m = tmp[lo][0];
                for v in &tmp[lo..=hi] {
                    m = f(m, v[0]);
                }
                if let Some(out) = buf.get_mut(idx(line, i)) {
                    out[0] = m;
                }
            }
        }
    };
    pass(buf, w, h, true, &mut tmp);
    pass(buf, w, h, false, &mut tmp);
}

/// Deterministic grain in `0..1` for pixel `i` of a `w`-wide buffer (a classic sine hash —
/// stable across renders and threads).
fn speckle(i: usize, w: usize) -> f32 {
    let (x, y) = ((i % w) as f32, (i / w) as f32);
    let v = (x * 12.9898 + y * 78.233).sin() * 43_758.547;
    v - v.floor()
}

/// Film grain over the artwork's colour (alpha kept): each pixel's straight colour shifts by
/// the same deterministic speckle, scaled by `amount`.
fn noise(buf: &mut [Premul], w: usize, _h: usize, amount: f32) {
    for (i, p) in buf.iter_mut().enumerate() {
        let a = p[3];
        if a <= 1e-6 {
            continue;
        }
        let d = (speckle(i, w) - 0.5) * amount;
        *p = [(p[0] + d * a).clamp(0.0, a), (p[1] + d * a).clamp(0.0, a), (p[2] + d * a).clamp(0.0, a), a];
    }
}

/// Mosaic: each `n`-pixel block takes the average of the pixels it actually covers.
fn pixelate(buf: &mut [Premul], w: usize, h: usize, n: usize) {
    if n <= 1 {
        return;
    }
    for by in 0..h.div_ceil(n) {
        for bx in 0..w.div_ceil(n) {
            let (x0, y0) = (bx * n, by * n);
            let (x1, y1) = ((x0 + n).min(w), (y0 + n).min(h));
            let mut sum = [0.0f32; 4];
            let mut count = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some(p) = buf.get(y * w + x) {
                        for c in 0..4 {
                            sum[c] += p[c];
                        }
                    }
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let avg = [sum[0] / count as f32, sum[1] / count as f32, sum[2] / count as f32, sum[3] / count as f32];
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some(p) = buf.get_mut(y * w + x) {
                        *p = avg;
                    }
                }
            }
        }
    }
}

/// Colours snap to black or white by their luminance against `level` (straight colour, alpha
/// kept).
fn threshold(buf: &mut [Premul], level: f32) {
    for p in buf.iter_mut() {
        let a = p[3];
        if a <= 1e-6 {
            continue;
        }
        let lum = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let v = (lum / a) >= level;
        *p = [if v { a } else { 0.0 }, if v { a } else { 0.0 }, if v { a } else { 0.0 }, a];
    }
}

/// A 4 × 5 colour matrix (rows r, g, b, a; columns r, g, b, a, offset), straight colour 0..1.
type Matrix = [[f32; 5]; 4];

const IDENTITY: Matrix = [[1.0, 0.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0, 0.0]];

/// `a` applied after `b`.
fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [[0.0; 5]; 4];
    for (i, row) in out.iter_mut().enumerate() {
        for j in 0..5 {
            let mut v: f32 = (0..4).map(|k| a[i][k] * b[k][j]).sum();
            if j == 4 {
                v += a[i][4];
            }
            row[j] = v;
        }
    }
    out
}

/// Brightness, contrast and saturation in −100..=100, hue in degrees.
pub(crate) fn color_matrix(brightness: f32, contrast: f32, saturation: f32, hue: f32) -> Matrix {
    let mut m = IDENTITY;
    // Brightness: an offset (100 adds the full range).
    let b = brightness / 100.0;
    m = mul(&[[1.0, 0.0, 0.0, 0.0, b], [0.0, 1.0, 0.0, 0.0, b], [0.0, 0.0, 1.0, 0.0, b], [0.0, 0.0, 0.0, 1.0, 0.0]], &m);
    // Contrast: scale about middle grey (−100 flattens to grey, +100 is ×10).
    let c = if contrast < 0.0 { 1.0 + contrast / 100.0 } else { 1.0 + contrast / 100.0 * 9.0 };
    let o = 0.5 * (1.0 - c);
    m = mul(&[[c, 0.0, 0.0, 0.0, o], [0.0, c, 0.0, 0.0, o], [0.0, 0.0, c, 0.0, o], [0.0, 0.0, 0.0, 1.0, 0.0]], &m);
    // Saturation: towards or away from luminance (Haeberli's weights).
    let s = if saturation < 0.0 { 1.0 + saturation / 100.0 } else { 1.0 + saturation / 100.0 * 2.0 };
    let (lr, lg, lb) = (0.3086, 0.6094, 0.0820);
    let k = 1.0 - s;
    m = mul(&[[k * lr + s, k * lg, k * lb, 0.0, 0.0], [k * lr, k * lg + s, k * lb, 0.0, 0.0], [k * lr, k * lg, k * lb + s, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0, 0.0]], &m);
    // Hue: rotation about the grey axis, keeping luminance.
    if hue != 0.0 {
        let (sin, cos) = hue.to_radians().sin_cos();
        let (lr, lg, lb) = (0.213, 0.715, 0.072);
        let r = [
            [lr + cos * (1.0 - lr) + sin * -lr, lg + cos * -lg + sin * -lg, lb + cos * -lb + sin * (1.0 - lb), 0.0, 0.0],
            [lr + cos * -lr + sin * 0.143, lg + cos * (1.0 - lg) + sin * 0.140, lb + cos * -lb + sin * -0.283, 0.0, 0.0],
            [lr + cos * -lr + sin * -(1.0 - lr), lg + cos * -lg + sin * lg, lb + cos * (1.0 - lb) + sin * lb, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0, 0.0],
        ];
        m = mul(&r, &m);
    }
    m
}

fn adjust(buf: &mut [Premul], m: &Matrix) {
    for p in buf.iter_mut() {
        let a = p[3];
        if a <= 1e-6 {
            continue;
        }
        let (r, g, b) = (p[0] / a, p[1] / a, p[2] / a);
        let ch = |row: &[f32; 5]| (row[0] * r + row[1] * g + row[2] * b + row[3] * a + row[4]).clamp(0.0, 1.0);
        let out_a = ch(&m[3]);
        *p = [ch(&m[0]) * out_a, ch(&m[1]) * out_a, ch(&m[2]) * out_a, out_a];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: usize, h: usize, x0: usize, x1: usize) -> Vec<Premul> {
        (0..w * h).map(|i| if (x0..x1).contains(&(i % w)) && (x0..x1).contains(&(i / w)) { [1.0, 0.0, 0.0, 1.0] } else { [0.0; 4] }).collect()
    }

    #[test]
    fn blur_spreads_and_keeps_the_total() {
        let (w, h) = (40, 40);
        let mut b = square(w, h, 15, 25);
        let before: f32 = b.iter().map(|p| p[3]).sum();
        apply(&mut b, w, h, &[Filter::Blur { blur_x: 8.0, blur_y: 8.0, quality: 3 }], 1.0);
        let after: f32 = b.iter().map(|p| p[3]).sum();
        assert!((before - after).abs() < 0.5, "{before} vs {after}");
        assert!(b[20 * w + 12][3] > 0.05, "spreads outside");
        assert!(b[20 * w + 20][3] < 0.9 && b[20 * w + 20][3] > 0.3, "softens inside: {}", b[20 * w + 20][3]);
        assert!((b[20 * w + 20][0] - b[20 * w + 20][3]).abs() < 1e-5, "stays premultiplied red");
    }

    #[test]
    fn drop_shadows_sit_under_and_offset() {
        let (w, h) = (40, 40);
        let mut b = square(w, h, 10, 20);
        let ds = Filter::DropShadow { blur_x: 0.0, blur_y: 0.0, strength: 1.0, quality: 1, angle: 0.0, distance: 5.0, color: Rgba::BLACK, inner: false, knockout: false, hide_object: false, expansion: 0.0, noise: 0.0, threshold: false };
        apply(&mut b, w, h, std::slice::from_ref(&ds), 1.0);
        assert_eq!(b[15 * w + 22], [0.0, 0.0, 0.0, 1.0], "shadow to the right");
        assert_eq!(b[15 * w + 15], [1.0, 0.0, 0.0, 1.0], "artwork on top");
        assert_eq!(b[15 * w + 27][3], 0.0);
        // Knockout: only the shadow outside the artwork; hide object: the whole shadow.
        let mut k = square(w, h, 10, 20);
        let Filter::DropShadow { .. } = ds else { panic!() };
        apply(&mut k, w, h, &[Filter::DropShadow { knockout: true, blur_x: 0.0, blur_y: 0.0, strength: 1.0, quality: 1, angle: 0.0, distance: 5.0, color: Rgba::BLACK, inner: false, hide_object: false, expansion: 0.0, noise: 0.0, threshold: false }], 1.0);
        assert_eq!(k[15 * w + 15][3], 0.0);
        assert_eq!(k[15 * w + 22][3], 1.0);
        // Inner glow: inside the edge only.
        let mut g = square(w, h, 10, 30);
        apply(&mut g, w, h, &[Filter::Glow { blur_x: 6.0, blur_y: 6.0, strength: 1.0, quality: 2, color: Rgba::WHITE, inner: true, knockout: false }], 1.0);
        assert!(g[20 * w + 11][1] > 0.2, "near the edge glows: {:?}", g[20 * w + 11]);
        assert!(g[20 * w + 20][1] < 0.01, "the middle doesn't");
        assert_eq!(g[20 * w + 5][3], 0.0, "nothing outside");
    }

    #[test]
    fn shadow_expansion_grows_and_shrinks_the_silhouette() {
        let (w, h) = (40, 40);
        // Hard shadow (no blur) 5px right: expansion 3 reaches 8px out, −3 only 2px.
        let mk = |expansion| Filter::DropShadow { blur_x: 0.0, blur_y: 0.0, strength: 1.0, quality: 1, angle: 0.0, distance: 5.0, color: Rgba::BLACK, inner: false, knockout: true, hide_object: false, expansion, noise: 0.0, threshold: true };
        let mut grown = square(w, h, 10, 20);
        apply(&mut grown, w, h, &[mk(3.0)], 1.0);
        assert_eq!(grown[15 * w + 27][3], 1.0, "dilated 3px past the edge (band 12..28)");
        assert_eq!(grown[15 * w + 11][3], 0.0);
        assert_eq!(grown[15 * w + 28][3], 0.0);
        let mut shrunk = square(w, h, 10, 20);
        apply(&mut shrunk, w, h, &[mk(-3.0)], 1.0);
        assert_eq!(shrunk[15 * w + 21][3], 1.0, "eroded keeps the middle (band 18..22, minus the knocked-out artwork)");
        assert_eq!(shrunk[15 * w + 14][3], 0.0, "the near side is gone");
        assert_eq!(shrunk[15 * w + 22][3], 0.0, "and the far 3px");
    }

    #[test]
    fn noise_pixelate_and_threshold_behave() {
        let (w, h) = (20, 20);
        // Noise: grey shifts by the same bounded speckle, deterministically.
        let mut n = vec![[0.5, 0.5, 0.5, 1.0]; w * h];
        apply(&mut n, w, h, &[Filter::Noise { amount: 0.6 }], 1.0);
        assert!(n[0][0] != n[1][0], "neighbouring pixels speckle apart");
        assert!(n.iter().all(|p| (0.19..=0.81).contains(&p[0])), "bounded: {:?}", n[0]);
        let mut n2 = vec![[0.5, 0.5, 0.5, 1.0]; w * h];
        apply(&mut n2, w, h, &[Filter::Noise { amount: 0.6 }], 1.0);
        assert_eq!(n, n2, "the same pixel always gets the same speckle");
        // Pixelate: 10px blocks average to one flat colour per block (red left edge against
        // green).
        let mut p: Vec<Premul> = (0..w * h).map(|i| if (i % w) < 5 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0, 1.0] }).collect();
        apply(&mut p, w, h, &[Filter::Pixelate { size: 10.0 }], 1.0);
        assert_eq!(p[0], [0.5, 0.5, 0.0, 1.0], "the first block averages red+green");
        assert_eq!(p[15 * w + 15], [0.0, 1.0, 0.0, 1.0], "flat green inside the second block");
        // Threshold: grey snaps to white above the level, black below.
        let mut t = vec![[0.2, 0.2, 0.2, 1.0], [0.8, 0.8, 0.8, 1.0], [0.5, 0.5, 0.5, 0.4]];
        apply(&mut t, 3, 1, &[Filter::Threshold { level: 0.5 }], 1.0);
        assert_eq!(t[0], [0.0, 0.0, 0.0, 1.0], "dark to black");
        assert_eq!(t[1], [1.0, 1.0, 1.0, 1.0], "light to white");
        assert_eq!(t[2][3], 0.4, "alpha kept");
    }

    #[test]
    fn adjust_color_matrix() {
        let mut p = vec![[0.5, 0.25, 0.0, 1.0]];
        apply(&mut p, 1, 1, &[Filter::AdjustColor { brightness: 0.0, contrast: 0.0, saturation: -100.0, hue: 0.0 }], 1.0);
        assert!((p[0][0] - p[0][1]).abs() < 1e-4 && (p[0][1] - p[0][2]).abs() < 1e-4, "greyscale: {:?}", p[0]);
        let mut q = vec![[0.2, 0.2, 0.2, 1.0]];
        apply(&mut q, 1, 1, &[Filter::AdjustColor { brightness: 50.0, contrast: 0.0, saturation: 0.0, hue: 0.0 }], 1.0);
        assert!((q[0][0] - 0.7).abs() < 1e-4);
        // Hue rotation keeps grey grey and turns red away from red.
        let mut r = vec![[1.0, 0.0, 0.0, 1.0], [0.5, 0.5, 0.5, 1.0]];
        apply(&mut r, 2, 1, &[Filter::AdjustColor { brightness: 0.0, contrast: 0.0, saturation: 0.0, hue: 120.0 }], 1.0);
        assert!(r[0][1] > r[0][0], "red turns green: {:?}", r[0]);
        assert!((r[1][0] - 0.5).abs() < 1e-3 && (r[1][2] - 0.5).abs() < 1e-3);
        assert_eq!(color_matrix(0.0, 0.0, 0.0, 0.0), IDENTITY);
    }
}
