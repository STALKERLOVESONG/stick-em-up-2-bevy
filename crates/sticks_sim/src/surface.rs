//! Geometry of level surfaces (`PolygonColliderRenderer.SetUp`): each collider path becomes
//!
//! * an inner polygon (the path) and, around it, a border ring offset outwards by 1/32 (2/32 for
//!   shadows) with `Clipper.OffsetPaths(jtMiter, limit 3)` on a 1/1024 grid;
//! * both with every edge longer than 20 split at its midpoint (repeatedly);
//! * triangulated with poly2tri (constrained Delaunay), the ring with the path as its hole, and
//!   refined: while a triangle is larger than `maximumTriangleArea` and all its corners are more
//!   than 1 from its centroid, the centroid is added as a Steiner point and it is triangulated
//!   again. The dense vertices matter: the game is lit per vertex;
//! * vertices de-duplicated on a 1/64 grid; the inner polygon's vertices are white, the ring's
//!   outer vertices white with alpha 0 (the soft edge); UV = local position × tiling + offset;
//!   triangles in the inner polygon's order, then the ring's, each with its corners reversed
//!   (poly2tri's counter-clockwise → Unity's clockwise front faces).
//!
//! TODO(UNCERTAIN): poly2tri's sweep is reproduced with `spade`'s constrained Delaunay
//! triangulation: the same triangles except where points are exactly cocircular, and in another
//! order. Clipper's offset is reproduced for simple paths only (concave corners use the miter
//! point; Clipper unions the offset outline instead).

use std::collections::HashMap;

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
use unity_data::level::Surface;

/// `PolygonColliderRenderer.borderOffset`.
pub const BORDER_OFFSET: f64 = 1.0 / 32.0;
/// `PolygonColliderRenderer.maximumTriangleArea` default.
pub const DEFAULT_MAXIMUM_TRIANGLE_AREA: f64 = 600.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceMesh {
    /// World-space positions (x, y, z) in Unity coordinates.
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Vertex colours (white; alpha 0 on the border ring's outer edge).
    pub colors: Vec<[f32; 4]>,
    /// Counter-clockwise triangles (as seen from the camera, looking along +z).
    pub indices: Vec<u32>,
}

type P = (f64, f64);

/// Split every edge longer than 20 (squared length > 400) at its midpoint, repeatedly (the
/// edge-splitting delegate of `SetUp`, including the closing edge).
fn split_long_edges(points: &mut Vec<P>) {
    let mut i = 0usize;
    while i < points.len() {
        let next = if i + 1 == points.len() { 0 } else { i + 1 };
        let (a, b) = (points[i], points[next]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        if dx * dx + dy * dy > 400.0 {
            let mid = (a.0 + dx * 0.5, a.1 + dy * 0.5);
            points.insert(i + 1, mid);
            continue;
        }
        i += 1;
    }
}

fn signed_area(points: &[P]) -> f64 {
    let n = points.len();
    (0..n).map(|i| points[i].0 * points[(i + 1) % n].1 - points[(i + 1) % n].0 * points[i].1).sum::<f64>() * 0.5
}

/// `Clipper.OffsetPaths(path, delta, jtMiter, etClosed, 3)` (`PolyOffsetBuilder`) for a simple
/// path, on the 1/1024 integer grid: `IntPoint(v * 1024f)` truncates, duplicates are stripped,
/// the path is turned counter-clockwise (`Orientation`), so a positive delta always grows it.
fn offset_outwards(path: &[P], delta: f64) -> Vec<P> {
    let scale = 1024.0;
    let mut pts: Vec<P> = Vec::with_capacity(path.len());
    for p in path {
        let q = (((p.0 as f32) * 1024.0) as i64 as f64, ((p.1 as f32) * 1024.0) as i64 as f64);
        if pts.last() != Some(&q) {
            pts.push(q);
        }
    }
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return path.to_vec();
    }
    if signed_area(&pts) < 0.0 {
        pts.reverse();
    }
    let d = delta * scale;
    // `GetUnitNormal` of edge i → i+1 (outwards for a counter-clockwise path).
    let normal = |i: usize| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        if dx == 0.0 && dy == 0.0 {
            return (0.0, 0.0);
        }
        let f = 1.0 / (dx * dx + dy * dy).sqrt();
        (dy * f, -dx * f)
    };
    let miter_limit = 2.0 / (3.0 * 3.0);
    let mut out = Vec::with_capacity(n * 2);
    let mut k = n - 1;
    for j in 0..n {
        let (nk, nj) = (normal(k), normal(j));
        let p = pts[j];
        // `OffsetPoint`: nearly collinear corners add nothing (and keep `m_k`).
        let sin_a = (nk.0 * nj.1 - nj.0 * nk.1).clamp(-1.0, 1.0);
        if sin_a.abs() < 5e-5 {
            continue;
        }
        let r = 1.0 + (nj.0 * nk.0 + nj.1 * nk.1);
        if sin_a * d < 0.0 || r >= miter_limit {
            // `DoMiter`. Concave corners add (p + nk·d, p, p + nj·d) instead, a loop that the
            // closing `ctUnion` (pftPositive) cuts off at the offset edges' intersection, which is
            // this miter point (up to grid rounding).
            let q = d / r;
            out.push(((p.0 + (nk.0 + nj.0) * q).round(), (p.1 + (nk.1 + nj.1) * q).round()));
        } else {
            // `DoSquare`.
            let t = (sin_a.atan2(nk.0 * nj.0 + nk.1 * nj.1) / 4.0).tan();
            out.push(((p.0 + d * (nk.0 - nk.1 * t)).round(), (p.1 + d * (nk.1 + nk.0 * t)).round()));
            out.push(((p.0 + d * (nj.0 + nj.1 * t)).round(), (p.1 + d * (nj.1 - nj.0 * t)).round()));
        }
        k = j;
    }
    // `new Vector2((float)v.X / 1024f, ...)`.
    out.into_iter().map(|p| (f64::from(p.0 as f32 / 1024.0), f64::from(p.1 as f32 / 1024.0))).collect()
}

