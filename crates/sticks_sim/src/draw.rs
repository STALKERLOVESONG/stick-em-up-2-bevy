//! What the original draws and in which order: Unity's transparent render queue as this game
//! uses it.
//!
//! Every visible thing in a level (tk2d sprites, `PolygonColliderRenderer` surfaces, the
//! characters' `LinesRenderer` meshes, `FlatSprite`s, projectiles and every shadow copy) is
//! drawn with one of the project's three shaders ("Sprite", "Textured Polygon",
//! "Color Polygon"; recovered from `assets_raw/Assets/Shader/*.bytes`).
//!
//! The project renders with the legacy **Vertex Lit** path (`ProjectSettings` `m_RenderingPath:
//! 0`; every camera uses the player setting), so of each shader only the `LOD 100` SubShader's
//! `LightMode = Vertex` pass runs (the per-pixel `LOD 110` SubShader has no vertex pass):
//!
//! * `Queue = Transparent`, `ZTest Always`, `ZWrite Off`, default `Cull Back`;
//! * `Lighting On`, `ColorMaterial AmbientAndDiffuse`: fixed-function per-vertex lighting with the
//!   vertex colour as the material, clamped to 0..1 per vertex ([`vertex_lit`]);
//! * `SetTexture [_MainTex] { combine texture * primary double, texture alpha * primary alpha }`
//!   ("Color Polygon": `primary * constant [_Color] double`);
//! * `Blend SrcAlpha OneMinusSrcAlpha`; gamma colour space (`m_ActiveColorSpace: 0`).
//!
//! With depth testing off, the picture is decided purely by draw order: Unity sorts the
//! transparent queue back to front by the distance from the camera to each renderer's bounds
//! centre, and draws each renderer's triangles in index order. This module reproduces that:
//! [`DrawQueue`] collects one entry per Unity renderer, sorts them, and lays the triangles out in
//! that order (split into runs that share a texture).
//!
//! The sort key is the depth along the view direction: every level camera (and the Lobby's)
//! carries `SetOrthographicTransparencySortMode`, which sets `TransparencySortMode.Orthographic`
//! ("distance along the view direction") on enable. The art is built for it: distinct z layers
//! (-0.15 decor, -0.125 walls, -0.1 characters, 0 back) with every shadow exactly 1/64 behind its
//! caster. The cameras never rotate, so the view direction is +z.
//!
//! TODO(UNCERTAIN): Unity's tie-break between renderers at exactly equal depth; entries keep
//! their submission order (scene order for level sprites).

use unity_compat::{Quat, Vec3, mathf};
use unity_data::math::Affine;
use unity_data::sprites::{Parallax, SceneSprite, SpriteDef, SpriteLibrary, TiledLayout};

/// `Vector3.back`: the normal of every flat mesh the game builds.
pub const BACK: [f32; 3] = [0.0, 0.0, -1.0];

/// `SpriteShadowPositioner.realShadowOffset`: a shadow copy sits 1/64 behind its caster.
pub const SHADOW_DEPTH: f32 = 1.0 / 64.0;

/// Triangles in Unity world space with the per-vertex data the lit shaders read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Vertex colours (`Color32` / 255, gamma space).
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl Geometry {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Centre of the axis-aligned bounds (`Renderer.bounds.center`).
    pub fn bounds_center(&self) -> Vec3 {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        if self.positions.is_empty() {
            return Vec3::ZERO;
        }
        Vec3::new((min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5)
    }

    /// Append `other` (indices shifted).
    pub fn extend(&mut self, other: &Geometry) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        self.colors.extend_from_slice(&other.colors);
        self.indices.extend(other.indices.iter().map(|i| base + i));
    }
}

/// `Color` → `Color32` → shader input: clamped, scaled by 255 and truncated, then back to 0..1.
pub fn color32(c: [f32; 4]) -> [f32; 4] {
    c.map(|v| f32::from((v.clamp(0.0, 1.0) * 255.0) as u8) / 255.0)
}

/// Inverse transpose of an affine's linear part (how Unity transforms normals, `_World2Object`),
/// and whether the transform mirrors (negative determinant: Unity then flips the winding).
fn normal_matrix(m: &[[f32; 3]; 3]) -> ([[f32; 3]; 3], bool) {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], false);
    }
    // Cofactor matrix / det = inverse transpose.
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let n = [
        [c(1, 1, 2, 2), -c(1, 0, 2, 2), c(1, 0, 2, 1)],
        [-c(0, 1, 2, 2), c(0, 0, 2, 2), -c(0, 0, 2, 1)],
        [c(0, 1, 1, 2), -c(0, 0, 1, 2), c(0, 0, 1, 1)],
    ];
    (n.map(|row| row.map(|v| v / det)), det < 0.0)
}

fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { v.map(|c| c / l) } else { v }
}

/// A `tk2dSprite`'s mesh (`SetPositions`: definition positions × `scale`; definition normals,
/// UVs and indices; one vertex colour) placed with `world`.
pub fn sprite_geometry(def: &SpriteDef, scale: [f32; 3], color: [f32; 4], world: &Affine) -> Geometry {
    let (normal_m, mirrored) = normal_matrix(&world.m);
    let color = color32(color);
    let mut g = Geometry::default();
    for (i, p) in def.positions.iter().enumerate() {
        g.positions.push(world.transform_point([p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]]));
        g.normals.push(normalize(mul3(&normal_m, def.normals.get(i).copied().unwrap_or(BACK))));
        g.uvs.push(def.uvs.get(i).copied().unwrap_or([0.0, 0.0]));
        g.colors.push(color);
    }
    for tri in def.indices.chunks_exact(3) {
        if mirrored {
            g.indices.extend([tri[0], tri[2], tri[1]]);
        } else {
            g.indices.extend_from_slice(tri);
        }
    }
    g
}

