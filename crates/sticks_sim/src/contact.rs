//! Contacts for dynamic rigid bodies (helicopters), ported from Box2D 2.3, the engine under
//! Unity 5.1's Physics2D: `b2CollidePolygons`, `b2CollidePolygonAndCircle`, `b2ContactSolver`
//! (sequential impulses with warm starting, friction, restitution, Baumgarte position
//! correction) and `b2PolygonShape::ComputeMass`.
//!
//! One [`step`] is Box2D's `b2World::Step` for these bodies:
//!
//! ```text
//! Collide (manifolds at the current transforms)
//! b2Island::Solve: integrate velocities → init velocity constraints → warm start →
//!   velocity iterations → store impulses → integrate positions → position iterations
//! ```
//!
//! Bodies collide with the static level (solid pieces on layers the matrix lets them touch) and
//! with each other.
//!
//! TODO(UNCERTAIN), recorded in the implementation map:
//! * edge colliders are treated as two-sided two-vertex polygons (Box2D's
//!   `b2CollideEdgeAndPolygon` with adjacency is not ported);
//! * two-point manifolds are solved point by point (Box2D's block solver is not ported);
//! * every collider uses Unity's default material (friction 0.4, bounciness 0); no
//!   `PhysicsMaterial2D` is read;
//! * no continuous collision (time of impact) and no sleeping;
//! * a `CircleCollider2D` (car wheels) is a rounded octagon ([`Polygon::disc`]): Box2D's
//!   polygon-and-circle pairs are approximated by polygon pairs with a large skin radius.
//!
//! [`WheelJoint`] is `b2WheelJoint` (Unity's `WheelJoint2D`) without its motor: a point-to-line
//! constraint across the suspension axis and a soft spring along it.

use std::collections::HashMap;

use unity_compat::Vec2;

use crate::physics::{PieceShape, PhysicsRules, World};
use crate::rigid_body::RigidBody;

/// `b2_linearSlop`.
pub const LINEAR_SLOP: f32 = 0.005;
/// `b2_polygonRadius`: the skin every polygon and edge carries.
pub const POLYGON_RADIUS: f32 = 2.0 * LINEAR_SLOP;
/// Unity's default `PhysicsMaterial2D` (no `m_DefaultMaterial` in `Physics2DSettings`).
pub const DEFAULT_FRICTION: f32 = 0.4;
pub const DEFAULT_RESTITUTION: f32 = 0.0;
/// `b2_maxManifoldPoints`.
const MAX_POINTS: usize = 2;
/// `b2PositionSolver`'s early-out (`minSeparation >= -3 * b2_linearSlop`).
const POSITION_TOLERANCE: f32 = -3.0 * LINEAR_SLOP;

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

/// `b2Cross(w, r)`.
fn cross_sv(w: f32, r: Vec2) -> Vec2 {
    Vec2::new(-w * r.y, w * r.x)
}

/// `b2Cross(v, 1)`.
fn perp(v: Vec2) -> Vec2 {
    Vec2::new(v.y, -v.x)
}

fn normalized(v: Vec2) -> Vec2 {
    let l = v.magnitude();
    if l < f32::EPSILON { v } else { v * (1.0 / l) }
}

/// `b2Transform`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub p: Vec2,
    pub c: f32,
    pub s: f32,
}

impl Xf {
    pub const IDENTITY: Xf = Xf { p: Vec2 { x: 0.0, y: 0.0 }, c: 1.0, s: 0.0 };

    pub fn new(p: Vec2, angle: f32) -> Xf {
        let (s, c) = angle.sin_cos();
        Xf { p, c, s }
    }

    fn rot(&self, v: Vec2) -> Vec2 {
        Vec2::new(self.c * v.x - self.s * v.y, self.s * v.x + self.c * v.y)
    }

    fn rot_t(&self, v: Vec2) -> Vec2 {
        Vec2::new(self.c * v.x + self.s * v.y, -self.s * v.x + self.c * v.y)
    }

    pub fn apply(&self, v: Vec2) -> Vec2 {
        self.rot(v) + self.p
    }

    fn apply_t(&self, v: Vec2) -> Vec2 {
        self.rot_t(v - self.p)
    }

    /// `b2MulT(A, B)`: B expressed in A's frame.
    fn mul_t(&self, b: &Xf) -> Xf {
        Xf { p: self.rot_t(b.p - self.p), c: self.c * b.c + self.s * b.s, s: self.c * b.s - self.s * b.c }
    }
}

/// A convex polygon with its edge normals (`b2PolygonShape`).
#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub vertices: Vec<Vec2>,
    pub normals: Vec<Vec2>,
    pub radius: f32,
}

impl Polygon {
    /// From counter-clockwise points (two points make a two-sided edge).
    pub fn new(vertices: Vec<Vec2>) -> Polygon {
        let n = vertices.len();
        let normals = (0..n).map(|i| normalized(perp(vertices[(i + 1) % n] - vertices[i]))).collect();
        Polygon { vertices, normals, radius: POLYGON_RADIUS }
    }

    /// A circle (`CircleCollider2D`) of `radius` about `center`: an octagon of a third of the
    /// radius with the rest as skin, so contacts are rounded.
    pub fn disc(center: Vec2, radius: f32) -> Polygon {
        let core = radius / 3.0;
        let vertices = (0..8)
            .map(|i| {
                let a = i as f32 * std::f32::consts::PI / 4.0;
                center + Vec2::new(a.cos() * core, a.sin() * core)
            })
            .collect();
        let mut p = Polygon::new(vertices);
        // The octagon's inradius is `core * cos(22.5°)`; the skin brings its flats out to the
        // circle (the corners stand out by a few percent of the radius).
        p.radius = radius - core * (std::f32::consts::PI / 8.0).cos();
        p
    }

    /// Counter-clockwise copy of any convex point list.
    pub fn ccw(mut points: Vec<Vec2>) -> Polygon {
        let n = points.len();
        let area: f32 = (0..n).map(|i| cross(points[i], points[(i + 1) % n])).sum();
        if area < 0.0 {
            points.reverse();
        }
        Polygon::new(points)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifoldKind {
    Circles,
    FaceA,
    FaceB,
}

/// `b2ManifoldPoint` (`id` packs `b2ContactFeature`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManifoldPoint {
    pub local_point: Vec2,
    pub id: u32,
}

/// `b2Manifold`.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifold {
    pub kind: ManifoldKind,
    pub local_normal: Vec2,
    pub local_point: Vec2,
    pub points: Vec<ManifoldPoint>,
}

const FEATURE_VERTEX: u32 = 0;
const FEATURE_FACE: u32 = 1;

fn feature(index_a: usize, index_b: usize, type_a: u32, type_b: u32) -> u32 {
    (index_a as u32 & 0xff) | (index_b as u32 & 0xff) << 8 | type_a << 16 | type_b << 24
}

