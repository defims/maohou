//! A `MeshLayer`'s `mesh` (points, curves, shapes) → AniCraft shape elements.
//!
//! Geometry stays in Moho's inch coordinates (y up); the layer's element transform carries
//! them onto the stage (see [`Builder::to_px`](crate::import::Builder)). A shape's outline is
//! the chain of `(curve, segment, flag)` runs in its `edges`: `segment` indexes a curve's
//! segments (a closed curve of n points has n, an open one n−1), and `flag` 1 traverses the
//! run backwards (corpus: stroke-only shapes flag their single run, filled loops run
//! everything forward). Handles are reconstructed per point from its neighbours: direction
//! along the curve, rotated by the stored `offset_*` (radians), length
//! `distance/3 · smoothness · weight_*`, zero at smoothness 0 — an approximation of Moho's
//! own handle placement, reported as such; the untouched mesh travels in the sidecar until a
//! later milestone checks reconstruction against Moho renders.

use crate::anim;
use crate::channel::{self, Channel};
use crate::import::Builder;
use crate::json::{self, array, get};
use anicraft_doc::{ColorEffect, Element, ElementId, ElementKind, GradientStop, Paint, PointTrack, Rgba, Shape, Stroke, Trim, WidthPoint, WidthProfile, path_point_count};
use anicraft_geom::{Affine, FillRule, Path, PathCmd, Vec2};
use serde_json::Value;
use std::sync::Arc;

/// Biggest mesh we take (about a hundred thousand points of hand-drawn work).
const MAX_POINTS: usize = 200_000;
const MAX_CURVES: usize = 40_000;
const MAX_SHAPES: usize = 20_000;
const MAX_STOPS: usize = 64;

/// One curve's point reference with its static handle parameters.
struct CurveRef {
    point: usize,
    smoothness: f32,
    weight_in: f32,
    weight_out: f32,
    offset_in: f32,
    offset_out: f32,
}

/// One parsed curve: point references plus handle data, closed or open.
struct Curve {
    closed: bool,
    pts: Vec<CurveRef>,
}

/// The mesh's static geometry, shared by all its shapes. `parents` mirrors `positions`:
/// each point's bound bone (Moho's `Point.parent`, negative = unbound). The per-point
/// channels are stage 5's point-level family (ADR 0105): a point whose position animates
/// carries its (converted) channel, and width and colour ride along at the import frame.
struct MeshData {
    positions: Vec<Vec2>,
    curves: Vec<Curve>,
    parents: Vec<i64>,
    /// A point's position channel when it animates (keys in Moho inches, as the path is).
    animated: Vec<Option<anicraft_doc::Channel<Vec2>>>,
    /// A point's line width at the import frame as a factor of the shape's (`None` = 1).
    widths: Vec<Option<f32>>,
    /// A point's colour, when the point is coloured.
    colors: Vec<Option<Rgba>>,
}

/// Builds the shape elements of one mesh. `world` is the layer's own inch-space transform
/// (ancestors live on their folder layers since stage 3).
pub(crate) fn mesh_elements(b: &mut Builder, mesh: &Value, world: Affine, out: &mut Vec<Arc<Element>>) {
    let Some(data) = parse_mesh(b, mesh) else { return };
    let Some(shapes) = array(get(mesh, "shapes")) else { return };
    let transform = b.to_px.mul(&world);
    for shape in shapes.iter().take(MAX_SHAPES) {
        shape_element(b, mesh, shape, &data, transform, out);
    }
    if shapes.len() > MAX_SHAPES {
        b.report.skip("meshes past 20,000 shapes");
    }
}

