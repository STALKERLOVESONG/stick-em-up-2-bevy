//! What the `Man` looks like: stick-figure lines and flat sprites derived from a solved rig.
//!
//! The original has no skinned mesh for the character. `LinesToChildren` draws a thick round-capped
//! line from each bone to its unskipped children (`LinesRenderer` batches them into one mesh and
//! adds a black shadow copy of every line), the head is one very wide line, and the helmet and
//! weapons are `FlatSprite`s that follow bones. This module reproduces the geometry those
//! components feed into rendering; the game layer turns it into meshes.

use unity_compat::{Quat, Vec2, Vec3};
use unity_data::rig::Rig;

use crate::pose::{CharacterRigModel, RigPose};

/// `LinesRenderer.realShadowOffset`: shadow lines sit slightly behind their line.
const SHADOW_Z_OFFSET: f32 = 0.003;

/// Colours of one character (`CharacterColor`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterPalette {
    pub body: [u8; 4],
    pub face: [u8; 4],
    pub helmet: [f32; 4],
}

impl CharacterPalette {
    /// The prefab's colours.
    pub fn of_rig(rig: &Rig) -> CharacterPalette {
        CharacterPalette { body: rig.colors.body_color, face: rig.colors.face_color, helmet: rig.colors.helmet_color }
    }
}

/// `Color` → `Color32`: each channel clamped to 0..1, scaled by 255 and truncated.
pub(crate) fn to_color32(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
}

/// `CharacterColors.DarkerFaceColor(f)`.
pub fn darker_face_color(f: f32) -> [f32; 4] {
    let num = 3.0 * f;
    [(168.8 + 38.5 * num) / 255.0, (122.5 + 32.1 * num) / 255.0, (96.7 + 26.3 * num) / 255.0, 1.0]
}

/// `CharacterColors.LighterFaceColor(f)`.
pub fn lighter_face_color(f: f32) -> [f32; 4] {
    let f = (f + 1.0) * 0.8 - 1.0;
    let num = 3.0 * f;
    [(224.3 + 9.6 * num) / 255.0, (193.1 + 17.0 * num) / 255.0, (177.6 + 21.0 * num) / 255.0, 1.0]
}

/// `CharacterColors.RandomFaceColor()`, stored into `CharacterColor.faceColor` (a `Color32`).
///
/// The original picks the shade with `Random.Range(0, 1) == 0`: the integer range is half-open,
/// so it is always 0 and every random face is a darker one. The draw still advances the
/// generator. Preserved as recovered.
pub fn random_face_color(rng: &mut crate::rng::GameRng) -> [u8; 4] {
    let f = rng.range_f32(-1.0, 1.0);
    let color = if rng.range_int(0, 1) == 0 { darker_face_color(f) } else { lighter_face_color(f) };
    to_color32(color)
}

impl CharacterPalette {
    /// `FlashCharacterOnDamage.GetHitVersion` applied to the body and helmet: every channel
    /// brightened by 0.2 (the face is not flashed). The body goes through `Color` and back to
    /// `Color32`, which clamps and truncates; the helmet stays a float colour, and its alpha gains
    /// the 1.0 of `new Color(0.2, 0.2, 0.2)`.
    pub fn hit_version(&self) -> CharacterPalette {
        let body = self.body.map(|c| (((f32::from(c) / 255.0) + 0.2).clamp(0.0, 1.0) * 255.0) as u8);
        let mut helmet = self.helmet.map(|c| c + 0.2);
        helmet[3] = self.helmet[3] + 1.0;
        CharacterPalette { body, face: self.face, helmet }
    }
}

/// World transform of the object that carries the `Animator` (the `Man` root).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyPlacement {
    pub position: Vec3,
    pub rotation: Quat,
}

/// One capsule: a line of `width` between `a` and `b` with round caps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickLine {
    pub a: Vec3,
    pub b: Vec3,
    pub width: f32,
    pub color: [u8; 4],
    /// `LinesRenderer.Line.shadowPlaneZ`: the plane its shadow is cast onto.
    pub shadow_plane_z: f32,
}

/// A sprite following a bone (`FlatSprite`), already in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatSpriteInstance {
    pub bone: usize,
    pub sprite_name: String,
    pub position: Vec3,
    /// The sprite is turned around its vertical axis when the bone faces away.
    pub flipped: bool,
    /// Roll in degrees about the view axis (the bone's `eulerAngles.z`).
    pub roll_degrees: f32,
    pub scale: [f32; 3],
    pub color: [f32; 4],
    pub shadow_plane_z: f32,
    /// Where the black "<name> shadow" copy lands relative to the sprite (world x, y) and how
    /// much larger it is (`SpriteShadowPositioner`; the copy also sits 1/64 behind). Zero offset
    /// and scale 1 without a light.
    pub shadow_offset: [f32; 2],
    pub shadow_scale: f32,
}

/// Lines and sprites to draw for one character this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharacterVisual {
    /// Black shadow capsules, drawn first.
    pub shadows: Vec<StickLine>,
    pub lines: Vec<StickLine>,
    pub sprites: Vec<FlatSpriteInstance>,
}

