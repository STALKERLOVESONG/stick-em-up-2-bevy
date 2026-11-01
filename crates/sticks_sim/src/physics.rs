//! Minimal 2D collision world and kinematic-ish body stepping.
//!
//! What the game actually needs from Unity's Physics2D (Box2D) for the player character:
//!
//! * static solid geometry (polygon paths, boxes, circles, edges) on layers with a collision
//!   matrix,
//! * a compound dynamic body (two circles + a box, rotation frozen) whose velocity is written by
//!   the character controller every step, then integrated with gravity, and pushed out of
//!   penetrations with zero friction and zero bounce,
//! * per-step contact reports (normal, other collider) for ground / wall detection,
//! * trigger overlap queries and `OverlapCircle` queries.
//!
//! Solver choices (documented, not Box2D-exact): positional push-out leaving a resting
//! penetration of [`LINEAR_SLOP`] (Box2D's `b2_linearSlop`), velocity projected off contact
//! normals, and sub-stepping so fast movement cannot tunnel thin geometry.
//! TODO(UNCERTAIN): Box2D's sequential-impulse solver and speculative contacts may differ in
//! corner cases (resting depth, corner rounding). Validate against original recordings.

use std::collections::HashMap;

use unity_compat::Vec2;

/// Box2D's `b2_linearSlop`: the resting penetration the solver tolerates.
pub const LINEAR_SLOP: f32 = 0.005;
/// A contact is reported while shapes are closer than this (Box2D's polygon skin is 0.01).
pub const CONTACT_MARGIN: f32 = 0.005;
/// Largest distance a body moves between collision resolutions.
const MAX_SUBSTEP: f32 = 0.2;
const MAX_SUBSTEPS: usize = 128;
const RESOLVE_ITERATIONS: usize = 8;
const GRID_CELL: f32 = 8.0;

// ---------------------------------------------------------------------------------------------
// Static world
// ---------------------------------------------------------------------------------------------

/// Metadata for one static collider (a `PolygonCollider2D` path, box, circle, ...).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaticCollider {
    /// Caller-defined identity (reported in contacts; the controller tracks ground by identity).
    pub id: u32,
    pub layer: u8,
    /// `Walljumpable` tag.
    pub walljumpable: bool,
    /// Tagged `Unsafe Ground`.
    pub unsafe_ground: bool,
    /// Tagged `One-way Platform` (always a trigger; characters land on it from above only).
    pub one_way: bool,
    pub trigger: bool,
    /// Carries `HorizontalScroller2D` or `Projectile`: standing on it never records a safe
    /// respawn position (`CharacterControl2D.OnCollisionStay2D`).
    pub no_safe_standing: bool,
    /// Carries `UnsafeRespawnArea`: positions inside it are never recorded as safe.
    pub unsafe_respawn_area: bool,
}

/// Geometry given to [`World::build`].
#[derive(Clone, Debug)]
pub enum StaticShape {
    /// A simple (possibly concave) solid polygon.
    Polygon(Vec<Vec2>),
    Circle { center: Vec2, radius: f32 },
    /// An open polyline treated as thin walls.
    Edge(Vec<Vec2>),
}

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    fn overlaps(&self, o: &Aabb) -> bool {
        self.min.x <= o.max.x && self.max.x >= o.min.x && self.min.y <= o.max.y && self.max.y >= o.min.y
    }

    fn of_points(pts: &[Vec2]) -> Aabb {
        let mut a = Aabb { min: Vec2::new(f32::MAX, f32::MAX), max: Vec2::new(f32::MIN, f32::MIN) };
        for p in pts {
            a.min = Vec2::new(a.min.x.min(p.x), a.min.y.min(p.y));
            a.max = Vec2::new(a.max.x.max(p.x), a.max.y.max(p.y));
        }
        a
    }

    fn expanded(&self, by: f32) -> Aabb {
        Aabb { min: self.min - Vec2::new(by, by), max: self.max + Vec2::new(by, by) }
    }
}

#[derive(Clone, Debug)]
enum Piece {
    /// Convex polygon, counter-clockwise.
    Convex(Vec<Vec2>),
    Disc { center: Vec2, radius: f32 },
    Segment(Vec2, Vec2),
}

#[derive(Clone, Debug)]
struct PieceEntry {
    collider: usize,
    piece: Piece,
    aabb: Aabb,
}

/// Static collision world with a uniform-grid broad phase.
#[derive(Default)]
pub struct World {
    colliders: Vec<StaticCollider>,
    by_id: HashMap<u32, usize>,
    pieces: Vec<PieceEntry>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    /// Colliders switched off at run time (their object deactivated or destroyed): no query sees
    /// them.
    disabled: Vec<bool>,
    /// Dynamic rigid bodies' colliders as characters see them this step (helicopter hulls),
    /// replaced every frame by [`World::set_moving`]; `PieceEntry::collider` indexes
    /// `moving_colliders`. Only character bodies collide with them; queries ignore them.
    moving_colliders: Vec<StaticCollider>,
    moving_pieces: Vec<PieceEntry>,
}

/// A static piece's geometry, for the rigid-body contact solver (`crate::contact`).
#[derive(Clone, Copy, Debug)]
pub enum PieceShape<'a> {
    /// Counter-clockwise convex polygon.
    Convex(&'a [Vec2]),
    Disc { center: Vec2, radius: f32 },
    Segment(Vec2, Vec2),
}

/// Point containment for a counter-clockwise convex polygon (boundary counts as inside).
fn point_in_convex(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    (0..n).all(|i| {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) >= 0.0
    })
}

fn cell_of(v: f32) -> i32 {
    (v / GRID_CELL).floor() as i32
}

impl World {
    pub fn build(items: Vec<(StaticCollider, StaticShape)>) -> World {
        let mut world = World::default();
        for (collider, shape) in items {
            world.add(collider, shape);
        }
        world
    }

    /// Add one static collider (an object created while the level runs, e.g. the letters of an
    /// `ExplodableText`).
    pub fn add(&mut self, collider: StaticCollider, shape: StaticShape) {
        let idx = self.colliders.len();
        self.by_id.insert(collider.id, idx);
        self.colliders.push(collider);
        self.disabled.push(false);
        match shape {
            StaticShape::Polygon(pts) => {
                for piece in merge_convex(triangulate(&pts)) {
                    let aabb = Aabb::of_points(&piece);
                    self.add_piece(idx, Piece::Convex(piece), aabb);
                }
            }
            StaticShape::Circle { center, radius } => {
                let aabb = Aabb { min: center - Vec2::new(radius, radius), max: center + Vec2::new(radius, radius) };
                self.add_piece(idx, Piece::Disc { center, radius }, aabb);
            }
            StaticShape::Edge(pts) => {
                for w in pts.windows(2) {
                    let aabb = Aabb::of_points(&[w[0], w[1]]);
                    self.add_piece(idx, Piece::Segment(w[0], w[1]), aabb);
                }
            }
        }
    }