/// Reads points and curves. Broken entries drop their point or curve with a report line,
/// never a panic.
fn parse_mesh(b: &mut Builder, mesh: &Value) -> Option<MeshData> {
    let points = array(get(mesh, "points"))?;
    if points.len() > MAX_POINTS {
        b.report.skip("meshes past 200,000 points");
        return None;
    }
    let mut positions = Vec::with_capacity(points.len());
    let parents: Vec<i64> = points.iter().map(|p| json::integer(get(p, "parent")).unwrap_or(-2)).collect();
    let mut animated = Vec::with_capacity(points.len());
    let mut widths = Vec::with_capacity(points.len());
    let mut colors = Vec::with_capacity(points.len());
    for p in points {
        // Position: the static sample bakes into the path; a channel that animates rides
        // the shape key by key (ADR 0105), its values Moho's own inches.
        let pos = get(p, "position").and_then(Channel::parse).and_then(|c| channel::sample_vec2(&c, b.frame).map(|v| (c, v)));
        match pos {
            Some((_, v)) if v.is_finite() => {
                let mut ch = anicraft_doc::Channel::default();
                let moves = get(p, "position").is_some_and(|v| anim::read_channel_value(&mut ch, v, &mut b.report, 2, |ch, i| channel::sample_vec2_at(ch, i).unwrap_or(Vec2::ZERO)).is_some());
                animated.push(moves.then_some(ch));
                positions.push(v);
            }
            _ => {
                b.report.skip("mesh points without a readable position");
                positions.push(Vec2::ZERO);
                animated.push(None);
            }
        }
        // Width: a factor of the shape's stroke width, sampled at the import frame.
        let wch = get(p, "width").and_then(Channel::parse);
        if wch.as_ref().is_some_and(|c| c.is_animated()) {
            b.report.skip("animated point widths (drawn at the import frame)");
        }
        let w = wch.and_then(|c| channel::sample_f64(&c, b.frame)).map(|v| v as f32).filter(|w| w.is_finite() && (*w - 1.0).abs() > 1e-6);
        widths.push(w);
        // Colour: only colored points carry one; a partial strength draws at full and says so.
        if json::boolean(get(p, "colored")) == Some(true) {
            let c = get(p, "color").and_then(Channel::parse).and_then(|c| channel::sample_color01(&c, b.frame));
            if get(p, "color_strength").and_then(Channel::parse).and_then(|c| channel::sample_f64(&c, b.frame)).is_some_and(|s| (s - 1.0).abs() > 1e-6) {
                b.report.skip("point colour strengths (drawn at full strength)");
            }
            colors.push(c);
        } else {
            colors.push(None);
        }
    }
    let curves_json = array(get(mesh, "curves"))?;
    if curves_json.len() > MAX_CURVES {
        b.report.skip("meshes past 40,000 curves");
        return None;
    }
    let mut curves = Vec::with_capacity(curves_json.len());
    for c in curves_json {
        // A variable-width stroke profiled by another layer's curve: stage 5 follows
        // upstream's ADR 0018 width profiles for point widths; a sampled profile curve has
        // no slot yet — the sidecar keeps it.
        if json::integer(get(c, "profile_curve_id")).is_some_and(|v| v != -1) {
            b.report.skip("stroke profiles drawn from profile curves");
        }
        let closed = json::boolean(get(c, "closed")).unwrap_or(false);
        let refs = array(get(c, "points")).unwrap_or(&[]);
        let mut pts = Vec::with_capacity(refs.len());
        for r in refs {
            let idx = json::integer(get(r, "point")).and_then(|i| usize::try_from(i).ok());
            let Some(point) = idx.filter(|i| *i < positions.len()) else {
                b.report.skip("curves pointing past the point list");
                continue;
            };
            let ch = |k: &str| get(r, k).and_then(Channel::parse).and_then(|c| channel::sample_f64(&c, b.frame)).unwrap_or(0.0) as f32;
            pts.push(CurveRef {
                point,
                smoothness: ch("smoothness").clamp(0.0, 1.0),
                weight_in: ch("weight_in").max(0.0),
                weight_out: ch("weight_out").max(0.0),
                offset_in: ch("offset_in"),
                offset_out: ch("offset_out"),
            });
        }
        if pts.len() >= 2 {
            curves.push(Curve { closed, pts });
        }
    }
    Some(MeshData { positions, curves, parents, animated, widths, colors })
}

