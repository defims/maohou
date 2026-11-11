//! Point-level shape animation (stage 5, Moho's per-point channels): animated path points,
//! and the trim that shows only part of each subpath.
//!
//! AniCraft's [`Path`] is a shared, immutable command list; Moho animates the *positions of
//! individual points*. [`PointTrack`] bridges the two: it names one point of the path —
//! endpoints and control points, numbered in command order — and carries the channel that
//! drives it. Evaluation rebuilds the path each frame: a tracked endpoint lands where its
//! channel samples, and the control points that hang off it follow rigidly (a moved point
//! takes its handles along, as the drawing tools do). A shape without tracks keeps its
//! shared path untouched: rebuilds cost nothing until something animates.
//!
//! [`Trim`] is Moho's curve `start_percent`/`end_percent`: each subpath draws only the arc
//! between the two fractions. A trimmed subpath flattens to line segments (the part between
//! the fractions is what draws; an exact Bézier split adds nothing an artist can see at
//! drawing tolerance). A trimmed shape draws its stroke; the fill of a cut-open ring is not
//! a Moho behaviour and is dropped while the trim is active.

use crate::Channel;
use anicraft_geom::{Path, PathCmd, Vec2};
use serde::{Deserialize, Serialize};

/// Most animated points one shape keeps (files and commands alike).
pub const MAX_POINT_TRACKS: usize = 100_000;

/// One animated point of a shape's path: which point (see [`path_point_count`]) and the
/// channel that moves it. A Moho point shared by several curves occurs at several path
/// points — each occurrence carries its own track sampling the same channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointTrack {
    /// The path point this track drives, in command order (0-based).
    pub index: u32,
    pub position: Channel<Vec2>,
}

/// The part of each subpath that draws: from the `start` to the `end` arc-length fraction
/// (`0..=1`; `start` past `end` wraps on a closed subpath).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trim {
    pub start: Channel<f32>,
    pub end: Channel<f32>,
}

impl Trim {
    /// Whether the trim is active at `frame`: some part of the subpath is cut away.
    pub fn cuts_at(&self, frame: u32) -> bool {
        let (s, e) = self.sample(frame);
        s > 1e-4 || e < 1.0 - 1e-4
    }
    /// The sampled `(start, end)` fractions, clamped to `0..=1`.
    pub fn sample(&self, frame: u32) -> (f32, f32) {
        let f = frame as f32;
        let s = self.start.sample_or(f, 0.0).clamp(0.0, 1.0);
        let e = self.end.sample_or(f, 1.0).clamp(0.0, 1.0);
        (s, e)
    }
    /// Repairs the channels after a load (sorted, finite, bounded).
    pub fn sanitize(&mut self) {
        self.start.sanitize();
        self.end.sanitize();
    }
}

/// How many points a path numbers (endpoints and control points, command order): `MoveTo`
/// and `LineTo` carry one, `QuadTo` two (control first), `CubicTo` three (controls first),
/// `Close` none.
pub fn path_point_count(path: &Path) -> usize {
    path.cmds.iter().map(path_points_of).sum()
}

fn path_points_of(cmd: &PathCmd) -> usize {
    match cmd {
        PathCmd::MoveTo(_) | PathCmd::LineTo(_) => 1,
        PathCmd::QuadTo(..) => 2,
        PathCmd::CubicTo(..) => 3,
        PathCmd::Close => 0,
    }
}

/// Each command's points and the endpoint each one follows: the point indices in order
/// (`None` for `Close`), and one owner per point — an endpoint owns itself, a control hangs
/// off an endpoint. A cubic's incoming control belongs to the command's own endpoint; a
/// quad's control to it too (v1: a quad bowstring bends with its end).
fn command_points(cmd: &PathCmd, base: usize, last_endpoint: usize) -> (Option<std::ops::Range<usize>>, Vec<usize>, usize) {
    match cmd {
        PathCmd::MoveTo(_) | PathCmd::LineTo(_) => (Some(base..base + 1), vec![base], base),
        PathCmd::QuadTo(..) => (Some(base..base + 2), vec![base + 1, base + 1], base + 1),
        PathCmd::CubicTo(..) => (Some(base..base + 3), vec![last_endpoint, base + 2, base + 2], base + 2),
        PathCmd::Close => (None, Vec::new(), last_endpoint),
    }
}