/// A `tk2dTiledSprite`'s mesh: `tk2dSpriteGeomGen.SetTiledSpriteGeom` (whole tiles copy the
/// definition; the last column/row is cut to the remaining fraction, its UVs lerped),
/// `SetTiledSpriteIndices` (the definition's first six indices per tile) and
/// `SetSpriteVertexNormals` (definition normals blended over the oddly scaled `pMin`..`pMax`
/// box), one vertex colour, placed with `world`.
pub fn tiled_sprite_geometry(def: &SpriteDef, scale: [f32; 3], layout: &TiledLayout, color: [f32; 4], world: &Affine) -> Geometry {
    let mut g = Geometry::default();
    let dimensions = layout.dimensions;
    let untrimmed = def.untrimmed_bounds[1];
    if def.positions.len() < 4 || def.uvs.len() < 4 || def.indices.len() < 6 || untrimmed[0] == 0.0 || untrimmed[1] == 0.0 {
        return g;
    }
    let columns = (dimensions[0] * def.texel_size[0] / untrimmed[0]).ceil() as i32;
    let rows = (dimensions[1] * def.texel_size[1] / untrimmed[1]).ceil() as i32;
    if columns <= 0 || rows <= 0 {
        return g;
    }
    let size = [dimensions[0] * def.texel_size[0] * scale[0], dimensions[1] * def.texel_size[1] * scale[1]];
    let tolerance = [def.texel_size[0] * scale[0] * 0.1, def.texel_size[1] * scale[1] * 0.1];
    let mut zero = [0.0f32; 3];
    match layout.anchor {
        1 | 4 | 7 => zero[0] = 0.0 - size[0] / 2.0,
        2 | 5 | 8 => zero[0] = 0.0 - size[0],
        _ => {}
    }
    match layout.anchor {
        3..=5 => zero[1] = 0.0 - size[1] / 2.0,
        6..=8 => zero[1] = 0.0 - size[1],
        _ => {}
    }
    let p = &def.positions;
    let uv = &def.uvs;
    for k in 0..3 {
        zero[k] -= p[0][k] * scale[k];
    }
    let tile = [untrimmed[0] * scale[0], untrimmed[1] * scale[1]];
    let mut local: Vec<[f32; 3]> = Vec::with_capacity((columns * rows * 4) as usize);
    let mut cursor = [0.0f32; 2];
    for _ in 0..rows {
        cursor[0] = 0.0;
        for _ in 0..columns {
            let mut fx = 1.0f32;
            let mut fy = 1.0f32;
            if (cursor[0] + tile[0]).abs() > size[0].abs() + tolerance[0] {
                fx = size[0] % tile[0] / tile[0];
            }
            if (cursor[1] + tile[1]).abs() > size[1].abs() + tolerance[1] {
                fy = size[1] % tile[1] / tile[1];
            }
            let origin = [cursor[0] + zero[0], cursor[1] + zero[1], 0.0 + zero[2]];
            if fx != 1.0 || fy != 1.0 {
                let lo = [mathf::lerp(p[0][0], p[3][0], 0.0) * scale[0], mathf::lerp(p[0][1], p[3][1], 0.0) * scale[1], p[0][2] * scale[2]];
                let hi = [mathf::lerp(p[0][0], p[3][0], fx) * scale[0], mathf::lerp(p[0][1], p[3][1], fy) * scale[1], p[0][2] * scale[2]];
                for (x, y) in [(lo[0], lo[1]), (hi[0], lo[1]), (lo[0], hi[1]), (hi[0], hi[1])] {
                    local.push([origin[0] + x, origin[1] + y, origin[2] + lo[2]]);
                }
                if def.flipped == 1 || def.flipped == 2 {
                    // `FlipMode.Tk2d` / `TPackerCW` (the same corners).
                    let a = [mathf::lerp(uv[0][0], uv[3][0], 0.0), mathf::lerp(uv[0][1], uv[3][1], 0.0)];
                    let b = [mathf::lerp(uv[0][0], uv[3][0], fy), mathf::lerp(uv[0][1], uv[3][1], fx)];
                    g.uvs.extend([[a[0], a[1]], [a[0], b[1]], [b[0], a[1]], [b[0], b[1]]]);
                } else {
                    let a = [mathf::lerp(uv[0][0], uv[3][0], 0.0), mathf::lerp(uv[0][1], uv[3][1], 0.0)];
                    let b = [mathf::lerp(uv[0][0], uv[3][0], fx), mathf::lerp(uv[0][1], uv[3][1], fy)];
                    g.uvs.extend([[a[0], a[1]], [b[0], a[1]], [a[0], b[1]], [b[0], b[1]]]);
                }
            } else {
                for c in 0..4 {
                    local.push([origin[0] + p[c][0] * scale[0], origin[1] + p[c][1] * scale[1], origin[2] + p[c][2] * scale[2]]);
                    g.uvs.push(uv[c]);
                }
            }
            cursor[0] += tile[0];
        }
        cursor[1] += tile[1];
    }
    // `SetSpriteVertexNormals` (only with four definition normals; otherwise the mesh normals
    // stay zero).
    let def_normals = def.normals.len() == 4;
    let p_min = [p[0][0] * dimensions[0] * def.texel_size[0] * scale[0], p[0][1] * dimensions[1] * def.texel_size[1] * scale[1]];
    let p_max = [p[3][0] * dimensions[0] * def.texel_size[0] * scale[0], p[3][1] * dimensions[1] * def.texel_size[1] * scale[1]];
    let extent = [p_max[0] - p_min[0], p_max[1] - p_min[1]];
    let (normal_m, mirrored) = normal_matrix(&world.m);
    let color = color32(color);
    for v in &local {
        let mut n = [0.0f32; 3];
        if def_normals {
            let u = (v[0] - p_min[0]) / extent[0];
            let w = (v[1] - p_min[1]) / extent[1];
            let weights = [(1.0 - u) * (1.0 - w), u * (1.0 - w), (1.0 - u) * w, u * w];
            for k in 0..3 {
                n[k] = def.normals[0][k] * weights[0] + def.normals[1][k] * weights[1] + def.normals[2][k] * weights[2] + def.normals[3][k] * weights[3];
            }
        }
        g.positions.push(world.transform_point(*v));
        g.normals.push(normalize(mul3(&normal_m, n)));
        g.colors.push(color);
    }
    for t in 0..(columns * rows) as u32 {
        let i = &def.indices;
        let tri = [i[0], i[1], i[2], i[3], i[4], i[5]].map(|k| k + 4 * t);
        if mirrored {
            g.indices.extend([tri[0], tri[2], tri[1], tri[3], tri[5], tri[4]]);
        } else {
            g.indices.extend(tri);
        }
    }
    g
}

/// A `tk2dClippedSprite`'s mesh (`SetClippedSpriteGeom` / `SetClippedSpriteIndices`): the quad
/// cut to `clip` (bottom-left, top-right fractions of the untrimmed bounds, mapped into the
/// trimmed geometry and clamped), UVs lerped alike, one vertex colour, the definition's corner
/// normals, placed with `world`. Definitions that are not single quads give nothing.
pub fn clipped_sprite_geometry(def: &SpriteDef, scale: [f32; 3], clip: [[f32; 2]; 2], color: [f32; 4], world: &Affine) -> Geometry {
    let mut g = Geometry::default();
    if def.positions.len() != 4 || def.uvs.len() < 4 {
        return g;
    }
    let [bottom_left, top_right] = clip;
    let ub = def.untrimmed_bounds;
    let lo = [ub[0][0] - ub[1][0] * 0.5, ub[0][1] - ub[1][1] * 0.5];
    let hi = [ub[0][0] + ub[1][0] * 0.5, ub[0][1] + ub[1][1] * 0.5];
    let x0 = mathf::lerp(lo[0], hi[0], bottom_left[0]);
    let x1 = mathf::lerp(lo[0], hi[0], top_right[0]);
    let y0 = mathf::lerp(lo[1], hi[1], bottom_left[1]);
    let y1 = mathf::lerp(lo[1], hi[1], top_right[1]);
    let size = def.bounds[1];
    let min = [def.bounds[0][0] - size[0] * 0.5, def.bounds[0][1] - size[1] * 0.5];
    let a = [mathf::clamp01((x0 - min[0]) / size[0]), mathf::clamp01((y0 - min[1]) / size[1])];
    let b = [mathf::clamp01((x1 - min[0]) / size[0]), mathf::clamp01((y1 - min[1]) / size[1])];
    let (p0, p3) = (def.positions[0], def.positions[3]);
    let v0 = [mathf::lerp(p0[0], p3[0], a[0]) * scale[0], mathf::lerp(p0[1], p3[1], a[1]) * scale[1], p0[2] * scale[2]];
    let v1 = [mathf::lerp(p0[0], p3[0], b[0]) * scale[0], mathf::lerp(p0[1], p3[1], b[1]) * scale[1], p0[2] * scale[2]];
    let local = [[v0[0], v0[1], v0[2]], [v1[0], v0[1], v0[2]], [v0[0], v1[1], v0[2]], [v1[0], v1[1], v0[2]]];
    let (u0, u3) = (def.uvs[0], def.uvs[3]);
    if def.flipped == 1 || def.flipped == 2 {
        let c = [mathf::lerp(u0[0], u3[0], a[1]), mathf::lerp(u0[1], u3[1], a[0])];
        let d = [mathf::lerp(u0[0], u3[0], b[1]), mathf::lerp(u0[1], u3[1], b[0])];
        g.uvs.extend([[c[0], c[1]], [c[0], d[1]], [d[0], c[1]], [d[0], d[1]]]);
    } else {
        let c = [mathf::lerp(u0[0], u3[0], a[0]), mathf::lerp(u0[1], u3[1], a[1])];
        let d = [mathf::lerp(u0[0], u3[0], b[0]), mathf::lerp(u0[1], u3[1], b[1])];
        g.uvs.extend([[c[0], c[1]], [d[0], c[1]], [c[0], d[1]], [d[0], d[1]]]);
    }
    let (normal_m, mirrored) = normal_matrix(&world.m);
    let color = color32(color);
    for (i, v) in local.into_iter().enumerate() {
        // `SetSpriteVertexNormals` over the clipped quad itself: each corner gets the
        // definition's corner normal (none without four of them).
        let n = if def.normals.len() == 4 { def.normals[i] } else { [0.0; 3] };
        g.positions.push(world.transform_point(v));
        g.normals.push(normalize(mul3(&normal_m, n)));
        g.colors.push(color);
    }
    g.indices = if mirrored { vec![0, 1, 3, 2, 0, 3] } else { vec![0, 3, 1, 2, 3, 0] };
    g
}