/// One shape: its outline plus fill/stroke, as one fill-only and one stroke-only element
/// (AniCraft draws them separately, as the XFL importer does). Both carry the shape's
/// bone binding (ADR 0104) when its points all hang off one bone, and the point-level
/// family (ADR 0105): animated point positions, the curve trim, the point widths as a
/// stroke profile, a uniform point colour.
fn shape_element(b: &mut Builder, mesh: &Value, shape: &Value, data: &MeshData, transform: Affine, out: &mut Vec<Arc<Element>>) {
    let has_fill = json::boolean(get(shape, "has_fill")).unwrap_or(false);
    let has_outline = json::boolean(get(shape, "has_outline")).unwrap_or(false);
    if !has_fill && !has_outline {
        return;
    }
    let (path, endpoints) = shape_path(b, shape, data);
    if path.is_empty() {
        return;
    }
    let bind = shape_bind(b, shape, data);
    // Animated points: one track per path occurrence of an animated Moho point, all
    // sampling that point's channel (a shared point moves everywhere at once).
    let mut tracks: Vec<PointTrack> = Vec::new();
    for (pi, mi) in &endpoints {
        if let Some(ch) = data.animated.get(*mi).cloned().flatten() {
            tracks.push(PointTrack { index: *pi, position: ch });
        }
    }
    let trim = shape_trim(b, mesh, shape);
    let (mut fill, mut stroke) = shape_paint(b, shape, &path, has_fill, has_outline);
    shape_flags(b, shape);
    // Point widths: a shape whose points agree multiplies the stroke width; a shape whose
    // points vary gets a width profile over its (single-subpath) outline.
    let weights: Vec<(u32, f32)> = endpoints
        .iter()
        .filter_map(|(pi, mi)| data.widths.get(*mi).cloned().flatten().map(|w| (*pi, w)))
        .collect();
    let uniform = weights.first().filter(|_| weights.iter().all(|(_, w)| (*w - weights[0].1).abs() < 1e-6)).map(|(_, w)| *w);
    if uniform.is_none() && weights.len() > 1
        && let Some(profile) = width_profile(b, &path, &weights)
        && let Some(st) = stroke.as_mut()
    {
        st.profile = Some(profile);
    } else if let Some(w) = uniform
        && let Some(st) = stroke.as_mut()
    {
        st.width *= w;
    }
    if weights.len() == 1 && uniform.is_none() {
        b.report.skip("single varying point widths (drawn uniform)");
    }
    // Point colours: when every coloured point of the shape agrees, it replaces a solid
    // fill and the stroke colour; anything else is counted and drawn as authored.
    let colored: Vec<Rgba> = endpoints.iter().filter_map(|(_, mi)| data.colors.get(*mi).cloned().flatten()).collect();
    if let Some(c) = colored.first().filter(|c| colored.iter().all(|o| (o.r - c.r).abs() < 1e-6 && (o.g - c.g).abs() < 1e-6 && (o.b - c.b).abs() < 1e-6 && (o.a - c.a).abs() < 1e-6)).copied() {
        match fill.as_mut() {
            Some(Paint::Solid(s)) => *s = c,
            Some(_) => b.report.skip("point colours on gradient fills (drawn with the gradient)"),
            None => {}
        }
        if let Some(st) = stroke.as_mut() {
            st.color = c;
        }
    } else if colored.len() > 1 {
        b.report.skip("point colours that vary on one shape (drawn uncoloured)");
    }
    let fx = ColorEffect::default();
    if let Some(p) = fill {
        out.push(Arc::new(Element {
            id: ElementId(b.doc.alloc_id()),
            name: String::new(),
            transform,
            color: fx,
            kind: ElementKind::Shape(Shape { path: Arc::new(path.clone()), fill: Some(p), stroke: None, fill_rule: FillRule::NonZero, merge: false, bind: bind.clone(), points: tracks.clone(), trim: trim.clone().map(Box::new) }),
        }));
    }
    if let Some(s) = stroke {
        out.push(Arc::new(Element {
            id: ElementId(b.doc.alloc_id()),
            name: String::new(),
            transform,
            color: fx,
            kind: ElementKind::Shape(Shape { path: Arc::new(path), fill: None, stroke: Some(s), fill_rule: FillRule::NonZero, merge: false, bind, points: tracks, trim: trim.map(Box::new) }),
        }));
    }
}