/// `SpriteShadowPositioner.GetShadowOffsetAndScale`: where a shadow cast onto the plane
/// `shadow_plane_z` lands relative to its caster, and how much larger it is.
pub fn shadow_offset_and_scale(light: Vec3, caster: Vec3, shadow_plane_z: f32) -> ([f32; 2], f32) {
    let v = caster - light;
    let num = -(caster.z - shadow_plane_z);
    let k = num / v.z;
    ([v.x * k, v.y * k], 1.0 + k)
}

impl CharacterRigModel {
    /// Lines and sprites for a solved pose.
    ///
    /// `active` says whether a bone's sprite is shown (weapons are only visible while
    /// equipped); lines are drawn for every bone that has a `LinesToChildren`.
    pub fn visual(
        &self,
        solved: &RigPose,
        placement: &BodyPlacement,
        palette: &CharacterPalette,
        light: Option<Vec3>,
        active: &dyn Fn(usize) -> bool,
    ) -> CharacterVisual {
        let rig = self.rig();
        let world = |bone: usize| -> (Vec3, Quat) {
            let (p, q) = solved.world[bone];
            (placement.position + placement.rotation * p, placement.rotation * q)
        };
        let mut visual = CharacterVisual::default();
        let root = placement.position;
        for (index, bone) in rig.bones.iter().enumerate() {
            if let Some(spec) = &bone.lines {
                let color = if index == rig.colors.face {
                    palette.face
                } else if rig.colors.body.contains(&index) {
                    palette.body
                } else {
                    spec.color
                };
                // `LinesToChildren.LateUpdate`: positions relative to the root, with the layer
                // offset `z` along the root's local x (or absolute for `dontRotateZ`).
                let mut a = world(index).0 - root;
                if spec.dont_rotate_z {
                    a.z = spec.z;
                } else {
                    a.z = 0.0;
                    a = a + placement.rotation * Vec3::new(spec.z, 0.0, 0.0);
                }
                let (shadow_delta, shadow_scale) = match light {
                    Some(light) => shadow_offset_and_scale(light, root, spec.shadow_plane_z),
                    None => ([0.0, 0.0], 1.0),
                };
                for &child in &bone.children {
                    if spec.skip_children.contains(&child) {
                        continue;
                    }
                    let mut b = world(child).0 - root;
                    b.z = a.z;
                    let line = StickLine { a: a + root, b: b + root, width: spec.width, color, shadow_plane_z: spec.shadow_plane_z };
                    visual.lines.push(line);
                    let shadow_color = [0, 0, 0, color[3]];
                    visual.shadows.push(StickLine {
                        a: Vec3::new(a.x + shadow_delta[0], a.y + shadow_delta[1], a.z + SHADOW_Z_OFFSET) + root,
                        b: Vec3::new(b.x + shadow_delta[0], b.y + shadow_delta[1], b.z + SHADOW_Z_OFFSET) + root,
                        width: spec.width * shadow_scale,
                        color: shadow_color,
                        shadow_plane_z: spec.shadow_plane_z,
                    });
                }
            }
            if let Some(sprite) = &bone.flat_sprite {
                if !active(index) {
                    continue;
                }
                let (position, rotation) = world(index);
                let euler = rotation.euler_angles_degrees();
                let color = if index == rig.colors.helmet { palette.helmet } else { sprite.color };
                let position = Vec3::new(position.x, position.y, root.z + sprite.root_relative_z);
                let (shadow_offset, shadow_scale) = match light {
                    Some(light) => shadow_offset_and_scale(light, position, sprite.shadow_plane_z),
                    None => ([0.0, 0.0], 1.0),
                };
                visual.sprites.push(FlatSpriteInstance {
                    bone: index,
                    sprite_name: sprite.sprite_name.clone(),
                    // `FlatSprite.LateUpdate`: z is the root's plus `rootRelativeZ`.
                    position,
                    flipped: euler.y >= 90.0 && euler.y < 270.0,
                    roll_degrees: euler.z,
                    scale: sprite.scale,
                    color,
                    shadow_plane_z: sprite.shadow_plane_z,
                    shadow_offset,
                    shadow_scale,
                });
            }
        }
        visual
    }

    /// Whether a bone is active (`activeInHierarchy`) with `equipped` — an index into the rig's
    /// weapons — switched on by `SetWeapon` and every other weapon off.
    pub fn active_with_weapon(&self, bone: usize, equipped: Option<usize>) -> bool {
        let rig = self.rig();
        let mut current = Some(bone);
        while let Some(i) = current {
            let active = match rig.weapons.iter().position(|w| w.bone == i) {
                Some(weapon) => Some(weapon) == equipped,
                None => rig.bones[i].active_self,
            };
            if !active {
                return false;
            }
            current = rig.bones[i].parent;
        }
        true
    }