fn flip_feature(id: u32) -> u32 {
    let (ia, ib, ta, tb) = (id & 0xff, id >> 8 & 0xff, id >> 16 & 0xff, id >> 24);
    ib | ia << 8 | tb << 16 | ta << 24
}

/// `b2FindMaxSeparation`: the edge of `poly1` with the largest separation from `poly2`.
fn find_max_separation(poly1: &Polygon, xf1: &Xf, poly2: &Polygon, xf2: &Xf) -> (usize, f32) {
    let xf = xf2.mul_t(xf1);
    let mut best = 0;
    let mut max = f32::MIN;
    for i in 0..poly1.vertices.len() {
        let n = xf.rot(poly1.normals[i]);
        let v1 = xf.apply(poly1.vertices[i]);
        let si = poly2.vertices.iter().map(|&v| n.dot(v - v1)).fold(f32::MAX, f32::min);
        if si > max {
            max = si;
            best = i;
        }
    }
    (best, max)
}

#[derive(Clone, Copy, Debug)]
struct ClipVertex {
    v: Vec2,
    id: u32,
}

/// `b2FindIncidentEdge`.
fn find_incident_edge(poly1: &Polygon, xf1: &Xf, edge1: usize, poly2: &Polygon, xf2: &Xf) -> [ClipVertex; 2] {
    let normal1 = xf2.rot_t(xf1.rot(poly1.normals[edge1]));
    let mut index = 0;
    let mut min = f32::MAX;
    for (i, n) in poly2.normals.iter().enumerate() {
        let d = normal1.dot(*n);
        if d < min {
            min = d;
            index = i;
        }
    }
    let i1 = index;
    let i2 = (i1 + 1) % poly2.vertices.len();
    [
        ClipVertex { v: xf2.apply(poly2.vertices[i1]), id: feature(edge1, i1, FEATURE_FACE, FEATURE_VERTEX) },
        ClipVertex { v: xf2.apply(poly2.vertices[i2]), id: feature(edge1, i2, FEATURE_FACE, FEATURE_VERTEX) },
    ]
}

/// `b2ClipSegmentToLine`.
fn clip_segment_to_line(input: &[ClipVertex; 2], normal: Vec2, offset: f32, vertex_index_a: usize) -> Vec<ClipVertex> {
    let mut out = Vec::with_capacity(2);
    let d0 = normal.dot(input[0].v) - offset;
    let d1 = normal.dot(input[1].v) - offset;
    if d0 <= 0.0 {
        out.push(input[0]);
    }
    if d1 <= 0.0 {
        out.push(input[1]);
    }
    if d0 * d1 < 0.0 {
        let interp = d0 / (d0 - d1);
        let v = input[0].v + (input[1].v - input[0].v) * interp;
        let index_b = (input[0].id >> 8 & 0xff) as usize;
        out.push(ClipVertex { v, id: feature(vertex_index_a, index_b, FEATURE_VERTEX, FEATURE_FACE) });
    }
    out
}

/// `b2CollidePolygons`.
pub fn collide_polygons(poly_a: &Polygon, xf_a: &Xf, poly_b: &Polygon, xf_b: &Xf) -> Option<Manifold> {
    let total_radius = poly_a.radius + poly_b.radius;
    let (edge_a, separation_a) = find_max_separation(poly_a, xf_a, poly_b, xf_b);
    if separation_a > total_radius {
        return None;
    }
    let (edge_b, separation_b) = find_max_separation(poly_b, xf_b, poly_a, xf_a);
    if separation_b > total_radius {
        return None;
    }
    let tolerance = 0.1 * LINEAR_SLOP;
    let (poly1, poly2, xf1, xf2, edge1, kind, flip) = if separation_b > separation_a + tolerance {
        (poly_b, poly_a, xf_b, xf_a, edge_b, ManifoldKind::FaceB, true)
    } else {
        (poly_a, poly_b, xf_a, xf_b, edge_a, ManifoldKind::FaceA, false)
    };
    let incident = find_incident_edge(poly1, xf1, edge1, poly2, xf2);
    let count1 = poly1.vertices.len();
    let iv1 = edge1;
    let iv2 = (edge1 + 1) % count1;
    let mut v11 = poly1.vertices[iv1];
    let mut v12 = poly1.vertices[iv2];
    let local_tangent = normalized(v12 - v11);
    let local_normal = perp(local_tangent);
    let plane_point = (v11 + v12) * 0.5;
    let tangent = xf1.rot(local_tangent);
    let normal = perp(tangent);
    v11 = xf1.apply(v11);
    v12 = xf1.apply(v12);
    let front_offset = normal.dot(v11);
    let side_offset1 = -tangent.dot(v11) + total_radius;
    let side_offset2 = tangent.dot(v12) + total_radius;
    let clip1 = clip_segment_to_line(&incident, -tangent, side_offset1, iv1);
    if clip1.len() < 2 {
        return None;
    }
    let clip2 = clip_segment_to_line(&[clip1[0], clip1[1]], tangent, side_offset2, iv2);
    if clip2.len() < 2 {
        return None;
    }
    let mut points = Vec::with_capacity(MAX_POINTS);
    for cv in clip2.iter().take(MAX_POINTS) {
        let separation = normal.dot(cv.v) - front_offset;
        if separation <= total_radius {
            points.push(ManifoldPoint { local_point: xf2.apply_t(cv.v), id: if flip { flip_feature(cv.id) } else { cv.id } });
        }
    }
    (!points.is_empty()).then_some(Manifold { kind, local_normal, local_point: plane_point, points })
}

/// `b2CollidePolygonAndCircle` (the circle's centre `center_b` in body B's frame).
pub fn collide_polygon_circle(poly_a: &Polygon, xf_a: &Xf, center_b: Vec2, radius_b: f32, xf_b: &Xf) -> Option<Manifold> {
    let c = xf_b.apply(center_b);
    let c_local = xf_a.apply_t(c);
    let radius = poly_a.radius + radius_b;
    let n = poly_a.vertices.len();
    let mut normal_index = 0;
    let mut separation = f32::MIN;
    for i in 0..n {
        let s = poly_a.normals[i].dot(c_local - poly_a.vertices[i]);
        if s > radius {
            return None;
        }
        if s > separation {
            separation = s;
            normal_index = i;
        }
    }
    let v1 = poly_a.vertices[normal_index];
    let v2 = poly_a.vertices[(normal_index + 1) % n];
    let point = vec![ManifoldPoint { local_point: center_b, id: 0 }];
    if separation < f32::EPSILON {
        return Some(Manifold { kind: ManifoldKind::FaceA, local_normal: poly_a.normals[normal_index], local_point: (v1 + v2) * 0.5, points: point });
    }
    let u1 = (c_local - v1).dot(v2 - v1);
    let u2 = (c_local - v2).dot(v1 - v2);
    if u1 <= 0.0 {
        if (c_local - v1).sqr_magnitude() > radius * radius {
            return None;
        }
        Some(Manifold { kind: ManifoldKind::FaceA, local_normal: normalized(c_local - v1), local_point: v1, points: point })
    } else if u2 <= 0.0 {
        if (c_local - v2).sqr_magnitude() > radius * radius {
            return None;
        }
        Some(Manifold { kind: ManifoldKind::FaceA, local_normal: normalized(c_local - v2), local_point: v2, points: point })
    } else {
        let face_center = (v1 + v2) * 0.5;
        if (c_local - face_center).dot(poly_a.normals[normal_index]) > radius {
            return None;
        }
        Some(Manifold { kind: ManifoldKind::FaceA, local_normal: poly_a.normals[normal_index], local_point: face_center, points: point })
    }
}