    /// Switch a collider (by id) on or off.
    pub fn set_enabled(&mut self, id: u32, enabled: bool) {
        if let Some(&i) = self.by_id.get(&id) {
            self.disabled[i] = !enabled;
        }
    }

    /// `GameObject.layer` changed at run time (`SetLayerInCampaignMode`, `Player.SetLayer`).
    pub fn set_layer(&mut self, id: u32, layer: u8) {
        if let Some(&i) = self.by_id.get(&id) {
            self.colliders[i].layer = layer;
        }
    }

    /// Whether the collider `id` exists and is enabled.
    pub fn is_enabled(&self, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|&i| !self.disabled[i])
    }

    fn add_piece(&mut self, collider: usize, piece: Piece, aabb: Aabb) {
        let id = self.pieces.len() as u32;
        for cx in cell_of(aabb.min.x)..=cell_of(aabb.max.x) {
            for cy in cell_of(aabb.min.y)..=cell_of(aabb.max.y) {
                self.grid.entry((cx, cy)).or_default().push(id);
            }
        }
        self.pieces.push(PieceEntry { collider, piece, aabb });
    }

    pub fn collider(&self, index: usize) -> &StaticCollider {
        &self.colliders[index]
    }

    /// Look a collider up by its caller-defined id.
    pub fn find(&self, id: u32) -> Option<&StaticCollider> {
        self.by_id.get(&id).map(|&i| &self.colliders[i])
    }

    /// `UnsafeRespawnArea.IsInUnsafeArea`: is `point` inside any collider flagged
    /// [`StaticCollider::unsafe_respawn_area`] (`Collider2D.OverlapPoint`, triggers included).
    pub fn in_unsafe_respawn_area(&self, point: Vec2) -> bool {
        let probe = Aabb { min: point, max: point };
        let mut inside = false;
        self.query(&probe, |entry| {
            if !inside && self.colliders[entry.collider].unsafe_respawn_area {
                inside = match &entry.piece {
                    Piece::Convex(pts) => point_in_convex(point, pts),
                    Piece::Disc { center, radius } => {
                        let d = point - *center;
                        d.x * d.x + d.y * d.y <= radius * radius
                    }
                    Piece::Segment(..) => false,
                };
            }
        });
        inside
    }

    pub fn collider_count(&self) -> usize {
        self.colliders.len()
    }

    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// Every enabled solid (non-trigger) static piece whose bounds overlap `min`..`max`, with its
    /// collider and a stable piece index.
    pub fn solid_pieces(&self, min: Vec2, max: Vec2, mut visit: impl FnMut(&StaticCollider, u32, PieceShape<'_>)) {
        let aabb = Aabb { min, max };
        let mut seen: Vec<u32> = Vec::new();
        for cx in cell_of(aabb.min.x)..=cell_of(aabb.max.x) {
            for cy in cell_of(aabb.min.y)..=cell_of(aabb.max.y) {
                let Some(ids) = self.grid.get(&(cx, cy)) else { continue };
                for &id in ids {
                    if seen.contains(&id) {
                        continue;
                    }
                    seen.push(id);
                    let entry = &self.pieces[id as usize];
                    let collider = &self.colliders[entry.collider];
                    if !entry.aabb.overlaps(&aabb) || self.disabled[entry.collider] || collider.trigger {
                        continue;
                    }
                    let shape = match &entry.piece {
                        Piece::Convex(pts) => PieceShape::Convex(pts),
                        Piece::Disc { center, radius } => PieceShape::Disc { center: *center, radius: *radius },
                        Piece::Segment(a, b) => PieceShape::Segment(*a, *b),
                    };
                    visit(collider, id, shape);
                }
            }
        }
    }

    /// Replace the moving colliders characters collide with: each entry is a collider and its
    /// convex pieces (counter-clockwise, world space).
    pub fn set_moving(&mut self, items: Vec<(StaticCollider, Vec<Vec<Vec2>>)>) {
        self.moving_colliders.clear();
        self.moving_pieces.clear();
        for (collider, pieces) in items {
            let index = self.moving_colliders.len();
            self.moving_colliders.push(collider);
            for piece in pieces {
                let aabb = Aabb::of_points(&piece);
                self.moving_pieces.push(PieceEntry { collider: index, piece: Piece::Convex(piece), aabb });
            }
        }
    }

    /// Visit each piece whose bounds overlap `aabb` exactly once.
    fn query(&self, aabb: &Aabb, mut visit: impl FnMut(&PieceEntry)) {
        let mut seen: Vec<u32> = Vec::new();
        for cx in cell_of(aabb.min.x)..=cell_of(aabb.max.x) {
            for cy in cell_of(aabb.min.y)..=cell_of(aabb.max.y) {
                if let Some(ids) = self.grid.get(&(cx, cy)) {
                    for &id in ids {
                        if !seen.contains(&id) {
                            seen.push(id);
                            let entry = &self.pieces[id as usize];
                            if entry.aabb.overlaps(aabb) && !self.disabled[entry.collider] {
                                visit(entry);
                            }
                        }
                    }
                }
            }
        }
    }

    /// `Physics2D.OverlapCircle(center, radius, layerMask)`: the first overlapping collider on a
    /// masked layer (triggers included, as Unity's default "queries hit triggers" does).
    pub fn overlap_circle(&self, center: Vec2, radius: f32, layer_mask: u32) -> Option<u32> {
        let probe = Aabb { min: center - Vec2::new(radius, radius), max: center + Vec2::new(radius, radius) };
        let mut hit = None;
        self.query(&probe, |entry| {
            let c = &self.colliders[entry.collider];
            if hit.is_none()
                && layer_mask >> c.layer & 1 != 0
                && circle_vs_piece(center, radius, &entry.piece).is_some_and(|m| m.depth > 0.0)
            {
                hit = Some(c.id);
            }
        });
        hit
    }

    /// Like [`Self::overlap_circle`] but for solid colliders only: what a moving circle collides
    /// with (triggers never produce collisions).
    pub fn overlap_circle_solid(&self, center: Vec2, radius: f32, layer_mask: u32) -> Option<u32> {
        let probe = Aabb { min: center - Vec2::new(radius, radius), max: center + Vec2::new(radius, radius) };
        let mut hit = None;
        self.query(&probe, |entry| {
            let c = &self.colliders[entry.collider];
            if hit.is_none()
                && !c.trigger
                && layer_mask >> c.layer & 1 != 0
                && circle_vs_piece(center, radius, &entry.piece).is_some_and(|m| m.depth > 0.0)
            {
                hit = Some(c.id);
            }
        });
        hit
    }

    /// `Physics2D.RaycastNonAlloc(origin, Vector2.down, ...)` over masked layers: ids of every
    /// collider the downward ray touches. A ray that starts inside a collider hits it (Unity's
    /// default `queriesStartInColliders`).
    pub fn raycast_down(&self, origin: Vec2, layer_mask: u32) -> Vec<u32> {
        const REACH: f32 = 2000.0;
        let probe = Aabb { min: Vec2::new(origin.x, origin.y - REACH), max: Vec2::new(origin.x, origin.y) };
        let mut hits: Vec<(f32, u32)> = Vec::new();
        self.query(&probe, |entry| {
            let c = &self.colliders[entry.collider];
            if layer_mask >> c.layer & 1 == 0 {
                return;
            }
            if let Some(top) = piece_top_below(&entry.piece, origin) {
                if !hits.iter().any(|&(_, id)| id == c.id) {
                    hits.push((origin.y - top, c.id));
                }
            }
        });
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.into_iter().map(|(_, id)| id).collect()
    }

    /// `Physics2D.Raycast(origin, Vector2.down, ∞, layerMask).point`: where the downward ray first
    /// meets a collider on a masked layer (triggers only with `hit_triggers`; a ray starting
    /// inside a collider hits it at the origin).
    pub fn raycast_down_point(&self, origin: Vec2, layer_mask: u32, hit_triggers: bool) -> Option<Vec2> {
        const REACH: f32 = 2000.0;
        let probe = Aabb { min: Vec2::new(origin.x, origin.y - REACH), max: Vec2::new(origin.x, origin.y) };
        let mut best: Option<f32> = None;
        self.query(&probe, |entry| {
            let c = &self.colliders[entry.collider];
            if layer_mask >> c.layer & 1 == 0 || self.disabled[entry.collider] || (c.trigger && !hit_triggers) {
                return;
            }
            if let Some(top) = piece_top_below(&entry.piece, origin) {
                let top = top.min(origin.y);
                if best.is_none_or(|b| top > b) {
                    best = Some(top);
                }
            }
        });
        best.map(|y| Vec2::new(origin.x, y))
    }

    /// `Physics2D.Linecast(start, end, layerMask)` used as a boolean: does any collider on a
    /// masked layer touch the segment? Triggers count when `hit_triggers` is set
    /// (`Physics2DSettings.m_RaycastsHitTriggers`), and a segment that starts inside a collider
    /// hits it (Unity's default `queriesStartInColliders`).
    ///
    /// TODO(UNCERTAIN): Box2D's ray cast against polygons includes their 0.01 skin radius;
    /// grazing contacts within that skin are not reproduced.
    pub fn linecast(&self, start: Vec2, end: Vec2, layer_mask: u32, hit_triggers: bool) -> bool {
        let probe = Aabb::of_points(&[start, end]);
        let mut hit = false;
        self.query(&probe, |entry| {
            if hit {
                return;
            }
            let c = &self.colliders[entry.collider];
            if layer_mask >> c.layer & 1 == 0 || (c.trigger && !hit_triggers) {
                return;
            }
            hit = segment_touches_piece(start, end, &entry.piece);
        });
        hit
    }
}