/// The path with every animated track applied at `frame`: tracked endpoints land on their
/// sampled positions and control points follow their endpoint. `None` when no track
/// animates (the caller keeps the shared path) or a track points past the path.
pub fn rebuild(path: &Path, tracks: &[PointTrack], frame: u32) -> Option<Path> {
    if !tracks.iter().any(|t| t.position.is_animated()) {
        return None;
    }
    let n = path_point_count(path);
    // Per point: its position and the endpoint it follows (itself for endpoints).
    let mut pts: Vec<Vec2> = Vec::with_capacity(n);
    let mut owner: Vec<usize> = Vec::with_capacity(n);
    let mut base = 0usize;
    let mut last_endpoint = usize::MAX;
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => pts.push(p),
            PathCmd::QuadTo(c, p) => {
                pts.push(c);
                pts.push(p);
            }
            PathCmd::CubicTo(c1, c2, p) => {
                pts.push(c1);
                pts.push(c2);
                pts.push(p);
            }
            PathCmd::Close => {}
        }
        let prev = last_endpoint;
        let (indices, owners, end) = command_points(cmd, base, prev);
        owner.extend(owners);
        if let Some(range) = indices {
            last_endpoint = end;
            base += range.len();
        }
    }
    if base != n {
        return None;
    }
    let mut delta = vec![Vec2::ZERO; n];
    let mut tracked = vec![false; n];
    for t in tracks {
        let i = t.index as usize;
        let cur = pts.get(i)?;
        let Some(v) = t.position.sample(frame as f32) else { continue };
        if !v.is_finite() {
            continue;
        }
        delta[i] = v - *cur;
        tracked[i] = true;
    }
    // Controls inherit their endpoint's movement (a moved point takes its handles along);
    // owners are always endpoints, whose own deltas are final after the pass above.
    for i in 0..n {
        let o = owner[i];
        if o != i && o < n && !tracked[i] {
            delta[i] = delta[o];
        }
    }
    let mut out = Path::default();
    let mut i = 0usize;
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(_) => {
                out.cmds.push(PathCmd::MoveTo(pts[i] + delta[i]));
                i += 1;
            }
            PathCmd::LineTo(_) => {
                out.cmds.push(PathCmd::LineTo(pts[i] + delta[i]));
                i += 1;
            }
            PathCmd::QuadTo(..) => {
                out.cmds.push(PathCmd::QuadTo(pts[i] + delta[i], pts[i + 1] + delta[i + 1]));
                i += 2;
            }
            PathCmd::CubicTo(..) => {
                out.cmds.push(PathCmd::CubicTo(pts[i] + delta[i], pts[i + 1] + delta[i + 1], pts[i + 2] + delta[i + 2]));
                i += 3;
            }
            PathCmd::Close => out.cmds.push(PathCmd::Close),
        }
    }
    Some(out)
}

