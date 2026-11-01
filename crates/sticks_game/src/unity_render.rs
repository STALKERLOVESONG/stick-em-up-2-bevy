//! Draws a scene (a level or the Lobby) the way the original's transparent queue does (see
//! `sticks_sim::draw`).
//!
//! Every frame the scene's renderers (the level's static sprites and surfaces from
//! [`LevelScenery`], the shadows that move with the camera-borne light, and whatever the level or
//! the Lobby pushes into [`FrameQueue`]) become one sorted queue. The queue is cut into runs of
//! one texture; each run is one mesh entity with a [`UnityLitMaterial`], placed along the
//! camera's view axis so that Bevy's back-to-front sort draws the runs in the queue's order.
//! Within a run the triangles are in queue order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor, ImageType};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, CompareFunction, Face, FrontFace, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;
use sticks_sim::draw::{BACK, CameraLight, DrawQueue, FIXED_FUNCTION_LIGHT_SCALE, Geometry, LevelArt, ShaderLight, View, surface_geometry, surface_shadow_geometry};
use unity_compat::Vec3 as SimVec3;
use unity_data::level::{LightSetup, Shape, Surface};
use unity_data::material::MaterialLibrary;
use unity_data::sprites::SpriteLibrary;

use crate::camera::{LevelCamera, Rig};
use crate::config::GameConfig;
use crate::level::ActiveLevel;
use crate::state::AppState;
use crate::FrameSet;

/// The constant colour of [`UnityLitMaterial`]'s combiner stage: white for "Sprite" and
/// "Textured Polygon", `_Color` for "Color Polygon" (black for the "Polygon Shadow" material).
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct TintUniform {
    pub color: Vec4,
}

/// The `LOD 100` vertex-lit pass of the "Sprite" / "Textured Polygon" / "Color Polygon" shaders
/// (the project renders with the legacy Vertex Lit path). The fixed-function per-vertex
/// lighting is done on the CPU into the vertex colours (`sticks_sim::draw::vertex_lit`); the GPU
/// does the combiner stage `texture * primary double` (see `unity_lit.wgsl`).
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct UnityLitMaterial {
    #[uniform(0)]
    pub tint: TintUniform,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Handle<Image>,
}

impl Material for UnityLitMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://sticks_game/unity_lit.wgsl".into()
    }

    /// `Blend SrcAlpha OneMinusSrcAlpha`.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    /// `ZTest Always`, `ZWrite Off`, `Cull Back` with Unity's clockwise front faces (game z is
    /// negated for Bevy, which keeps every vertex's screen position, so the screen winding is
    /// the original's).
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = Some(Face::Back);
        descriptor.primitive.front_face = FrontFace::Cw;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_compare = Some(CompareFunction::Always);
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
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

/// Images the queue draws with and one material per (image, tint); the id indexes `materials`.
#[derive(Resource, Default)]
pub struct UnityTextures {
    ids: HashMap<(PathBuf, [u32; 4]), u32>,
    images: HashMap<PathBuf, Handle<Image>>,
    pub materials: Vec<Handle<UnityLitMaterial>>,
}

const WHITE: [f32; 4] = [1.0; 4];

/// What a texture is loaded with.
pub struct TextureLoad<'a, 'w> {
    pub assets_dir: &'a Path,
    pub asset_server: &'a AssetServer,
    pub images: &'a mut Assets<Image>,
    pub materials: &'a mut Assets<UnityLitMaterial>,
    _marker: std::marker::PhantomData<&'w ()>,
}

impl<'a, 'w> TextureLoad<'a, 'w> {
    pub fn new(assets_dir: &'a Path, asset_server: &'a AssetServer, images: &'a mut Assets<Image>, materials: &'a mut Assets<UnityLitMaterial>) -> Self {
        TextureLoad { assets_dir, asset_server, images, materials, _marker: std::marker::PhantomData }
    }
}