/// Does the segment `a`–`b` touch `piece` (including starting or ending inside it)?
fn segment_touches_piece(a: Vec2, b: Vec2, piece: &Piece) -> bool {
    match piece {
        Piece::Convex(poly) => segment_touches_convex(a, b, poly),
        Piece::Disc { center, radius } => {
            let d = b - a;
            let len_sqr = d.x * d.x + d.y * d.y;
            let t = if len_sqr > 0.0 { ((*center - a).dot(d) / len_sqr).clamp(0.0, 1.0) } else { 0.0 };
            let closest = a + d * t;
            let off = *center - closest;
            off.x * off.x + off.y * off.y <= radius * radius
        }
        Piece::Segment(p, q) => segments_intersect(a, b, *p, *q),
    }
}

/// Cyrus–Beck clipping of a segment against a counter-clockwise convex polygon.
fn segment_touches_convex(a: Vec2, b: Vec2, poly: &[Vec2]) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let n = poly.len();
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let edge = q - p;
        // Outward normal of a counter-clockwise edge.
        let normal = Vec2::new(edge.y, -edge.x);
        let num = normal.dot(p - a);
        let den = normal.dot(d);
        if den == 0.0 {
            if num < 0.0 {
                return false;
            }
        } else {
            let t = num / den;
            if den < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

fn segments_intersect(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let d1 = cross(c, d, a);
    let d2 = cross(c, d, b);
    let d3 = cross(a, b, c);
    let d4 = cross(a, b, d);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    let on = |p: Vec2, q: Vec2, r: Vec2, side: f32| {
        side == 0.0 && r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
    };
    on(c, d, a, d1) || on(c, d, b, d2) || on(a, b, c, d3) || on(a, b, d, d4)
}

/// Highest y (clamped to the ray origin) at which the downward ray from `origin` meets `piece`.
fn piece_top_below(piece: &Piece, origin: Vec2) -> Option<f32> {
    let (lo, hi) = match piece {
        Piece::Convex(poly) => {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.x - origin.x) * (b.x - origin.x) <= 0.0 && a.x != b.x {
                    let t = (origin.x - a.x) / (b.x - a.x);
                    let y = a.y + (b.y - a.y) * t;
                    lo = lo.min(y);
                    hi = hi.max(y);
                } else if a.x == b.x && a.x == origin.x {
                    lo = lo.min(a.y.min(b.y));
                    hi = hi.max(a.y.max(b.y));
                }
            }
            (lo, hi)
        }
        Piece::Disc { center, radius } => {
            let dx = origin.x - center.x;
            if dx.abs() > *radius {
                return None;
            }
            let h = (radius * radius - dx * dx).sqrt();
            (center.y - h, center.y + h)
        }
        Piece::Segment(a, b) => {
            if (a.x - origin.x) * (b.x - origin.x) > 0.0 || a.x == b.x {
                return None;
            }
            let t = (origin.x - a.x) / (b.x - a.x);
            let y = a.y + (b.y - a.y) * t;
            (y, y)
        }
    };
    if lo > hi || lo > origin.y {
        return None;
    }
    Some(hi.min(origin.y))
}

