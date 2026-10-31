//! The launcher's look, taken from the game's own Lobby scene: the cardboard behind every Lobby
//! screen, the title, the torn-paper button backgrounds of the title screen and the font of
//! their labels (with the shadows the Lobby gives them).
//!
//! The pieces are drawn flat (unlit 2D meshes): the Lobby's vertex lighting barely changes these
//! textures, and the launcher has no light to light them with.

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use sticks_sim::draw::{Geometry, apply_texture_transform, sprite_geometry, surface_geometry};
use sticks_sim::text::{TextLayout, estimated_bounds, format_text, text_local_geometry};
use unity_data::fonts::{Font, FontLibrary};
use unity_data::interactive::{InteractiveScene, load_interactive_scene};
use unity_data::material::MaterialLibrary;
use unity_data::sprites::SpriteLibrary;

const LOBBY_SCENE: &str = "Assets/Game/Lobby/Lobby.unity";
/// The title screen's centre in the Lobby; the launcher's origin.
const TITLE_SCREEN: [f32; 2] = [-4.5, 0.0];
/// The title screen's buttons, whose backgrounds the launcher's buttons wear.
const PAPER_BUTTONS: [&str; 4] = ["Play Offline", "Start Network Game", "Join Network Game", "-1 Exit"];
/// The Lobby camera's clear colour.
pub const CLEAR: [f32; 3] = [0.19215687, 0.3019608, 0.4745098];
/// `Lobby` `HIGHLIGHTED_TEXT` / `UNAVAILABLE_TEXT`.
pub const HIGHLIGHTED: [f32; 4] = [1.0, 143.0 / 255.0, 56.0 / 255.0, 1.0];
pub const UNAVAILABLE: [f32; 4] = [52.0 / 85.0, 52.0 / 85.0, 52.0 / 85.0, 1.0];
pub const WHITE: [f32; 4] = [1.0; 4];
/// How far a paper's shadow falls (the Lobby's light is above and to the left).
pub const PAPER_SHADOW_OFFSET: Vec2 = Vec2::new(0.22, -0.26);
const PAPER_SHADOW_ALPHA: f32 = 0.45;

/// A mesh prototype and its texture.
#[derive(Clone)]
pub struct Piece {
    pub texture: Handle<Image>,
    pub geometry: Geometry,
}

#[derive(Resource)]
pub struct Art {
    pub background: Option<Piece>,
    pub title: Option<Piece>,
    pub title_shadow: Option<(Vec2, [f32; 4])>,
    /// Button backgrounds, each scaled to a 1 × 1 box about the origin.
    pub papers: Vec<Piece>,
    pub font: Font,
    pub font_texture: Handle<Image>,
    pub text: TextLayout,
    /// Text height (scale y) per unit of button height.
    pub text_per_height: f32,
    pub text_shadow: (Vec2, [f32; 4]),
}

fn sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    })
}

fn texture(asset_server: &AssetServer, assets_dir: &Path, path: &Path) -> Handle<Image> {
    let relative: PathBuf = path.strip_prefix(assets_dir).unwrap_or(path).to_path_buf();
    asset_server.load_builder().with_settings(|s: &mut ImageLoaderSettings| s.sampler = sampler()).load::<Image>(relative)
}

fn bounds(g: &Geometry) -> (Vec2, Vec2) {
    let mut min = Vec2::splat(f32::MAX);
    let mut max = Vec2::splat(f32::MIN);
    for p in &g.positions {
        min = min.min(Vec2::new(p[0], p[1]));
        max = max.max(Vec2::new(p[0], p[1]));
    }
    (min, max)
}

fn transformed(g: &Geometry, f: impl Fn(Vec2) -> Vec2) -> Geometry {
    let mut out = g.clone();
    for p in &mut out.positions {
        let q = f(Vec2::new(p[0], p[1]));
        *p = [q.x, q.y, 0.0];
    }
    out
}

fn name_of(scene: &InteractiveScene, object: Option<usize>) -> &str {
    object.map_or("", |o| scene.objects[o].name.as_str())
}