impl UnityTextures {
    /// The id of the texture file `path` (absolute or relative to the asset root), loaded raw
    /// (gamma values, no sRGB decode), bilinear, no mipmaps, repeating (the atlases'
    /// `wrapMode: 0`, and `Texture2D`'s default for tk2d's PNG-byte atlases).
    pub fn id(&mut self, path: &Path, load: &mut TextureLoad<'_, '_>) -> u32 {
        self.tinted(path, WHITE, load)
    }

    /// [`Self::id`] with a combiner constant colour.
    pub fn tinted(&mut self, path: &Path, tint: [f32; 4], load: &mut TextureLoad<'_, '_>) -> u32 {
        let relative = path.strip_prefix(load.assets_dir).unwrap_or(path).to_path_buf();
        let key = (relative.clone(), tint.map(f32::to_bits));
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let image = match self.images.get(&relative) {
            Some(image) => image.clone(),
            None => {
                let image = Self::load_image(&relative, load);
                self.images.insert(relative, image.clone());
                image
            }
        };
        self.add(key, image, tint, load.materials)
    }

    fn load_image(relative: &Path, load: &mut TextureLoad<'_, '_>) -> Handle<Image> {
        if relative.extension().is_some_and(|e| e == "bytes") {
            // A `TextAsset` with PNG bytes (`tk2dSpriteCollectionData.pngTextures`).
            let bytes = std::fs::read(load.assets_dir.join(&relative)).unwrap_or_default();
            match Image::from_buffer(&bytes, ImageType::Extension("png"), CompressedImageFormats::NONE, false, sampler(), RenderAssetUsages::default()) {
                Ok(image) => load.images.add(image),
                Err(e) => {
                    warn!("{}: {e}", relative.display());
                    Handle::default()
                }
            }
        } else {
            load.asset_server
                .load_builder()
                .with_settings(|s: &mut ImageLoaderSettings| {
                    s.is_srgb = false;
                    s.sampler = sampler();
                })
                .load::<Image>(relative.to_path_buf())
        }
    }

    fn add(&mut self, key: (PathBuf, [u32; 4]), image: Handle<Image>, tint: [f32; 4], materials: &mut Assets<UnityLitMaterial>) -> u32 {
        let id = self.materials.len() as u32;
        debug!("material {id}: {} tint {tint:?}", key.0.display());
        self.materials.push(materials.add(UnityLitMaterial { tint: TintUniform { color: Vec4::from_array(tint) }, texture: image }));
        self.ids.insert(key, id);
        id
    }

    /// "Color Polygon": no texture (a 1x1 white image), the combiner constant `_Color`.
    pub fn color(&mut self, tint: [f32; 4], load: &mut TextureLoad<'_, '_>) -> u32 {
        let white = PathBuf::from("<white>");
        if !self.images.contains_key(&white) {
            let image = load.images.add(Image::new_fill(
                bevy::render::render_resource::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                bevy::render::render_resource::TextureDimension::D2,
                &[255, 255, 255, 255],
                bevy::render::render_resource::TextureFormat::Rgba8Unorm,
                RenderAssetUsages::default(),
            ));
            self.images.insert(white.clone(), image);
        }
        self.tinted(&white, tint, load)
    }

    /// The "Polygon Shadow" material (`Resources/Polygon Shadow.mat`: "Color Polygon", `_Color`
    /// black).
    pub fn polygon_shadow(&mut self, load: &mut TextureLoad<'_, '_>) -> u32 {
        self.color([0.0, 0.0, 0.0, 1.0], load)
    }
}

/// How one `PolygonColliderRenderer` path is drawn: its material and the mesh's vertex colour.
/// "Textured Polygon" has no `_Color`; "Color Polygon" has no texture and multiplies by `_Color`
/// in the combiner stage (after the per-vertex lighting).
/// TODO(UNCERTAIN): the material's shader is inferred from whether it has a `_MainTex`.
/// Also returns the material's `_MainTex` scale and offset (applied to the surface's UVs by the
/// fixed-function texture stage).
pub fn surface_style(surface: &Surface, library: &MaterialLibrary, textures: &mut UnityTextures, load: &mut TextureLoad<'_, '_>) -> (u32, [f32; 4], [[f32; 2]; 2]) {
    let recovered = library.load(&surface.material);
    let st = recovered.as_ref().map_or([[1.0, 1.0], [0.0, 0.0]], |m| [m.tiling, m.offset]);
    match recovered.as_ref().and_then(|m| m.main_texture.as_ref()) {
        Some(path) => (textures.id(path, load), WHITE, st),
        None => (textures.color(recovered.map_or(WHITE, |m| m.color), load), WHITE, st),
    }
}