/// `tk2dSpriteGeomGen.SetSlicedSpriteIndices`: nine cells of two triangles over a 4×4 grid
/// (the last six indices are the centre cell, left out for `borderOnly`).
const SLICED_INDICES: [u32; 54] = [
    0, 4, 1, 1, 4, 5, 1, 5, 2, 2, 5, 6, 2, 6, 3, 3, 6, 7, 4, 8, 5, 5, 8, 9, 6, 10, 7, 7, 10, 11, 8, 12, 9, 9, 12, 13, 9, 13, 10, 10, 13, 14, 10, 14, 11, 11, 14, 15,
    5, 9, 6, 6, 9, 10,
];

/// A `tk2dSlicedSprite`'s mesh (`SetSlicedSpriteGeom`): a `dimensions` (texels) box anchored
/// at `anchor` (offsets truncated to whole texels), its corners kept at the definition's size by
/// `borders` (left, bottom, right, top fractions of the definition), UVs sliced alike, scaled
/// by `scale`, one vertex colour, placed with `world`. Normals face back (the UI shader is
/// unlit). Definitions that are not single quads give nothing.
#[allow(clippy::too_many_arguments)]
pub fn sliced_sprite_geometry(def: &SpriteDef, scale: [f32; 3], dimensions: [f32; 2], borders: [f32; 4], anchor: i64, border_only: bool, color: [f32; 4], world: &Affine) -> Geometry {
    let mut g = Geometry::default();
    if def.positions.len() != 4 || def.uvs.len() < 3 {
        return g;
    }
    let [left, bottom, right, top] = borders;
    let [tx, ty] = def.texel_size;
    let p = &def.positions;
    let width = p[1][0] - p[0][0];
    let height = p[2][1] - p[0][1];
    let top_h = top * height;
    let bottom_h = bottom * height;
    let right_w = right * width;
    let left_w = left * width;
    let size = [dimensions[0] * tx, dimensions[1] * ty];
    let mut origin = [0.0f32; 2];
    match anchor {
        1 | 4 | 7 => origin[0] = -((dimensions[0] / 2.0) as i32 as f32),
        2 | 5 | 8 => origin[0] = -(dimensions[0] as i32 as f32),
        _ => {}
    }
    match anchor {
        3..=5 => origin[1] = -((dimensions[1] / 2.0) as i32 as f32),
        6..=8 => origin[1] = -(dimensions[1] as i32 as f32),
        _ => {}
    }
    origin[0] *= tx;
    origin[1] *= ty;
    let uv = &def.uvs;
    let du = [uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]];
    let dv = [uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]];
    let rows_y = [0.0, bottom_h, size[1] - top_h, size[1]];
    let rows_v = [0.0, bottom, 1.0 - top, 1.0];
    let cols_x = [0.0, left_w, size[0] - right_w, size[0]];
    let cols_u = [0.0, left, 1.0 - right, 1.0];
    let (_, mirrored) = normal_matrix(&world.m);
    let color = color32(color);
    for i in 0..4 {
        let row_uv = [uv[0][0] + dv[0] * rows_v[i], uv[0][1] + dv[1] * rows_v[i]];
        for j in 0..4 {
            let local = [(origin[0] + cols_x[j]) * scale[0], (origin[1] + rows_y[i]) * scale[1], 0.0];
            g.positions.push(world.transform_point(local));
            g.normals.push(BACK);
            g.uvs.push([row_uv[0] + du[0] * cols_u[j], row_uv[1] + du[1] * cols_u[j]]);
            g.colors.push(color);
        }
    }
    let count = if border_only { 48 } else { 54 };
    for tri in SLICED_INDICES[..count].chunks_exact(3) {
        if mirrored {
            g.indices.extend([tri[0], tri[2], tri[1]]);
        } else {
            g.indices.extend_from_slice(tri);
        }
    }
    g
}

/// A level sprite's mesh, plain or tiled.
pub fn scene_sprite_geometry(def: &SpriteDef, tiled: Option<&TiledLayout>, scale: [f32; 3], color: [f32; 4], world: &Affine) -> Geometry {
    match tiled {
        Some(layout) => tiled_sprite_geometry(def, scale, layout, color, world),
        None => sprite_geometry(def, scale, color, world),
    }
}

/// What moves a level sprite with the camera every frame (`Update`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub parallax: Option<Parallax>,
    /// `ScaleSpriteWithCameraDistance.factor`.
    pub scale_with_camera: Option<f32>,
    /// The sprite rides on a body moved by the game (a `HorizontalScroller2D` cloud): the index
    /// of that body's offset in the frame's offsets ([`Motion::apply_with`]).
    pub follow: Option<usize>,
}

/// `FakeParallax.Update` as a world translation of everything below the parallax object.
pub fn parallax_delta(p: &Parallax, camera: Vec3) -> [f32; 3] {
    let moved = fake_parallax_local_position(p, camera);
    p.parent_world.transform_vector([moved[0] - p.local_position[0], moved[1] - p.local_position[1], moved[2] - p.local_position[2]])
}

/// `g` moved by `d`.
pub fn translated(g: &Geometry, d: [f32; 3]) -> Geometry {
    let mut g = g.clone();
    if d != [0.0; 3] {
        for p in &mut g.positions {
            for k in 0..3 {
                p[k] += d[k];
            }
        }
    }
    g
}

impl Motion {
    pub fn of(sprite: &SceneSprite) -> Motion {
        Motion { parallax: sprite.parallax, scale_with_camera: sprite.scale_with_camera, follow: None }
    }

    pub fn is_still(&self) -> bool {
        self.parallax.is_none() && self.scale_with_camera.is_none() && self.follow.is_none()
    }

    /// The sprite's world transform and `scale` this frame, from its serialized ones and the
    /// main camera's position.
    pub fn apply(&self, world: &Affine, scale: [f32; 3], camera: Vec3) -> (Affine, [f32; 3]) {
        self.apply_with(world, scale, camera, &[])
    }

    /// [`Motion::apply`] with the frame's body offsets (x, y) for followed sprites.
    pub fn apply_with(&self, world: &Affine, scale: [f32; 3], camera: Vec3, offsets: &[[f32; 2]]) -> (Affine, [f32; 3]) {
        let mut world = *world;
        let mut scale = scale;
        if let Some(d) = self.follow.and_then(|i| offsets.get(i)) {
            world.t[0] += d[0];
            world.t[1] += d[1];
        }
        if let Some(p) = &self.parallax {
            let delta = parallax_delta(p, camera);
            for k in 0..3 {
                world.t[k] += delta[k];
            }
        }
        if let Some(factor) = self.scale_with_camera {
            // `minimumScale` is the scale at `Awake`.
            let s = ((0.0 - camera.z) * factor).max(scale[0]);
            scale = [s, s, 1.0];
        }
        (world, scale)
    }
}

/// `FakeParallax.Update`: the helper camera (a default perspective `Camera` at the main camera's
/// x, y and z = -10, looking along +z) projects `realPosition` at depth `fakeZ`, and the object is
/// put where that screen point lies at its own depth (`localPosition.z`); y stays at
/// `realPosition.y` when `xOnly`.
/// TODO(UNCERTAIN): written as the projection's closed form (field of view and aspect cancel);
/// Unity goes through the camera matrices, which round differently.
pub fn fake_parallax_local_position(p: &Parallax, camera: Vec3) -> [f32; 3] {
    let helper = [camera.x, camera.y, -10.0f32];
    let z = p.local_position[2];
    let ratio = (z - helper[2]) / (p.fake_z - helper[2]);
    let x = helper[0] + (p.real_position[0] - helper[0]) * ratio;
    let y = helper[1] + (p.real_position[1] - helper[1]) * ratio;
    [x, if p.x_only { p.real_position[1] } else { y }, z]
}