fn parent_name(scene: &InteractiveScene, object: Option<usize>) -> &str {
    name_of(scene, object.and_then(|o| scene.objects[o].parent))
}

impl Art {
    /// Everything from the Lobby scene. `Err` when the scene or its font cannot be read.
    pub fn load(export_root: &Path, asset_server: &AssetServer) -> Result<Art, String> {
        let assets_dir = export_root.join("Assets");
        let scene = load_interactive_scene(export_root, LOBBY_SCENE).map_err(|e| format!("{LOBBY_SCENE}: {e}"))?;
        let materials = MaterialLibrary::scan(&assets_dir);
        let origin = Vec2::from(TITLE_SCREEN);

        // Surfaces: the cardboard (the root "Background", under every screen) and the papers.
        let mut background = None;
        let mut papers: Vec<(usize, Piece, Vec2)> = Vec::new();
        for collider in &scene.colliders {
            let Some(surface) = &collider.surface else { continue };
            let object = scene.object_of(collider.object);
            let Some(material) = materials.load(&surface.material) else { continue };
            let Some(path) = material.main_texture.clone() else { continue };
            let mut geometry = surface_geometry(surface, material.color);
            apply_texture_transform(&mut geometry, material.tiling, material.offset);
            if geometry.is_empty() {
                continue;
            }
            let is_root = object.is_some_and(|o| scene.objects[o].parent.is_none());
            if name_of(&scene, object) == "Background" && is_root {
                background = Some(Piece { texture: texture(asset_server, &assets_dir, &path), geometry: transformed(&geometry, |p| p - origin) });
                continue;
            }
            let button = object.and_then(|o| scene.objects[o].parent);
            let grand = parent_name(&scene, button);
            if let Some(rank) = PAPER_BUTTONS.iter().position(|n| *n == parent_name(&scene, object)).filter(|_| grand == "Title Screen") {
                let (min, max) = bounds(&geometry);
                let (centre, size) = ((min + max) * 0.5, (max - min).max(Vec2::splat(1e-3)));
                let unit = transformed(&geometry, |p| (p - centre) / size);
                papers.push((rank, Piece { texture: texture(asset_server, &assets_dir, &path), geometry: unit }, size));
            }
        }
        papers.sort_by_key(|(rank, ..)| *rank);
        let first_paper_height = papers.first().map_or(3.6, |(_, _, size)| size.y);

        // The title and its shadow copy.
        let mut title = None;
        let mut title_shadow = None;
        let title_index = scene
            .sprites
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == "Title" && s.shadow_of.is_none())
            .min_by(|(_, a), (_, b)| {
                let d = |s: &unity_data::sprites::SceneSprite| (Vec2::new(s.world.t[0], s.world.t[1]) - Vec2::new(-4.23, 7.4)).length_squared();
                d(a).total_cmp(&d(b))
            })
            .map(|(i, _)| i);
        if let Some(i) = title_index {
            let sprite = &scene.sprites[i];
            if let Ok(library) = SpriteLibrary::load(&assets_dir, &sprite.collection) {
                if let Some(def) = library.collection.sprites.get(sprite.sprite_id) {
                    if let Some(path) = library.texture_of(def) {
                        let mut world = sprite.world;
                        world.t = [0.0; 3];
                        let geometry = sprite_geometry(def, sprite.scale, sprite.color, &world);
                        let (min, max) = bounds(&geometry);
                        let centre = (min + max) * 0.5;
                        title = Some(Piece { texture: texture(asset_server, &assets_dir, path), geometry: transformed(&geometry, |p| p - centre) });
                    }
                }
            }
            if let Some(shadow) = scene.sprites.iter().find(|s| s.shadow_of.is_some_and(|(c, _)| c == i)) {
                let offset = Vec2::new(shadow.world.t[0] - sprite.world.t[0], shadow.world.t[1] - sprite.world.t[1]);
                title_shadow = Some((offset, shadow.color));
            }
        }