/// The light and ambient colour of the scene being drawn.
#[derive(Resource, Clone, Default)]
pub struct SceneSetup {
    pub light: Option<LightSetup>,
    pub ambient: [f32; 4],
}

/// A level's static art, built when the level starts.
#[derive(Resource, Default)]
pub struct LevelScenery {
    pub art: LevelArt,
    /// Geometry of every static renderer (sprites, then surfaces).
    pub statics: Vec<Geometry>,
    /// Texture, centre and half size of each static renderer.
    static_entries: Vec<(u32, SimVec3, f32)>,
    /// Surfaces that cast a "Polygon Shadow".
    shadowed_surfaces: Vec<Surface>,
    /// Renderers the camera moves (`FakeParallax` above a surface or a built-in mesh): texture,
    /// the mesh where the scene put it, and the parallax.
    parallax_meshes: Vec<(u32, Geometry, unity_data::sprites::Parallax)>,
}

/// Level sprites (indices into `Level::sprites`) whose objects are gone or switched off (a prop
/// that exploded): neither they nor their shadow copies are drawn.
#[derive(Resource, Default)]
pub struct HiddenSprites(pub std::collections::HashSet<usize>);

/// Where the game moves bodies that level sprites ride on this frame (`Motion::follow` offsets;
/// the `HorizontalScroller2D` clouds).
#[derive(Resource, Default)]
pub struct SpriteFollowOffsets(pub Vec<[f32; 2]>);

/// The scene light this frame.
#[derive(Resource, Default)]
pub struct SceneLight {
    camera_light: Option<CameraLight>,
    /// Where `SpriteShadowPositioner` / `LinesRenderer` see the light (before `FixWorldZ`).
    pub for_shadows: Option<SimVec3>,
    /// What the frame is lit with (game space, after `FixWorldZ`).
    pub shading: Option<ShaderLight>,
}

impl SceneLight {
    pub fn new(setup: &SceneSetup) -> SceneLight {
        SceneLight { camera_light: setup.light.as_ref().and_then(|l| l.camera_chain.clone()).map(CameraLight::new), ..default() }
    }
}

/// This frame's transparent queue; the level's figures and the Lobby push into it.
#[derive(Resource, Default)]
pub struct FrameQueue(pub DrawQueue);

/// The mesh entities that draw the queue's runs.
#[derive(Resource, Default)]
struct RunPool(Vec<(Entity, Handle<Mesh>)>);

/// Marker for run entities (removed with the scene).
#[derive(Component)]
pub struct QueueRun;

/// Ordering of the drawing work, after the frame's `LateUpdate`: the light, then everything
/// that pushes into [`FrameQueue`], then the upload.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum QueueSet {
    Light,
    Collect,
    Draw,
}

/// Whether a scene is being drawn (a level or the Lobby).
pub fn drawing_scene(state: Res<State<AppState>>) -> bool {
    matches!(state.get(), AppState::InGame | AppState::Lobby | AppState::Loading)
}

pub struct UnityRenderPlugin;