/// `SpriteShadowPositioner.GetShadowOffsetAndScale`: where a shadow copy lands (world x, y offset
/// from its caster) and how much it is scaled, for a light at `light` and a caster at `caster`.
pub fn shadow_offset_and_scale(light: Vec3, caster: Vec3, shadow_plane_z: f32) -> ([f32; 2], f32) {
    let vector = caster - light;
    let num = 0.0 - (caster.z - shadow_plane_z);
    let num2 = num / vector.z;
    ([vector.x * num2, vector.y * num2], 1.0 + num2)
}

// ---------------------------------------------------------------------------------------------
// The scene light
// ---------------------------------------------------------------------------------------------

/// The light as fixed-function lighting sees it: position, colour x intensity, range, and the
/// flat ambient colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderLight {
    pub position: Vec3,
    /// `Light.color × intensity`.
    pub color: [f32; 3],
    pub range: f32,
    pub ambient: [f32; 3],
    /// Point lights are the only kind any scene uses; other kinds light nothing here.
    pub point: bool,
}

/// Scale of the light and ambient colours fixed-function lighting receives: Unity hands vertex
/// lighting half-intensity colours, which the shaders' `combine ... double` restores.
///
/// Calibrated against the shipped game (Steam screenshot `ss_32e51c52...`, Skyline): the grey
/// enemies (vertex colour 120/255 on the white `Line` sprite) show 63/255 and their helmets
/// (90/255) 48/255, i.e. a lighting gain of 0.26 before the doubling. At their place relative to
/// the camera-borne light (attenuation 0.39, N.L 0.38) that is `0.5 * 0.2 + 0.5 * 2 * 0.15`;
/// full-scale inputs would give 0.50, half-scale light with full ambient 0.35.
/// TODO(UNCERTAIN): from one screenshot (JPEG); not read from Unity's code.
pub const FIXED_FUNCTION_LIGHT_SCALE: f32 = 0.5;

/// Unity's vertex-light falloff for a point light: `1 / (1 + 25 (d / range)^2)` inside the
/// range, nothing beyond it.
/// TODO(UNCERTAIN): the fixed-function attenuation constants are Unity's documented vertex-light
/// set-up (`quadAtten = 25 / range^2`, range cut-off), not read from the original.
pub fn vertex_light_attenuation(normalized_distance_sqr: f32) -> f32 {
    if normalized_distance_sqr > 1.0 {
        return 0.0;
    }
    1.0 / (1.0 + 25.0 * normalized_distance_sqr)
}

/// Fixed-function per-vertex lighting with `ColorMaterial AmbientAndDiffuse`: the vertex colour
/// lit by the global ambient and the light (Lambert), clamped to 0..1; alpha is the vertex
/// alpha.
/// TODO(UNCERTAIN): the ambient term is taken as `RenderSettings` ambient x vertex colour (no
/// extra factor).
pub fn vertex_lit(color: [f32; 4], position: [f32; 3], normal: [f32; 3], light: &ShaderLight) -> [f32; 4] {
    let mut gain = light.ambient;
    if light.point {
        let to_light = light.position - Vec3::new(position[0], position[1], position[2]);
        let d2 = to_light.sqr_magnitude() / (light.range * light.range);
        let n = Vec3::new(normal[0], normal[1], normal[2]);
        let ndl = n.dot(to_light.normalized()).max(0.0);
        let k = vertex_light_attenuation(d2) * ndl;
        for i in 0..3 {
            gain[i] += light.color[i] * k;
        }
    }
    [(color[0] * gain[0]).clamp(0.0, 1.0), (color[1] * gain[1]).clamp(0.0, 1.0), (color[2] * gain[2]).clamp(0.0, 1.0), color[3]]
}

/// What the combiner stage then makes of a texel (reference for the GPU shader):
/// `texture * primary double`, alpha `texture * primary`, clamped.
pub fn combine_double(texel: [f32; 4], lit: [f32; 4], tint: [f32; 4]) -> [f32; 4] {
    let rgb = |k: usize| (texel[k] * tint[k] * lit[k] * 2.0).clamp(0.0, 1.0);
    [rgb(0), rgb(1), rgb(2), (texel[3] * tint[3] * lit[3]).clamp(0.0, 1.0)]
}

/// The light hung under the camera (`Camera/Fix Z/Point Light` in every level).
///
/// `FixWorldZ` (execution order 500) re-pins its object's world z in `LateUpdate`, after the
/// camera moved (order 0) and after `SpriteShadowPositioner` (200) read the light. So shadows are
/// placed with the light's z still carried along by this frame's camera move, and the shaders
/// draw with the re-pinned z.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraLight {
    links: Vec<unity_data::level::LightLink>,
    /// Local position of each link after the last `LateUpdate` (they change when `FixWorldZ`
    /// writes a world position).
    local: Vec<[f32; 3]>,
}

impl CameraLight {
    pub fn new(links: Vec<unity_data::level::LightLink>) -> CameraLight {
        let local = links.iter().map(|l| l.local_position).collect();
        CameraLight { links, local }
    }

    /// World position of the light for the current local positions.
    fn position(&self, camera: Vec3) -> Vec3 {
        let mut p = camera;
        for l in &self.local {
            p = p + Vec3::new(l[0], l[1], l[2]);
        }
        p
    }

    /// Where the light is while `SpriteShadowPositioner.LateUpdate` runs (before `FixWorldZ`).
    pub fn before_fix(&self, camera: Vec3) -> Vec3 {
        self.position(camera)
    }

    /// Run the `FixWorldZ` components (parents first) and return the light's final position, the
    /// one the frame is drawn with.
    pub fn apply_fix(&mut self, camera: Vec3) -> Vec3 {
        let mut parent = camera;
        for (i, link) in self.links.iter().enumerate() {
            let mut world = parent + Vec3::new(self.local[i][0], self.local[i][1], self.local[i][2]);
            if let Some(z) = link.fix_world_z {
                world.z = z;
                self.local[i][2] = z - parent.z;
            }
            parent = world;
        }
        parent
    }
}

// ---------------------------------------------------------------------------------------------
// Level sprites
// ---------------------------------------------------------------------------------------------

/// A sprite that never moves: its mesh is built once.
#[derive(Clone, Debug)]
pub struct StaticSprite {
    pub texture: u32,
    pub geometry: Geometry,
    pub center: Vec3,
}

/// A level sprite that the camera moves (`FakeParallax`, `ScaleSpriteWithCameraDistance`): its
/// mesh is built every frame.
#[derive(Clone, Debug)]
pub struct MovingSprite {
    pub texture: u32,
    def: SpriteDef,
    tiled: Option<TiledLayout>,
    world: Affine,
    scale: [f32; 3],
    color: [f32; 4],
    motion: Motion,
}

impl MovingSprite {
    pub fn geometry(&self, camera: Vec3) -> Geometry {
        self.geometry_with(camera, &[])
    }

    /// The mesh this frame, with followed bodies at `offsets`.
    pub fn geometry_with(&self, camera: Vec3, offsets: &[[f32; 2]]) -> Geometry {
        let (world, scale) = self.motion.apply_with(&self.world, self.scale, camera, offsets);
        scene_sprite_geometry(&self.def, self.tiled.as_ref(), scale, self.color, &world)
    }
}

/// A `"Sprite Shadow"` copy of a level sprite; placed from the light every frame.
#[derive(Clone, Debug)]
pub struct LevelShadow {
    pub texture: u32,
    /// The shadow sprite's definition (normally `"<caster> shadow"`).
    def: SpriteDef,
    /// Tiled shadows of tiled casters: the caster's anchor, and dimensions scaled from the
    /// caster's by the two definitions' untrimmed sizes (`Awake`).
    tiled: Option<TiledLayout>,
    caster_world: Affine,
    caster_rotation: Quat,
    caster_scale: [f32; 3],
    caster_alpha: f32,
    caster_motion: Motion,
    plane_z: f32,
}