/// `b2WorldManifold::Initialize`: world normal (A → B), points and separations.
fn world_manifold(m: &Manifold, xf_a: &Xf, radius_a: f32, xf_b: &Xf, radius_b: f32) -> (Vec2, Vec<(Vec2, f32)>) {
    match m.kind {
        ManifoldKind::Circles => {
            let point_a = xf_a.apply(m.local_point);
            let point_b = xf_b.apply(m.points[0].local_point);
            let normal = if (point_a - point_b).sqr_magnitude() > f32::EPSILON * f32::EPSILON { normalized(point_b - point_a) } else { Vec2::new(1.0, 0.0) };
            let ca = point_a + normal * radius_a;
            let cb = point_b - normal * radius_b;
            (normal, vec![((ca + cb) * 0.5, (cb - ca).dot(normal))])
        }
        ManifoldKind::FaceA => {
            let normal = xf_a.rot(m.local_normal);
            let plane_point = xf_a.apply(m.local_point);
            let pts = m
                .points
                .iter()
                .map(|p| {
                    let clip = xf_b.apply(p.local_point);
                    let ca = clip + normal * (radius_a - (clip - plane_point).dot(normal));
                    let cb = clip - normal * radius_b;
                    ((ca + cb) * 0.5, (cb - ca).dot(normal))
                })
                .collect();
            (normal, pts)
        }
        ManifoldKind::FaceB => {
            let normal = xf_b.rot(m.local_normal);
            let plane_point = xf_b.apply(m.local_point);
            let pts = m
                .points
                .iter()
                .map(|p| {
                    let clip = xf_a.apply(p.local_point);
                    let cb = clip + normal * (radius_b - (clip - plane_point).dot(normal));
                    let ca = clip - normal * radius_a;
                    ((ca + cb) * 0.5, (ca - cb).dot(normal))
                })
                .collect();
            (-normal, pts)
        }
    }
}

/// `b2PositionSolverManifold::Initialize` for point `i`: normal (A → B), point, separation.
fn position_manifold(m: &Manifold, i: usize, xf_a: &Xf, xf_b: &Xf, radius_a: f32, radius_b: f32) -> (Vec2, Vec2, f32) {
    match m.kind {
        ManifoldKind::Circles => {
            let point_a = xf_a.apply(m.local_point);
            let point_b = xf_b.apply(m.points[0].local_point);
            let normal = normalized(point_b - point_a);
            (normal, (point_a + point_b) * 0.5, (point_b - point_a).dot(normal) - radius_a - radius_b)
        }
        ManifoldKind::FaceA => {
            let normal = xf_a.rot(m.local_normal);
            let plane_point = xf_a.apply(m.local_point);
            let clip = xf_b.apply(m.points[i].local_point);
            (normal, clip, (clip - plane_point).dot(normal) - radius_a - radius_b)
        }
        ManifoldKind::FaceB => {
            let normal = xf_b.rot(m.local_normal);
            let plane_point = xf_b.apply(m.local_point);
            let clip = xf_a.apply(m.points[i].local_point);
            (-normal, clip, (clip - plane_point).dot(normal) - radius_a - radius_b)
        }
    }
}

/// `b2PolygonShape::ComputeMass` summed over a body's pieces and scaled so the total is `mass`
/// (Unity sets `Rigidbody2D.mass` explicitly; the density follows). Returns the centre of mass
/// in body coordinates and the rotational inertia about it.
pub fn mass_properties(pieces: &[Polygon], mass: f32) -> (Vec2, f32) {
    let mut total_area = 0.0;
    let mut first_moment = Vec2::ZERO;
    let mut inertia_origin = 0.0;
    const INV3: f32 = 1.0 / 3.0;
    for poly in pieces {
        let v = &poly.vertices;
        let n = v.len();
        if n < 3 {
            continue;
        }
        let mut s = Vec2::ZERO;
        for p in v {
            s += *p;
        }
        s = s * (1.0 / n as f32);
        let mut area = 0.0;
        let mut center = Vec2::ZERO;
        let mut inertia = 0.0;
        for i in 0..n {
            let e1 = v[i] - s;
            let e2 = v[(i + 1) % n] - s;
            let d = cross(e1, e2);
            let tri = 0.5 * d;
            area += tri;
            center += (e1 + e2) * (tri * INV3);
            let intx2 = e1.x * e1.x + e2.x * e1.x + e2.x * e2.x;
            let inty2 = e1.y * e1.y + e2.y * e1.y + e2.y * e2.y;
            inertia += (0.25 * INV3 * d) * (intx2 + inty2);
        }
        if area <= 0.0 {
            continue;
        }
        center = center * (1.0 / area);
        let c = center + s;
        // Density 1 here; about the body origin.
        inertia_origin += inertia + area * (c.dot(c) - center.dot(center));
        first_moment += c * area;
        total_area += area;
    }
    if total_area <= 0.0 || mass <= 0.0 {
        return (Vec2::ZERO, 0.0);
    }
    let density = mass / total_area;
    let local_center = first_moment * (1.0 / total_area);
    let inertia_center = density * inertia_origin - mass * local_center.dot(local_center);
    (local_center, inertia_center.max(0.0))
}

/// A dynamic body taking part in [`step`].
#[derive(Clone, Debug)]
pub struct DynamicBody {
    /// Stable identity (warm-starting key).
    pub key: u64,
    pub body: RigidBody,
    /// Convex pieces in body coordinates.
    pub pieces: Vec<Polygon>,
    pub layer: u8,
}

/// Accumulated impulses kept between steps for warm starting (`b2ManifoldPoint` impulses of
/// persisting contacts), keyed by the bodies/pieces and the contact feature.
#[derive(Clone, Debug, Default)]
pub struct WarmStart {
    impulses: HashMap<(u64, u64, u32), (f32, f32)>,
}

/// What a body touched during a step (`OnCollisionEnter2D`/`Stay2D` material for callers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Touch {
    Static { body: usize, collider: u32 },
    Bodies { a: usize, b: usize },
}