impl Plugin for UnityRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "unity_lit.wgsl");
        app.add_plugins(MaterialPlugin::<UnityLitMaterial>::default())
            .init_resource::<UnityTextures>()
            .init_resource::<FrameQueue>()
            .init_resource::<RunPool>()
            .init_resource::<SceneLight>()
            .init_resource::<SceneSetup>()
            .configure_sets(Update, (QueueSet::Light, QueueSet::Collect, QueueSet::Draw).chain().after(FrameSet::Present).run_if(drawing_scene))
            .add_systems(OnEnter(AppState::InGame), build_scenery)
            .add_systems(OnExit(AppState::InGame), forget_scene)
            .add_systems(OnExit(AppState::Lobby), forget_scene)
            .add_systems(OnExit(AppState::Loading), forget_scene)
            .add_systems(Update, update_light.in_set(QueueSet::Light))
            .add_systems(Update, draw_queue.in_set(QueueSet::Draw));
    }
}

/// Load every sprite collection the level uses and build its static art; surfaces
/// (`PolygonColliderRenderer`) become static renderers too.
#[allow(clippy::too_many_arguments)]
fn build_scenery(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    // A collection that fails to load is remembered as `None` so it is tried (and reported) once,
    // not once per sprite.
    let mut libraries: HashMap<String, Option<SpriteLibrary>> = HashMap::new();
    for sprite in &level.0.sprites {
        libraries.entry(sprite.collection.clone()).or_insert_with(|| match SpriteLibrary::load(&assets_dir, &sprite.collection) {
            Ok(lib) => Some(lib),
            Err(e) => {
                warn!("sprite collection {}: {e}", sprite.collection);
                None
            }
        });
    }
    // Helicopters draw their own sprites (they move with the body).
    // Helicopters and cars draw their own sprites (they move with their bodies).
    let helicopter_roots: Vec<_> = level.0.helicopters.iter().map(|h| h.root).chain(level.0.cars.iter().map(|c| c.root)).collect();
    // `HorizontalScroller2D` bodies carry their sprites (`crate::scenery`).
    // Then the `Wave` sprites (and their shadow copies), after the scrollers (`crate::scenery`).
    let wave_of = |o: unity_data::scene::FileId| level.0.waves.iter().position(|w| w.object == o).map(|i| level.0.scrollers.len() + i);
    let follow = |s: &unity_data::sprites::SceneSprite| {
        s.rigid_root
            .and_then(|r| level.0.scrollers.iter().position(|sc| sc.object == r))
            .or_else(|| wave_of(s.object))
            .or_else(|| s.shadow_of.and_then(|(c, _)| level.0.sprites.get(c)).and_then(|c| wave_of(c.object)))
            // Then the `Whale`s' sprites (`crate::scenery`).
            .or_else(|| s.rigid_root.and_then(|r| level.0.whales.iter().position(|w| w.object == r)).map(|i| level.0.scrollers.len() + level.0.waves.len() + i))
    };
    let art = LevelArt::build_following(
        &level.0.sprites,
        // The sprites turning with a cannon's pivot are drawn by `crate::cannon`.
        |s| s.rigid_root.is_some_and(|r| helicopter_roots.contains(&r)) || level.0.cannons.iter().any(|c| c.parts.iter().any(|(o, _)| *o == s.object)),
        follow,
        |guid| libraries.get(guid).and_then(Option::as_ref),
        |path| textures.id(path, &mut load),
    );

    let mut statics = Vec::new();
    let mut entries = Vec::new();
    for sprite in &art.sprites {
        entries.push((sprite.texture, sprite.center, extent(&sprite.geometry)));
        statics.push(sprite.geometry.clone());
    }
    // TODO(UNCERTAIN): the surfaces' faded border ring and per-vertex colours are not built.
    let library = MaterialLibrary::scan(&assets_dir);
    let mut shadowed_surfaces = Vec::new();
    let mut parallax_meshes = Vec::new();
    let mut surfaces = 0usize;
    for collider in &level.0.colliders {
        let (Some(surface), Shape::Polygon(_)) = (&collider.surface, &collider.shape) else { continue };
        let (texture, color, st) = surface_style(surface, &library, &mut textures, &mut load);
        let mut geometry = surface_geometry(surface, color);
        sticks_sim::draw::apply_texture_transform(&mut geometry, st[0], st[1]);
        if geometry.is_empty() {
            continue;
        }
        // `FakeParallax` (Japan's mountains) moves the drawn mesh with the camera.
        // TODO(UNCERTAIN): a parallax surface's "Polygon Shadow" (none recovered so far).
        if let Some(parallax) = surface.parallax {
            parallax_meshes.push((texture, geometry, parallax));
            surfaces += 1;
            continue;
        }
        entries.push((texture, geometry.bounds_center(), extent(&geometry)));
        statics.push(geometry);
        if surface.shadow_plane_z.is_some() {
            shadowed_surfaces.push(surface.clone());
        }
        surfaces += 1;
    }
    // Built-in mesh renderers (Japan's sky plane): "Color Polygon" / "Textured Polygon".
    for mesh in &level.0.meshes {
        let recovered = library.load(&mesh.material);
        let texture = match recovered.as_ref().and_then(|m| m.main_texture.as_ref()) {
            Some(path) => textures.id(path, &mut load),
            None => textures.color(recovered.as_ref().map_or([1.0; 4], |m| m.color), &mut load),
        };
        let mut geometry = sticks_sim::draw::builtin_mesh_geometry(mesh.mesh, &mesh.world);
        if let Some(m) = &recovered {
            sticks_sim::draw::apply_texture_transform(&mut geometry, m.tiling, m.offset);
        }
        match mesh.parallax {
            Some(parallax) => parallax_meshes.push((texture, geometry, parallax)),
            None => {
                entries.push((texture, geometry.bounds_center(), extent(&geometry)));
                statics.push(geometry);
            }
        }
    }
    let white = textures.polygon_shadow(&mut load);
    info!(
        "level art: {} sprites ({} moved by the camera), {} sprite shadows, {} surfaces ({} with shadows), {} textures",
        art.sprites.len(),
        art.moving.len(),
        art.shadows.len(),
        surfaces,
        shadowed_surfaces.len(),
        textures.materials.len()
    );
    if art.sprites.is_empty() && surfaces == 0 {
        warn!("level {} has no sprite or surface art", level.0.name);
    }
    let setup = SceneSetup { light: level.0.light_setup.clone(), ambient: level.0.ambient };
    commands.insert_resource(SceneLight::new(&setup));
    commands.insert_resource(setup);
    commands.insert_resource(ShadowTexture(white));
    commands.insert_resource(HiddenSprites::default());
    commands.insert_resource(SpriteFollowOffsets::default());
    commands.insert_resource(LevelScenery { art, statics, static_entries: entries, shadowed_surfaces, parallax_meshes });
}