impl LevelShadow {
    /// `SpriteShadowPositioner.LateUpdate`: the copy sits at `(dx, dy, 1/64)` from the caster
    /// (a world direction converted with `InverseTransformDirection`, then placed through the
    /// caster's transform), scaled by the shadow scale, black with the caster's alpha. `camera`
    /// moves casters that follow it.
    pub fn geometry(&self, light: Vec3, camera: Vec3) -> Geometry {
        self.geometry_with(light, camera, &[])
    }

    /// [`LevelShadow::geometry`] with followed casters at `offsets`.
    pub fn geometry_with(&self, light: Vec3, camera: Vec3, offsets: &[[f32; 2]]) -> Geometry {
        let (caster_world, caster_scale) = self.caster_motion.apply_with(&self.caster_world, self.caster_scale, camera, offsets);
        let caster = Vec3::new(caster_world.t[0], caster_world.t[1], caster_world.t[2]);
        let (delta, k) = shadow_offset_and_scale(light, caster, self.plane_z);
        let local = self.caster_rotation.conjugate() * Vec3::new(delta[0], delta[1], SHADOW_DEPTH);
        let mut world = caster_world;
        world.t = caster_world.transform_point([local.x, local.y, local.z]);
        let scale = [caster_scale[0] * k, caster_scale[1] * k, 1.0];
        scene_sprite_geometry(&self.def, self.tiled.as_ref(), scale, [0.0, 0.0, 0.0, self.caster_alpha], &world)
    }
}

/// `SpriteShadowPositioner.LateUpdate` for a caster placed at `caster_world` (rotation
/// `caster_rotation`, `tk2dBaseSprite.scale` `caster_scale`): the shadow sprite `def` sits at the
/// light's shadow offset and 1/64 deeper (a world direction turned into the caster's frame),
/// scaled by the shadow scale, black with the caster's alpha.
#[allow(clippy::too_many_arguments)]
pub fn sprite_shadow_geometry(def: &SpriteDef, tiled: Option<&TiledLayout>, caster_world: &Affine, caster_rotation: Quat, caster_scale: [f32; 3], alpha: f32, plane_z: f32, light: Vec3) -> Geometry {
    let caster = Vec3::new(caster_world.t[0], caster_world.t[1], caster_world.t[2]);
    let (delta, k) = shadow_offset_and_scale(light, caster, plane_z);
    let local = caster_rotation.conjugate() * Vec3::new(delta[0], delta[1], SHADOW_DEPTH);
    let mut world = *caster_world;
    world.t = caster_world.transform_point([local.x, local.y, local.z]);
    scene_sprite_geometry(def, tiled, [caster_scale[0] * k, caster_scale[1] * k, 1.0], [0.0, 0.0, 0.0, alpha], &world)
}

/// The level's sprite art.
#[derive(Clone, Debug, Default)]
pub struct LevelArt {
    pub sprites: Vec<StaticSprite>,
    pub moving: Vec<MovingSprite>,
    pub shadows: Vec<LevelShadow>,
    /// Index into the scene sprite list each entry of `sprites` / `moving` / `shadows` came from.
    pub sprite_sources: Vec<usize>,
    pub moving_sources: Vec<usize>,
    pub shadow_sources: Vec<usize>,
}

impl LevelArt {
    /// Build from the scene's sprites. `library` finds a collection by GUID; `texture` gives the
    /// id of an atlas image (by file path).
    /// `skip` leaves out sprites drawn elsewhere (those of moving bodies such as helicopters).
    pub fn build<'a>(
        scene: &[SceneSprite],
        skip: impl Fn(&SceneSprite) -> bool,
        library: impl FnMut(&str) -> Option<&'a SpriteLibrary>,
        texture: impl FnMut(&std::path::Path) -> u32,
    ) -> LevelArt {
        Self::build_following(scene, skip, |_| None, library, texture)
    }

    /// [`LevelArt::build`] where `follow` names, for sprites riding on a game-moved body, that
    /// body's index in the frame offsets (see [`Motion::follow`]).
    pub fn build_following<'a>(
        scene: &[SceneSprite],
        skip: impl Fn(&SceneSprite) -> bool,
        follow: impl Fn(&SceneSprite) -> Option<usize>,
        mut library: impl FnMut(&str) -> Option<&'a SpriteLibrary>,
        mut texture: impl FnMut(&std::path::Path) -> u32,
    ) -> LevelArt {
        let mut art = LevelArt::default();
        for (source, sprite) in scene.iter().enumerate() {
            if skip(sprite) {
                continue;
            }
            let Some(lib) = library(&sprite.collection) else { continue };
            match sprite.shadow_of {
                None => {
                    let Some(def) = lib.collection.sprites.get(sprite.sprite_id) else { continue };
                    let Some(path) = lib.texture_of(def) else { continue };
                    let mut motion = Motion::of(sprite);
                    motion.follow = follow(sprite);
                    if !motion.is_still() {
                        art.moving.push(MovingSprite {
                            texture: texture(path),
                            def: def.clone(),
                            tiled: sprite.tiled,
                            world: sprite.world,
                            scale: sprite.scale,
                            color: sprite.color,
                            motion,
                        });
                        art.moving_sources.push(source);
                        continue;
                    }
                    let geometry = scene_sprite_geometry(def, sprite.tiled.as_ref(), sprite.scale, sprite.color, &sprite.world);
                    if geometry.is_empty() {
                        continue;
                    }
                    art.sprites.push(StaticSprite { texture: texture(path), center: geometry.bounds_center(), geometry });
                    art.sprite_sources.push(source);
                }
                Some((caster_index, plane_z)) => {
                    let caster = &scene[caster_index];
                    let Some(caster_lib) = library(&caster.collection) else { continue };
                    let Some(caster_def) = caster_lib.collection.sprites.get(caster.sprite_id) else { continue };
                    // `Awake` keeps the serialized sprite when it is in the caster's collection,
                    // otherwise looks up "<name> shadow" (falling back to the caster's sprite).
                    let def = if sprite.collection == caster.collection {
                        caster_lib.collection.sprites.get(sprite.sprite_id)
                    } else {
                        caster_lib.collection.sprite(&format!("{} shadow", caster_def.name)).or(Some(caster_def))
                    };
                    let Some(def) = def else { continue };
                    let Some(path) = caster_lib.texture_of(def) else { continue };
                    // `Awake`: a tiled shadow takes the caster's anchor and its dimensions in
                    // shadow-sprite units.
                    let tiled = caster.tiled.map(|c| {
                        let ppm = caster_lib.collection.pixels_per_meter;
                        let caster_size = [caster_def.untrimmed_bounds[1][0] * ppm, caster_def.untrimmed_bounds[1][1] * ppm];
                        let ratio = [c.dimensions[0] / caster_size[0], c.dimensions[1] / caster_size[1]];
                        let size = [def.untrimmed_bounds[1][0] * ppm, def.untrimmed_bounds[1][1] * ppm];
                        TiledLayout { dimensions: [size[0] * ratio[0], size[1] * ratio[1]], anchor: c.anchor }
                    });
                    art.shadows.push(LevelShadow {
                        texture: texture(path),
                        def: def.clone(),
                        tiled,
                        caster_motion: Motion { follow: follow(caster), ..Motion::of(caster) },
                        caster_world: caster.world,
                        caster_rotation: Quat::from_array(caster.world_rotation),
                        caster_scale: caster.scale,
                        caster_alpha: caster.color[3],
                        plane_z,
                    });
                    art.shadow_sources.push(source);
                }
            }
        }
        art
    }
}

// ---------------------------------------------------------------------------------------------
// Built-in meshes
// ---------------------------------------------------------------------------------------------