    /// Bones whose `GameObject` is active in the prefab (`activeSelf` all the way up).
    pub fn active_in_prefab(&self, bone: usize) -> bool {
        let bones = &self.rig().bones;
        let mut current = Some(bone);
        while let Some(i) = current {
            if !bones[i].active_self {
                return false;
            }
            current = bones[i].parent;
        }
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Mesh generation (`LinesRenderer` and `tk2dSprite`)
// ---------------------------------------------------------------------------------------------

/// Triangles of one line capsule (`LinesRenderer.trianglesPrototype`): three quads over 8
/// vertices — start cap, body, end cap.
pub const LINE_TRIANGLES: [u32; 18] = [0, 3, 1, 3, 0, 2, 2, 5, 3, 5, 2, 4, 4, 7, 5, 7, 4, 6];

/// `LinesRenderer.uvPrototype`: where each of the 8 vertices samples the `Line` sprite, as
/// fractions of its UV rectangle. The left third is the start cap, the right third the end cap.
const LINE_UV_FRACTIONS: [[f32; 2]; 8] =
    [[0.0, 1.0], [0.0, 0.0], [0.5, 1.0], [0.5, 0.0], [0.5, 1.0], [0.5, 0.0], [1.0, 1.0], [1.0, 0.0]];

/// `tk2dSpriteDefinition.uvs` → the 8 per-vertex UVs of a line (`LinesRenderer.LateUpdate`).
pub fn line_uv_prototype(sprite_uvs: &[[f32; 2]]) -> Option<[[f32; 2]; 8]> {
    let (origin, right, up) = (*sprite_uvs.first()?, *sprite_uvs.get(1)?, *sprite_uvs.get(2)?);
    let size = [right[0] - origin[0], up[1] - origin[1]];
    Some(LINE_UV_FRACTIONS.map(|f| [f[0] * size[0] + origin[0], f[1] * size[1] + origin[1]]))
}

/// The 8 vertices of a line capsule, in the order `LINE_TRIANGLES` expects
/// (`LinesRenderer.LateUpdate`). `None` for a zero-length line, where the original's
/// normalization divides by zero and produces no visible geometry.
pub fn line_vertices(line: &StickLine) -> Option<[Vec3; 8]> {
    let (a, b) = (line.a, line.b);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let inverse = 1.0 / (dx * dx + dy * dy).sqrt();
    if !inverse.is_finite() {
        return None;
    }
    let half = inverse * line.width * 0.5;
    let (ux, uy) = (dx * half, dy * half);
    let (nx, ny) = (-uy, ux);
    Some([
        Vec3::new(a.x - ux + nx, a.y - uy + ny, a.z),
        Vec3::new(a.x - ux - nx, a.y - uy - ny, a.z),
        Vec3::new(a.x + nx, a.y + ny, a.z),
        Vec3::new(a.x - nx, a.y - ny, a.z),
        Vec3::new(b.x + nx, b.y + ny, b.z),
        Vec3::new(b.x - nx, b.y - ny, b.z),
        Vec3::new(b.x + ux + nx, b.y + uy + ny, b.z),
        Vec3::new(b.x + ux - nx, b.y + uy - ny, b.z),
    ])
}

/// One sprite quad from a sprite collection: the first quad of its definition (later quads of
/// the exported definitions are the back face, which is not culled here).
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteQuad {
    pub positions: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    /// Two triangles into the four vertices.
    pub indices: [u32; 6],
}

impl SpriteQuad {
    fn of(def: &unity_data::sprites::SpriteDef) -> Option<SpriteQuad> {
        if def.positions.len() < 4 || def.uvs.len() < 4 || def.indices.len() < 6 {
            return None;
        }
        let indices: [u32; 6] = def.indices[..6].try_into().ok()?;
        if indices.iter().any(|&i| i >= 4) {
            return None;
        }
        Some(SpriteQuad {
            positions: def.positions[..4].try_into().ok()?,
            uvs: def.uvs[..4].try_into().ok()?,
            indices,
        })
    }
}

/// A flat sprite and the silhouette sprite its `SpriteShadower` uses (`"<name> shadow"`).
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteArt {
    pub name: String,
    pub quad: SpriteQuad,
    pub shadow: Option<SpriteQuad>,
    /// The full definitions (double-sided sprites carry a back quad).
    pub def: unity_data::sprites::SpriteDef,
    pub shadow_def: Option<unity_data::sprites::SpriteDef>,
}

impl SpriteArt {
    fn of(collection: &unity_data::sprites::SpriteCollection, name: &str) -> Option<SpriteArt> {
        let def = collection.sprite(name)?;
        let quad = SpriteQuad::of(def)?;
        let shadow_def = collection.sprite(&format!("{name} shadow")).cloned();
        let shadow = shadow_def.as_ref().and_then(SpriteQuad::of);
        Some(SpriteArt { name: name.to_owned(), quad, shadow, def: def.clone(), shadow_def })
    }
}

/// Atlas geometry the character needs: the `Line` sprite's UVs and every flat sprite.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualArt {
    line_uvs: [[f32; 2]; 8],
    sprites: Vec<SpriteArt>,
}

impl VisualArt {
    /// Collect the art for the sprites a rig uses from the sprite collection that holds `Line`.
    pub fn new(collection: &unity_data::sprites::SpriteCollection, rig: &Rig) -> Option<VisualArt> {
        let line_uvs = line_uv_prototype(&collection.sprite("Line")?.uvs)?;
        let mut sprites: Vec<SpriteArt> = Vec::new();
        for bone in &rig.bones {
            let Some(flat) = &bone.flat_sprite else { continue };
            if sprites.iter().any(|s| s.name == flat.sprite_name) {
                continue;
            }
            // Weapon sprites live in other collections; whatever this one lacks is not drawn.
            let Some(art) = SpriteArt::of(collection, &flat.sprite_name) else { continue };
            sprites.push(art);
        }
        Some(VisualArt { line_uvs, sprites })
    }