/// The material polygon shadows are drawn with ("Polygon Shadow").
#[derive(Resource, Clone, Copy)]
pub struct ShadowTexture(pub u32);

/// Half the largest side of a geometry's bounds (for culling).
pub fn extent(g: &Geometry) -> f32 {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in &g.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    (0..3).map(|k| (max[k] - min[k]) * 0.5).fold(0.0, f32::max)
}

fn forget_scene(mut commands: Commands, mut pool: ResMut<RunPool>, mut queue: ResMut<FrameQueue>, runs: Query<Entity, With<QueueRun>>) {
    commands.remove_resource::<LevelScenery>();
    for entity in &runs {
        commands.entity(entity).try_despawn();
    }
    pool.0.clear();
    queue.0.clear();
}

/// The camera-borne light: where shadows see it this frame and what the shaders light with.
fn update_light(setup: Res<SceneSetup>, rigs: Query<&Rig, With<LevelCamera>>, mut light: ResMut<SceneLight>, mut queue: ResMut<FrameQueue>) {
    queue.0.clear();
    let half = FIXED_FUNCTION_LIGHT_SCALE;
    let ambient = [setup.ambient[0] * half, setup.ambient[1] * half, setup.ambient[2] * half];
    let Some(l) = &setup.light else {
        light.for_shadows = None;
        light.shading = Some(ShaderLight { position: SimVec3::ZERO, color: [0.0; 3], range: 1.0, ambient, point: false });
        return;
    };
    let camera = rigs.single().map(|r| SimVec3::new(r.position.x, r.position.y, r.position.z)).ok();
    let (shadow_position, drawn) = match (light.camera_light.as_mut(), camera) {
        (Some(rig), Some(camera)) => {
            let before = rig.before_fix(camera);
            (before, rig.apply_fix(camera))
        }
        _ => {
            let p = SimVec3::new(l.position[0], l.position[1], l.position[2]);
            (p, p)
        }
    };
    light.for_shadows = Some(shadow_position);
    let k = l.intensity * half;
    light.shading = Some(ShaderLight {
        position: drawn,
        color: [l.color[0] * k, l.color[1] * k, l.color[2] * k],
        range: l.range,
        ambient,
        point: l.kind == 2,
    });
}