/// A built-in mesh (`unity_data::scenery::BuiltinMesh`) placed by `world`, white, with the
/// mesh's UVs and normals turned into the world. Unity's "Plane" is 10 × 10 in local x/z with
/// 11 × 11 vertices (normal +y); the "Quad" 1 × 1 in local x/y (normal -z). Triangles are
/// emitted clockwise as seen along the normal's opposite (Unity's front faces).
/// TODO(UNCERTAIN): the built-in plane's UV orientation (u, v = 1 - (x + 5) / 10, 1 - (z + 5) / 10)
/// and vertex order are from memory of Unity's primitive; only untextured planes ("Color
/// Polygon") are drawn with it so far.
pub fn builtin_mesh_geometry(mesh: unity_data::scenery::BuiltinMesh, world: &Affine) -> Geometry {
    use unity_data::scenery::BuiltinMesh;
    let mut g = Geometry::default();
    let (n, size, normal): (usize, f32, [f32; 3]) = match mesh {
        BuiltinMesh::Plane => (11, 10.0, [0.0, 1.0, 0.0]),
        BuiltinMesh::Quad => (2, 1.0, [0.0, 0.0, -1.0]),
    };
    let world_normal = {
        let v = world.transform_vector(normal);
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    for j in 0..n {
        for i in 0..n {
            let a = i as f32 / (n - 1) as f32;
            let b = j as f32 / (n - 1) as f32;
            let local = match mesh {
                BuiltinMesh::Plane => [(a - 0.5) * size, 0.0, (b - 0.5) * size],
                BuiltinMesh::Quad => [(a - 0.5) * size, (b - 0.5) * size, 0.0],
            };
            g.positions.push(world.transform_point(local));
            g.normals.push(world_normal);
            g.uvs.push(match mesh {
                BuiltinMesh::Plane => [1.0 - a, 1.0 - b],
                BuiltinMesh::Quad => [a, b],
            });
            g.colors.push([1.0; 4]);
        }
    }
    // Front faces: clockwise seen from the side the normal points to.
    let toward = [-world_normal[0], -world_normal[1], -world_normal[2]];
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let k = (j * n + i) as u32;
            let quad = [k, k + 1, k + n as u32 + 1, k + n as u32];
            for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                let (p0, p1, p2) = (g.positions[tri[0] as usize], g.positions[tri[1] as usize], g.positions[tri[2] as usize]);
                let u = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                let v = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                let cross = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                // The level's front faces (sprites, surfaces facing -z) are clockwise in x/y, i.e.
                // their (right-handed) cross product points along the normal, against `toward`.
                let dot = cross[0] * toward[0] + cross[1] * toward[1] + cross[2] * toward[2];
                if dot > 0.0 { g.indices.extend([tri[0], tri[2], tri[1]]) } else { g.indices.extend(tri) }
            }
        }
    }
    g
}

// ---------------------------------------------------------------------------------------------
// Polygon surfaces
// ---------------------------------------------------------------------------------------------

/// A `PolygonColliderRenderer` path as drawn ("Textured Polygon"): the triangulation reversed to
/// Unity's clockwise front faces, `Vector3.back` normals, the mesh's colours times `color`.
pub fn surface_geometry(surface: &unity_data::level::Surface, color: [f32; 4]) -> Geometry {
    surface_geometry_placed(surface, color, [0.0, 0.0, 0.0], 1.0)
}

fn surface_geometry_placed(surface: &unity_data::level::Surface, color: [f32; 4], offset: [f32; 3], scale: f32) -> Geometry {
    let mut placed = surface.clone();
    placed.origin = [surface.origin[0] + offset[0], surface.origin[1] + offset[1], surface.origin[2] + offset[2]];
    placed.axis_x = surface.axis_x.map(|v| v * scale);
    placed.axis_y = surface.axis_y.map(|v| v * scale);
    let mesh = crate::surface::surface_mesh(&placed);
    let mut g = Geometry::default();
    for ((p, uv), c) in mesh.positions.iter().zip(&mesh.uvs).zip(&mesh.colors) {
        g.positions.push(*p);
        g.normals.push(BACK);
        g.uvs.push(*uv);
        // The mesh colours (opaque white inside, alpha 0 on the border ring's outer edge) times
        // the material colour.
        g.colors.push(color32([color[0] * c[0], color[1] * c[1], color[2] * c[2], color[3] * c[3]]));
    }
    for tri in mesh.indices.chunks_exact(3) {
        g.indices.extend([tri[0], tri[2], tri[1]]);
    }
    g
}

/// The material's `_MainTex` scale and offset (`m_TexEnvs`), which the fixed-function
/// `SetTexture [_MainTex]` stage applies to the mesh UVs (`uv * scale + offset`).
pub fn apply_texture_transform(g: &mut Geometry, scale: [f32; 2], offset: [f32; 2]) {
    if scale == [1.0, 1.0] && offset == [0.0, 0.0] {
        return;
    }
    for uv in &mut g.uvs {
        *uv = [uv[0] * scale[0] + offset[0], uv[1] * scale[1] + offset[1]];
    }
}

/// `PolygonColliderRendererShadowPositioner.LateUpdate`: the "Polygon Shadow" copy of a surface
/// (black `Color Polygon` material), moved by the shadow offset, 0.03 deeper, scaled by the
/// shadow scale about the object's origin.
pub fn surface_shadow_geometry(surface: &unity_data::level::Surface, light: Vec3) -> Option<Geometry> {
    let plane = surface.shadow_plane_z?;
    let source = Vec3::new(surface.origin[0], surface.origin[1], surface.origin[2]);
    let (delta, k) = shadow_offset_and_scale(light, source, plane);
    Some(surface_geometry_placed(surface, [0.0, 0.0, 0.0, 1.0], [delta[0], delta[1], 0.03], k))
}

// ---------------------------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------------------------

/// Where an entry's triangles come from: built once, or built this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Static(usize),
    Frame(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub texture: u32,
    /// `Renderer.bounds.center`.
    pub center: Vec3,
    pub source: Source,
    /// The renderer's sorting layer (its rank in the project's list) and `sortingOrder`: Unity
    /// sorts transparent renderers by these before their depth. 0 for the levels' renderers,
    /// which all sit on one layer.
    pub sorting: (i32, i32),
}

/// One frame's transparent queue.
#[derive(Default, Debug)]
pub struct DrawQueue {
    pub entries: Vec<Entry>,
    /// Geometry built this frame ([`Source::Frame`]).
    pub frame: Vec<Geometry>,
}

/// The camera's view of the world, for culling renderers outside it (Unity skips renderers whose
/// bounds are outside the frustum; they would not change the picture).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub camera: Vec3,
    /// `tan(fov / 2)` vertically and horizontally.
    pub tan_half: [f32; 2],
}

impl View {
    /// Whether a box of half size `extent` around `center` may be visible (looking along +z).
    pub fn may_see(&self, center: Vec3, extent: f32) -> bool {
        let depth = center.z - self.camera.z + extent;
        if depth <= 0.0 {
            return false;
        }
        let half_w = depth * self.tan_half[0] + extent;
        let half_h = depth * self.tan_half[1] + extent;
        (center.x - self.camera.x).abs() <= half_w && (center.y - self.camera.y).abs() <= half_h
    }
}