enum Other {
    Static,
    Body(usize),
}

struct VelocityPoint {
    r_a: Vec2,
    r_b: Vec2,
    normal_impulse: f32,
    tangent_impulse: f32,
    normal_mass: f32,
    tangent_mass: f32,
    velocity_bias: f32,
    key: (u64, u64, u32),
}

struct ContactEntry {
    a: usize,
    b: Other,
    manifold: Manifold,
    radius_a: f32,
    radius_b: f32,
    friction: f32,
    restitution: f32,
    normal: Vec2,
    points: Vec<VelocityPoint>,
}

fn aabb_of(poly: &Polygon, xf: &Xf, margin: f32) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f32::MAX, f32::MAX);
    let mut max = Vec2::new(f32::MIN, f32::MIN);
    for v in &poly.vertices {
        let p = xf.apply(*v);
        min = Vec2::new(min.x.min(p.x), min.y.min(p.y));
        max = Vec2::new(max.x.max(p.x), max.y.max(p.y));
    }
    let m = Vec2::new(poly.radius + margin, poly.radius + margin);
    (min - m, max + m)
}

fn overlaps(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> bool {
    a.0.x <= b.1.x && a.1.x >= b.0.x && a.0.y <= b.1.y && a.1.y >= b.0.y
}

/// Box2D's transform of a body from its centre of mass and angle.
fn xf_of(body: &RigidBody) -> Xf {
    Xf::new(body.position, body.angle)
}

/// `WheelJoint2D` / `b2WheelJoint` between two of the stepped bodies (by index): body B (the
/// wheel) keeps its anchor on a line through A's anchor along the suspension axis, held there
/// by a spring. The motor is not modelled (`CarControl` sets the wheels' angular velocity).
#[derive(Clone, Debug, PartialEq)]
pub struct WheelJoint {
    pub a: usize,
    pub b: usize,
    /// Anchors in each body's coordinates (`anchor`, `connectedAnchor`).
    pub local_anchor_a: Vec2,
    pub local_anchor_b: Vec2,
    /// The suspension axis in A's coordinates (unit).
    pub local_axis_a: Vec2,
    /// `suspension.frequency` (Hz; 0: rigid) and `dampingRatio`.
    pub frequency: f32,
    pub damping_ratio: f32,
    /// Accumulated impulses (warm starting).
    pub impulse: f32,
    pub spring_impulse: f32,
}

impl WheelJoint {
    pub fn new(a: usize, b: usize, local_anchor_a: Vec2, local_anchor_b: Vec2, local_axis_a: Vec2, frequency: f32, damping_ratio: f32) -> WheelJoint {
        WheelJoint { a, b, local_anchor_a, local_anchor_b, local_axis_a: normalized(local_axis_a), frequency, damping_ratio, impulse: 0.0, spring_impulse: 0.0 }
    }
}

/// A joint's per-step solver data (`b2WheelJoint::InitVelocityConstraints`).
struct JointSolver {
    ax: Vec2,
    ay: Vec2,
    s_ax: f32,
    s_bx: f32,
    s_ay: f32,
    s_by: f32,
    mass: f32,
    spring_mass: f32,
    bias: f32,
    gamma: f32,
}

fn rotate(angle: f32, v: Vec2) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}

fn joint_frame(bodies: &[DynamicBody], j: &WheelJoint) -> (Vec2, Vec2, Vec2, Vec2) {
    let (a, b) = (&bodies[j.a].body, &bodies[j.b].body);
    let r_a = rotate(a.angle, j.local_anchor_a - a.local_center);
    let r_b = rotate(b.angle, j.local_anchor_b - b.local_center);
    let d = b.world_center() + r_b - a.world_center() - r_a;
    let ax = rotate(a.angle, j.local_axis_a);
    (r_a, r_b, d, ax)
}

fn init_joint(bodies: &[DynamicBody], j: &WheelJoint, h: f32) -> JointSolver {
    let (a, b) = (&bodies[j.a].body, &bodies[j.b].body);
    let (m_a, m_b, i_a, i_b) = (a.inv_mass, b.inv_mass, a.inv_inertia, b.inv_inertia);
    let (r_a, r_b, d, ax) = joint_frame(bodies, j);
    let ay = perp(ax);
    let s_ay = cross(d + r_a, ay);
    let s_by = cross(r_b, ay);
    let k = m_a + m_b + i_a * s_ay * s_ay + i_b * s_by * s_by;
    let mass = if k > 0.0 { 1.0 / k } else { 0.0 };
    let s_ax = cross(d + r_a, ax);
    let s_bx = cross(r_b, ax);
    let (mut spring_mass, mut bias, mut gamma) = (0.0, 0.0, 0.0);
    if j.frequency > 0.0 {
        let inv_mass = m_a + m_b + i_a * s_ax * s_ax + i_b * s_bx * s_bx;
        if inv_mass > 0.0 {
            spring_mass = 1.0 / inv_mass;
            let c = d.dot(ax);
            let omega = 2.0 * std::f32::consts::PI * j.frequency;
            let damping = 2.0 * spring_mass * j.damping_ratio * omega;
            let stiffness = spring_mass * omega * omega;
            gamma = h * (damping + h * stiffness);
            if gamma > 0.0 {
                gamma = 1.0 / gamma;
            }
            bias = c * h * stiffness * gamma;
            spring_mass = inv_mass + gamma;
            if spring_mass > 0.0 {
                spring_mass = 1.0 / spring_mass;
            }
        }
    }
    JointSolver { ax, ay, s_ax, s_bx, s_ay, s_by, mass, spring_mass, bias, gamma }
}

/// Apply a joint impulse: linear `p`, angular `l_a` / `l_b` (−A, +B).
fn apply_joint(bodies: &mut [DynamicBody], j: &WheelJoint, p: Vec2, l_a: f32, l_b: f32) {
    let a = &mut bodies[j.a].body;
    a.linear_velocity = a.linear_velocity - p * a.inv_mass;
    a.angular_velocity -= a.inv_inertia * l_a;
    let b = &mut bodies[j.b].body;
    b.linear_velocity = b.linear_velocity + p * b.inv_mass;
    b.angular_velocity += b.inv_inertia * l_b;
}

fn solve_joint_velocity(bodies: &mut [DynamicBody], j: &mut WheelJoint, s: &JointSolver) {
    // The spring along the axis.
    let (va, wa, vb, wb) = {
        let (a, b) = (&bodies[j.a].body, &bodies[j.b].body);
        (a.linear_velocity, a.angular_velocity, b.linear_velocity, b.angular_velocity)
    };
    let cdot = s.ax.dot(vb - va) + s.s_bx * wb - s.s_ax * wa;
    let impulse = -s.spring_mass * (cdot + s.bias + s.gamma * j.spring_impulse);
    j.spring_impulse += impulse;
    apply_joint(bodies, j, s.ax * impulse, impulse * s.s_ax, impulse * s.s_bx);
    // The point-to-line constraint.
    let (va, wa, vb, wb) = {
        let (a, b) = (&bodies[j.a].body, &bodies[j.b].body);
        (a.linear_velocity, a.angular_velocity, b.linear_velocity, b.angular_velocity)
    };
    let cdot = s.ay.dot(vb - va) + s.s_by * wb - s.s_ay * wa;
    let impulse = -s.mass * cdot;
    j.impulse += impulse;
    apply_joint(bodies, j, s.ay * impulse, impulse * s.s_ay, impulse * s.s_by);
}