// ---------------------------------------------------------------------------------------------
// Triangulation
// ---------------------------------------------------------------------------------------------

fn signed_area(pts: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

fn cross(o: Vec2, a: Vec2, b: Vec2) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn point_in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
}

/// Ear-clipping triangulation of a simple polygon. Returns counter-clockwise triangles.
/// Duplicate and collinear points are tolerated; degenerate triangles are dropped.
pub fn triangulate(points: &[Vec2]) -> Vec<Vec<Vec2>> {
    let mut pts: Vec<Vec2> = Vec::with_capacity(points.len());
    for &p in points {
        if pts.last().is_none_or(|&l| (l - p).sqr_magnitude() > 1e-12) {
            pts.push(p);
        }
    }
    if pts.len() > 1 && (pts[0] - pts[pts.len() - 1]).sqr_magnitude() <= 1e-12 {
        pts.pop();
    }
    if pts.len() < 3 {
        return Vec::new();
    }
    if signed_area(&pts) < 0.0 {
        pts.reverse();
    }
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::new();
    let mut guard = 0usize;
    while idx.len() > 3 && guard < pts.len() * pts.len() + 16 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) <= 1e-9 {
                continue; // reflex or collinear
            }
            let blocked = idx.iter().any(|&j| {
                j != ia && j != ib && j != ic && {
                    let p = pts[j];
                    // Points coincident with a triangle corner do not block (touching vertices).
                    p != a && p != b && p != c && point_in_triangle(p, a, b, c)
                }
            });
            if !blocked {
                out.push(vec![a, b, c]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Numerically stuck (collinear run): drop the flattest vertex.
            let n = idx.len();
            let flattest = (0..n)
                .min_by(|&x, &y| {
                    let ax = cross(pts[idx[(x + n - 1) % n]], pts[idx[x]], pts[idx[(x + 1) % n]]).abs();
                    let ay = cross(pts[idx[(y + n - 1) % n]], pts[idx[y]], pts[idx[(y + 1) % n]]).abs();
                    ax.partial_cmp(&ay).unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0);
            idx.remove(flattest);
        }
    }
    if idx.len() == 3 {
        let (a, b, c) = (pts[idx[0]], pts[idx[1]], pts[idx[2]]);
        if cross(a, b, c).abs() > 1e-9 {
            out.push(if cross(a, b, c) > 0.0 { vec![a, b, c] } else { vec![a, c, b] });
        }
    }
    out
}