/// The camera's view for culling.
pub fn camera_view(rig: &Rig, windows: &Query<&Window, With<PrimaryWindow>>) -> View {
    let camera = SimVec3::new(rig.position.x, rig.position.y, rig.position.z);
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    let tan = (rig.field_of_view.to_radians() * 0.5).tan();
    View { camera, tan_half: [tan * aspect, tan] }
}

/// Queue the scenery, sort everything like Unity, and upload the runs.
#[allow(clippy::too_many_arguments)]
fn draw_queue(
    mut commands: Commands,
    scenery: Option<Res<LevelScenery>>,
    hidden: Option<Res<HiddenSprites>>,
    follow: Option<Res<SpriteFollowOffsets>>,
    shadow_texture: Option<Res<ShadowTexture>>,
    light: Res<SceneLight>,
    textures: Res<UnityTextures>,
    windows: Query<&Window, With<PrimaryWindow>>,
    rigs: Query<&Rig, With<LevelCamera>>,
    mut queue: ResMut<FrameQueue>,
    mut pool: ResMut<RunPool>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut runs_query: Query<(&mut Transform, &mut Visibility, &mut MeshMaterial3d<UnityLitMaterial>), With<QueueRun>>,
) {
    let Ok(rig) = rigs.single() else { return };
    if textures.materials.is_empty() {
        return;
    }
    let view = camera_view(rig, &windows);
    let camera = view.camera;
    let queue = &mut queue.0;
    let empty: Vec<Geometry> = Vec::new();
    let statics = scenery.as_ref().map_or(&empty, |s| &s.statics);
    let no_offsets: Vec<[f32; 2]> = Vec::new();
    let offsets = follow.as_ref().map_or(&no_offsets, |f| &f.0);
    let is_hidden = |source: Option<&usize>| source.is_some_and(|s| hidden.as_ref().is_some_and(|h| h.0.contains(s)));
    if let Some(scenery) = &scenery {
        for (index, &(texture, center, extent)) in scenery.static_entries.iter().enumerate() {
            // Entries below the sprite count are sprites (`sprite_sources` index-matched).
            if index < scenery.art.sprites.len() && is_hidden(scenery.art.sprite_sources.get(index)) {
                continue;
            }
            if view.may_see(center, extent) {
                queue.push_static(texture, center, index);
            }
        }
        // Sprites the camera moves (`FakeParallax`, `ScaleSpriteWithCameraDistance`) or that ride
        // on a moved body (`HorizontalScroller2D`).
        for (k, sprite) in scenery.art.moving.iter().enumerate() {
            if is_hidden(scenery.art.moving_sources.get(k)) {
                continue;
            }
            let geometry = sprite.geometry_with(camera, offsets);
            if !geometry.is_empty() && view.may_see(geometry.bounds_center(), extent(&geometry)) {
                queue.push_frame(sprite.texture, geometry);
            }
        }
        for (texture, geometry, parallax) in &scenery.parallax_meshes {
            let moved = sticks_sim::draw::translated(geometry, sticks_sim::draw::parallax_delta(parallax, camera));
            if view.may_see(moved.bounds_center(), extent(&moved)) {
                queue.push_frame(*texture, moved);
            }
        }
        if let Some(light) = light.for_shadows {
            for (k, shadow) in scenery.art.shadows.iter().enumerate() {
                if is_hidden(scenery.art.shadow_sources.get(k)) {
                    continue;
                }
                let geometry = shadow.geometry_with(light, camera, offsets);
                if view.may_see(geometry.bounds_center(), extent(&geometry)) {
                    queue.push_frame(shadow.texture, geometry);
                }
            }
            if let Some(texture) = shadow_texture.as_deref() {
                for surface in &scenery.shadowed_surfaces {
                    if let Some(geometry) = surface_shadow_geometry(surface, light) {
                        if view.may_see(geometry.bounds_center(), extent(&geometry)) {
                            queue.push_frame(texture.0, geometry);
                        }
                    }
                }
            }
        }
    }
    queue.sort(camera);
    let runs = queue.runs(statics, light.shading.as_ref());

    let base = Vec3::new(camera.x, camera.y, -camera.z);
    upload_runs(&mut commands, &mut meshes, &textures, &mut pool.0, &runs, base, |e| {
        e.insert(QueueRun);
    }, &mut runs_query);
}