/// The shape's trim (ADR 0105): the `start_percent`/`end_percent` channels of the curves its
/// edges run along. Moho's untrimmed state is `−0.1 → 1.1` (slightly past both ends); the
/// raw values store into the trim and clamp at sample time, so that reads as no trim. One
/// trim per shape: the first cutting curve's, when others disagree.
fn shape_trim(b: &mut Builder, mesh: &Value, shape: &Value) -> Option<Trim> {
    let curves = array(get(mesh, "curves")).unwrap_or(&[]);
    let cs = array(get(get(shape, "edges")?, "curve")).unwrap_or(&[]);
    let mut found: Option<Trim> = None;
    let mut static_cut: Option<(f64, f64)> = None;
    for ci in cs.iter().filter_map(|v| json::integer(Some(v)).and_then(|i| usize::try_from(i).ok())) {
        let Some(c) = curves.get(ci) else { continue };
        let mut t = Trim::default();
        let cut_s = get(c, "start_percent").is_some_and(|v| anim::read_channel_value(&mut t.start, v, &mut b.report, 1, |ch, i| channel::sample_f64_at(ch, i).unwrap_or(-0.1) as f32).is_some());
        let cut_e = get(c, "end_percent").is_some_and(|v| anim::read_channel_value(&mut t.end, v, &mut b.report, 1, |ch, i| channel::sample_f64_at(ch, i).unwrap_or(1.1) as f32).is_some());
        let (sv, ev) = (t.start.sample_or(b.frame as f32, -0.1) as f64, t.end.sample_or(b.frame as f32, 1.1) as f64);
        let cuts = cut_s || cut_e || (sv + 0.1).abs() > 1e-3 || (ev - 1.1).abs() > 1e-3;
        if !cuts {
            continue;
        }
        match (&found, &static_cut) {
            (None, None) => {
                found = Some(t);
                static_cut = Some((sv, ev));
            }
            (_, Some((s0, e0))) if (s0 - sv).abs() > 1e-3 || (e0 - ev).abs() > 1e-3 => {
                b.report.skip("per-curve trims that differ on one shape (the first curve's trim draws)");
            }
            _ => {}
        }
    }
    found
}

/// A stroke's width profile from varying point widths (ADR 0105): each point's factor sits
/// at its arc fraction along the outline. One subpath maps exactly; with several, the
/// profile repeats per subpath (the report says so) — Moho's per-point widths along a
/// multi-run stroke have no closer slot yet.
fn width_profile(b: &mut Builder, path: &Path, weights: &[(u32, f32)]) -> Option<WidthProfile> {
    // Cumulative length at each endpoint, per subpath (control-polygon chords).
    let mut subs: Vec<Vec<(u32, f32)>> = Vec::new();
    let mut cur: Vec<(u32, f32)> = Vec::new();
    let mut total = 0.0f32;
    let mut prev = Vec2::ZERO;
    let mut idx = 0usize;
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) => {
                if !cur.is_empty() {
                    subs.push(std::mem::take(&mut cur));
                    total = 0.0;
                }
                idx += 1;
                cur.push(((idx - 1) as u32, 0.0));
                prev = p;
            }
            PathCmd::LineTo(p) => {
                total += (p - prev).length();
                idx += 1;
                cur.push(((idx - 1) as u32, total));
                prev = p;
            }
            PathCmd::QuadTo(c, p) => {
                total += (c - prev).length() + (p - c).length();
                idx += 2;
                cur.push(((idx - 1) as u32, total));
                prev = p;
            }
            PathCmd::CubicTo(c1, c2, p) => {
                total += (c1 - prev).length() + (c2 - c1).length() + (p - c2).length();
                idx += 3;
                cur.push(((idx - 1) as u32, total));
                prev = p;
            }
            // A Close ends the run at its start: nothing new to record.
            PathCmd::Close => {}
        }
    }
    if !cur.is_empty() {
        subs.push(cur);
    }
    let mut points: Vec<WidthPoint> = Vec::new();
    let mut subs_used = 0usize;
    for sub in &subs {
        let end = sub.last().map(|(_, t)| *t).unwrap_or(0.0);
        if end <= 1e-6 {
            continue;
        }
        let mut used = false;
        for (pi, w) in weights {
            let Some((_, cum)) = sub.iter().find(|(i, _)| i == pi) else { continue };
            points.push(WidthPoint::new(cum / end, *w, *w));
            used = true;
        }
        subs_used += used as usize;
    }
    if subs_used > 1 {
        b.report.skip("point widths across subpaths (the profile repeats per subpath)");
    }
    WidthProfile::new(points)
}