fn inside(p: P, poly: &[P]) -> bool {
    let n = poly.len();
    let mut c = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
        j = i;
    }
    c
}

/// Constrained Delaunay triangulation of `outer` minus `hole` (with Steiner points), as
/// counter-clockwise triangles of coordinates.
fn triangulate(outer: &[P], hole: Option<&[P]>, steiner: &[P]) -> Vec<[P; 3]> {
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> = ConstrainedDelaunayTriangulation::new();
    let add_loop = |cdt: &mut ConstrainedDelaunayTriangulation<Point2<f64>>, pts: &[P]| {
        let handles: Vec<_> = pts.iter().filter_map(|p| cdt.insert(Point2::new(p.0, p.1)).ok()).collect();
        for i in 0..handles.len() {
            let (a, b) = (handles[i], handles[(i + 1) % handles.len()]);
            if a != b && !cdt.intersects_constraint(cdt.vertex(a).position(), cdt.vertex(b).position()) {
                cdt.add_constraint(a, b);
            }
        }
    };
    add_loop(&mut cdt, outer);
    if let Some(h) = hole {
        add_loop(&mut cdt, h);
    }
    for p in steiner {
        let _ = cdt.insert(Point2::new(p.0, p.1));
    }
    let mut tris = Vec::new();
    for face in cdt.inner_faces() {
        let [a, b, c] = face.positions();
        let centroid = ((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
        if !inside(centroid, outer) || hole.is_some_and(|h| inside(centroid, h)) {
            continue;
        }
        tris.push([(a.x, a.y), (b.x, b.y), (c.x, c.y)]);
    }
    tris
}

/// The refinement loop of `SetUp` (`maximumTriangleArea`).
fn refined(outer: &[P], hole: Option<&[P]>, maximum_area: f64) -> Vec<[P; 3]> {
    let mut steiner: Vec<P> = Vec::new();
    for _ in 0..64 {
        let tris = triangulate(outer, hole, &steiner);
        if maximum_area == 0.0 {
            return tris;
        }
        let mut done = true;
        for t in &tris {
            let area = ((t[1].0 - t[0].0) * (t[2].1 - t[0].1) - (t[2].0 - t[0].0) * (t[1].1 - t[0].1)).abs() * 0.5;
            if area > maximum_area {
                let c = ((t[0].0 + t[1].0 + t[2].0) / 3.0, (t[0].1 + t[1].1 + t[2].1) / 3.0);
                if t.iter().all(|p| (p.0 - c.0).powi(2) + (p.1 - c.1).powi(2) > 1.0) {
                    steiner.push(c);
                    done = false;
                }
            }
        }
        if done {
            return tris;
        }
    }
    triangulate(outer, hole, &steiner)
}

/// Build the mesh for one polygon path (`is_shadow` widens the border as `isShadow` does).
pub fn surface_mesh(surface: &Surface) -> SurfaceMesh {
    surface_mesh_with(surface, surface.is_shadow)
}

pub fn surface_mesh_with(surface: &Surface, is_shadow: bool) -> SurfaceMesh {
    let mut inner: Vec<P> = surface.local_path.iter().map(|p| (f64::from(p[0]), f64::from(p[1]))).collect();
    let mut mesh = SurfaceMesh::default();
    if inner.len() < 3 {
        return mesh;
    }
    let border = if is_shadow { 2.0 } else { 1.0 } * BORDER_OFFSET;
    let mut ring = offset_outwards(&inner, border);
    split_long_edges(&mut ring);
    split_long_edges(&mut inner);
    let area = surface.maximum_triangle_area;
    let inner_tris = refined(&inner, None, area);
    let ring_tris = refined(&ring, Some(&inner), area);

    // Points of the inner triangulation are opaque.
    let key = |p: P| (((p.0 as f32) * 64.0) as i32, ((p.1 as f32) * 64.0) as i32);
    let opaque: std::collections::HashSet<(i64, i64)> =
        inner_tris.iter().flat_map(|t| t.iter().map(|p| ((p.0 * 1e6).round() as i64, (p.1 * 1e6).round() as i64))).collect();
    let mut index_of: HashMap<(i32, i32), u32> = HashMap::new();
    for t in inner_tris.iter().chain(&ring_tris) {
        // Counter-clockwise; the caller reverses it into Unity's clockwise order
        // (`item2.Points.Reverse()`).
        for &p in t {
            let k = key(p);
            let index = *index_of.entry(k).or_insert_with(|| {
                let (x, y) = (p.0 as f32, p.1 as f32);
                let world_x = surface.origin[0] + surface.axis_x[0] * x + surface.axis_y[0] * y;
                let world_y = surface.origin[1] + surface.axis_x[1] * x + surface.axis_y[1] * y;
                mesh.positions.push([world_x, world_y, surface.origin[2]]);
                mesh.uvs.push([x * surface.tiling[0] + surface.offset[0], y * surface.tiling[1] + surface.offset[1]]);
                let solid = opaque.contains(&((p.0 * 1e6).round() as i64, (p.1 * 1e6).round() as i64));
                mesh.colors.push([1.0, 1.0, 1.0, if solid { 1.0 } else { 0.0 }]);
                (mesh.positions.len() - 1) as u32
            });
            mesh.indices.push(index);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f32, area: f64) -> Surface {
        Surface {
            material: "m".into(),
            tiling: [0.5, 0.25],
            offset: [0.1, 0.0],
            is_shadow: false,
            local_path: vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]],
            origin: [10.0, 20.0, 0.5],
            axis_x: [1.0, 0.0],
            axis_y: [0.0, 1.0],
            shadow_plane_z: None,
            maximum_triangle_area: area,
            parallax: None,
        }
    }

    fn ccw(m: &SurfaceMesh) -> bool {
        m.indices.chunks(3).all(|t| {
            let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0
        })
    }

    #[test]
    fn small_square_has_a_soft_border_ring() {
        let m = surface_mesh(&square(4.0, 600.0));
        // The square (2 triangles) and the ring around it (8 triangles).
        assert_eq!(m.indices.len() / 3, 10);
        assert!(ccw(&m));
        let (min_x, max_x) = m.positions.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
        assert!((min_x - (10.0 - 1.0 / 32.0)).abs() < 2e-3 && (max_x - (14.0 + 1.0 / 32.0)).abs() < 2e-3, "{min_x}..{max_x}");
        assert!(m.positions.iter().all(|p| p[2] == 0.5));
        // Inner corners opaque, outer ring transparent.
        let solid = m.colors.iter().filter(|c| c[3] == 1.0).count();
        assert_eq!((solid, m.colors.len() - solid), (4, 4));
        let inner_uv = m.positions.iter().zip(&m.uvs).find(|(p, _)| p[0] == 14.0 && p[1] == 20.0).map(|(_, uv)| *uv).unwrap();
        assert!((inner_uv[0] - 2.1).abs() < 1e-6, "UV = local x tiling + offset");
    }

    #[test]
    fn long_edges_are_split_and_large_triangles_refined() {
        let m = surface_mesh(&square(100.0, 600.0));
        // Edges of 100 are split into 8 pieces of 12.5.
        let corners_on_bottom = m.positions.iter().filter(|p| p[1] == 20.0 && m.colors[0][3] >= 0.0).count();
        assert!(corners_on_bottom >= 9, "{corners_on_bottom}");
        let max_area = m.indices.chunks(3).map(|t| {
            let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
        });
        assert!(max_area.fold(0.0f32, f32::max) <= 600.0);
        assert!(ccw(&m));
        // Without the area limit the interior is only split along the edges.
        let coarse = surface_mesh(&square(100.0, 0.0));
        assert!(coarse.positions.len() < m.positions.len());
    }

    #[test]
    fn split_long_edges_halves_until_short() {
        let mut p = vec![(0.0, 0.0), (50.0, 0.0), (50.0, 10.0)];
        split_long_edges(&mut p);
        assert!(p.windows(2).all(|w| (w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2) <= 400.0));
        assert_eq!(p[1], (12.5, 0.0));
    }
}