impl DrawQueue {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.frame.clear();
    }

    /// Queue geometry built this frame as one renderer.
    pub fn push_frame(&mut self, texture: u32, geometry: Geometry) {
        if geometry.is_empty() {
            return;
        }
        self.push_frame_sorted(texture, geometry, (0, 0));
    }

    /// [`Self::push_frame`] for a renderer on sorting layer rank `sorting.0` with
    /// `sortingOrder` `sorting.1`.
    pub fn push_frame_sorted(&mut self, texture: u32, geometry: Geometry, sorting: (i32, i32)) {
        if geometry.is_empty() {
            return;
        }
        let center = geometry.bounds_center();
        self.entries.push(Entry { texture, center, source: Source::Frame(self.frame.len()), sorting });
        self.frame.push(geometry);
    }

    /// Queue a static renderer.
    pub fn push_static(&mut self, texture: u32, center: Vec3, index: usize) {
        self.entries.push(Entry { texture, center, source: Source::Static(index), sorting: (0, 0) });
    }

    /// The transparent sort: by sorting layer and order, then deepest bounds centre (along the
    /// view axis, +z) first, stable for ties.
    pub fn sort(&mut self, camera: Vec3) {
        self.entries.sort_by(|a, b| {
            let da = a.center.z - camera.z;
            let db = b.center.z - camera.z;
            a.sorting.cmp(&b.sorting).then(db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal))
        });
    }

    /// Lay the sorted entries out as consecutive runs that share a texture: drawing the runs in
    /// order, each run's triangles in order, reproduces Unity's draw order. With a light, the
    /// vertex colours become the per-vertex lit colours ([`vertex_lit`]).
    pub fn runs(&self, statics: &[Geometry], light: Option<&ShaderLight>) -> Vec<(u32, Geometry)> {
        let mut runs: Vec<(u32, Geometry)> = Vec::new();
        for entry in &self.entries {
            let geometry = match entry.source {
                Source::Static(i) => &statics[i],
                Source::Frame(i) => &self.frame[i],
            };
            let start = match runs.last_mut() {
                Some((texture, run)) if *texture == entry.texture => {
                    let start = run.colors.len();
                    run.extend(geometry);
                    start
                }
                _ => {
                    let mut run = Geometry::default();
                    run.extend(geometry);
                    runs.push((entry.texture, run));
                    0
                }
            };
            if let (Some(light), Some((_, run))) = (light, runs.last_mut()) {
                for i in start..run.colors.len() {
                    run.colors[i] = vertex_lit(run.colors[i], run.positions[i], run.normals[i], light);
                }
            }
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_def() -> SpriteDef {
        SpriteDef {
            name: "q".into(),
            positions: vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![BACK; 4],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            indices: vec![0, 3, 1, 2, 3, 0],
            material_guid: "m".into(),
            texel_size: [0.5, 0.5],
            untrimmed_bounds: [[0.0; 3], [2.0, 2.0, 0.0]],
            bounds: [[0.0; 3], [2.0, 2.0, 0.0]],
            flipped: 0,
        }
    }

    #[test]
    fn sliced_sprite_keeps_its_corners() {
        // 2×2 units = 4×4 texels; a 20×10 texel box centred (anchor 4), quarter borders.
        let g = sliced_sprite_geometry(&quad_def(), [1.0; 3], [20.0, 10.0], [0.25; 4], 4, false, [1.0; 4], &Affine::IDENTITY);
        assert_eq!(g.positions.len(), 16);
        assert_eq!(g.indices.len(), 54);
        assert_eq!(g.positions[0], [-5.0, -2.5, 0.0]);
        assert_eq!(g.positions[15], [5.0, 2.5, 0.0]);
        // The corner cells keep the definition's border size (0.25 × 2 units).
        assert_eq!(g.positions[5], [-4.5, -2.0, 0.0]);
        assert_eq!(g.uvs[5], [0.25, 0.25]);
        assert_eq!(g.uvs[15], [1.0, 1.0]);
        let border = sliced_sprite_geometry(&quad_def(), [1.0; 3], [20.0, 10.0], [0.25; 4], 0, true, [1.0; 4], &Affine::IDENTITY);
        assert_eq!(border.indices.len(), 48);
        assert_eq!(border.positions[0], [0.0, 0.0, 0.0], "lower-left anchor");
    }

    #[test]
    fn builtin_plane_turned_to_the_camera() {
        // Japan's sky plane: rotated -90° about x, scaled 20 × 20.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let world = Affine::from_trs([0.0, 0.0, 0.4], (h, 0.0, 0.0, -h), [20.0, 1.0, 20.0]);
        let g = builtin_mesh_geometry(unity_data::scenery::BuiltinMesh::Plane, &world);
        assert_eq!(g.positions.len(), 121);
        assert_eq!(g.indices.len(), 600);
        // It spans ±100 in x and y at z 0.4, facing -z.
        let (min_y, max_y) = g.positions.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        assert!((min_y + 100.0).abs() < 1e-3 && (max_y - 100.0).abs() < 1e-3);
        assert!(g.positions.iter().all(|p| (p[2] - 0.4).abs() < 1e-4));
        assert!((g.normals[0][2] + 1.0).abs() < 1e-5);
        // Clockwise in x/y like the level's other front faces.
        for t in g.indices.chunks(3) {
            let (a, b, c) = (g.positions[t[0] as usize], g.positions[t[1] as usize], g.positions[t[2] as usize]);
            assert!((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) < 0.0);
        }
    }

    #[test]
    fn sprite_geometry_scales_places_and_mirrors_winding() {
        let world = Affine::from_trs([10.0, 5.0, -0.1], (0.0, 0.0, 0.0, 1.0), [1.0, 1.0, 1.0]);
        let g = sprite_geometry(&quad_def(), [2.0, 3.0, 1.0], [1.0, 0.5, 0.0, 1.0], &world);
        assert_eq!(g.positions[3], [12.0, 8.0, -0.1]);
        assert_eq!(g.indices, vec![0, 3, 1, 2, 3, 0]);
        assert_eq!(g.colors[0], [1.0, 127.0 / 255.0, 0.0, 1.0], "Color32 truncation");
        assert_eq!(g.normals[0], BACK);
        // A mirrored transform flips the winding, and the normal follows the mirror.
        let mirrored = Affine::from_trs([0.0; 3], (0.0, 0.0, 0.0, 1.0), [-1.0, 1.0, 1.0]);
        let m = sprite_geometry(&quad_def(), [1.0; 3], [1.0; 4], &mirrored);
        assert_eq!(&m.indices[..3], &[0, 1, 3]);
        assert_eq!(m.normals[0], BACK);
        // Turned half way about y (a flipped `FlatSprite`): the front normal faces away.
        let turned = Affine::from_trs([0.0; 3], (0.0, 1.0, 0.0, 0.0), [1.0, 1.0, 1.0]);
        let t = sprite_geometry(&quad_def(), [1.0; 3], [1.0; 4], &turned);
        assert!((t.normals[0][2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tiled_sprite_repeats_and_cuts_the_last_tile() {
        // Tiles 2 units (4 texels) wide; 10 x 4 texels = 2.5 x 1 tiles, anchored lower left.
        let layout = TiledLayout { dimensions: [10.0, 4.0], anchor: 0 };
        let g = tiled_sprite_geometry(&quad_def(), [1.0; 3], &layout, [1.0; 4], &Affine::IDENTITY);
        assert_eq!((g.positions.len(), g.indices.len()), (12, 18));
        assert_eq!(g.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(g.positions[7], [4.0, 2.0, 0.0]);
        // The third tile is cut at half: x 4..5, UVs 0..0.5.
        assert_eq!(g.positions[8], [4.0, 0.0, 0.0]);
        assert_eq!(g.positions[11], [5.0, 2.0, 0.0]);
        assert_eq!(g.uvs[11], [0.5, 1.0]);
        assert_eq!(&g.indices[12..], &[8, 11, 9, 10, 11, 8]);
        assert!(g.normals.iter().all(|n| (n[2] + 1.0).abs() < 1e-6));
        // Middle centre: centred on the origin; scale stretches everything.
        let centred = TiledLayout { dimensions: [10.0, 4.0], anchor: 4 };
        let c = tiled_sprite_geometry(&quad_def(), [2.0, 2.0, 1.0], &centred, [1.0; 4], &Affine::IDENTITY);
        assert_eq!(c.positions[0], [-5.0, -2.0, 0.0]);
        assert_eq!(c.positions[11], [5.0, 2.0, 0.0]);
    }

    #[test]
    fn clipped_sprite_keeps_the_clipped_fraction_of_the_quad() {
        // The quad spans -1..1 with UVs 0..1; clipping x to 0.25..0.75 keeps the middle half.
        let g = clipped_sprite_geometry(&quad_def(), [2.0, 1.0, 1.0], [[0.25, 0.0], [0.75, 1.0]], [1.0, 0.0, 0.0, 0.5], &Affine::IDENTITY);
        assert_eq!(g.positions, vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 1.0, 0.0], [1.0, 1.0, 0.0]]);
        assert_eq!(g.uvs, vec![[0.25, 0.0], [0.75, 0.0], [0.25, 1.0], [0.75, 1.0]]);
        assert_eq!(g.indices, vec![0, 3, 1, 2, 3, 0]);
        assert_eq!(g.colors[0], [1.0, 0.0, 0.0, 127.0 / 255.0]);
        // Nothing left (health 0): a zero-width quad at the left edge.
        let empty = clipped_sprite_geometry(&quad_def(), [1.0; 3], [[0.0, 0.0], [0.0, 1.0]], [1.0; 4], &Affine::IDENTITY);
        assert_eq!(empty.positions[1][0], -1.0);
    }

    #[test]
    fn fake_parallax_projects_through_the_helper_camera() {
        let p = Parallax {
            object: 1,
            real_position: [500.0, -86.0],
            fake_z: 50.0,
            x_only: false,
            local_position: [81.3, -21.2, 0.2],
            parent_world: Affine::IDENTITY,
        };
        let moved = fake_parallax_local_position(&p, Vec3::new(100.0, 20.0, -30.0));
        assert!((moved[0] - 168.0).abs() < 1e-3 && (moved[1] - 1.98).abs() < 1e-3 && moved[2] == 0.2, "{moved:?}");
        // At the real position the object stays put whatever its depth.
        let still = fake_parallax_local_position(&p, Vec3::new(500.0, -86.0, -30.0));
        assert_eq!([still[0], still[1]], [500.0, -86.0]);
        let x_only = fake_parallax_local_position(&Parallax { x_only: true, ..p }, Vec3::new(100.0, 20.0, -30.0));
        assert_eq!(x_only[1], -86.0);
        // The sprite moves with its parallax object; the camera-scaled one grows past its minimum.
        let motion = Motion { parallax: Some(p), scale_with_camera: Some(0.045), follow: None };
        let world = Affine::from_trs([81.3, -41.2, 0.2], (0.0, 0.0, 0.0, 1.0), [1.0; 3]);
        let (w, s) = motion.apply(&world, [2.0, 2.0, 1.0], Vec3::new(100.0, 20.0, -60.0));
        assert!((w.t[0] - (81.3 + moved[0] - 81.3)).abs() < 1e-3);
        assert!((s[0] - 2.7).abs() < 1e-5 && s[2] == 1.0);
        let (_, near) = motion.apply(&world, [2.0, 2.0, 1.0], Vec3::new(100.0, 20.0, -20.0));
        assert_eq!(near, [2.0, 2.0, 1.0]);
    }

    #[test]
    fn shadow_offset_matches_the_recovered_formula() {
        // Light in front of and below-left of a caster at z = -0.125, shadow plane z = 0.
        let light = Vec3::new(-19.63, -25.4, -18.2);
        let caster = Vec3::new(0.0, 0.0, -0.125);
        let (d, k) = shadow_offset_and_scale(light, caster, 0.0);
        let num2 = 0.125 / (-0.125 + 18.2);
        assert!((d[0] - 19.63 * num2).abs() < 1e-6 && (d[1] - 25.4 * num2).abs() < 1e-6);
        assert!((k - (1.0 + num2)).abs() < 1e-6);
        assert!(d[0] > 0.0 && d[1] > 0.0, "shadows fall away from the light");
    }

    #[test]
    fn camera_light_carries_z_until_fix_world_z() {
        use unity_data::level::LightLink;
        let mut light = CameraLight::new(vec![
            LightLink { local_position: [0.0, 0.0, 0.0], fix_world_z: Some(-23.7) },
            LightLink { local_position: [-19.63, -25.4, 5.5], fix_world_z: None },
        ]);
        let cam = Vec3::new(100.0, 10.0, -40.0);
        // First frame: Fix Z still sits on the camera.
        let before = light.before_fix(cam);
        assert!((before.z - (-40.0 + 5.5)).abs() < 1e-5);
        let drawn = light.apply_fix(cam);
        assert!((drawn.x - 80.37).abs() < 1e-4 && (drawn.z - (-23.7 + 5.5)).abs() < 1e-5);
        // The camera moves back 2: shadows see the light carried 2 back, the drawing re-pinned.
        let cam2 = Vec3::new(101.0, 10.0, -42.0);
        assert!((light.before_fix(cam2).z - (-18.2 - 2.0)).abs() < 1e-4);
        assert!((light.apply_fix(cam2).z - (-18.2)).abs() < 1e-4);
    }

    #[test]
    fn vertex_light_falloff_and_range() {
        assert_eq!(vertex_light_attenuation(0.0), 1.0);
        assert!((vertex_light_attenuation(0.04) - 0.5).abs() < 1e-6);
        assert!((vertex_light_attenuation(1.0) - 1.0 / 26.0).abs() < 1e-6, "no fade inside the range");
        assert_eq!(vertex_light_attenuation(1.01), 0.0, "cut off beyond the range");
    }

    #[test]
    fn vertex_lighting_is_ambient_plus_lambert_clamped_then_doubled() {
        let light = ShaderLight { position: Vec3::new(0.0, 0.0, -10.0), color: [2.0; 3], range: 100.0, ambient: [0.2; 3], point: true };
        let lit = vertex_lit([0.5, 0.5, 0.5, 0.8], [0.0; 3], BACK, &light);
        let want = (0.5 * (0.2 + 2.0 * vertex_light_attenuation(0.01))).min(1.0);
        assert!((lit[0] - want).abs() < 1e-5, "{lit:?}");
        assert_eq!(lit[3], 0.8, "alpha is the vertex alpha");
        // Facing away from the light: ambient only.
        let away = vertex_lit([0.5; 4], [0.0; 3], [0.0, 0.0, 1.0], &light);
        assert!((away[0] - 0.1).abs() < 1e-6);
        // A white vertex next to the light saturates per vertex before the combiner doubles.
        let hot = vertex_lit([1.0; 4], [0.0; 3], BACK, &light);
        assert_eq!(hot[0], 1.0);
        assert_eq!(combine_double([0.4, 0.6, 1.0, 0.5], hot, [1.0; 4]), [0.8, 1.0, 1.0, 0.5]);
    }

    #[test]
    fn queue_sorts_back_to_front_and_merges_runs() {
        let mut q = DrawQueue::default();
        let quad = |x: f32| sprite_geometry(&quad_def(), [1.0; 3], [1.0; 4], &Affine::from_trs([x, 0.0, 0.0], (0.0, 0.0, 0.0, 1.0), [1.0; 3]));
        let quad_at = |x: f32, z: f32| sprite_geometry(&quad_def(), [1.0; 3], [1.0; 4], &Affine::from_trs([x, 0.0, z], (0.0, 0.0, 0.0, 1.0), [1.0; 3]));
        q.push_frame(1, quad_at(0.0, -0.15)); // nearest
        q.push_frame(1, quad_at(30.0, 0.0)); // deepest
        q.push_frame(2, quad_at(10.0, -0.1));
        // A shadow copy 1/64 behind the nearest one stays behind it, even though it sits off
        // towards the camera's side.
        q.push_frame(2, quad_at(-0.2, -0.15 + SHADOW_DEPTH));
        q.sort(Vec3::new(-20.0, 0.0, -20.0));
        let order: Vec<Source> = q.entries.iter().map(|e| e.source).collect();
        assert_eq!(order, vec![Source::Frame(1), Source::Frame(2), Source::Frame(3), Source::Frame(0)]);
        let runs = q.runs(&[], None);
        assert_eq!(runs.iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 2, 1]);
        let _ = quad;
    }

    #[test]
    fn culling_keeps_what_the_frustum_can_hold() {
        let view = View { camera: Vec3::new(0.0, 0.0, -20.0), tan_half: [0.577 * 16.0 / 9.0, 0.577] };
        assert!(view.may_see(Vec3::new(0.0, 0.0, 0.0), 1.0));
        assert!(view.may_see(Vec3::new(20.0, 0.0, 0.0), 1.0));
        assert!(!view.may_see(Vec3::new(40.0, 0.0, 0.0), 1.0));
        assert!(!view.may_see(Vec3::new(0.0, 0.0, -30.0), 1.0), "behind the camera");
    }
}