/// `b2WheelJoint::SolvePositionConstraints`; true when within the slop.
fn solve_joint_position(bodies: &mut [DynamicBody], j: &WheelJoint) -> bool {
    let (r_a, r_b, d, ax) = joint_frame(bodies, j);
    let ay = perp(ax);
    let s_ay = cross(d + r_a, ay);
    let s_by = cross(r_b, ay);
    let c = d.dot(ay);
    let (m_a, m_b, i_a, i_b) = {
        let (a, b) = (&bodies[j.a].body, &bodies[j.b].body);
        (a.inv_mass, b.inv_mass, a.inv_inertia, b.inv_inertia)
    };
    let k = m_a + m_b + i_a * s_ay * s_ay + i_b * s_by * s_by;
    let impulse = if k != 0.0 { -c / k } else { 0.0 };
    let p = ay * impulse;
    let (l_a, l_b) = (impulse * s_ay, impulse * s_by);
    {
        let a = &mut bodies[j.a].body;
        let center = a.world_center() - p * a.inv_mass;
        a.angle -= a.inv_inertia * l_a;
        set_center(a, center);
    }
    {
        let b = &mut bodies[j.b].body;
        let center = b.world_center() + p * b.inv_mass;
        b.angle += b.inv_inertia * l_b;
        set_center(b, center);
    }
    c.abs() <= LINEAR_SLOP
}

/// Place the body so its centre of mass is at `center` (at its current angle).
fn set_center(body: &mut RigidBody, center: Vec2) {
    let lc = rotate(body.angle, body.local_center);
    body.position = center - lc;
}

/// One physics step for `bodies` against `world` and each other. Returns what touched what.
pub fn step(bodies: &mut [DynamicBody], world: &World, rules: &PhysicsRules, h: f32, velocity_iterations: u32, position_iterations: u32, warm: &mut WarmStart) -> Vec<Touch> {
    step_with_joints(bodies, &mut [], world, rules, h, velocity_iterations, position_iterations, warm)
}

