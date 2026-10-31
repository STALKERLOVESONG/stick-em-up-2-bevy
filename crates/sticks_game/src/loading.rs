//! The original "Level Loading" scene (`LevelLoader.LoadLevel` → `Application.LoadLevel("Level
//! Loading")`, then `LoadLevelAsync` of the level): a camera at (0, 0, -30) clearing to grey
//! (skybox clear without a skybox material), its `Fix Z` point light, and the "UI" text whose
//! `LevelLoaderUI` shows `"Loading\n" + (int)(progress / 0.67 * 100) + "%"` (or "Please wait…"
//! from 100 on), with its `TextShadower` copy. Drawn through the same vertex-lit queue as the
//! levels.
//!
//! TODO(UNCERTAIN): the level is parsed in one blocking step, so the progress never advances
//! past what the first frame shows (`AsyncOperation.progress` 0 → "Loading\n0%").

use bevy::prelude::*;
use sticks_sim::camera::{CameraPosition, CameraRig};
use sticks_sim::draw::shadow_offset_and_scale;
use sticks_sim::text::{TextLayout, format_text, place, text_local_geometry};
use unity_compat::Vec3 as SimVec3;
use unity_data::fonts::{FontLibrary, SceneText};
use unity_data::interactive::load_interactive_scene;
use unity_data::level::PositionerSettings;

use crate::camera::{LevelCamera, Rig};
use crate::config::GameConfig;
use crate::state::AppState;
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, SceneSetup, TextureLoad, UnityLitMaterial, UnityTextures};

/// `TextShadowPositioner.realShadowOffset`.
const TEXT_SHADOW_DEPTH: f32 = 0.025;

#[derive(Component)]
struct LoadingCamera;

#[derive(Resource)]
struct LoadingScreen {
    fonts: FontLibrary,
    text: SceneText,
    font_texture: u32,
    /// The shadow copy's font and texture, and its plane.
    shadow: Option<(String, u32, f32)>,
    /// `AsyncOperation.progress`.
    progress: f32,
}

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Loading), spawn_loading_scene)
            .add_systems(OnExit(AppState::Loading), leave_loading_scene)
            .add_systems(Update, queue_loading_text.in_set(QueueSet::Collect).run_if(in_state(AppState::Loading)).run_if(resource_exists::<LoadingScreen>));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_loading_scene(
    mut commands: Commands,
    config: Res<GameConfig>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let scene = match load_interactive_scene(&config.export_root, "Assets/Game/Level Loading.unity") {
        Ok(scene) => scene,
        Err(e) => {
            error!("Level Loading.unity: {e}");
            return;
        }
    };
    let Some(camera) = scene.level.camera.clone() else {
        error!("Level Loading.unity has no camera");
        return;
    };
    let [r, g, b, _] = camera.background;
    commands.insert_resource(ClearColor(Color::srgb(r, g, b)));
    let [x, y, z] = camera.position;
    commands.spawn((
        Name::new("Level Loading camera"),
        LoadingCamera,
        LevelCamera,
        Rig(CameraRig::new(PositionerSettings::default(), camera.field_of_view, CameraPosition { x, y, z })),
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Projection::Perspective(PerspectiveProjection { fov: camera.field_of_view.to_radians(), near: camera.near, far: camera.far, ..default() }),
        Transform::from_xyz(x, y, -z),
    ));
    let setup = SceneSetup { light: scene.level.light_setup.clone(), ambient: scene.level.ambient };
    commands.insert_resource(SceneLight::new(&setup));
    commands.insert_resource(setup);

    // The `LevelLoaderUI` text and its shadow copy.
    let ui = scene.scripts_named("LevelLoaderUI").next().map(|s| s.object);
    let Some(index) = scene.texts.iter().position(|t| t.shadow_of.is_none() && ui.is_some_and(|o| scene.object_of(t.object) == Some(o))) else {
        error!("Level Loading.unity has no LevelLoaderUI text");
        return;
    };
    let text = scene.texts[index].clone();
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut fonts = FontLibrary::new(&assets_dir);
    let mut texture_of = |fonts: &mut FontLibrary, guid: &str| fonts.get(guid).and_then(|f| f.texture.clone()).map(|p| textures.id(&p, &mut load));
    let Some(font_texture) = texture_of(&mut fonts, &text.font) else {
        error!("Level Loading font {} not found", text.font);
        return;
    };
    let shadow = scene.texts.iter().find_map(|t| match &t.shadow_of {
        Some((caster, font, plane)) if *caster == index => Some((font.clone().unwrap_or_else(|| text.font.clone()), *plane)),
        _ => None,
    });
    let shadow = shadow.and_then(|(font, plane)| texture_of(&mut fonts, &font).map(|t| (font, t, plane)));
    commands.insert_resource(LoadingScreen { fonts, text, font_texture, shadow, progress: 0.0 });
}

fn leave_loading_scene(mut commands: Commands, cameras: Query<Entity, With<LoadingCamera>>) {
    for entity in &cameras {
        commands.entity(entity).try_despawn();
    }
    commands.remove_resource::<LoadingScreen>();
}

/// `LevelLoaderUI.Update` and the text's `TextShadowPositioner`.
fn queue_loading_text(mut screen: ResMut<LoadingScreen>, light: Res<SceneLight>, mut queue: ResMut<FrameQueue>) {
    let screen = &mut *screen;
    let percent = (screen.progress / 0.67 * 100.0) as i32;
    let message = if percent >= 100 { "Loading\nPlease wait\u{2026}".to_owned() } else { format!("Loading\n{percent}%") };
    let layout = TextLayout::of(&screen.text);
    let world = screen.text.world;
    let position = SimVec3::new(world.t[0], world.t[1], world.t[2]);
    let Some(font) = screen.fonts.get(&screen.text.font) else { return };
    let formatted = format_text(font, &layout, &message);
    let local = text_local_geometry(font, &layout, &formatted, screen.text.color, screen.text.color2, screen.text.use_gradient);
    let alpha = screen.text.color[3];
    if let (Some(light), Some((shadow_font, texture, plane))) = (light.for_shadows, screen.shadow.clone()) {
        let (delta, k) = shadow_offset_and_scale(light, position, plane);
        let mut shadow_layout = layout.clone();
        shadow_layout.formatting = false;
        shadow_layout.scale = [layout.scale[0] * k, layout.scale[1] * k, 1.0];
        if let Some(f) = screen.fonts.get(&shadow_font) {
            let g = text_local_geometry(f, &shadow_layout, &formatted, [0.0, 0.0, 0.0, alpha], [0.0, 0.0, 0.0, alpha], false);
            let mut at = world;
            at.t = [world.t[0] + delta[0], world.t[1] + delta[1], world.t[2] + TEXT_SHADOW_DEPTH];
            queue.0.push_frame(texture, place(&g, &at));
        }
    }
    queue.0.push_frame(screen.font_texture, place(&local, &world));
}