/// The shape's bone binding: the distinct bone its points hang off (Moho's single-bone
/// `Point.parent` reads as one pair at weight 1, ADR 0104). A shape spanning several bones
/// — mixed weights the model doesn't hold yet — draws at rest, and the report says so.
fn shape_bind(b: &mut Builder, shape: &Value, data: &MeshData) -> Vec<(anicraft_doc::BoneId, f32)> {
    let Some(edges) = get(shape, "edges") else { return Vec::new() };
    let cs = array(get(edges, "curve")).unwrap_or(&[]);
    let mut bones: Vec<u32> = Vec::new();
    for ci in cs.iter().filter_map(|v| json::integer(Some(v)).and_then(|i| usize::try_from(i).ok())) {
        let Some(curve) = data.curves.get(ci) else { continue };
        for r in &curve.pts {
            let Some(p) = data.parents.get(r.point) else { continue };
            let Ok(bone) = u32::try_from(*p) else { continue }; // negative = unbound
            if !bones.contains(&bone) {
                bones.push(bone);
            }
        }
    }
    match bones.as_slice() {
        [] => Vec::new(),
        [one] => vec![(anicraft_doc::BoneId(*one), 1.0)],
        _ => {
            b.report.skip("shapes bound to several bones (drawn at rest)");
            Vec::new()
        }
    }
}

/// The shape's outline: `edges` runs chained into subpaths. Also returns, per path point
/// (in command order), which Moho point each endpoint came from — the hook the animated
/// point channels hang on (ADR 0105).
fn shape_path(b: &mut Builder, shape: &Value, data: &MeshData) -> (Path, Vec<(u32, usize)>) {
    let Some(edges) = get(shape, "edges") else { return (Path::default(), Vec::new()) };
    let cs = array(get(edges, "curve")).unwrap_or(&[]);
    let ss = array(get(edges, "segment")).unwrap_or(&[]);
    let fs = array(get(edges, "flag")).unwrap_or(&[]);
    if cs.is_empty() {
        return (Path::default(), Vec::new());
    }
    if cs.len() != ss.len() || cs.len() != fs.len() || cs.len() > MAX_SHAPES {
        b.report.skip("shapes with mismatched edge lists");
        return (Path::default(), Vec::new());
    }
    let mut path = Path::default();
    let mut endpoints: Vec<(u32, usize)> = Vec::new();
    let mut pen: Option<Vec2> = None;
    let mut first: Option<Vec2> = None;
    for i in 0..cs.len() {
        let (Some(ci), Some(si), Some(fi)) = (json::integer(cs.get(i)), json::integer(ss.get(i)), json::integer(fs.get(i))) else { continue };
        let (Ok(ci), Ok(si)) = (usize::try_from(ci), usize::try_from(si)) else { continue };
        let Some(curve) = data.curves.get(ci) else {
            b.report.skip("shape edges pointing past the curve list");
            continue;
        };
        let n = curve.pts.len();
        let segments = if curve.closed { n } else { n - 1 };
        if si >= segments {
            b.report.skip("shape edges with out-of-range segments");
            continue;
        }
        let after = if si + 1 < n { si + 1 } else { 0 };
        let (a, d) = (&curve.pts[si], &curve.pts[after]);
        let (Some(p0), Some(p1)) = (data.positions.get(a.point), data.positions.get(d.point)) else { continue };
        let (start, end, h0, h1) = if fi == 1 {
            (*p1, *p0, handle(data, curve, after, true), handle(data, curve, si, false))
        } else {
            (*p0, *p1, handle(data, curve, si, false), handle(data, curve, after, true))
        };
        // Chained runs continue the subpath; a gap starts a new one.
        if !pen.is_some_and(|p| (p - start).length() < 1e-4) {
            path.cmds.push(PathCmd::MoveTo(start));
            endpoints.push((path_point_count(&path).saturating_sub(1) as u32, a.point));
            first = Some(start);
        }
        if h0.length() > 1e-9 || h1.length() > 1e-9 {
            path.cmds.push(PathCmd::CubicTo(start + h0, end + h1, end));
        } else {
            path.cmds.push(PathCmd::LineTo(end));
        }
        endpoints.push((path_point_count(&path).saturating_sub(1) as u32, d.point));
        pen = Some(end);
    }
    // A run that came home closes explicitly, so fills see a closed ring.
    if let (Some(f), Some(l)) = (first, pen)
        && path.cmds.len() > 1
        && (f - l).length() < 1e-4
    {
        path.cmds.push(PathCmd::Close);
    }
    (path, endpoints)
}