/// Put sorted runs into a pool of mesh entities in front of a camera at `base` (Bevy space):
/// Bevy (game z negated, camera looking down -z) draws transparent entities far to near by view
/// depth, so run k sits a little nearer than run k - 1. `tag` marks newly spawned entities.
#[allow(clippy::too_many_arguments)]
pub fn upload_runs<F: bevy::ecs::query::QueryFilter>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    textures: &UnityTextures,
    pool: &mut Vec<(Entity, Handle<Mesh>)>,
    runs: &[(u32, Geometry)],
    base: Vec3,
    tag: impl Fn(&mut EntityCommands),
    runs_query: &mut Query<(&mut Transform, &mut Visibility, &mut MeshMaterial3d<UnityLitMaterial>), F>,
) {
    while pool.len() < runs.len() {
        let mesh = meshes.add(empty_mesh());
        let mut entity = commands.spawn((
            Name::new("Unity queue run"),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(textures.materials[0].clone()),
            Transform::default(),
            Visibility::Hidden,
            NoFrustumCulling,
        ));
        tag(&mut entity);
        pool.push((entity.id(), mesh));
    }
    for (k, (entity, mesh_handle)) in pool.iter().enumerate() {
        let Ok((mut transform, mut visibility, mut material)) = runs_query.get_mut(*entity) else { continue };
        let Some((texture, geometry)) = runs.get(k) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let origin = base - Vec3::Z * (500.0 - k as f32 * 0.01);
        transform.translation = origin;
        visibility.set_if_neq(Visibility::Inherited);
        let handle = &textures.materials[*texture as usize];
        if material.0 != *handle {
            material.0 = handle.clone();
        }
        if let Some(mut mesh) = meshes.get_mut(mesh_handle) {
            write_mesh(&mut mesh, geometry, origin);
        }
    }
}

/// A placeholder (one degenerate triangle: zero-sized vertex buffers upset Bevy's allocator).
fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    let g = Geometry { positions: vec![[0.0; 3]; 3], normals: vec![BACK; 3], uvs: vec![[0.0; 2]; 3], colors: vec![[0.0; 4]; 3], indices: vec![0, 1, 2] };
    write_mesh(&mut mesh, &g, Vec3::ZERO);
    mesh
}

/// Game-space geometry → the run's mesh, relative to the run entity (z negated for Bevy, V
/// flipped for Bevy's top-down texture origin).
fn write_mesh(mesh: &mut Mesh, g: &Geometry, origin: Vec3) {
    let positions: Vec<[f32; 3]> = g.positions.iter().map(|p| [p[0] - origin.x, p[1] - origin.y, -p[2] - origin.z]).collect();
    let normals: Vec<[f32; 3]> = g.normals.iter().map(|n| [n[0], n[1], -n[2]]).collect();
    let uvs: Vec<[f32; 2]> = g.uvs.iter().map(|uv| [uv[0], 1.0 - uv[1]]).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, g.colors.clone());
    mesh.insert_indices(Indices::U32(g.indices.clone()));
}
