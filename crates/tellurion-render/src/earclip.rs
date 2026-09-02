//! Small, robust 2D polygon triangulation: ear-clipping with hole bridging.
//!
//! Not a general-purpose `earcut` replacement — no spatial indexing, no
//! z-order-curve heuristics. Brute-force candidate search is the right
//! tradeoff for a single MVT tile's footprint polygons (small, bounded
//! vertex counts per tile), not the huge inputs a tuned earcut targets.
//! Every entry point here is bounded and infallible: pathological input
//! (collinear runs, no valid hole bridge) degrades to a fallback
//! triangulation rather than looping forever or panicking.

use std::cmp::Ordering;
use std::ops::Range;

/// Below this magnitude, a 2D cross product is treated as exactly zero.
/// Ear-clipping and bridging both run on raw (un-normalized) MVT tile
/// coordinates, which are exact integers promoted to `f64`, so cross
/// products of them are exact too — this only guards against a genuine
/// collinear/degenerate case, not float rounding.
const EPS: f64 = 1e-9;

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Signed area (shoelace, x2) of a closed ring. Positive means
/// counter-clockwise.
pub(crate) fn signed_area(ring: &[(f64, f64)]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let (x0, y0) = ring[i];
        let (x1, y1) = ring[(i + 1) % n];
        sum += x0 * y1 - x1 * y0;
    }
    sum
}

/// Reverses `ring` in place if its winding doesn't match `want_ccw`. Used to
/// enforce the standard exterior-CCW / hole-CW convention before bridging
/// and ear-clipping.
pub(crate) fn orient(ring: &mut [(f64, f64)], want_ccw: bool) {
    if (signed_area(ring) > 0.0) != want_ccw {
        ring.reverse();
    }
}

fn point_strictly_inside_triangle(
    p: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
    c: (f64, f64),
) -> bool {
    cross(a, b, p) > EPS && cross(b, c, p) > EPS && cross(c, a, p) > EPS
}

/// True if segment `p1-p2` properly crosses segment `p3-p4` (a transversal
/// crossing in the interior of both segments; segments that merely touch at
/// a shared endpoint, or overlap collinearly, do not count).
fn segments_properly_intersect(
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    p4: (f64, f64),
) -> bool {
    let d1 = cross(p3, p4, p1);
    let d2 = cross(p3, p4, p2);
    let d3 = cross(p1, p2, p3);
    let d4 = cross(p1, p2, p4);
    ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS))
        && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS))
}