    pub fn sprite(&self, name: &str) -> Option<&SpriteArt> {
        self.sprites.iter().find(|s| s.name == name)
    }

    /// The per-vertex UVs of every line capsule.
    pub fn line_uvs(&self) -> [[f32; 2]; 8] {
        self.line_uvs
    }

    /// Also collect `names` (projectile sprites, say) from the collection; unknown names are
    /// skipped.
    pub fn add_sprites<'a>(&mut self, collection: &unity_data::sprites::SpriteCollection, names: impl IntoIterator<Item = &'a str>) {
        for name in names {
            if self.sprite(name).is_some() {
                continue;
            }
            let Some(art) = SpriteArt::of(collection, name) else { continue };
            self.sprites.push(art);
        }
    }
}

/// The sprites of a projectile (`CurrentCollectionSprite`s, each with a `SpriteShadower`) with
/// the projectile at `position` (game z `spawn_z`) turned by `angle_degrees` about the view axis.
pub fn projectile_visual(
    sprites: &[unity_data::projectile::ProjectileSprite],
    position: Vec2,
    spawn_z: f32,
    angle_degrees: f32,
    light: Option<Vec3>,
) -> CharacterVisual {
    let turn = Quat::about_z(angle_degrees.to_radians());
    let mut visual = CharacterVisual::default();
    for sprite in sprites {
        let local = Vec3::new(sprite.position[0], sprite.position[1], sprite.position[2]);
        let world = turn * local;
        let at = Vec3::new(position.x + world.x, position.y + world.y, spawn_z + world.z);
        let local_roll = Quat::from_array(sprite.rotation).euler_angles_degrees().z;
        let (shadow_offset, shadow_scale) = match light {
            Some(light) => shadow_offset_and_scale(light, at, sprite.shadow_plane_z),
            None => ([0.0, 0.0], 1.0),
        };
        visual.sprites.push(FlatSpriteInstance {
            bone: 0,
            sprite_name: sprite.sprite_name.clone(),
            position: at,
            flipped: false,
            roll_degrees: angle_degrees + local_roll,
            scale: sprite.scale,
            color: sprite.color,
            shadow_plane_z: sprite.shadow_plane_z,
            shadow_offset,
            shadow_scale,
        });
    }
    visual
}

/// Triangle soup in Unity space (left-handed, texture `v` up), colours in the gamma space
/// the original blends in (`0..=1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    fn push_quad(&mut self, positions: [Vec3; 4], uvs: [[f32; 2]; 4], indices: [u32; 6], color: [f32; 4]) {
        let base = self.positions.len() as u32;
        self.positions.extend(positions.iter().map(|p| [p.x, p.y, p.z]));
        self.uvs.extend(uvs);
        self.colors.extend([color; 4]);
        self.indices.extend(indices.iter().map(|i| base + i));
    }
}

fn color32(c: [u8; 4]) -> [f32; 4] {
    c.map(|v| f32::from(v) / 255.0)
}

fn color32_to_f32(c: [u8; 4]) -> [f32; 4] {
    color32(c)
}