/// The path with each subpath cut to the trim's arc fraction at `frame`: `None` when the
/// trim cuts nothing (the caller keeps the shared path). A trimmed subpath comes back as
/// the vertices of its flattened curve between the two arc fractions; a closed subpath
/// wraps past its end.
pub fn trim(path: &Path, t: &Trim, frame: u32) -> Option<Path> {
    if !t.cuts_at(frame) {
        return None;
    }
    let (start, end) = t.sample(frame);
    let mut out = Path::default();
    let mut cut_any = false;
    for line in path.flatten(&anicraft_geom::Affine::IDENTITY, 0.25) {
        // A closed subpath's cut runs over the segment back to its start too.
        let mut pts = line.points;
        if line.closed
            && let Some(&first) = pts.first()
        {
            pts.push(first);
        }
        let n = pts.len();
        if n < 2 {
            continue;
        }
        // Cumulative arc length up to each vertex (the length of segment i is cum[i]−cum[i−1]).
        let mut cum = Vec::with_capacity(n - 1);
        let mut s = 0.0;
        for w in pts.windows(2) {
            s += (w[1] - w[0]).length();
            cum.push(s);
        }
        let total = s;
        if !total.is_finite() || total <= 1e-6 {
            continue;
        }
        // The point at arc length `d`: the vertex that reaches it, carried the rest of the way.
        let at = |d: f32| -> Vec2 {
            let d = d.clamp(0.0, total);
            let i = cum.partition_point(|c| *c < d).min(n - 2);
            let prev = if i == 0 { 0.0 } else { cum[i - 1] };
            let seg = (cum[i] - prev).max(1e-9);
            pts[i] + (pts[i + 1] - pts[i]) * ((d - prev) / seg)
        };
        // The drawn arc: [start, end], wrapping a closed subpath whose start passed its end.
        let (from, to) = if line.closed && start > end { (start * total, (end + 1.0) * total) } else { (start * total, end.max(start) * total) };
        let mut first = true;
        let walk_range = |a: f32, b: f32, first: &mut bool, out: &mut Path| {
            // The exact cut point, the interior vertices in order, the exact end.
            if *first {
                out.cmds.push(PathCmd::MoveTo(at(a)));
                *first = false;
            } else {
                out.cmds.push(PathCmd::LineTo(at(a)));
            }
            for i in 0..n - 1 {
                if cum[i] <= a + 1e-4 || cum[i] >= b - 1e-4 {
                    continue;
                }
                out.cmds.push(PathCmd::LineTo(pts[i]));
            }
            out.cmds.push(PathCmd::LineTo(at(b)));
        };
        if from > total {
            // A wrapped arc that starts past the subpath's end: the tail runs from the start.
            walk_range(from - total, to - total, &mut first, &mut out);
        } else if to <= total {
            walk_range(from, to, &mut first, &mut out);
        } else {
            walk_range(from, total, &mut first, &mut out);
            walk_range(0.0, to - total, &mut first, &mut out);
        }
        cut_any = true;
    }
    cut_any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(index: u32, keys: Vec<(u32, Vec2)>) -> PointTrack {
        let mut c = Channel::default();
        for (f, v) in keys {
            c.set_key(f, v);
        }
        PointTrack { index, position: c }
    }

    fn square() -> Path {
        let mut p = Path::new();
        p.move_to(Vec2::new(0.0, 0.0)).line_to(Vec2::new(10.0, 0.0)).line_to(Vec2::new(10.0, 10.0)).line_to(Vec2::new(0.0, 10.0)).close();
        p
    }

    #[test]
    fn point_numbering() {
        assert_eq!(path_point_count(&square()), 4);
        let mut c = Path::new();
        c.move_to(Vec2::ZERO).cubic_to(Vec2::new(1.0, 1.0), Vec2::new(2.0, 1.0), Vec2::new(3.0, 0.0));
        assert_eq!(path_point_count(&c), 4, "MoveTo + three cubic points");
    }

    #[test]
    fn static_tracks_change_nothing() {
        let sq = square();
        let mut t = track(1, vec![(0, Vec2::new(10.0, 0.0))]);
        t.position.keys.clear();
        assert_eq!(rebuild(&sq, &[t], 5), None, "an empty channel animates nothing");
        let t = track(1, vec![(0, Vec2::new(10.0, 0.0))]);
        assert_eq!(rebuild(&sq, &[t], 5), None, "a single key is the static state");
    }

    #[test]
    fn a_moved_endpoint_takes_its_handles_along() {
        let mut p = Path::new();
        p.move_to(Vec2::ZERO).cubic_to(Vec2::new(1.0, 0.0), Vec2::new(9.0, 0.0), Vec2::new(10.0, 0.0)).line_to(Vec2::new(10.0, 10.0));
        // Move the cubic's end (path point 3) down by 5: its incoming handle (point 2) follows.
        let mut c = Channel::default();
        c.set_key(0, Vec2::new(10.0, 0.0));
        c.set_key(10, Vec2::new(10.0, 5.0));
        let t = PointTrack { index: 3, position: c };
        let out = rebuild(&p, &[t], 5).unwrap();
        let PathCmd::CubicTo(c1, c2, end) = out.cmds[1] else { panic!("cubic kept") };
        assert_eq!(end, Vec2::new(10.0, 2.5));
        assert_eq!(c2, Vec2::new(9.0, 2.5), "the handle moved with its endpoint");
        assert_eq!(c1, Vec2::new(1.0, 0.0), "the outgoing handle stayed");
    }

    #[test]
    fn a_moved_point_moves_every_frame() {
        let sq = square();
        let t = track(2, vec![(0, Vec2::new(10.0, 10.0)), (10, Vec2::new(20.0, 10.0))]);
        let half = rebuild(&sq, std::slice::from_ref(&t), 5).unwrap();
        let PathCmd::LineTo(p) = half.cmds[2] else { panic!("line kept") };
        assert_eq!(p, Vec2::new(15.0, 10.0));
        let done = rebuild(&sq, &[t], 10).unwrap();
        let PathCmd::LineTo(p) = done.cmds[2] else { panic!("line kept") };
        assert_eq!(p, Vec2::new(20.0, 10.0));
    }

    #[test]
    fn a_track_past_the_path_refuses() {
        let sq = square();
        let t = track(99, vec![(0, Vec2::ZERO), (5, Vec2::new(1.0, 1.0))]);
        assert_eq!(rebuild(&sq, &[t], 3), None);
    }

    #[test]
    fn trim_cuts_the_middle_out() {
        let mut p = Path::new();
        p.move_to(Vec2::ZERO).line_to(Vec2::new(100.0, 0.0));
        let mut t = Trim::default();
        t.start.set_key(0, 0.25);
        t.end.set_key(0, 0.75);
        let out = trim(&p, &t, 0).unwrap();
        assert_eq!(out.cmds.len(), 2, "move at the cut, line to the other cut");
        let PathCmd::MoveTo(a) = out.cmds[0] else { panic!("starts at the cut") };
        let PathCmd::LineTo(b) = out.cmds[1] else { panic!("ends at the cut") };
        assert_eq!(a, Vec2::new(25.0, 0.0));
        assert_eq!(b, Vec2::new(75.0, 0.0));
    }

    #[test]
    fn a_full_trim_is_no_trim() {
        let sq = square();
        let t = Trim::default();
        assert_eq!(trim(&sq, &t, 0), None);
        let mut t = Trim::default();
        t.start.set_key(0, 0.0);
        t.end.set_key(0, 1.0);
        assert_eq!(trim(&sq, &t, 0), None);
    }

    #[test]
    fn a_closed_subpath_trim_wraps() {
        let sq = square();
        let mut t = Trim::default();
        t.start.set_key(0, 0.75);
        t.end.set_key(0, 0.25);
        let out = trim(&sq, &t, 0).unwrap();
        let PathCmd::MoveTo(a) = out.cmds[0] else { panic!("starts at the cut") };
        assert_eq!(a, Vec2::new(0.0, 10.0), "wraps from three quarters around (arc 30)");
        let PathCmd::LineTo(b) = *out.cmds.last().unwrap() else { panic!("lines") };
        assert_eq!(b, Vec2::new(10.0, 0.0), "a quarter past the wrap (arc 10)");
    }

    #[test]
    fn trim_samples_hold_like_channels() {
        let mut t = Trim::default();
        t.start.set_key(0, 0.0);
        t.start.set_key(10, 0.5);
        t.end.set_key(0, 1.0);
        assert_eq!(t.sample(5), (0.25, 1.0));
        assert!(t.cuts_at(5));
        assert!(!t.cuts_at(0));
    }
}
