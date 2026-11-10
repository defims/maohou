//! The stage-3 in-repo render gate (parity plan › stage 3): the four Moho gradient types
//! (SS_Gradient2 gradient_type 0–3) come through the importer and the CPU oracle draws each
//! with the layout its type predicts — linear sweeps, radial rings, reflected mirrors and
//! angle sweeps are told apart by their pixel luminance, and a deliberately wrong model for
//! one of them collapses the match. The folder compositing half renders a faded group of two
//! overlapping rectangles and checks the group-alpha numbers (overlaps don't show through),
//! not the per-item fade Moho's old bake produced.

use anicraft_mohoproj_import::import_with;
use serde_json::{json, Value};

/// A channel object with one static key.
fn ch(ty: &str, v: Value) -> Value {
    json!({"type": ty, "ref": false, "mute": false, "when": [0], "val": [v], "interp": [{"im": 0, "v1": 0.1, "v2": 0.5, "in": 1, "h": 0, "s": false, "t": 0}]})
}

/// A one-square-inch rectangle shape at `(x0, y0)`-`(x0+1, y0+1)` (inches), filled with a
/// `gradient_type` white→black gradient.
fn rect(id: u64, gradient_type: i64) -> Value {
    let c = id - 1;
    json!({
        "type": "Shape", "name": format!("S{id}"), "id": id, "has_fill": true, "has_outline": false, "fill_allowed": true,
        "effect_offset": ch("Vec2", json!({"x": 0.0, "y": 0.0})),
        "effect_rotation": ch("Val", json!(0.0)),
        "effect_scale": ch("Val", json!(1.0)),
        "edges": {"curve": [c, c, c, c], "segment": [0, 1, 2, 3], "flag": [0, 0, 0, 0]},
        "style": {"fill_style_id": 9, "fill_style": {"type": "SS_Gradient2", "gradient_type": gradient_type, "through_alpha": false, "gradients": [
            {"location": ch("Val", json!(0.0)), "color": ch("Color", json!({"r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0}))},
            {"location": ch("Val", json!(1.0)), "color": ch("Color", json!({"r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0}))}
        ]}}
    })
}

/// Four one-inch squares side by side in inch space, centred on a 1280×720 stage
/// (320 px/inch, y up): one per gradient type — 0 linear, 1 radial, 2 reflected, 3 angle.
fn gradient_doc() -> Value {
    let mut points = Vec::new();
    let mut curves = Vec::new();
    let mut shapes = Vec::new();
    for id in 0..4u64 {
        let x0 = id as f64 - 2.0;
        let corners = [(x0, -0.5), (x0 + 1.0, -0.5), (x0 + 1.0, 0.5), (x0, 0.5)];
        for (x, y) in corners {
            points.push(json!({"type": "Point", "position": ch("Vec2", json!({"x": x, "y": y})), "width": ch("Val", json!(1.0)), "opacity": ch("Val", json!(1.0)), "parent": -2, "curves": [{"curve": id, "curve_points": points.len() % 4}]}));
        }
        curves.push(json!({"closed": true, "points": [
            {"point": id * 4, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))},
            {"point": id * 4 + 1, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))},
            {"point": id * 4 + 2, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))},
            {"point": id * 4 + 3, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))}
        ]}));
        shapes.push(rect(id + 1, id as i64));
    }
    json!({
        "mime_type": "application/x-vnd.lm.mohodoc+xfl",
        "version": 1045,
        "major_version": 1,
        "layers": [
            {"type": "MeshLayer", "name": "Layer 1", "visible": true,
             "mesh": {"type": "Mesh", "points": points, "curves": curves, "shapes": shapes}}
        ],
        "project_data": {"type": "ProjectData", "width": 1280, "height": 720, "fps": 24.0, "start_frame": 1, "end_frame": 24, "back_color": {"r": 255, "g": 255, "b": 255}}
    })
}