/// Ear-clips a simple polygon boundary (`indices` names points in `points`,
/// implicitly closed) into triangles, each a triple of values from
/// `indices` (i.e. point indices, not positions within `indices`).
///
/// Bounded: each pass either removes one ear or, if none is found (a
/// numerically degenerate remainder — collinear or self-touching input),
/// stops and fan-triangulates whatever is left from its first vertex. Always
/// terminates; never panics.
pub(crate) fn ear_clip(indices: &[usize], points: &[(f64, f64)]) -> Vec<[usize; 3]> {
    let mut ring: Vec<usize> = indices.to_vec();
    let mut triangles = Vec::new();

    while ring.len() > 3 {
        let n = ring.len();
        let mut clipped = false;
        for i in 0..n {
            let prev = ring[(i + n - 1) % n];
            let curr = ring[i];
            let next = ring[(i + 1) % n];
            if cross(points[prev], points[curr], points[next]) <= EPS {
                continue; // reflex or degenerate vertex: not a valid ear tip
            }
            let is_ear = ring.iter().all(|&idx| {
                idx == prev
                    || idx == curr
                    || idx == next
                    || !point_strictly_inside_triangle(
                        points[idx],
                        points[prev],
                        points[curr],
                        points[next],
                    )
            });
            if is_ear {
                triangles.push([prev, curr, next]);
                ring.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }

    match ring.len() {
        3 => triangles.push([ring[0], ring[1], ring[2]]),
        n if n > 3 => {
            // Robust fallback: a fan is always a valid (if occasionally
            // ugly) triangulation of whatever ear-clipping couldn't resolve.
            for i in 1..n - 1 {
                triangles.push([ring[0], ring[i], ring[i + 1]]);
            }
        }
        _ => {}
    }
    triangles
}

/// Bridges hole rings into the exterior ring's index cycle, producing one
/// simple (hole-free) boundary suitable for [`ear_clip`]. `points[0..ext_len]`
/// must be the exterior ring; `hole_ranges` names each hole's span within
/// `points`.
///
/// Each hole is connected to the nearest already-bridged-or-exterior vertex
/// reachable by a segment that crosses no ring edge, duplicating both bridge
/// endpoints (the classical hole-elimination technique). A hole with no
/// crossing-free candidate (pathological input) bridges to the first
/// exterior vertex anyway rather than being dropped — the seam may look
/// wrong, but the output stays a valid, parseable mesh.
pub(crate) fn bridge_holes(
    ext_len: usize,
    hole_ranges: &[Range<usize>],
    points: &[(f64, f64)],
) -> Vec<usize> {
    let mut merged: Vec<usize> = (0..ext_len).collect();

    for (hole_i, hole) in hole_ranges.iter().enumerate() {
        if hole.is_empty() {
            continue;
        }
        let hole_rightmost = hole
            .clone()
            .max_by(|&a, &b| cmp_f64(points[a].0, points[b].0))
            .unwrap_or(hole.start);

        let mut blocking_edges: Vec<(usize, usize)> = ring_edges(&merged);
        for (other_i, other) in hole_ranges.iter().enumerate() {
            if other_i == hole_i || other.is_empty() {
                continue;
            }
            let other_ring: Vec<usize> = other.clone().collect();
            blocking_edges.extend(ring_edges(&other_ring));
        }

        let best = merged
            .iter()
            .copied()
            .filter(|&cand| cand != hole_rightmost)
            .filter(|&cand| {
                blocking_edges.iter().all(|&(a, b)| {
                    !segments_properly_intersect(
                        points[hole_rightmost],
                        points[cand],
                        points[a],
                        points[b],
                    )
                })
            })
            .min_by(|&a, &b| {
                cmp_f64(
                    dist2(points[hole_rightmost], points[a]),
                    dist2(points[hole_rightmost], points[b]),
                )
            })
            .unwrap_or(merged[0]);

        let start_offset = hole_rightmost - hole.start;
        let mut hole_loop: Vec<usize> = (0..hole.len())
            .map(|step| hole.start + (start_offset + step) % hole.len())
            .collect();
        hole_loop.push(hole_rightmost);

        let pos = merged.iter().position(|&v| v == best).unwrap_or(0);
        merged.splice(
            pos + 1..pos + 1,
            hole_loop.into_iter().chain(std::iter::once(best)),
        );
    }

    merged
}

fn ring_edges(ring: &[usize]) -> Vec<(usize, usize)> {
    if ring.len() < 2 {
        return Vec::new();
    }
    ring.windows(2)
        .map(|w| (w[0], w[1]))
        .chain(std::iter::once((ring[ring.len() - 1], ring[0])))
        .collect()
}

fn dist2(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// Relative planarity tolerance [`triangulate_face`] allows, scaled by a
/// face's own bounding-box diagonal rather than a fixed absolute number —
/// the same face shape should pass or fail consistently whether its
/// coordinates are small (a local test fixture) or huge (real-world
/// EPSG:3857 meters), and a fixed absolute epsilon would not do that.
const PLANARITY_RELATIVE_TOLERANCE: f64 = 1e-6;

fn strip_closing_duplicate_3d(ring: &[[f64; 3]]) -> Vec<[f64; 3]> {
    match (ring.first(), ring.last()) {
        (Some(&first), Some(&last)) if ring.len() > 1 && first == last => {
            ring[..ring.len() - 1].to_vec()
        }
        _ => ring.to_vec(),
    }
}

/// Best-fit normal of a (possibly non-convex) 3D ring via Newell's method:
/// robust to points that aren't exactly coplanar and, unlike a three-point
/// cross product, doesn't depend on which three points happen to be picked.
/// Magnitude is proportional to twice the ring's projected area; direction
/// follows the ring's own winding by the right-hand rule.
fn newell_normal(ring: &[[f64; 3]]) -> [f64; 3] {
    let mut normal = [0.0; 3];
    let n = ring.len();
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        normal[0] += (p[1] - q[1]) * (p[2] + q[2]);
        normal[1] += (p[2] - q[2]) * (p[0] + q[0]);
        normal[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    normal
}

/// Which two axes to project onto to flatten a face whose best-fit normal
/// is `normal`: drop whichever axis the normal points most strongly along
/// (the "dominant axis"), keeping the other two as the 2D plane ear-clip
/// runs in. Cheap and robust — no trigonometry, degenerates gracefully for
/// the common axis-aligned case (a flat roof, a vertical wall), and stays
/// well-conditioned for a tilted face too since the dropped axis is always
/// the one that would otherwise foreshorten the projection the most.
fn dominant_axis_pair(normal: [f64; 3]) -> (usize, usize) {
    let (ax, ay, az) = (normal[0].abs(), normal[1].abs(), normal[2].abs());
    if az >= ax && az >= ay {
        (0, 1) // drop Z
    } else if ax >= ay {
        (1, 2) // drop X
    } else {
        (0, 2) // drop Y
    }
}

/// Reverses `ring_2d`/`ring_3d` together, in lockstep, when `ring_2d`'s
/// winding doesn't match `want_ccw` — the 3D companion of [`orient`], needed
/// because [`triangulate_face`] must keep each projected 2D point's index
/// aligned with its original 3D position through whatever reordering
/// orientation requires.
fn orient_pair(ring_2d: &mut [(f64, f64)], ring_3d: &mut [[f64; 3]], want_ccw: bool) {
    if (signed_area(ring_2d) > 0.0) != want_ccw {
        ring_2d.reverse();
        ring_3d.reverse();
    }
}

/// Triangulates one arbitrary planar 3D face — the polyhedral-faces
/// counterpart of [`ear_clip`] for a driver whose source geometry is real
/// solid geometry (`VolumeSource`, `#41`) rather than a flat MVT footprint:
/// `rings[0]` is the exterior boundary, `rings[1..]` are holes, each ring
/// either closed (first point repeats as the last) or not — both accepted,
/// matching how a driver's raw WKB ring naturally arrives.
///
/// Projects the face onto its own best-fit plane (Newell's method for the
/// normal, then a dominant-axis drop to flatten it to 2D — see
/// [`newell_normal`]/[`dominant_axis_pair`]), runs the existing 2D
/// [`ear_clip`]/[`bridge_holes`] pipeline in that plane, then looks up each
/// resulting triangle's *original* 3D positions by the same index the 2D
/// ear-clip returned — "lifting" the 2D triangulation back to 3D without
/// ever approximating a vertex's real position.
///
/// Returns `None` — skip this face, the caller counts/logs it — for an
/// exterior ring with fewer than 3 distinct points, one whose points are
/// exactly collinear (no well-defined normal), or one whose points don't
/// lie on a common plane within [`PLANARITY_RELATIVE_TOLERANCE`] of its own
/// bounding-box diagonal. Never panics.
pub fn triangulate_face(rings: &[Vec<[f64; 3]>]) -> Option<Vec<[[f64; 3]; 3]>> {
    let exterior = strip_closing_duplicate_3d(rings.first()?);
    if exterior.len() < 3 {
        return None;
    }
    let holes: Vec<Vec<[f64; 3]>> = rings[1..]
        .iter()
        .map(|ring| strip_closing_duplicate_3d(ring))
        .filter(|ring| ring.len() >= 3)
        .collect();

    let normal = newell_normal(&exterior);
    let mag = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    if !mag.is_finite() || mag <= f64::EPSILON {
        return None; // collinear/degenerate exterior: no well-defined plane
    }
    let unit_normal = [normal[0] / mag, normal[1] / mag, normal[2] / mag];

    // Planarity: every point (exterior + holes) must lie close to the
    // plane through the exterior's own first point, within a tolerance
    // scaled to the face's own bounding-box diagonal.
    let all_points = || exterior.iter().chain(holes.iter().flatten());
    let p0 = exterior[0];
    let mut min = p0;
    let mut max = p0;
    for p in all_points() {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let diag =
        ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2)).sqrt();
    let tolerance = PLANARITY_RELATIVE_TOLERANCE * diag.max(1.0);
    for p in all_points() {
        let d = (p[0] - p0[0]) * unit_normal[0]
            + (p[1] - p0[1]) * unit_normal[1]
            + (p[2] - p0[2]) * unit_normal[2];
        if !d.is_finite() || d.abs() > tolerance {
            return None; // not planar within tolerance
        }
    }

    let (a, b) = dominant_axis_pair(unit_normal);
    let project = |p: &[f64; 3]| (p[a], p[b]);

    let mut rings_2d: Vec<Vec<(f64, f64)>> = Vec::with_capacity(1 + holes.len());
    let mut rings_3d: Vec<Vec<[f64; 3]>> = Vec::with_capacity(1 + holes.len());
    rings_2d.push(exterior.iter().map(project).collect());
    rings_3d.push(exterior);
    for hole in holes {
        rings_2d.push(hole.iter().map(project).collect());
        rings_3d.push(hole);
    }
    for (i, (ring_2d, ring_3d)) in rings_2d.iter_mut().zip(rings_3d.iter_mut()).enumerate() {
        orient_pair(ring_2d, ring_3d, i == 0);
    }

    let ext_len = rings_2d[0].len();
    let mut flat_2d = rings_2d[0].clone();
    let mut flat_3d = rings_3d[0].clone();
    let mut hole_ranges: Vec<Range<usize>> = Vec::with_capacity(rings_2d.len() - 1);
    for (ring_2d, ring_3d) in rings_2d[1..].iter().zip(rings_3d[1..].iter()) {
        let start = flat_2d.len();
        flat_2d.extend_from_slice(ring_2d);
        flat_3d.extend_from_slice(ring_3d);
        hole_ranges.push(start..flat_2d.len());
    }

    let merged = bridge_holes(ext_len, &hole_ranges, &flat_2d);
    let triangles = ear_clip(&merged, &flat_2d);

    Some(
        triangles
            .into_iter()
            .map(|tri| [flat_3d[tri[0]], flat_3d[tri[1]], flat_3d[tri[2]]])
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_area_is_positive_for_ccw_square() {
        let square = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(signed_area(&square) > 0.0);
    }

    #[test]
    fn signed_area_is_negative_for_cw_square() {
        let square = vec![(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)];
        assert!(signed_area(&square) < 0.0);
    }

    #[test]
    fn orient_reverses_only_when_needed() {
        let mut cw = vec![(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)];
        orient(&mut cw, true);
        assert!(signed_area(&cw) > 0.0);

        let mut ccw = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let before = ccw.clone();
        orient(&mut ccw, true);
        assert_eq!(ccw, before, "already-CCW ring must not be touched");
    }

    #[test]
    fn ear_clips_a_simple_square_into_two_triangles() {
        let points = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let triangles = ear_clip(&[0, 1, 2, 3], &points);
        assert_eq!(triangles.len(), 2);
    }

    #[test]
    fn ear_clips_a_convex_pentagon_into_three_triangles() {
        let points = vec![(0.0, 0.0), (4.0, 0.0), (5.0, 3.0), (2.0, 5.0), (-1.0, 3.0)];
        let triangles = ear_clip(&[0, 1, 2, 3, 4], &points);
        assert_eq!(triangles.len(), 3);
    }

    #[test]
    // A single-hole test case genuinely wants a one-element `&[Range]`, not
    // the "did you mean a range of values" case this lint guards against.
    #[allow(clippy::single_range_in_vec_init)]
    fn bridges_a_square_hole_into_a_ten_point_ring() {
        // Exterior 0..4, hole 4..8 (already CCW / CW oriented by the
        // caller's convention, which bridge_holes does not itself enforce).
        let points = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (3.0, 3.0),
            (3.0, 7.0),
            (7.0, 7.0),
            (7.0, 3.0),
        ];
        let merged = bridge_holes(4, &[4..8], &points);
        assert_eq!(merged.len(), 10, "ext(4) + hole(4) + 2 bridge duplicates");

        let triangles = ear_clip(&merged, &points);
        assert_eq!(
            triangles.len(),
            8,
            "any triangulation of a simple 10-gon has 10-2 triangles"
        );
    }

    #[test]
    fn ear_clip_never_panics_on_a_degenerate_two_point_ring() {
        let points = vec![(0.0, 0.0), (1.0, 1.0)];
        assert_eq!(ear_clip(&[0, 1], &points), Vec::<[usize; 3]>::new());
    }

    // -- triangulate_face (`#41`) --------------------------------------

    #[test]
    fn triangulate_face_handles_a_flat_z_up_square() {
        let square = vec![vec![
            [0.0, 0.0, 5.0],
            [10.0, 0.0, 5.0],
            [10.0, 10.0, 5.0],
            [0.0, 10.0, 5.0],
        ]];
        let triangles = triangulate_face(&square).expect("a flat square is planar");
        assert_eq!(triangles.len(), 2, "a quad ear-clips into two triangles");
        for tri in &triangles {
            for p in tri {
                assert_eq!(p[2], 5.0, "every lifted vertex keeps its real Z");
            }
        }
    }

    #[test]
    fn triangulate_face_handles_a_vertical_wall_in_the_xz_plane() {
        // A wall face with normal along Y: X/Z vary, Y is constant. Exercises
        // the dominant-axis branch that drops Y rather than Z.
        let wall = vec![vec![
            [0.0, 3.0, 0.0],
            [10.0, 3.0, 0.0],
            [10.0, 3.0, 4.0],
            [0.0, 3.0, 4.0],
        ]];
        let triangles = triangulate_face(&wall).expect("a flat wall is planar");
        assert_eq!(triangles.len(), 2);
        for tri in &triangles {
            for p in tri {
                assert_eq!(p[1], 3.0, "every lifted vertex keeps its constant Y");
            }
        }
    }

    #[test]
    fn triangulate_face_handles_a_tilted_planar_quad() {
        // A non-axis-aligned but exactly planar quad (a sloped roof panel):
        // z = x/2, so every point satisfies 0.5*x - z = 0.
        let roof = vec![vec![
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 2.0],
            [4.0, 4.0, 2.0],
            [0.0, 4.0, 0.0],
        ]];
        let triangles = triangulate_face(&roof).expect("a tilted planar quad is still planar");
        assert_eq!(triangles.len(), 2);
        for tri in &triangles {
            for p in tri {
                assert!(
                    (0.5 * p[0] - p[2]).abs() < 1e-9,
                    "lifted vertex {p:?} must still lie on the source plane"
                );
            }
        }
    }

    #[test]
    fn triangulate_face_handles_a_hole() {
        let outer = vec![
            [0.0, 0.0, 2.0],
            [10.0, 0.0, 2.0],
            [10.0, 10.0, 2.0],
            [0.0, 10.0, 2.0],
        ];
        let hole = vec![
            [3.0, 3.0, 2.0],
            [3.0, 7.0, 2.0],
            [7.0, 7.0, 2.0],
            [7.0, 3.0, 2.0],
        ];
        let triangles =
            triangulate_face(&[outer, hole]).expect("a square with a square hole is planar");
        assert_eq!(
            triangles.len(),
            8,
            "the same 10-gon-after-bridging count ear_clip's own hole test expects"
        );
    }

    #[test]
    fn triangulate_face_handles_a_bare_triangle_with_no_bridging_needed() {
        let tri = vec![vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]]];
        let triangles = triangulate_face(&tri).expect("a triangle is trivially planar");
        assert_eq!(triangles.len(), 1);
    }

    #[test]
    fn triangulate_face_accepts_a_closed_ring_with_a_repeated_first_point() {
        let closed = vec![vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
            [0.0, 0.0, 0.0], // repeats the first point
        ]];
        let triangles = triangulate_face(&closed).expect("a closed ring is still planar");
        assert_eq!(triangles.len(), 2);
    }

    #[test]
    fn triangulate_face_rejects_a_non_planar_quad() {
        // Three corners at Z=0, one pulled far off the plane.
        let warped = vec![vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 50.0],
            [0.0, 10.0, 0.0],
        ]];
        assert!(
            triangulate_face(&warped).is_none(),
            "a badly warped quad must be skipped, not silently distorted"
        );
    }

    #[test]
    fn triangulate_face_rejects_collinear_points() {
        let collinear = vec![vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]];
        assert!(
            triangulate_face(&collinear).is_none(),
            "collinear points have no well-defined normal"
        );
    }

    #[test]
    fn triangulate_face_rejects_a_degenerate_exterior_ring() {
        let too_few = vec![vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]];
        assert!(triangulate_face(&too_few).is_none());
    }

    #[test]
    fn triangulate_face_never_panics_on_an_empty_input() {
        assert!(triangulate_face(&[]).is_none());
    }

    #[test]
    fn triangulate_face_drops_a_degenerate_hole_but_keeps_the_face() {
        let outer = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ];
        let degenerate_hole = vec![[3.0, 3.0, 0.0], [3.0, 3.0, 0.0]]; // < 3 distinct points
        let triangles = triangulate_face(&[outer, degenerate_hole])
            .expect("a degenerate hole is dropped, not the whole face");
        assert_eq!(triangles.len(), 2, "falls back to the plain square");
    }
}