/// Hertel–Mehlhorn style merge of counter-clockwise convex pieces that share an edge, whenever the
/// union stays convex. Fewer pieces means fewer internal seams: a body sliding along a flat
/// surface must not collide with the shared vertices between triangles of the same polygon
/// (Box2D never sees them because it works on the polygon's real convex decomposition).
/// Merge adjacent convex pieces of a triangulation back into larger convex polygons.
pub fn merge_convex(mut pieces: Vec<Vec<Vec2>>) -> Vec<Vec<Vec2>> {
    fn key(a: Vec2, b: Vec2) -> [u32; 4] {
        [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()]
    }
    fn try_merge(a: &[Vec2], b: &[Vec2]) -> Option<Vec<Vec2>> {
        for p in 0..a.len() {
            let (ea, eb) = (a[p], a[(p + 1) % a.len()]);
            for q in 0..b.len() {
                if b[q] == eb && b[(q + 1) % b.len()] == ea {
                    let mut merged: Vec<Vec2> = (0..a.len()).map(|k| a[(p + 1 + k) % a.len()]).collect();
                    // `merged` now runs eb .. ea; continue with b's vertices after ea, before eb.
                    merged.extend((2..b.len()).map(|k| b[(q + k) % b.len()]));
                    let n = merged.len();
                    let convex = (0..n).all(|i| cross(merged[(i + n - 1) % n], merged[i], merged[(i + 1) % n]) >= -1e-6);
                    if convex {
                        // Drop collinear vertices so flat runs become single edges.
                        let keep: Vec<Vec2> = (0..n)
                            .filter(|&i| cross(merged[(i + n - 1) % n], merged[i], merged[(i + 1) % n]).abs() > 1e-6)
                            .map(|i| merged[i])
                            .collect();
                        return if keep.len() >= 3 { Some(keep) } else { None };
                    }
                    return None;
                }
            }
        }
        None
    }

    loop {
        let mut edges: HashMap<[u32; 4], usize> = HashMap::new();
        for (i, poly) in pieces.iter().enumerate() {
            for k in 0..poly.len() {
                edges.insert(key(poly[k], poly[(k + 1) % poly.len()]), i);
            }
        }
        let mut dead = vec![false; pieces.len()];
        let mut touched = vec![false; pieces.len()];
        let mut merged_any = false;
        for i in 0..pieces.len() {
            if touched[i] {
                continue;
            }
            let n = pieces[i].len();
            for k in 0..n {
                let (a, b) = (pieces[i][k], pieces[i][(k + 1) % n]);
                let Some(&j) = edges.get(&key(b, a)) else { continue };
                if j == i || touched[j] {
                    continue;
                }
                if let Some(m) = try_merge(&pieces[i], &pieces[j]) {
                    pieces[i] = m;
                    dead[j] = true;
                    touched[i] = true;
                    touched[j] = true;
                    merged_any = true;
                    break;
                }
            }
        }
        if !merged_any {
            return pieces;
        }
        let mut idx = 0;
        pieces.retain(|_| {
            idx += 1;
            !dead[idx - 1]
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Narrow phase
// ---------------------------------------------------------------------------------------------

/// Penetration of a shape into a static piece. `normal` points from the static piece towards the
/// dynamic shape; `depth` is how far to move along `normal` to separate.
#[derive(Clone, Copy, Debug)]
struct Manifold {
    normal: Vec2,
    depth: f32,
}

fn circle_vs_convex(c: Vec2, r: f32, poly: &[Vec2]) -> Option<Manifold> {
    let n = poly.len();
    let mut best_sep = f32::MIN;
    let mut best_edge = 0;
    let mut best_normal = Vec2::ZERO;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let e = b - a;
        let len = e.magnitude();
        if len < 1e-9 {
            continue;
        }
        let normal = Vec2::new(e.y, -e.x) / len; // outward for CCW
        let sep = (c - a).dot(normal);
        if sep > best_sep {
            best_sep = sep;
            best_edge = i;
            best_normal = normal;
        }
    }
    if best_sep > r + CONTACT_MARGIN {
        return None;
    }
    if best_sep <= 0.0 {
        // Center inside: push out through the nearest edge.
        return Some(Manifold { normal: best_normal, depth: r - best_sep });
    }
    let (a, b) = (poly[best_edge], poly[(best_edge + 1) % n]);
    let e = b - a;
    let t = (c - a).dot(e) / e.sqr_magnitude();
    if (0.0..=1.0).contains(&t) {
        return Some(Manifold { normal: best_normal, depth: r - best_sep });
    }
    let v = if t < 0.0 { a } else { b };
    let d = c - v;
    let dist = d.magnitude();
    if dist > r + CONTACT_MARGIN || dist < 1e-9 {
        return None;
    }
    Some(Manifold { normal: d / dist, depth: r - dist })
}

fn circle_vs_segment(c: Vec2, r: f32, a: Vec2, b: Vec2) -> Option<Manifold> {
    let e = b - a;
    let t = ((c - a).dot(e) / e.sqr_magnitude().max(1e-12)).clamp(0.0, 1.0);
    let closest = a + e * t;
    let d = c - closest;
    let dist = d.magnitude();
    if dist > r + CONTACT_MARGIN {
        return None;
    }
    let normal = if dist > 1e-9 {
        d / dist
    } else {
        let n = e.perp();
        n / n.magnitude().max(1e-9)
    };
    Some(Manifold { normal, depth: r - dist })
}

fn circle_vs_disc(c: Vec2, r: f32, center: Vec2, radius: f32) -> Option<Manifold> {
    let d = c - center;
    let dist = d.magnitude();
    if dist > r + radius + CONTACT_MARGIN {
        return None;
    }
    let normal = if dist > 1e-9 { d / dist } else { Vec2::new(0.0, 1.0) };
    Some(Manifold { normal, depth: r + radius - dist })
}

fn circle_vs_piece(c: Vec2, r: f32, piece: &Piece) -> Option<Manifold> {
    match piece {
        Piece::Convex(poly) => circle_vs_convex(c, r, poly),
        Piece::Disc { center, radius } => circle_vs_disc(c, r, *center, *radius),
        Piece::Segment(a, b) => circle_vs_segment(c, r, *a, *b),
    }
}

/// SAT between convex polygon `a` (dynamic) and convex polygon `b` (static); normal points from
/// `b` to `a`.
fn convex_vs_convex(a: &[Vec2], b: &[Vec2]) -> Option<Manifold> {
    let mut best = Manifold { normal: Vec2::ZERO, depth: f32::MAX };
    for (poly, flip) in [(a, false), (b, true)] {
        let n = poly.len();
        for i in 0..n {
            let e = poly[(i + 1) % n] - poly[i];
            let len = e.magnitude();
            if len < 1e-9 {
                continue;
            }
            let axis = Vec2::new(e.y, -e.x) / len;
            let (amin, amax) = project(a, axis);
            let (bmin, bmax) = project(b, axis);
            // Overlap along `axis`; the push direction for `a` is +axis if a lies on the +side.
            let overlap_pos = bmax - amin; // push a along +axis
            let overlap_neg = amax - bmin; // push a along -axis
            if overlap_pos < -CONTACT_MARGIN || overlap_neg < -CONTACT_MARGIN {
                return None;
            }
            let _ = flip;
            if overlap_pos < best.depth {
                best = Manifold { normal: axis, depth: overlap_pos };
            }
            if overlap_neg < best.depth {
                best = Manifold { normal: -axis, depth: overlap_neg };
            }
        }
    }
    (best.depth < f32::MAX).then_some(best)
}

fn project(poly: &[Vec2], axis: Vec2) -> (f32, f32) {
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for p in poly {
        let d = p.dot(axis);
        lo = lo.min(d);
        hi = hi.max(d);
    }
    (lo, hi)
}

fn convex_vs_piece(poly: &[Vec2], piece: &Piece) -> Option<Manifold> {
    match piece {
        Piece::Convex(other) => convex_vs_convex(poly, other),
        Piece::Disc { center, radius } => {
            // Treat the disc as the dynamic side then flip the normal.
            let m = circle_vs_convex(*center, *radius, poly)?;
            Some(Manifold { normal: -m.normal, depth: m.depth })
        }
        Piece::Segment(a, b) => convex_vs_convex(poly, &[*a, *b]),
    }
}

// ---------------------------------------------------------------------------------------------
// Dynamic body
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CircleShape {
    pub offset: Vec2,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShape {
    pub offset: Vec2,
    pub half_extents: Vec2,
}

/// Collider layout of a body (rotation is frozen, so shapes are axis-aligned).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BodyShape {
    pub circles: Vec<CircleShape>,
    pub boxes: Vec<BoxShape>,
    /// Counter-clockwise convex pieces relative to the body position, already rotated (bodies
    /// that do rotate, like helicopters, rebuild them every step).
    pub polygons: Vec<Vec<Vec2>>,
}

/// Collision rules shared by every body: gravity and the layer matrix.
#[derive(Clone, Debug)]
pub struct PhysicsRules {
    pub gravity: Vec2,
    /// `matrix[a]` bit `b` set when layers `a` and `b` collide.
    pub layer_matrix: [u32; 32],
    /// `Physics2DSettings.m_RaycastsHitTriggers`: ray and line casts report triggers.
    pub raycasts_hit_triggers: bool,
    /// The Box2D tuning for rigid bodies (`crate::rigid_body`).
    pub box2d: unity_data::settings::Box2dTuning,
    /// `Physics2DSettings.m_VelocityIterations` / `m_PositionIterations` (`crate::contact`).
    pub velocity_iterations: u32,
    pub position_iterations: u32,
}

impl PhysicsRules {
    pub fn layers_collide(&self, a: u8, b: u8) -> bool {
        self.layer_matrix[(a & 31) as usize] >> (b & 31) & 1 != 0
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub position: Vec2,
    pub velocity: Vec2,
    /// `Rigidbody2D.gravityScale`.
    pub gravity_scale: f32,
    pub layer: u8,
    pub shape: BodyShape,
}

/// One touching static collider (the analogue of a `Collision2D` contact).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// Identity of the static collider (`StaticCollider::id`).
    pub collider: u32,
    /// Physics layer of that collider.
    pub layer: u8,
    /// Points from the static collider towards the body.
    pub normal: Vec2,
    pub depth: f32,
    pub walljumpable: bool,
    pub unsafe_ground: bool,
}

/// Result of one physics step.
#[derive(Clone, Debug, Default)]
pub struct StepReport {
    /// Solid contacts at the end of the step (what `OnCollisionStay2D` would have seen).
    pub contacts: Vec<Contact>,
    /// Ids of trigger colliders the body overlaps at the end of the step.
    pub triggers: Vec<u32>,
    /// Subset of `triggers` that were not overlapped after the previous step
    /// (`OnTriggerEnter2D`). Filled in by the owner of the body, which remembers the overlaps.
    pub entered_triggers: Vec<u32>,
    /// Triggers overlapped after the previous step but not any more (`OnTriggerExit2D`). Filled
    /// in by the owner of the body.
    pub exited_triggers: Vec<u32>,
}

impl Body {
    /// The trigger colliders the body's shape overlaps where it is now (no step), sorted.
    pub fn overlapping_triggers(&self, world: &World, rules: &PhysicsRules) -> Vec<u32> {
        let mut found = Vec::new();
        self.gather(world, rules, self.position, false, &mut found);
        let mut ids: Vec<u32> = found.iter().filter(|c| c.depth > 0.0).map(|c| c.collider).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn gather(&self, world: &World, rules: &PhysicsRules, position: Vec2, solid: bool, out: &mut Vec<Contact>) {
        let mut probe_pts = Vec::new();
        for c in &self.shape.circles {
            let p = position + c.offset;
            probe_pts.push(p - Vec2::new(c.radius, c.radius));
            probe_pts.push(p + Vec2::new(c.radius, c.radius));
        }
        for b in &self.shape.boxes {
            let p = position + b.offset;
            probe_pts.push(p - b.half_extents);
            probe_pts.push(p + b.half_extents);
        }
        for poly in &self.shape.polygons {
            probe_pts.extend(poly.iter().map(|v| position + *v));
        }
        if probe_pts.is_empty() {
            return;
        }
        let probe = Aabb::of_points(&probe_pts).expanded(CONTACT_MARGIN);
        let moving = world.moving_pieces.iter().filter(|e| e.aabb.overlaps(&probe)).map(|e| (e, &world.moving_colliders[e.collider]));
        let mut visit = |entry: &PieceEntry, collider: &StaticCollider| {
            if collider.trigger == solid || !rules.layers_collide(self.layer, collider.layer) {
                return;
            }
            let mut push = |m: Manifold| {
                out.push(Contact {
                    collider: collider.id,
                    layer: collider.layer,
                    normal: m.normal,
                    depth: m.depth,
                    walljumpable: collider.walljumpable,
                    unsafe_ground: collider.unsafe_ground,
                });
            };
            for c in &self.shape.circles {
                if let Some(m) = circle_vs_piece(position + c.offset, c.radius, &entry.piece) {
                    push(m);
                }
            }
            for b in &self.shape.boxes {
                let p = position + b.offset;
                let h = b.half_extents;
                let quad = [p + Vec2::new(-h.x, -h.y), p + Vec2::new(h.x, -h.y), p + Vec2::new(h.x, h.y), p + Vec2::new(-h.x, h.y)];
                if let Some(m) = convex_vs_piece(&quad, &entry.piece) {
                    push(m);
                }
            }
            for poly in &self.shape.polygons {
                let placed: Vec<Vec2> = poly.iter().map(|v| position + *v).collect();
                if let Some(m) = convex_vs_piece(&placed, &entry.piece) {
                    push(m);
                }
            }
        };
        world.query(&probe, |entry| visit(entry, &world.colliders[entry.collider]));
        // Moving bodies are offset by their position each step (`set_moving`), so they act as
        // obstacles placed where they are this frame.
        for (entry, collider) in moving {
            visit(entry, collider);
        }
    }

    fn resolve_penetration(&mut self, world: &World, rules: &PhysicsRules) {
        let mut contacts = Vec::new();
        for _ in 0..RESOLVE_ITERATIONS {
            contacts.clear();
            self.gather(world, rules, self.position, true, &mut contacts);
            let deepest = contacts.iter().filter(|c| c.depth > LINEAR_SLOP).max_by(|a, b| a.depth.total_cmp(&b.depth));
            let Some(c) = deepest else { break };
            self.position += c.normal * (c.depth - LINEAR_SLOP);
        }
    }

    /// Integrate one physics step: gravity, motion with push-out, velocity projection.
    /// (Mirrors Box2D's order: velocity update, position update, contact resolution.)
    pub fn step(&mut self, world: &World, rules: &PhysicsRules, dt: f32) -> StepReport {
        self.velocity += rules.gravity * (self.gravity_scale * dt);
        let displacement = self.velocity * dt;
        let substeps = ((displacement.magnitude() / MAX_SUBSTEP).ceil() as usize).clamp(1, MAX_SUBSTEPS);
        let delta = displacement / substeps as f32;
        for _ in 0..substeps {
            self.position += delta;
            self.resolve_penetration(world, rules);
        }

        let mut report = StepReport::default();
        self.gather(world, rules, self.position, true, &mut report.contacts);
        report.contacts.retain(|c| c.depth > 0.0);
        // Stop motion into touching surfaces (zero bounce, zero friction).
        for c in &report.contacts {
            let vn = self.velocity.dot(c.normal);
            if vn < 0.0 {
                self.velocity -= c.normal * vn;
            }
        }
        let mut triggers = Vec::new();
        self.gather(world, rules, self.position, false, &mut triggers);
        report.triggers = triggers.iter().filter(|c| c.depth > 0.0).map(|c| c.collider).collect();
        report.triggers.sort_unstable();
        report.triggers.dedup();
        report
    }
}

/// Whether a convex, counter-clockwise polygon (world space) overlaps any collider of a body
/// placed at `position`.
pub fn convex_touches_body(poly: &[Vec2], position: Vec2, shape: &BodyShape) -> bool {
    shape.circles.iter().any(|c| circle_vs_convex(position + c.offset, c.radius, poly).is_some_and(|m| m.depth > 0.0))
        || shape.boxes.iter().any(|b| {
            let p = position + b.offset;
            let h = b.half_extents;
            let quad = [p + Vec2::new(-h.x, -h.y), p + Vec2::new(h.x, -h.y), p + Vec2::new(h.x, h.y), p + Vec2::new(-h.x, h.y)];
            convex_vs_convex(poly, &quad).is_some_and(|m| m.depth > 0.0)
        })
        || shape.polygons.iter().any(|piece| {
            let world: Vec<Vec2> = piece.iter().map(|&p| position + p).collect();
            convex_vs_convex(poly, &world).is_some_and(|m| m.depth > 0.0)
        })
}

/// When a circle overlaps a convex, counter-clockwise polygon: the unit normal pointing from the
/// polygon towards the circle.
pub fn circle_convex_normal(center: Vec2, radius: f32, poly: &[Vec2]) -> Option<Vec2> {
    circle_vs_convex(center, radius, poly).filter(|m| m.depth > 0.0).map(|m| m.normal)
}

/// Whether a circle overlaps any collider of a body placed at `position`.
pub fn circle_touches_body(center: Vec2, radius: f32, position: Vec2, shape: &BodyShape) -> bool {
    shape.circles.iter().any(|c| (center - (position + c.offset)).magnitude() < radius + c.radius)
        || shape.boxes.iter().any(|b| {
            let p = position + b.offset;
            let h = b.half_extents;
            let quad = [p + Vec2::new(-h.x, -h.y), p + Vec2::new(h.x, -h.y), p + Vec2::new(h.x, h.y), p + Vec2::new(-h.x, h.y)];
            circle_vs_convex(center, radius, &quad).is_some_and(|m| m.depth > 0.0)
        })
        || shape.polygons.iter().any(|piece| {
            let world: Vec<Vec2> = piece.iter().map(|&p| position + p).collect();
            circle_vs_convex(center, radius, &world).is_some_and(|m| m.depth > 0.0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: f32 = -16.076;

    fn rules() -> PhysicsRules {
        PhysicsRules { gravity: Vec2::new(0.0, G), layer_matrix: [u32::MAX; 32], raycasts_hit_triggers: true, box2d: Default::default(), velocity_iterations: 8, position_iterations: 3 }
    }

    fn ground_collider(id: u32) -> StaticCollider {
        StaticCollider { id, layer: 0, ..Default::default() }
    }

    fn flat_world() -> World {
        // Solid slab with its top surface at y = 0.
        let slab = vec![Vec2::new(-50.0, -10.0), Vec2::new(50.0, -10.0), Vec2::new(50.0, 0.0), Vec2::new(-50.0, 0.0)];
        World::build(vec![(ground_collider(1), StaticShape::Polygon(slab))])
    }

    #[test]
    fn linecast_sees_walls_layers_and_triggers() {
        // A wall on layer 0 between x = 4 and x = 6, a trigger disc on layer 0 at x = -5 and an
        // edge on layer 3 at x = 10.
        let wall = vec![Vec2::new(4.0, 0.0), Vec2::new(6.0, 0.0), Vec2::new(6.0, 10.0), Vec2::new(4.0, 10.0)];
        let world = World::build(vec![
            (ground_collider(1), StaticShape::Polygon(wall)),
            (StaticCollider { id: 2, layer: 0, trigger: true, ..Default::default() }, StaticShape::Circle { center: Vec2::new(-5.0, 2.0), radius: 1.0 }),
            (StaticCollider { id: 3, layer: 3, ..Default::default() }, StaticShape::Edge(vec![Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)])),
        ]);
        let from = Vec2::new(0.0, 2.0);
        assert!(world.linecast(from, Vec2::new(8.0, 2.0), 1, true), "through the wall");
        // At x = 4 the line is at y = 16, above the 10 high wall.
        assert!(!world.linecast(from, Vec2::new(8.0, 30.0), 1, true), "over the wall");
        assert!(!world.linecast(from, Vec2::new(3.9, 2.0), 1, true), "stops short of the wall");
        assert!(world.linecast(Vec2::new(5.0, 2.0), Vec2::new(5.5, 3.0), 1, true), "starts inside");
        assert!(world.linecast(from, Vec2::new(-8.0, 2.0), 1, true), "trigger counts");
        assert!(!world.linecast(from, Vec2::new(-8.0, 2.0), 1, false), "triggers ignored");
        assert!(!world.linecast(Vec2::new(7.0, 2.0), Vec2::new(12.0, 2.0), 1, true), "edge layer masked out");
        assert!(world.linecast(Vec2::new(7.0, 2.0), Vec2::new(12.0, 2.0), 1 << 3, true), "edge on its layer");
    }

    fn man(position: Vec2) -> Body {
        Body {
            position,
            velocity: Vec2::ZERO,
            gravity_scale: 1.0,
            layer: 8,
            shape: BodyShape {
                circles: vec![
                    CircleShape { offset: Vec2::new(0.0, 0.6), radius: 0.6 },
                    CircleShape { offset: Vec2::new(0.0, 2.4), radius: 0.6 },
                ],
                boxes: vec![BoxShape { offset: Vec2::new(0.0, 1.5), half_extents: Vec2::new(0.575, 0.9) }], polygons: Vec::new()
            },
        }
    }

    #[test]
    fn triangulation_preserves_area_of_concave_polygon() {
        // U shape: area = outer 10x6 minus inner 4x4 notch = 60 - 16 = 44.
        let u = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 6.0),
            Vec2::new(7.0, 6.0),
            Vec2::new(7.0, 2.0),
            Vec2::new(3.0, 2.0),
            Vec2::new(3.0, 6.0),
            Vec2::new(0.0, 6.0),
        ];
        let tris = triangulate(&u);
        let area: f32 = tris.iter().map(|t| signed_area(t)).sum();
        assert!((area - 44.0).abs() < 1e-3, "{area}");
        assert!(tris.iter().all(|t| signed_area(t) > 0.0));
        // Clockwise input gives the same result.
        let mut rev = u.to_vec();
        rev.reverse();
        let area2: f32 = triangulate(&rev).iter().map(|t| signed_area(t)).sum();
        assert!((area2 - 44.0).abs() < 1e-3);
    }

    #[test]
    fn body_falls_and_rests_on_flat_ground_with_slop_penetration() {
        let world = flat_world();
        let mut b = man(Vec2::new(0.0, 5.0));
        let dt = 1.0 / 60.0;
        let mut last = StepReport::default();
        for _ in 0..180 {
            last = b.step(&world, &rules(), dt);
        }
        // Resting: feet at -SLOP (within a hair), no vertical velocity, touching with an up normal.
        assert!((b.position.y + LINEAR_SLOP).abs() < 2e-3, "y = {}", b.position.y);
        assert!(b.velocity.y.abs() < 1e-3, "vy = {}", b.velocity.y);
        assert!(last.contacts.iter().any(|c| c.collider == 1 && c.normal.y > 0.99));
    }

    #[test]
    fn fast_fall_does_not_tunnel_through_thin_ground() {
        let thin = vec![Vec2::new(-5.0, -0.2), Vec2::new(5.0, -0.2), Vec2::new(5.0, 0.0), Vec2::new(-5.0, 0.0)];
        let world = World::build(vec![(ground_collider(7), StaticShape::Polygon(thin))]);
        let mut b = man(Vec2::new(0.0, 3.0));
        b.velocity = Vec2::new(0.0, -90.0);
        for _ in 0..30 {
            b.step(&world, &rules(), 1.0 / 60.0);
        }
        assert!(b.position.y > -0.3, "tunneled: y = {}", b.position.y);
    }

    #[test]
    fn wall_contact_reports_horizontal_normal_and_stops_motion() {
        // Wall face at x = 10 (solid for x >= 10).
        let wall = vec![Vec2::new(10.0, -10.0), Vec2::new(20.0, -10.0), Vec2::new(20.0, 50.0), Vec2::new(10.0, 50.0)];
        let mut c = ground_collider(2);
        c.walljumpable = true;
        let world = World::build(vec![(c, StaticShape::Polygon(wall)), flat_world_item()]);
        let mut b = man(Vec2::new(5.0, 0.0));
        let mut touched_wall = false;
        for _ in 0..60 {
            b.velocity.x = 12.0;
            let r = b.step(&world, &rules(), 1.0 / 60.0);
            touched_wall |= r.contacts.iter().any(|c| c.collider == 2 && c.normal.x < -0.99 && c.walljumpable);
        }
        assert!(touched_wall);
        assert!(b.position.x < 10.0 - 0.6 + 0.02, "x = {}", b.position.x);
    }

    #[test]
    fn characters_collide_with_moving_bodies_but_queries_ignore_them() {
        let mut world = flat_world();
        // A helicopter hull parked at x 10..14, 0..3, on layer 9 (an enemy's).
        let hull = vec![Vec2::new(10.0, 0.0), Vec2::new(14.0, 0.0), Vec2::new(14.0, 3.0), Vec2::new(10.0, 3.0)];
        world.set_moving(vec![(StaticCollider { id: 900, layer: 9, ..Default::default() }, vec![hull.clone()])]);
        let mut b = man(Vec2::new(5.0, 0.0));
        let mut touched = false;
        for _ in 0..60 {
            b.velocity.x = 12.0;
            let r = b.step(&world, &rules(), 1.0 / 60.0);
            touched |= r.contacts.iter().any(|c| c.collider == 900 && c.normal.x < -0.99);
        }
        assert!(touched);
        assert!(b.position.x < 10.0 - 0.6 + 0.02, "x = {}", b.position.x);
        // Line casts and overlap queries are for the static level only.
        assert!(!world.linecast(Vec2::new(9.0, 1.0), Vec2::new(15.0, 1.0), 1 << 9, true));
        // A layer the body does not collide with is passed through.
        let mut r = rules();
        r.layer_matrix[8] &= !(1 << 9);
        world.set_moving(vec![(StaticCollider { id: 900, layer: 9, ..Default::default() }, vec![hull])]);
        let mut b = man(Vec2::new(5.0, 0.0));
        for _ in 0..60 {
            b.velocity.x = 12.0;
            b.step(&world, &r, 1.0 / 60.0);
        }
        assert!(b.position.x > 14.0);
    }

    fn flat_world_item() -> (StaticCollider, StaticShape) {
        let slab = vec![Vec2::new(-50.0, -10.0), Vec2::new(50.0, -10.0), Vec2::new(50.0, 0.0), Vec2::new(-50.0, 0.0)];
        (ground_collider(1), StaticShape::Polygon(slab))
    }

    #[test]
    fn layer_matrix_can_disable_collision() {
        let world = flat_world();
        let mut r = rules();
        r.layer_matrix[8] &= !1; // layer 8 no longer collides with layer 0
        let mut b = man(Vec2::new(0.0, 1.0));
        for _ in 0..120 {
            b.step(&world, &r, 1.0 / 60.0);
        }
        assert!(b.position.y < -20.0);
    }

    #[test]
    fn trigger_overlap_is_reported_without_blocking() {
        let zone = vec![Vec2::new(-5.0, -3.0), Vec2::new(5.0, -3.0), Vec2::new(5.0, 3.0), Vec2::new(-5.0, 3.0)];
        let mut c = ground_collider(9);
        c.trigger = true;
        let world = World::build(vec![(c, StaticShape::Polygon(zone))]);
        let mut b = man(Vec2::new(0.0, 0.0));
        let r = b.step(&world, &rules(), 1.0 / 60.0);
        assert_eq!(r.triggers, vec![9]);
        assert!(r.contacts.is_empty());
    }

    #[test]
    fn overlap_circle_respects_layer_mask() {
        let world = flat_world();
        assert_eq!(world.overlap_circle(Vec2::new(0.0, 0.5), 0.6, 1), Some(1));
        assert_eq!(world.overlap_circle(Vec2::new(0.0, 0.7), 0.6, 1), None);
        assert_eq!(world.overlap_circle(Vec2::new(0.0, 0.5), 0.6, 1 << 27), None);
    }

    #[test]
    fn walking_across_triangle_seams_keeps_ground_contact() {
        // Ground made of one polygon with many collinear top vertices, so triangulation produces
        // many seams under the character's path.
        let mut pts = vec![Vec2::new(-30.0, -5.0), Vec2::new(30.0, -5.0)];
        for i in (0..=24).rev() {
            pts.push(Vec2::new(-30.0 + i as f32 * 2.5, 0.0));
        }
        let world = World::build(vec![(ground_collider(3), StaticShape::Polygon(pts))]);
        let mut b = man(Vec2::new(-20.0, 0.0));
        for _ in 0..30 {
            b.step(&world, &rules(), 1.0 / 60.0);
        }
        let mut grounded_steps = 0;
        for _ in 0..120 {
            b.velocity.x = 11.9;
            let r = b.step(&world, &rules(), 1.0 / 60.0);
            if r.contacts.iter().any(|c| c.normal.y > 0.2) {
                grounded_steps += 1;
            }
        }
        assert!(grounded_steps >= 118, "grounded in {grounded_steps}/120 steps");
        assert!((b.position.y + LINEAR_SLOP).abs() < 0.02, "y = {}", b.position.y);
    }
}