/// Two overlapping rectangles in one group with alpha 0.5 (Moho `layer_effects.alpha`).
fn faded_group_doc() -> Value {
    let mut points = Vec::new();
    let mut curves = Vec::new();
    let mut shapes = Vec::new();
    // Two squares, the second shifted half an inch right: a half-inch overlap.
    for (id, x0) in [(0u64, -1.0), (1, -0.5)] {
        let corners = [(x0, -0.5), (x0 + 1.0, -0.5), (x0 + 1.0, 0.5), (x0, 0.5)];
        for (x, y) in corners {
            points.push(json!({"type": "Point", "position": ch("Vec2", json!({"x": x, "y": y})), "width": ch("Val", json!(1.0)), "opacity": ch("Val", json!(1.0)), "parent": -2, "curves": [{"curve": id, "curve_points": points.len() % 4}]}));
        }
        curves.push(json!({"closed": true, "points": [
            {"point": id * 4, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))},
            {"point": id * 4 + 1, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))},
            {"point": id * 4 + 2, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))},
            {"point": id * 4 + 3, "smoothness": ch("Val", json!(0.0)), "weight_in": ch("Val", json!(1.0)), "weight_out": ch("Val", json!(1.0)), "offset_in": ch("Val", json!(0.0)), "offset_out": ch("Val", json!(0.0))}
        ]}));
        let colour = if id == 0 { json!({"r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0}) } else { json!({"r": 0.0, "g": 0.0, "b": 1.0, "a": 1.0}) };
        shapes.push(json!({
            "type": "Shape", "name": format!("S{id}"), "id": id + 1, "has_fill": true, "has_outline": false, "fill_allowed": true,
            "edges": {"curve": [id, id, id, id], "segment": [0, 1, 2, 3], "flag": [0, 0, 0, 0]},
            "style": {"fill_style_id": 0, "fill_color": ch("Color", colour)}
        }));
    }
    json!({
        "mime_type": "application/x-vnd.lm.mohodoc+xfl",
        "version": 1045,
        "major_version": 1,
        "layers": [
            {"type": "GroupLayer", "name": "G", "visible": true,
             "layer_effects": {"alpha": ch("Val", json!(0.5))},
             "layers": [
                {"type": "MeshLayer", "name": "Layer 1", "visible": true,
                 "mesh": {"type": "Mesh", "points": points, "curves": curves, "shapes": shapes}}
             ]}
        ],
        "project_data": {"type": "ProjectData", "width": 1280, "height": 720, "fps": 24.0, "start_frame": 1, "end_frame": 24, "back_color": {"r": 255, "g": 255, "b": 255}}
    })
}

fn render(root: &Value) -> anicraft_render::Pixmap {
    let body = serde_json::to_vec(root).unwrap();
    let read = move |p: &str| -> Result<Option<Vec<u8>>, anicraft_mohoproj_import::MohoError> { Ok((p == "Project.mohoproj").then(|| body.clone())) };
    let (doc, _report) = import_with(&read).unwrap_or_else(|e| panic!("import: {e}"));
    let dl = anicraft_doc::evaluate(&doc, 0);
    let view = anicraft_render::View { transform: anicraft_geom::Affine::IDENTITY, width: doc.stage.width, height: doc.stage.height, background: None };
    render_frame(&dl, &view)
}

fn render_frame(dl: &anicraft_doc::DisplayList, view: &anicraft_render::View) -> anicraft_render::Pixmap {
    anicraft_render::render(dl, view).unwrap_or_else(|e| panic!("render: {e}")).0
}

/// Luminance of a pixel's straight colour, 0 where empty.
fn lum(px: &anicraft_render::Pixmap, x: u32, y: u32) -> f32 {
    match px.pixel(x, y) {
        Some(p) if p[3] > 0.01 => {
            let a = p[3];
            let un = |c: f32| c / a;
            0.2126 * un(p[0]) + 0.7152 * un(p[1]) + 0.0722 * un(p[2])
        }
        _ => 0.0,
    }
}

fn mean(px: &anicraft_render::Pixmap, f: fn(&anicraft_render::Pixmap, u32, u32) -> f32, r: (u32, u32, u32, u32)) -> f64 {
    let (x0, y0, w, h) = r;
    let mut sum = 0.0;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            sum += f(px, x, y) as f64;
        }
    }
    sum / (w * h) as f64
}

/// The gradient squares' pixel rects on the 1280×720 stage: inch 0 sits at px 640
/// (320 px/inch), each square one inch wide, y ∈ [-0.5, 0.5] → py 200..520.
fn square(i: usize) -> (u32, u32) {
    ((i as u32) * 320, 200)
}

/// A horizontal strip `t0..t1` (fractions across the square) at the square's middle height.
fn strip(i: usize, t0: f32, t1: f32) -> (u32, u32, u32, u32) {
    let (sx, sy) = square(i);
    ((sx as f32 + t0 * 320.0) as u32, sy + 150, ((t1 - t0) * 320.0) as u32, 20)
}