/// [`step`] with wheel joints between the bodies (jointed bodies do not collide with each
/// other, as `WheelJoint2D.enableCollision` is off by default).
#[allow(clippy::too_many_arguments)]
pub fn step_with_joints(
    bodies: &mut [DynamicBody],
    joints: &mut [WheelJoint],
    world: &World,
    rules: &PhysicsRules,
    h: f32,
    velocity_iterations: u32,
    position_iterations: u32,
    warm: &mut WarmStart,
) -> Vec<Touch> {
    // ---- Collide, at the transforms the bodies start the step with.
    let mut contacts: Vec<ContactEntry> = Vec::new();
    for (ia, a) in bodies.iter().enumerate() {
        let xf_a = xf_of(&a.body);
        for (pa, poly) in a.pieces.iter().enumerate() {
            let bounds = aabb_of(poly, &xf_a, LINEAR_SLOP);
            world.solid_pieces(bounds.0, bounds.1, |collider, piece_id, shape| {
                if !rules.layers_collide(a.layer, collider.layer) {
                    return;
                }
                let (manifold, radius_b) = match shape {
                    PieceShape::Convex(pts) => {
                        let Some(m) = collide_polygons(poly, &xf_a, &Polygon::new(pts.to_vec()), &Xf::IDENTITY) else { return };
                        (m, POLYGON_RADIUS)
                    }
                    PieceShape::Segment(p, q) => {
                        let Some(m) = collide_polygons(poly, &xf_a, &Polygon::new(vec![p, q]), &Xf::IDENTITY) else { return };
                        (m, POLYGON_RADIUS)
                    }
                    PieceShape::Disc { center, radius } => {
                        let Some(m) = collide_polygon_circle(poly, &xf_a, center, radius, &Xf::IDENTITY) else { return };
                        (m, radius)
                    }
                };
                let key_b = 1u64 << 63 | u64::from(piece_id) << 8 | pa as u64;
                contacts.push(ContactEntry {
                    a: ia,
                    b: Other::Static,
                    manifold,
                    radius_a: poly.radius,
                    radius_b,
                    friction: (DEFAULT_FRICTION * DEFAULT_FRICTION).sqrt(),
                    restitution: DEFAULT_RESTITUTION,
                    normal: Vec2::ZERO,
                    points: Vec::new(),
                });
                // The collider id rides in the key for the touch report.
                let last = contacts.last_mut().expect("just pushed");
                last.points.push(VelocityPoint { r_a: Vec2::ZERO, r_b: Vec2::ZERO, normal_impulse: 0.0, tangent_impulse: 0.0, normal_mass: 0.0, tangent_mass: 0.0, velocity_bias: 0.0, key: (a.key, key_b, collider.id) });
            });
        }
        for (ib, b) in bodies.iter().enumerate().skip(ia + 1) {
            if !rules.layers_collide(a.layer, b.layer) || joints.iter().any(|j| (j.a == ia && j.b == ib) || (j.a == ib && j.b == ia)) {
                continue;
            }
            let xf_b = xf_of(&b.body);
            for (pa, poly_a) in a.pieces.iter().enumerate() {
                let bounds_a = aabb_of(poly_a, &xf_a, LINEAR_SLOP);
                for (pb, poly_b) in b.pieces.iter().enumerate() {
                    if !overlaps(bounds_a, aabb_of(poly_b, &xf_b, LINEAR_SLOP)) {
                        continue;
                    }
                    let Some(m) = collide_polygons(poly_a, &xf_a, poly_b, &xf_b) else { continue };
                    contacts.push(ContactEntry {
                        a: ia,
                        b: Other::Body(ib),
                        manifold: m,
                        radius_a: poly_a.radius,
                        radius_b: poly_b.radius,
                        friction: DEFAULT_FRICTION,
                        restitution: DEFAULT_RESTITUTION,
                        normal: Vec2::ZERO,
                        points: vec![VelocityPoint { r_a: Vec2::ZERO, r_b: Vec2::ZERO, normal_impulse: 0.0, tangent_impulse: 0.0, normal_mass: 0.0, tangent_mass: 0.0, velocity_bias: 0.0, key: (a.key, b.key << 16 | (pa as u64) << 8 | pb as u64, u32::MAX) }],
                    });
                }
            }
        }
    }
    let touches: Vec<Touch> = contacts
        .iter()
        .map(|c| match c.b {
            Other::Static => Touch::Static { body: c.a, collider: c.points[0].key.2 },
            Other::Body(b) => Touch::Bodies { a: c.a, b },
        })
        .collect();

    // ---- Integrate velocities.
    for b in bodies.iter_mut() {
        b.body.integrate_velocities(h, rules.gravity);
    }

    // ---- Velocity constraints (initialise, warm start).
    let tuning = &rules.box2d;
    let mut next_warm = WarmStart::default();
    for c in &mut contacts {
        let (key_a, key_b, _) = c.points[0].key;
        let a = &bodies[c.a].body;
        let xf_a = xf_of(a);
        let (xf_b, center_b, v_b, w_b, m_b, i_b) = match c.b {
            Other::Static => (Xf::IDENTITY, Vec2::ZERO, Vec2::ZERO, 0.0, 0.0, 0.0),
            Other::Body(ib) => {
                let b = &bodies[ib].body;
                (xf_of(b), b.world_center(), b.linear_velocity, b.angular_velocity, b.inv_mass, b.inv_inertia)
            }
        };
        let (normal, world_points) = world_manifold(&c.manifold, &xf_a, c.radius_a, &xf_b, c.radius_b);
        c.normal = normal;
        let tangent = perp(normal);
        let center_a = a.world_center();
        let (m_a, i_a) = (a.inv_mass, a.inv_inertia);
        c.points.clear();
        for (k, (point, _)) in world_points.iter().enumerate() {
            let id = c.manifold.points[k].id;
            let r_a = *point - center_a;
            let r_b = *point - center_b;
            let rn_a = cross(r_a, normal);
            let rn_b = cross(r_b, normal);
            let k_normal = m_a + m_b + i_a * rn_a * rn_a + i_b * rn_b * rn_b;
            let rt_a = cross(r_a, tangent);
            let rt_b = cross(r_b, tangent);
            let k_tangent = m_a + m_b + i_a * rt_a * rt_a + i_b * rt_b * rt_b;
            let dv = v_b + cross_sv(w_b, r_b) - a.linear_velocity - cross_sv(a.angular_velocity, r_a);
            let v_rel = normal.dot(dv);
            let key = (key_a, key_b, id);
            let (normal_impulse, tangent_impulse) = warm.impulses.get(&key).copied().unwrap_or((0.0, 0.0));
            c.points.push(VelocityPoint {
                r_a,
                r_b,
                normal_impulse,
                tangent_impulse,
                normal_mass: if k_normal > 0.0 { 1.0 / k_normal } else { 0.0 },
                tangent_mass: if k_tangent > 0.0 { 1.0 / k_tangent } else { 0.0 },
                velocity_bias: if v_rel < -tuning.velocity_threshold { -c.restitution * v_rel } else { 0.0 },
                key,
            });
        }
    }
    // Joints: initialise and warm start (Box2D's island inits joints after contacts).
    let solvers: Vec<JointSolver> = joints.iter().map(|j| init_joint(bodies, j, h)).collect();
    for (j, s) in joints.iter().zip(&solvers) {
        let p = s.ay * j.impulse + s.ax * j.spring_impulse;
        let l_a = j.impulse * s.s_ay + j.spring_impulse * s.s_ax;
        let l_b = j.impulse * s.s_by + j.spring_impulse * s.s_bx;
        apply_joint(bodies, j, p, l_a, l_b);
    }
    // Warm start.
    for c in &contacts {
        let tangent = perp(c.normal);
        for p in &c.points {
            let impulse = c.normal * p.normal_impulse + tangent * p.tangent_impulse;
            apply_impulse(bodies, c.a, &c.b, p.r_a, p.r_b, impulse);
        }
    }
    // Velocity iterations: friction first, then the normal, point by point.
    for _ in 0..velocity_iterations {
        // `b2Island::Solve`: joints before contacts.
        for (j, s) in joints.iter_mut().zip(&solvers) {
            solve_joint_velocity(bodies, j, s);
        }
        for c in &mut contacts {
            let tangent = perp(c.normal);
            for k in 0..c.points.len() {
                let (r_a, r_b) = (c.points[k].r_a, c.points[k].r_b);
                let dv = relative_velocity(bodies, c.a, &c.b, r_a, r_b);
                let vt = dv.dot(tangent);
                let p = &mut c.points[k];
                let max_friction = c.friction * p.normal_impulse;
                let new_impulse = (p.tangent_impulse - p.tangent_mass * vt).clamp(-max_friction, max_friction);
                let lambda = new_impulse - p.tangent_impulse;
                p.tangent_impulse = new_impulse;
                apply_impulse(bodies, c.a, &c.b, r_a, r_b, tangent * lambda);
            }
            for k in 0..c.points.len() {
                let (r_a, r_b) = (c.points[k].r_a, c.points[k].r_b);
                let dv = relative_velocity(bodies, c.a, &c.b, r_a, r_b);
                let vn = dv.dot(c.normal);
                let p = &mut c.points[k];
                let new_impulse = (p.normal_impulse - p.normal_mass * (vn - p.velocity_bias)).max(0.0);
                let lambda = new_impulse - p.normal_impulse;
                p.normal_impulse = new_impulse;
                apply_impulse(bodies, c.a, &c.b, r_a, r_b, c.normal * lambda);
            }
        }
    }
    for c in &contacts {
        for p in &c.points {
            next_warm.impulses.insert(p.key, (p.normal_impulse, p.tangent_impulse));
        }
    }
    *warm = next_warm;

    // ---- Integrate positions.
    for b in bodies.iter_mut() {
        b.body.integrate_positions(h, tuning);
    }

    // ---- Position iterations (Baumgarte).
    for _ in 0..position_iterations {
        let mut min_separation: f32 = 0.0;
        for c in &contacts {
            for k in 0..c.manifold.points.len() {
                let a = &bodies[c.a].body;
                let xf_a = xf_of(a);
                let (xf_b, center_b, inv_mass_b, inv_inertia_b) = match c.b {
                    Other::Static => (Xf::IDENTITY, Vec2::ZERO, 0.0, 0.0),
                    Other::Body(ib) => {
                        let b = &bodies[ib].body;
                        (xf_of(b), b.world_center(), b.inv_mass, b.inv_inertia)
                    }
                };
                let (normal, point, separation) = position_manifold(&c.manifold, k, &xf_a, &xf_b, c.radius_a, c.radius_b);
                let r_a = point - a.world_center();
                let r_b = point - center_b;
                min_separation = min_separation.min(separation);
                let correction = (tuning.baumgarte_scale * (separation + LINEAR_SLOP)).clamp(-tuning.max_linear_correction, 0.0);
                let rn_a = cross(r_a, normal);
                let rn_b = cross(r_b, normal);
                let k_normal = a.inv_mass + inv_mass_b + a.inv_inertia * rn_a * rn_a + inv_inertia_b * rn_b * rn_b;
                let impulse = if k_normal > 0.0 { -correction / k_normal } else { 0.0 };
                let p = normal * impulse;
                move_body(&mut bodies[c.a].body, -p, -cross(r_a, p));
                if let Other::Body(ib) = c.b {
                    move_body(&mut bodies[ib].body, p, cross(r_b, p));
                }
            }
        }
        let mut joints_ok = true;
        for j in joints.iter() {
            joints_ok &= solve_joint_position(bodies, j);
        }
        if min_separation >= POSITION_TOLERANCE && joints_ok {
            break;
        }
    }
    touches
}