/// The outgoing (or, with `in_side`, incoming) handle of curve point `i`, from its
/// neighbour's tangent and the stored smoothness/weight/offset. Closed curves wrap; an open
/// curve's ends have no handle on the outside (no segment arrives there).
fn handle(data: &MeshData, curve: &Curve, i: usize, in_side: bool) -> Vec2 {
    let n = curve.pts.len();
    if n < 2 {
        return Vec2::ZERO;
    }
    let r = &curve.pts[i];
    if r.smoothness <= 0.001 {
        return Vec2::ZERO;
    }
    let j = if in_side {
        if i > 0 {
            i - 1
        } else if curve.closed {
            n - 1
        } else {
            return Vec2::ZERO;
        }
    } else if i + 1 < n {
        i + 1
    } else if curve.closed {
        0
    } else {
        return Vec2::ZERO;
    };
    let (Some(p), Some(q)) = (data.positions.get(r.point), data.positions.get(curve.pts[j].point)) else { return Vec2::ZERO };
    let (weight, offset) = if in_side { (r.weight_in, r.offset_in) } else { (r.weight_out, r.offset_out) };
    let dir = if in_side { *p - *q } else { *q - *p };
    let len = dir.length() / 3.0 * r.smoothness * weight;
    if !len.is_finite() || len <= 1e-9 || dir.length() <= 1e-9 {
        return Vec2::ZERO;
    }
    let unit = dir.normalized();
    // The stored offset is a small angle (radians) off the tangent.
    let (sin, cos) = offset.sin_cos();
    let rotated = Vec2::new(unit.x * cos - unit.y * sin, unit.x * sin + unit.y * cos);
    if !rotated.is_finite() {
        return Vec2::ZERO;
    }
    let sign = if in_side { -1.0 } else { 1.0 };
    rotated * (len * sign)
}

/// Fill and stroke paint for one shape, from its inline `style`.
fn shape_paint(b: &mut Builder, shape: &Value, path: &Path, has_fill: bool, has_outline: bool) -> (Option<Paint>, Option<Stroke>) {
    let empty = Value::Null;
    let style = get(shape, "style").unwrap_or(&empty);
    let frame = b.frame;
    let fill = if has_fill {
        let gradient = get(style, "fill_style").filter(|_| json::integer(get(style, "fill_style_id")) == Some(9));
        match gradient {
            Some(g) => gradient_paint(b, g, shape, path).or_else(|| {
                b.report.skip("unreadable gradient fills (drawn solid)");
                solid_fill(style, frame)
            }),
            None => {
                // Moho's fill-effect transform belongs to gradient fills; on a solid fill it
                // has nothing to act on, and the report says so when it carries values.
                let (off, rot, scale) = effect_transform(shape, b.frame);
                if off != Vec2::ZERO || rot.abs() > 1e-6 || (scale - 1.0).abs() > 1e-6 {
                    b.report.skip("shape effect transforms on non-gradient fills");
                }
                solid_fill(style, frame)
            }
        }
    } else {
        None
    };
    let stroke = if has_outline {
        let color = get(style, "line_color").and_then(Channel::parse).and_then(|c| channel::sample_color01(&c, frame)).unwrap_or(Rgba::BLACK);
        let width = json::num(get(style, "line_width")).unwrap_or(0.022) as f32;
        Some(Stroke::new(color, width.clamp(1e-4, 10.0)))
    } else {
        None
    };
    (fill, stroke)
}

fn solid_fill(style: &Value, frame: f64) -> Option<Paint> {
    let c = get(style, "fill_color").and_then(Channel::parse).and_then(|c| channel::sample_color01(&c, frame))?;
    Some(Paint::Solid(c))
}