        // The label font: the title screen's "Play Offline" text, and its shadow copy.
        let label = scene
            .texts
            .iter()
            .position(|t| t.shadow_of.is_none() && parent_name(&scene, scene.object_of(t.object)) == "Play Offline")
            .ok_or("the Lobby has no \"Play Offline\" label")?;
        let template = &scene.texts[label];
        let text_shadow = scene
            .texts
            .iter()
            .find(|t| t.shadow_of.as_ref().is_some_and(|s| s.0 == label))
            .map_or((Vec2::new(0.2, -0.07), [0.0, 0.0, 0.0, 0.5]), |s| (Vec2::new(s.world.t[0] - template.world.t[0], s.world.t[1] - template.world.t[1]), s.color));
        let mut fonts = FontLibrary::new(&assets_dir);
        let font = fonts.get(&template.font).cloned().ok_or("the label font cannot be read")?;
        let font_texture = font.texture.as_deref().map(|p| texture(asset_server, &assets_dir, p)).ok_or("the label font has no atlas")?;
        let mut text = TextLayout::of(template);
        // Centred, on one line.
        text.anchor = 4;
        text.word_wrap_width = 0;
        text.max_chars = 200;
        let text_per_height = text.scale[1] / first_paper_height;
        Ok(Art { background, title, title_shadow, papers: papers.into_iter().map(|(_, p, _)| p).collect(), font, font_texture, text, text_per_height, text_shadow })
    }

    /// The text layout at a scale.
    fn layout(&self, scale: f32) -> TextLayout {
        let mut l = self.text.clone();
        l.scale = [scale * self.text.scale[0].signum(), scale, self.text.scale[2]];
        l
    }

    /// The text scale that fits `text` into a `width` × `height` box.
    pub fn fit_scale(&self, text: &str, width: f32, height: f32) -> f32 {
        let scale = self.text_per_height * height;
        let (_, size) = estimated_bounds(&self.font, &self.layout(scale), text);
        let w = size[0].abs();
        if w > width && w > 0.0 { scale * width / w } else { scale }
    }

    /// `text` centred on the origin.
    pub fn text_geometry(&self, text: &str, scale: f32, color: [f32; 4]) -> Geometry {
        let layout = self.layout(scale);
        let formatted = format_text(&self.font, &layout, text);
        let g = text_local_geometry(&self.font, &layout, &formatted, color, color, false);
        if g.is_empty() {
            return g;
        }
        let (min, max) = bounds(&g);
        let centre = (min + max) * 0.5;
        transformed(&g, |p| p - centre)
    }
}

/// A paper's shadow: the paper in see-through black.
pub fn shadow_of(g: &Geometry, alpha: Option<f32>, color: Option<[f32; 4]>) -> Geometry {
    let mut out = g.clone();
    let c = color.unwrap_or([0.0, 0.0, 0.0, alpha.unwrap_or(PAPER_SHADOW_ALPHA)]);
    for v in &mut out.colors {
        *v = [c[0], c[1], c[2], c[3] * v[3]];
    }
    out
}

/// A piece scaled about the origin.
pub fn scaled(g: &Geometry, size: Vec2) -> Geometry {
    transformed(g, |p| p * size)
}

/// Game geometry (Unity UVs, gamma vertex colours) as a 2D mesh.
pub fn mesh(g: &Geometry) -> Mesh {
    // Zero-sized vertex buffers upset Bevy's allocator: an empty piece is one invisible triangle.
    let empty = Geometry { positions: vec![[0.0; 3]; 3], normals: Vec::new(), uvs: vec![[0.0; 2]; 3], colors: vec![[0.0; 4]; 3], indices: vec![0, 1, 2] };
    let g = if g.is_empty() { &empty } else { g };
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    let positions: Vec<[f32; 3]> = g.positions.iter().map(|p| [p[0], p[1], 0.0]).collect();
    let uvs: Vec<[f32; 2]> = g.uvs.iter().map(|uv| [uv[0], 1.0 - uv[1]]).collect();
    let colors: Vec<[f32; 4]> = g.colors.iter().map(|c| Color::srgba(c[0], c[1], c[2], c[3]).to_linear().to_f32_array()).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(g.indices.clone()));
    mesh
}