enum Piece<'a> {
    Line(&'a StickLine),
    Sprite { sprite: &'a FlatSpriteInstance, shadow: bool },
}

impl CharacterVisual {
    /// The figure as the original's renderers, each to be sorted on its own in the transparent
    /// queue ([`crate::draw`]): first the `LinesRenderer` mesh (every shadow line, then every
    /// line, in the order they were added), then for each `FlatSprite` its `tk2dSprite` and the
    /// `SpriteShadower` copy.
    pub fn renderers(&self, art: &VisualArt) -> Vec<crate::draw::Geometry> {
        use crate::draw::{BACK, Geometry, SHADOW_DEPTH, color32, sprite_geometry};
        let mut out = Vec::new();
        let mut lines = Geometry::default();
        for line in self.shadows.iter().chain(&self.lines) {
            let Some(vertices) = line_vertices(line) else { continue };
            let base = lines.positions.len() as u32;
            lines.positions.extend(vertices.iter().map(|v| [v.x, v.y, v.z]));
            lines.normals.extend([BACK; 8]);
            lines.uvs.extend(art.line_uvs);
            lines.colors.extend([color32(color32_to_f32(line.color)); 8]);
            lines.indices.extend(LINE_TRIANGLES.iter().map(|i| base + i));
        }
        out.push(lines);
        for sprite in &self.sprites {
            let Some(sprite_art) = art.sprite(&sprite.sprite_name) else { continue };
            // `FlatSprite.LateUpdate`: `localEulerAngles = (0, flip ? 180 : 0, roll)`.
            let turn = Quat::euler_y_degrees(if sprite.flipped { 180.0 } else { 0.0 }) * Quat::about_z(sprite.roll_degrees.to_radians());
            let rotation = (turn.x, turn.y, turn.z, turn.w);
            let p = sprite.position;
            let world = unity_data::math::Affine::from_trs([p.x, p.y, p.z], rotation, [1.0; 3]);
            out.push(sprite_geometry(&sprite_art.def, sprite.scale, sprite.color, &world));
            if let Some(shadow) = &sprite_art.shadow_def {
                // The copy's local position is the world offset turned into the sprite's frame,
                // so in the world it is exactly that offset; it inherits the turn.
                let at = [p.x + sprite.shadow_offset[0], p.y + sprite.shadow_offset[1], p.z + SHADOW_DEPTH];
                let world = unity_data::math::Affine::from_trs(at, rotation, [1.0; 3]);
                let k = sprite.shadow_scale;
                out.push(sprite_geometry(shadow, [sprite.scale[0] * k, sprite.scale[1] * k, 1.0], [0.0, 0.0, 0.0, sprite.color[3]], &world));
            }
        }
        out
    }

    /// Draw the figure into one triangle list.
    ///
    /// The original draws one mesh per depth layer with a depth-tested transparent shader whose
    /// state is not recoverable (`TODO(UNCERTAIN)`), so pieces are emitted far-to-near by `z`
    /// (stable within a layer) and alpha-blended, which gives the same picture for opaque
    /// stick colours.
    pub fn build_mesh(&self, art: &VisualArt) -> MeshData {
        let mut pieces: Vec<(f32, Piece)> = Vec::new();
        for line in self.shadows.iter().chain(&self.lines) {
            pieces.push((line.a.z, Piece::Line(line)));
        }
        for sprite in &self.sprites {
            if art.sprite(&sprite.sprite_name).is_some_and(|a| a.shadow.is_some()) {
                pieces.push((sprite.position.z + SPRITE_SHADOW_Z_OFFSET, Piece::Sprite { sprite, shadow: true }));
            }
            pieces.push((sprite.position.z, Piece::Sprite { sprite, shadow: false }));
        }
        // Larger z is farther from the camera (which looks along +z).
        pieces.sort_by(|a, b| b.0.total_cmp(&a.0));

        let mut mesh = MeshData::default();
        for (_, piece) in pieces {
            match piece {
                Piece::Line(line) => {
                    let Some(vertices) = line_vertices(line) else { continue };
                    let base = mesh.positions.len() as u32;
                    mesh.positions.extend(vertices.iter().map(|v| [v.x, v.y, v.z]));
                    mesh.uvs.extend(art.line_uvs);
                    mesh.colors.extend([color32(line.color); 8]);
                    mesh.indices.extend(LINE_TRIANGLES.iter().map(|i| base + i));
                }
                Piece::Sprite { sprite, shadow } => {
                    let Some(sprite_art) = art.sprite(&sprite.sprite_name) else { continue };
                    // `tk2dSprite`: vertices scaled, then the sprite object's turn (flip about Y,
                    // roll about Z) and translation.
                    let turn = Quat::euler_y_degrees(if sprite.flipped { 180.0 } else { 0.0 }) * Quat::about_z(sprite.roll_degrees.to_radians());
                    let (quad, scale, offset, color) = if shadow {
                        let quad = sprite_art.shadow.as_ref().unwrap_or(&sprite_art.quad);
                        let k = sprite.shadow_scale;
                        (
                            quad,
                            [sprite.scale[0] * k, sprite.scale[1] * k, 1.0],
                            Vec3::new(sprite.shadow_offset[0], sprite.shadow_offset[1], SPRITE_SHADOW_Z_OFFSET),
                            [0.0, 0.0, 0.0, sprite.color[3]],
                        )
                    } else {
                        (&sprite_art.quad, sprite.scale, Vec3::ZERO, sprite.color)
                    };
                    let corner = |i: usize| {
                        let p = quad.positions[i];
                        sprite.position + offset + turn * Vec3::new(p[0] * scale[0], p[1] * scale[1], p[2] * scale[2])
                    };
                    mesh.push_quad([corner(0), corner(1), corner(2), corner(3)], quad.uvs, quad.indices, color);
                }
            }
        }
        mesh
    }
}

/// `SpriteShadowPositioner.realShadowOffset`: the shadow sprite sits 1/64 behind its caster.
const SPRITE_SHADOW_Z_OFFSET: f32 = 1.0 / 64.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::ClipPose;
    use std::path::PathBuf;

    #[test]
    fn random_faces_are_always_the_darker_shade() {
        let mut rng = crate::rng::GameRng::from_seed(11);
        let mut shadow = crate::rng::GameRng::from_seed(11);
        for _ in 0..200 {
            let face = random_face_color(&mut rng);
            // Same draws: the float, then the (always zero) integer.
            let f = shadow.range_f32(-1.0, 1.0);
            assert_eq!(shadow.range_int(0, 1), 0);
            assert_eq!(face, to_color32(darker_face_color(f)));
            assert_eq!(face[3], 255);
        }
        // f = 0: (168.8, 122.5, 96.7) truncated.
        assert_eq!(to_color32(darker_face_color(0.0)), [168, 122, 96, 255]);
    }
    use unity_data::humanoid::parse_avatar;
    use unity_data::rig::load_man_rig;

    fn assets() -> Option<PathBuf> {
        let root = std::env::var_os("STICK_EXPORT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
        let assets = root.join("Assets");
        assets.join("Game").is_dir().then_some(assets)
    }

    fn model() -> Option<(CharacterRigModel, PathBuf)> {
        let assets = assets()?;
        let rig = load_man_rig(&assets).unwrap();
        let avatar = parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap()).unwrap();
        Some((CharacterRigModel::new(avatar, rig).unwrap(), assets))
    }

    fn pose_of(assets: &std::path::Path, clip: &str, t: f32) -> crate::pose::HumanPose {
        let text = std::fs::read_to_string(assets.join(format!("AnimationClip/{clip}.anim"))).unwrap();
        ClipPose::compile(&unity_data::anim::parse_clip(&text).unwrap()).sample(t)
    }

    const LIGHT: Vec3 = Vec3::new(20.0, -51.0, -11.21);

    fn placement(facing_right: bool) -> BodyPlacement {
        BodyPlacement {
            position: Vec3::new(40.0, -58.0, -0.05),
            rotation: Quat::euler_y_degrees(if facing_right { 90.0 } else { -90.0 }),
        }
    }

    #[test]
    fn the_man_is_twelve_limb_lines_plus_the_head_blob() {
        let Some((model, assets)) = model() else { return };
        let solved = model.solve(&pose_of(&assets, "Idle", 0.0));
        let palette = CharacterPalette::of_rig(model.rig());
        let v = model.visual(&solved, &placement(true), &palette, Some(LIGHT), &|b| model.active_in_prefab(b));
        // 12 body lines (hips→waist, 2×(hip→knee, knee→ankle), waist→spine1→spine2→neck,
        // 2×(shoulder→elbow, elbow→wrist)) and the head line.
        assert_eq!(v.lines.len(), 13);
        assert_eq!(v.shadows.len(), 13);
        let head: Vec<_> = v.lines.iter().filter(|l| l.color == palette.face).collect();
        assert_eq!(head.len(), 1);
        assert!((head[0].width - 0.72).abs() < 1e-6);
        assert_eq!(v.lines.iter().filter(|l| l.color == palette.body).count(), 12);
        // The head blob is a point-like segment.
        assert!((head[0].b - head[0].a).magnitude() < 0.01);
    }

    #[test]
    fn lines_sit_in_depth_layers_around_the_root() {
        let Some((model, assets)) = model() else { return };
        let solved = model.solve(&pose_of(&assets, "Idle", 0.0));
        let palette = CharacterPalette::of_rig(model.rig());
        let right = placement(true);
        let v = model.visual(&solved, &right, &palette, Some(LIGHT), &|b| model.active_in_prefab(b));
        for l in &v.lines {
            // Both ends share one z; layers are within ±0.035 of the root. The line mesh sits at the
            // root's position and holds root-relative vertices, so `dontRotateZ` (the head, z =
            // -0.025) is -0.025 relative to the root, not absolute.
            assert_eq!(l.a.z, l.b.z);
            if l.color == palette.face {
                assert!((l.a.z - (right.position.z - 0.025)).abs() < 1e-5, "{}", l.a.z);
            } else {
                assert!((l.a.z - right.position.z).abs() < 0.04, "{}", l.a.z);
            }
        }
        // Shadows sit 0.003 behind their lines and are black.
        for (l, s) in v.lines.iter().zip(v.shadows.iter()) {
            assert!((s.a.z - l.a.z - SHADOW_Z_OFFSET).abs() < 1e-5);
            assert_eq!(&s.color[..3], &[0, 0, 0]);
            assert!((s.width - l.width).abs() < 0.01, "the light is nearly head-on");
        }
    }

    #[test]
    fn feet_stand_on_the_collider_bottom_and_the_head_is_a_body_height_above() {
        let Some((model, assets)) = model() else { return };
        let palette = CharacterPalette::of_rig(model.rig());
        let right = BodyPlacement { position: Vec3::ZERO, rotation: Quat::euler_y_degrees(90.0) };
        for clip in ["Idle", "Run"] {
            let solved = model.solve(&pose_of(&assets, clip, 0.1));
            let v = model.visual(&solved, &right, &palette, None, &|b| model.active_in_prefab(b));
            let lowest = v.lines.iter().flat_map(|l| [l.a.y, l.b.y]).fold(f32::MAX, f32::min);
            let highest = v.lines.iter().flat_map(|l| [l.a.y + l.width * 0.5, l.b.y + l.width * 0.5]).fold(f32::MIN, f32::max);
            // Joints end a few tenths above the ground (the lines' round caps reach lower).
            assert!((0.1..0.5).contains(&lowest), "{clip}: lowest joint {lowest}");
            // Collider is 3 units tall; the head blob reaches close to its top.
            assert!((2.6..3.2).contains(&highest), "{clip}: head top {highest}");
        }
    }

    #[test]
    fn the_run_cycle_swings_the_figure_along_the_facing_direction() {
        let Some((model, assets)) = model() else { return };
        let palette = CharacterPalette::of_rig(model.rig());
        let ankle = |v: &CharacterVisual| v.lines.iter().map(|l| l.b.x).fold(f32::MIN, f32::max) - v.lines.iter().map(|l| l.b.x).fold(f32::MAX, f32::min);
        for facing_right in [true, false] {
            let p = BodyPlacement { position: Vec3::ZERO, rotation: Quat::euler_y_degrees(if facing_right { 90.0 } else { -90.0 }) };
            let a = model.visual(&model.solve(&pose_of(&assets, "Run", 0.2)), &p, &palette, None, &|b| model.active_in_prefab(b));
            // The stride spreads the limbs mostly in x (the side-scrolling plane), not in z.
            let z_extent = a.lines.iter().map(|l| l.b.z).fold(f32::MIN, f32::max) - a.lines.iter().map(|l| l.b.z).fold(f32::MAX, f32::min);
            assert!(ankle(&a) > 1.0, "stride width {}", ankle(&a));
            assert!(z_extent < 0.1);
            // Moving in the facing direction: the torso leans toward it.
            let hips = a.lines.iter().find(|l| l.color == palette.body).unwrap();
            let head = a.lines.iter().find(|l| l.color == palette.face).unwrap();
            let lean = (head.a.x - hips.a.x) * if facing_right { 1.0 } else { -1.0 };
            assert!(lean > 0.0, "facing_right={facing_right}: lean {lean}");
        }
    }

    #[test]
    fn the_helmet_follows_the_head_and_mirrors_with_facing() {
        let Some((model, assets)) = model() else { return };
        let palette = CharacterPalette::of_rig(model.rig());
        let solved = model.solve(&pose_of(&assets, "Run", 0.2));
        let on = |facing_right: bool| {
            let v = model.visual(&solved, &placement(facing_right), &palette, None, &|b| model.active_in_prefab(b));
            let helmets: Vec<_> = v.sprites.iter().filter(|s| s.sprite_name == "Helmet").cloned().collect();
            assert_eq!(helmets.len(), 1);
            helmets[0].clone()
        };
        let (right, left) = (on(true), on(false));
        assert_eq!(right.color, palette.helmet);
        assert_eq!(right.scale, [0.325, 0.325, 1.0]);
        // `FlatSprite`: z = root z + rootRelativeZ.
        assert!((right.position.z - (-0.05 - 0.04)).abs() < 1e-5);
        println!("helmet flipped: right={} left={} roll right={} left={}", right.flipped, left.flipped, right.roll_degrees, left.roll_degrees);
        assert_ne!(right.flipped, left.flipped, "the helmet mirrors with the facing");
    }

    #[test]
    fn weapons_are_hidden_unless_their_pivot_is_active() {
        let Some((model, assets)) = model() else { return };
        let palette = CharacterPalette::of_rig(model.rig());
        let solved = model.solve(&pose_of(&assets, "Idle", 0.0));
        let v = model.visual(&solved, &placement(true), &palette, None, &|b| model.active_in_prefab(b));
        let names: Vec<_> = v.sprites.iter().map(|s| s.sprite_name.as_str()).collect();
        println!("prefab-active sprites: {names:?}");
        assert!(names.contains(&"Helmet"));
    }

    #[test]
    fn shadow_projection_matches_the_recovered_formula() {
        // Light at (0, 10, -10); caster on the plane z = 0 → no offset, scale 1.
        let (d, s) = shadow_offset_and_scale(Vec3::new(0.0, 10.0, -10.0), Vec3::new(5.0, 0.0, 0.0), 0.0);
        assert_eq!(d, [0.0, 0.0]);
        assert_eq!(s, 1.0);
        // Caster 2 units in front of the plane (z = -2): the shadow lands 2/10 of the way
        // along the light ray beyond it.
        let (d, s) = shadow_offset_and_scale(Vec3::new(0.0, 10.0, -10.0), Vec3::new(0.0, 0.0, -2.0), 0.0);
        let k = 2.0 / 8.0;
        assert!((d[1] - (-10.0 * k)).abs() < 1e-5 && (s - (1.0 + k)).abs() < 1e-6);
    }

    fn art(model: &CharacterRigModel, assets: &std::path::Path) -> VisualArt {
        let scene = assets.join("Game/Levels/Platforms/Platforms.unity");
        let library = unity_data::sprites::load_collection_with_sprites(assets, &scene, &["Line", "Helmet"]).unwrap();
        VisualArt::new(&library.collection, model.rig()).expect("line and helmet art")
    }

    #[test]
    fn a_line_capsule_extends_half_a_width_beyond_each_end() {
        let line = StickLine { a: Vec3::new(1.0, 2.0, 0.5), b: Vec3::new(4.0, 2.0, 0.5), width: 0.4, color: [255; 4], shadow_plane_z: 0.0 };
        let v = line_vertices(&line).unwrap();
        // Cap vertices reach 0.2 past the ends; the body is 0.4 wide.
        let near = |p: Vec3, x: f32, y: f32| (p.x - x).abs() < 1e-5 && (p.y - y).abs() < 1e-5;
        assert!(near(v[0], 0.8, 2.2) && near(v[1], 0.8, 1.8));
        assert!(near(v[6], 4.2, 2.2) && near(v[7], 4.2, 1.8));
        assert!(near(v[2], 1.0, 2.2) && near(v[3], 1.0, 1.8));
        assert!(v.iter().all(|p| p.z == 0.5));
        assert!(line_vertices(&StickLine { b: line.a, ..line }).is_none());
        // Every triangle index addresses one of the 8 vertices.
        assert!(LINE_TRIANGLES.iter().all(|&i| i < 8));
    }

    #[test]
    fn the_line_sprite_is_split_into_cap_body_cap() {
        let Some((model, assets)) = model() else { return };
        let art = art(&model, &assets);
        let uvs = art.line_uvs;
        // Start cap = left half of the disc's rectangle, end cap = right half, shared body column.
        assert_eq!(uvs[0][0], uvs[1][0]);
        assert_eq!(uvs[2][0], uvs[4][0]);
        assert!(uvs[0][0] < uvs[2][0] && uvs[2][0] < uvs[6][0]);
        assert!(uvs[0][1] > uvs[1][1], "vertex 0 is the top edge, vertex 1 the bottom");
    }

    #[test]
    fn the_whole_man_becomes_one_far_to_near_triangle_list() {
        let Some((model, assets)) = model() else { return };
        let art = art(&model, &assets);
        let palette = CharacterPalette::of_rig(model.rig());
        let solved = model.solve(&pose_of(&assets, "Run", 0.2));
        let v = model.visual(&solved, &placement(true), &palette, Some(LIGHT), &|b| model.active_in_prefab(b));
        let mesh = v.build_mesh(&art);
        // 13 lines + 13 shadows (8 vertices each) and the helmet with its shadow (4 each).
        assert_eq!(mesh.positions.len(), 26 * 8 + 2 * 4);
        assert_eq!(mesh.indices.len(), 26 * 18 + 2 * 6);
        assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
        assert!(mesh.positions.iter().all(|p| p.iter().all(|c| c.is_finite())));
        // Far pieces first: the first vertex of each successive piece is never nearer.
        let zs: Vec<f32> = (0..mesh.positions.len()).step_by(4).map(|i| mesh.positions[i][2]).collect();
        let farthest_first = zs.windows(2).all(|w| w[0] >= w[1] - 1e-6 || (w[0] - w[1]).abs() < 0.2);
        assert!(farthest_first);
        // Black shadows come out before the coloured figure.
        assert_eq!(&mesh.colors[0][..3], &[0.0, 0.0, 0.0]);
        // Hidden weapons add nothing: an inactive helmet removes its two quads.
        let none = model.visual(&solved, &placement(true), &palette, Some(LIGHT), &|_| false);
        assert_eq!(none.build_mesh(&art).positions.len(), 26 * 8);
    }

    #[test]
    fn only_the_equipped_weapons_sprite_is_drawn() {
        let Some((model, assets)) = model() else { return };
        let palette = CharacterPalette::of_rig(model.rig());
        let solved = model.solve(&pose_of(&assets, "Idle", 0.0));
        let names = |equipped: Option<usize>| -> Vec<String> {
            model
                .visual(&solved, &placement(true), &palette, None, &|b| model.active_with_weapon(b, equipped))
                .sprites
                .iter()
                .map(|s| s.sprite_name.clone())
                .collect()
        };
        assert_eq!(names(None), ["Helmet"]);
        assert_eq!(names(Some(0)), ["Helmet", "AR-15"]);
        assert_eq!(names(Some(1)), ["Helmet", "Rocket Launcher"]);
        // The sword is made of three sprites.
        let sword = model.rig().weapons.iter().position(|w| w.name == "Sword").unwrap();
        assert_eq!(names(Some(sword)).len(), 4);
    }

    #[test]
    fn the_hit_flash_brightens_body_and_helmet_but_not_the_face() {
        let p = CharacterPalette { body: [10, 250, 100, 255], face: [1, 2, 3, 255], helmet: [0.5, 0.9, 0.0, 1.0] };
        let hit = p.hit_version();
        assert_eq!(hit.face, p.face);
        // +0.2 is +51 of 255 (10 → 61, 100 → 151); 250 clamps to 255; alpha stays 255.
        assert_eq!(hit.body, [61, 255, 151, 255]);
        assert!((hit.helmet[0] - 0.7).abs() < 1e-6 && (hit.helmet[1] - 1.1).abs() < 1e-6 && (hit.helmet[3] - 2.0).abs() < 1e-6);
    }
}