fn relative_velocity(bodies: &[DynamicBody], a: usize, b: &Other, r_a: Vec2, r_b: Vec2) -> Vec2 {
    let ba = &bodies[a].body;
    let (v_b, w_b) = match b {
        Other::Static => (Vec2::ZERO, 0.0),
        Other::Body(ib) => (bodies[*ib].body.linear_velocity, bodies[*ib].body.angular_velocity),
    };
    v_b + cross_sv(w_b, r_b) - ba.linear_velocity - cross_sv(ba.angular_velocity, r_a)
}

/// Apply `impulse` at the contact: −P to A, +P to B.
fn apply_impulse(bodies: &mut [DynamicBody], a: usize, b: &Other, r_a: Vec2, r_b: Vec2, impulse: Vec2) {
    {
        let ba = &mut bodies[a].body;
        ba.linear_velocity = ba.linear_velocity - impulse * ba.inv_mass;
        ba.angular_velocity -= ba.inv_inertia * cross(r_a, impulse);
    }
    if let Other::Body(ib) = b {
        let bb = &mut bodies[*ib].body;
        bb.linear_velocity = bb.linear_velocity + impulse * bb.inv_mass;
        bb.angular_velocity += bb.inv_inertia * cross(r_b, impulse);
    }
}

/// Position correction: move the centre of mass by `inv_mass * p` and turn by
/// `inv_inertia * angular` (the origin follows the centre).
fn move_body(body: &mut RigidBody, p: Vec2, angular: f32) {
    let center = body.world_center() + p * body.inv_mass;
    body.angle += body.inv_inertia * angular;
    let (s, c) = body.angle.sin_cos();
    let lc = body.local_center;
    body.position = Vec2::new(center.x - (c * lc.x - s * lc.y), center.y - (s * lc.x + c * lc.y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{StaticCollider, StaticShape};

    fn rules() -> PhysicsRules {
        PhysicsRules { gravity: Vec2::new(0.0, -16.076), layer_matrix: [u32::MAX; 32], raycasts_hit_triggers: true, box2d: Default::default(), velocity_iterations: 8, position_iterations: 3 }
    }

    fn square(half: f32) -> Polygon {
        Polygon::new(vec![Vec2::new(-half, -half), Vec2::new(half, -half), Vec2::new(half, half), Vec2::new(-half, half)])
    }

    fn ground() -> World {
        World::build(vec![(
            StaticCollider { id: 7, ..Default::default() },
            StaticShape::Polygon(vec![Vec2::new(-50.0, -1.0), Vec2::new(50.0, -1.0), Vec2::new(50.0, 0.0), Vec2::new(-50.0, 0.0)]),
        )])
    }

    fn falling_box(y: f32, mass: f32) -> DynamicBody {
        let pieces = vec![square(0.5)];
        let mut body = RigidBody::new(Vec2::new(0.0, y), 0.0, mass, 0.0, 0.05, 1.0);
        let (lc, inertia) = mass_properties(&pieces, mass);
        body.local_center = lc;
        body.inertia = inertia;
        body.inv_inertia = 1.0 / inertia;
        DynamicBody { key: 1, body, pieces, layer: 0 }
    }

    #[test]
    fn box_mass_properties_match_box2d() {
        // A 1×1 box of mass 2: centre at the origin, I = m (w² + h²) / 12.
        let (c, i) = mass_properties(&[square(0.5)], 2.0);
        assert!(c.magnitude() < 1e-6);
        assert!((i - 2.0 * 2.0 / 12.0).abs() < 1e-5, "{i}");
        // Offset piece: the centre moves with it.
        let shifted = Polygon::new(square(0.5).vertices.iter().map(|v| *v + Vec2::new(3.0, 0.0)).collect());
        let (c, i) = mass_properties(&[shifted], 1.0);
        assert!((c.x - 3.0).abs() < 1e-5 && c.y.abs() < 1e-5);
        assert!((i - 1.0 / 6.0).abs() < 1e-4, "{i}");
    }

    #[test]
    fn face_contact_has_two_points_and_the_right_separation() {
        let ground = square(5.0);
        let xf_g = Xf::new(Vec2::new(0.0, -5.0), 0.0);
        let b = square(0.5);
        let xf_b = Xf::new(Vec2::new(0.0, 0.49), 0.0);
        let m = collide_polygons(&b, &xf_b, &ground, &xf_g).expect("touching");
        assert_eq!(m.points.len(), 2);
        let (normal, pts) = world_manifold(&m, &xf_b, POLYGON_RADIUS, &xf_g, POLYGON_RADIUS);
        // A → B: from the box down into the ground.
        assert!((normal.y + 1.0).abs() < 1e-5, "{normal:?}");
        for (_, sep) in pts {
            assert!((sep - (-0.01 - 2.0 * POLYGON_RADIUS)).abs() < 1e-4, "{sep}");
        }
        // Far apart: no manifold.
        assert!(collide_polygons(&b, &Xf::new(Vec2::new(0.0, 3.0), 0.0), &ground, &xf_g).is_none());
    }

    #[test]
    fn a_falling_box_comes_to_rest_on_the_ground() {
        let world = ground();
        let mut bodies = vec![falling_box(3.0, 1.0)];
        let mut warm = WarmStart::default();
        let mut touched = false;
        for _ in 0..300 {
            let t = step(&mut bodies, &world, &rules(), 0.02, 8, 3, &mut warm);
            touched |= t.iter().any(|t| matches!(t, Touch::Static { collider: 7, .. }));
        }
        let b = &bodies[0].body;
        assert!(touched);
        // Resting on y = 0 with the box half height 0.5 plus the skins, within the slop.
        assert!((b.position.y - (0.5 + 2.0 * POLYGON_RADIUS)).abs() < 0.02, "{:?}", b.position);
        assert!(b.linear_velocity.magnitude() < 0.05, "{:?}", b.linear_velocity);
        assert!(b.angle.abs() < 1e-3);
    }

    #[test]
    fn a_body_flying_into_a_wall_is_stopped() {
        let world = World::build(vec![(StaticCollider { id: 3, ..Default::default() }, StaticShape::Polygon(vec![Vec2::new(5.0, -10.0), Vec2::new(6.0, -10.0), Vec2::new(6.0, 10.0), Vec2::new(5.0, 10.0)]))]);
        let mut b = falling_box(0.0, 1.0);
        b.body.gravity_scale = 0.0;
        b.body.linear_velocity = Vec2::new(10.0, 0.0);
        let mut bodies = vec![b];
        let mut warm = WarmStart::default();
        for _ in 0..100 {
            step(&mut bodies, &world, &rules(), 0.02, 8, 3, &mut warm);
        }
        let b = &bodies[0].body;
        assert!(b.position.x < 5.0 - 0.5 + 0.05, "{:?}", b.position);
        assert!(b.position.x > 4.0, "{:?}", b.position);
    }

    #[test]
    fn layers_that_do_not_collide_pass_through() {
        let world = ground();
        let mut r = rules();
        r.layer_matrix[0] = 0;
        let mut bodies = vec![falling_box(3.0, 1.0)];
        let mut warm = WarmStart::default();
        for _ in 0..100 {
            step(&mut bodies, &world, &r, 0.02, 8, 3, &mut warm);
        }
        assert!(bodies[0].body.position.y < -5.0);
    }

    #[test]
    fn a_heavy_body_pushes_a_light_one() {
        let world = World::default();
        let mut heavy = falling_box(0.0, 1000.0);
        heavy.body.gravity_scale = 0.0;
        heavy.body.linear_velocity = Vec2::new(4.0, 0.0);
        let mut light = falling_box(0.0, 1.0);
        light.key = 2;
        light.body.gravity_scale = 0.0;
        light.body.position = Vec2::new(1.5, 0.0);
        let mut bodies = vec![heavy, light];
        let mut warm = WarmStart::default();
        for _ in 0..50 {
            step(&mut bodies, &world, &rules(), 0.02, 8, 3, &mut warm);
        }
        let (h, l) = (&bodies[0].body, &bodies[1].body);
        assert!(l.position.x > h.position.x + 0.95, "{:?} {:?}", h.position, l.position);
        assert!(h.linear_velocity.x > 3.9, "{:?}", h.linear_velocity);
        assert!(l.linear_velocity.x >= h.linear_velocity.x - 0.05);
    }

    #[test]
    fn polygon_meets_a_disc() {
        let b = square(0.5);
        let m = collide_polygon_circle(&b, &Xf::new(Vec2::ZERO, 0.0), Vec2::new(0.0, -1.0), 0.55, &Xf::IDENTITY).expect("overlap");
        let (normal, pts) = world_manifold(&m, &Xf::new(Vec2::ZERO, 0.0), POLYGON_RADIUS, &Xf::IDENTITY, 0.55);
        assert!((normal.y + 1.0).abs() < 1e-5);
        assert!(pts[0].1 < 0.0);
        assert!(collide_polygon_circle(&b, &Xf::IDENTITY, Vec2::new(0.0, -2.0), 0.5, &Xf::IDENTITY).is_none());
    }

    /// A 4×1 car body with two wheels of radius 0.5 hung 1 below it on springs.
    fn car(y: f32) -> (Vec<DynamicBody>, Vec<WheelJoint>) {
        let chassis = vec![Polygon::new(vec![Vec2::new(-2.0, -0.5), Vec2::new(2.0, -0.5), Vec2::new(2.0, 0.5), Vec2::new(-2.0, 0.5)])];
        let mut body = RigidBody::new(Vec2::new(0.0, y), 0.0, 4.0, 0.0, 0.05, 1.0);
        let (lc, inertia) = mass_properties(&chassis, 4.0);
        body.local_center = lc;
        body.inertia = inertia;
        body.inv_inertia = 1.0 / inertia;
        let mut bodies = vec![DynamicBody { key: 1, body, pieces: chassis, layer: 0 }];
        let mut joints = Vec::new();
        for (k, x) in [-1.5f32, 1.5].into_iter().enumerate() {
            let mut wheel = RigidBody::new(Vec2::new(x, y - 1.0), 0.0, 1.0, 0.0, 0.05, 1.0);
            wheel.inertia = 0.5 * 1.0 * 0.25;
            wheel.inv_inertia = 1.0 / wheel.inertia;
            bodies.push(DynamicBody { key: 2 + k as u64, body: wheel, pieces: vec![Polygon::disc(Vec2::ZERO, 0.5)], layer: 0 });
            joints.push(WheelJoint::new(0, 1 + k, Vec2::new(x, -1.0), Vec2::ZERO, Vec2::new(0.0, 1.0), 4.0, 0.7));
        }
        (bodies, joints)
    }

    #[test]
    fn disc_is_round_enough() {
        let d = Polygon::disc(Vec2::ZERO, 0.5);
        // Flats and corners are both within 5% of the radius.
        let core = d.vertices[0].magnitude();
        assert!((core + d.radius - 0.5).abs() < 0.03, "corner {}", core + d.radius);
        assert!((core * (std::f32::consts::PI / 8.0).cos() + d.radius - 0.5).abs() < 1e-5);
    }

    #[test]
    fn car_settles_on_its_wheels_and_drives() {
        let world = ground();
        let rules = rules();
        let (mut bodies, mut joints) = car(3.0);
        let mut warm = WarmStart::default();
        for _ in 0..240 {
            step_with_joints(&mut bodies, &mut joints, &world, &rules, 1.0 / 60.0, 8, 3, &mut warm);
        }
        // Wheels rest on the ground (top at y = 0), the body above them.
        for w in &bodies[1..] {
            assert!((w.body.position.y - 0.5).abs() < 0.06, "wheel at {:?}", w.body.position);
        }
        let chassis = bodies[0].body.position;
        assert!(chassis.y > 0.6 && chassis.y < 1.6, "chassis at {chassis:?}");
        assert!(bodies[0].body.angle.abs() < 0.02);
        // Spin the wheels clockwise (forward, +x) as `CarControl` does: the car rolls right.
        let start = chassis.x;
        for _ in 0..120 {
            for w in &mut bodies[1..] {
                w.body.angular_velocity = -10.0;
            }
            step_with_joints(&mut bodies, &mut joints, &world, &rules, 1.0 / 60.0, 8, 3, &mut warm);
        }
        let moved = bodies[0].body.position.x - start;
        assert!(moved > 3.0, "moved {moved}");
        // The wheels stay under the body (the joint holds them on their axis lines).
        let a = bodies[0].body.angle;
        for (k, w) in bodies[1..].iter().enumerate() {
            let anchor = bodies[0].body.position + rotate(a, Vec2::new(if k == 0 { -1.5 } else { 1.5 }, -1.0));
            let off = w.body.position - anchor;
            assert!(off.dot(rotate(a, Vec2::new(1.0, 0.0))).abs() < 0.05, "wheel {k} off its line by {off:?}");
        }
    }
}