/// The shape's fill-effect transform at `frame`: the offset in inches, the rotation in
/// radians, the scale (Moho's `effect_offset`/`effect_rotation`/`effect_scale` channels —
/// the gradient handles an artist drags in Moho land here).
fn effect_transform(shape: &Value, frame: f64) -> (Vec2, f32, f32) {
    let ch = |k: &str| get(shape, k).and_then(Channel::parse);
    let off = ch("effect_offset").and_then(|c| channel::sample_vec2(&c, frame)).unwrap_or(Vec2::ZERO);
    let rot = ch("effect_rotation").and_then(|c| channel::sample_f64(&c, frame)).unwrap_or(0.0) as f32;
    let scale = ch("effect_scale").and_then(|c| channel::sample_f64(&c, frame)).unwrap_or(1.0) as f32;
    let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
    (Vec2::new(finite(off.x, 0.0), finite(off.y, 0.0)), finite(rot, 0.0), finite(scale, 1.0).max(1e-4))
}

/// An `SS_Gradient2` fill: stops from `{location, color}` channel pairs, laid out across the
/// shape's bounds and placed by the shape's fill-effect transform (`gradient_type` 0 linear,
/// 1 radial, 2 reflected, 3 angle — the corpus's four values). With `through_alpha` false
/// Moho ignores the stops' alpha and draws opaque.
fn gradient_paint(b: &mut Builder, g: &Value, shape: &Value, path: &Path) -> Option<Paint> {
    let kind = json::integer(get(g, "gradient_type")).unwrap_or(0);
    let marks = array(get(g, "gradients"))?;
    let frame = b.frame;
    let through_alpha = json::boolean(get(g, "through_alpha")).unwrap_or(false);
    if marks.len() > MAX_STOPS {
        b.report.skip("gradient stops past 64");
    }
    let mut stops: Vec<GradientStop> = marks
        .iter()
        .take(MAX_STOPS)
        .filter_map(|m| {
            let o = get(m, "location").and_then(Channel::parse).and_then(|c| channel::sample_f64(&c, frame))?;
            let c = get(m, "color").and_then(Channel::parse).and_then(|c| channel::sample_color01(&c, frame))?;
            let c = if through_alpha { c } else { c.with_alpha(1.0) };
            Some(GradientStop { offset: (o as f32).clamp(0.0, 1.0), color: c })
        })
        .collect();
    if stops.is_empty() {
        return None;
    }
    stops.sort_by(|a, s| a.offset.total_cmp(&s.offset));
    // The geometry: a canonical layout over the shape's bounds, rotated, scaled and shifted
    // by the fill-effect transform.
    let bounds = path.bounds();
    let centre = Vec2::new((bounds.min.x + bounds.max.x) * 0.5, (bounds.min.y + bounds.max.y) * 0.5);
    let (off, rot, scale) = effect_transform(shape, frame);
    let (sin, cos) = rot.sin_cos();
    let place = |p: Vec2| {
        let q = (p - centre) * scale;
        Vec2::new(q.x * cos - q.y * sin, q.x * sin + q.y * cos) + centre + off
    };
    let radius = (bounds.max.x - bounds.min.x).hypot(bounds.max.y - bounds.min.y) * 0.5 * scale;
    match kind {
        0 => Some(Paint::Linear { start: place(Vec2::new(bounds.min.x, centre.y)), end: place(Vec2::new(bounds.max.x, centre.y)), stops }),
        1 => Some(Paint::Radial { center: place(centre), radius: radius.max(1e-4), stops }),
        // The reflected gradient mirrors about the shape's centre: the axis runs from the
        // centre to the left edge, and the triangle-wave paint mirrors it to the right, so
        // the fill is symmetric (centre = first stop, edges = last).
        2 => Some(Paint::Reflected { start: place(centre), end: place(Vec2::new(bounds.min.x, centre.y)), stops }),
        3 => Some(Paint::Angle { center: place(centre), stops }),
        other => {
            b.report.skip(&format!("gradient type {other} fills (drawn solid)"));
            None
        }
    }
}

/// Per-shape features stage 1 draws at rest (or not at all), counted when non-default.
/// (`3d_thickness` is not counted: Moho writes its default 0.125 on every shape, and it only
/// renders under a layer `3d_mode`, which is counted at the layer. The fill-effect
/// transform belongs to gradient fills and is applied there.)
fn shape_flags(b: &mut Builder, shape: &Value) {
    if json::integer(get(shape, "combo_mode")).is_some_and(|v| v != 0) {
        b.report.skip("shape combo modes");
    }
}