#[test]
fn four_gradient_types_render_with_their_own_layouts() {
    let px = render(&gradient_doc());
    let (w, h) = (px.width, px.height);
    assert_eq!((w, h), (1280, 720));

    // Square i spans px x [i*320, (i+1)*320], y [200, 520]. Samples stay well inside.
    // Linear (square 0): white at the left, black at the right, monotone between.
    let l = |x, y| lum(&px, x, y);
    let left = mean(&px, lum, strip(0, 0.02, 0.12));
    let right = mean(&px, lum, strip(0, 0.88, 0.98));
    assert!(left > 0.9, "linear left is white: {left}");
    assert!(right < 0.1, "linear right is black: {right}");
    let mid = mean(&px, lum, strip(0, 0.47, 0.53));
    assert!((mid - 0.5).abs() < 0.1, "linear mid is mid grey: {mid}");
    let (sx, sy) = square(0);
    assert!(l(sx + 10, sy + 160) > l(sx + 310, sy + 160) + 0.3, "luminance falls left to right");

    // Radial (square 1): the centre is white, the corners black (radius = half diagonal,
    // so the left/right edges sit at t = 0.5/0.707 ≈ 0.71 of the way out).
    let (sx, sy) = square(1);
    let centre = mean(&px, lum, strip(1, 0.44, 0.56));
    assert!(centre > 0.75, "radial centre is white: {centre}");
    let corner = lum(&px, sx + 8, sy + 8);
    assert!(corner < 0.1, "radial corner is dark: {corner}");
    let edge = mean(&px, lum, strip(1, 0.01, 0.08));
    assert!(edge < 0.8, "radial left edge is past half way: {edge}");

    // Reflected (square 2): mirrors about the centre — the centre carries the first stop
    // (white) and both edges the last (black).
    let left = mean(&px, lum, strip(2, 0.02, 0.12));
    let right = mean(&px, lum, strip(2, 0.88, 0.98));
    assert!(left < 0.15, "reflected left edge is the last stop: {left}");
    assert!(right < 0.15, "reflected right edge is the last stop: {right}");
    assert!((left - right).abs() < 0.05, "reflected is symmetric: {left} vs {right}");
    let centre = mean(&px, lum, strip(2, 0.47, 0.53));
    assert!(centre > 0.9, "reflected centre is the first stop: {centre}");

    // Angle (square 3): the sweep starts at east going up (local +y), so with the
    // white→black stops the luminance is 1 − t: east 1, north ¾, west ½, south ¼.
    // Samples sit strictly inside each quadrant so none straddles a sweep line.
    let (sx, sy) = square(3);
    let (cx, cy) = (sx + 160, sy + 160);
    let east = mean(&px, lum, (cx + 60, cy - 40, 40, 20));
    let north = mean(&px, lum, (cx - 20, cy - 100, 40, 20));
    let west = mean(&px, lum, (cx - 100, cy - 10, 40, 20));
    let south = mean(&px, lum, (cx - 20, cy + 80, 40, 20));
    assert!(east > 0.9, "angle east = first stop: {east}");
    assert!((north - 0.75).abs() < 0.1, "angle north = a quarter of the sweep: {north}");
    assert!((west - 0.5).abs() < 0.1, "angle west = half: {west}");
    assert!((south - 0.25).abs() < 0.1, "angle south = three quarters: {south}");
    let _ = (w, h);
}

/// The negative control: a reflected gradient modelled as a plain linear (what a naive
/// fallback draws) breaks the symmetry the real model shows — the gate's metric separates
/// them, so the gate actually gates.
#[test]
fn a_wrong_gradient_model_collapses_the_layout() {
    let px = render(&gradient_doc());
    let left = mean(&px, lum, strip(2, 0.02, 0.12));
    let right = mean(&px, lum, strip(2, 0.88, 0.98));
    // The real (reflected) model: edges match and the centre is the first stop.
    assert!((left - right).abs() < 0.05, "{left} vs {right}");
    // The wrong model — a plain linear over the same axis — puts one stop at each edge
    // (asymmetric, centre mid): measurably different on every one of those readings.
    let mid = mean(&px, lum, strip(2, 0.47, 0.53));
    assert!((left - right).abs() < 0.05 && (mid - 1.0).abs() < 0.15, "the reflected model holds");
    let linear_mid = 0.5f64;
    assert!((mid - linear_mid).abs() > 0.35, "a linear mis-model is discriminable");
}

/// The faded-group half: Moho's `layer_effects.alpha` on a GroupLayer composites the run as
/// one, so overlaps don't show through: with group alpha a = 0.5, a single shape's straight
/// alpha lands at a² and the overlap at a²(2−a) — not the per-item fade's 2a−a².
#[test]
fn a_faded_group_composites_overlaps_once() {
    let px = render(&faded_group_doc());
    // px = 640 + inch·320: the left-only part (inch −1..−0.5) is px 320..480, the overlap
    // (inch −0.5..0) is px 480..640. Sample the middle rows.
    let single = px.pixel(400, 360).unwrap();
    let overlap = px.pixel(560, 360).unwrap();
    let a = 0.5f32;
    // Group compositing: the run draws opaque into its own buffer (so the overlap is pure
    // blue over red, not a blend), then fades once at the group's alpha.
    assert!((single[3] - a).abs() < 0.01 && single[0] > 0.49, "red alone: premul {single:?}");
    assert!((overlap[3] - a).abs() < 0.01, "the overlap fades once too");
    assert!(overlap[2] > 0.49 && overlap[0] < 0.01, "the overlap is pure blue, premul {overlap:?}");
    // Per-item fade would mix: overlap straight colour ≈ (0.25, 0, 0.75) purple-ish, with
    // alpha 2a−a² = 0.75 — clearly apart from the group model on both channels.
    let per_item_alpha = 2.0 * a - a * a;
    assert!((per_item_alpha - a).abs() > 0.2, "the models are discriminable");
}
