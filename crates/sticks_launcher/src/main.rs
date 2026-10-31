//! The launcher (not in the original): one cardboard screen in the Lobby's style where the cheats,
//! the settings and the name other players see are picked, then the game is started with them.
//!
//! * Cheats travel to the game in `STICK_LAUNCH_OPTIONS` (`sticks_sim::options::LaunchOptions`);
//!   they need a game built with its local debug hacks, except "Unlock All Levels" (the
//!   original's own `AllLevelUnlockCheat`).
//! * Settings are written to `Settings.txt`, which the game reads (and its settings page writes).
//! * The launcher remembers its choices in `Launcher.txt` next to the save files.

mod art;

use std::path::PathBuf;

use bevy::asset::AssetMetaCheck;
use bevy::camera::ScalingMode;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResolution};
use sticks_sim::options::{GAME_SPEEDS, LAUNCH_OPTIONS_VARIABLE, LaunchOptions, Settings, clean_name, find_export_root, save_dir};

use crate::art::{Art, CLEAR, HIGHLIGHTED, PAPER_SHADOW_OFFSET, UNAVAILABLE, WHITE, mesh, scaled, shadow_of};

const LAUNCHER_FILENAME: &str = "Launcher.txt";
/// Whether the game next to the launcher has its cheats (see `build.rs`).
const CHEATS_BUILT_IN: bool = cfg!(game_hacks);

/// The visible area (world units) the layout is made for.
const VIEW: Vec2 = Vec2::new(58.0, 36.0);
const COLUMNS: [f32; 3] = [-19.0, 0.0, 19.0];
const COLUMN_WIDTH: f32 = 17.0;
const ROW_HEIGHT: f32 = 2.15;
const ROW_STEP: f32 = 2.5;
const HEADING_Y: f32 = 4.6;
const FIRST_ROW_Y: f32 = 2.0;
const HOVER_SCALE: f32 = 1.04;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    God,
    InfiniteLives,
    RapidFire,
    AllWeapons,
    HelicopterGod,
    Speed,
    ShowHotkeys,
    UnlockLevels,
    Music,
    Sound,
    Fullscreen,
    VSync,
    Name,
    Play,
    Quit,
}

const CHEAT_ROWS: [Action; 8] = [Action::God, Action::InfiniteLives, Action::RapidFire, Action::AllWeapons, Action::HelicopterGod, Action::Speed, Action::ShowHotkeys, Action::UnlockLevels];
const SETTING_ROWS: [Action; 4] = [Action::Music, Action::Sound, Action::Fullscreen, Action::VSync];

impl Action {
    /// Needs the game's debug hacks.
    fn is_hack(self) -> bool {
        matches!(self, Action::God | Action::InfiniteLives | Action::RapidFire | Action::AllWeapons | Action::HelicopterGod | Action::Speed | Action::ShowHotkeys)
    }

    fn available(self) -> bool {
        !self.is_hack() || CHEATS_BUILT_IN
    }
}

#[derive(Resource)]
struct Choices {
    launch: LaunchOptions,
    settings: Settings,
    editing_name: Option<String>,
    status: Option<String>,
    export_root: PathBuf,
}

fn machine_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Game".to_owned())
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

fn percent(v: f32) -> String {
    if v <= 0.0 { "Off".to_owned() } else { format!("{}%", (v * 100.0).round() as i32) }
}

impl Choices {
    fn label(&self, action: Action, blink: bool) -> String {
        let c = &self.launch.cheats;
        if action.is_hack() && !CHEATS_BUILT_IN {
            return match action {
                Action::God => "God Mode: not built in".to_owned(),
                _ => "(cheats not built in)".to_owned(),
            };
        }
        match action {
            Action::God => format!("God Mode: {}", on_off(c.god)),
            Action::InfiniteLives => format!("Infinite Lives: {}", on_off(c.infinite_lives)),
            Action::RapidFire => format!("Rapid Fire: {}", on_off(c.rapid_fire)),
            Action::AllWeapons => format!("All Weapons: {}", on_off(c.all_weapons)),
            Action::HelicopterGod => format!("Unbreakable Helis: {}", on_off(c.helicopter_god)),
            Action::Speed => format!("Game Speed: {}x", GAME_SPEEDS.get(c.speed).copied().unwrap_or(1.0)),
            Action::ShowHotkeys => format!("Show Cheat Keys: {}", on_off(c.show_hotkeys)),
            Action::UnlockLevels => format!("Unlock All Levels: {}", on_off(self.launch.unlock_all_levels)),
            Action::Music => format!("Music Volume: {}", percent(self.settings.music_volume)),
            Action::Sound => format!("Sound Volume: {}", percent(self.settings.sound_volume)),
            Action::Fullscreen => format!("Fullscreen: {}", on_off(self.settings.fullscreen)),
            Action::VSync => format!("Vertical Sync: {}", on_off(self.settings.vsync)),
            Action::Name => match &self.editing_name {
                Some(name) => format!("Name: {name}{}", if blink { "_" } else { " " }),
                None => format!("Name: {}", self.launch.player_name.clone().unwrap_or_else(machine_name)),
            },
            Action::Play => "Play".to_owned(),
            Action::Quit => "Quit".to_owned(),
        }
    }

    fn press(&mut self, action: Action) {
        if !action.available() {
            return;
        }
        if action != Action::Name {
            self.finish_editing();
        }
        let c = &mut self.launch.cheats;
        match action {
            Action::God => c.god = !c.god,
            Action::InfiniteLives => c.infinite_lives = !c.infinite_lives,
            Action::RapidFire => c.rapid_fire = !c.rapid_fire,
            Action::AllWeapons => c.all_weapons = !c.all_weapons,
            Action::HelicopterGod => c.helicopter_god = !c.helicopter_god,
            Action::Speed => c.speed = (c.speed + 1) % GAME_SPEEDS.len(),
            Action::ShowHotkeys => c.show_hotkeys = !c.show_hotkeys,
            Action::UnlockLevels => self.launch.unlock_all_levels = !self.launch.unlock_all_levels,
            Action::Music => self.settings.music_volume = Settings::next_volume(self.settings.music_volume),
            Action::Sound => self.settings.sound_volume = Settings::next_volume(self.settings.sound_volume),
            Action::Fullscreen => self.settings.fullscreen = !self.settings.fullscreen,
            Action::VSync => self.settings.vsync = !self.settings.vsync,
            Action::Name => {
                if self.editing_name.is_some() {
                    self.finish_editing();
                } else {
                    self.editing_name = Some(self.launch.player_name.clone().unwrap_or_default());
                }
            }
            Action::Play | Action::Quit => {}
        }
    }

    fn finish_editing(&mut self) {
        if let Some(name) = self.editing_name.take() {
            self.launch.player_name = clean_name(&name);
        }
    }

    fn save(&self) {
        if let Err(e) = self.settings.save() {
            warn!("settings: {e}");
        }
        if let Some(dir) = save_dir() {
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(LAUNCHER_FILENAME), self.launch.to_text())) {
                warn!("{LAUNCHER_FILENAME}: {e}");
            }
        }
    }

    /// Start the game next to this program with the choices.
    fn launch(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(if cfg!(windows) { "sticks_game.exe" } else { "sticks_game" });
        if !exe.is_file() {
            return Err("The game was not found next to the launcher.\nBuild sticks_game first.".to_owned());
        }
        let mut command = std::process::Command::new(&exe);
        command.env(LAUNCH_OPTIONS_VARIABLE, self.launch.to_environment()).env("STICK_EXPORT_ROOT", &self.export_root);
        if let Some(dir) = exe.parent() {
            command.current_dir(dir);
        }
        command.spawn().map(|_| ()).map_err(|e| format!("The game did not start:\n{e}"))
    }
}

#[derive(Component)]
struct LauncherButton {
    action: Action,
    size: Vec2,
    /// Text scale for this button's label.
    big: bool,
    hovered: bool,
    shown: Option<(String, [f32; 4])>,
    text: Handle<Mesh>,
    text_shadow: Handle<Mesh>,
}

/// The status line under the Play button.
#[derive(Component)]
struct StatusText {
    text: Handle<Mesh>,
    shadow: Handle<Mesh>,
    shown: Option<String>,
}

fn main() {
    let export_root = find_export_root(std::env::args().nth(1).as_deref());
    let assets_dir = export_root.as_ref().map_or_else(|| PathBuf::from("assets"), |r| r.join("Assets"));
    let launch = save_dir().and_then(|d| std::fs::read_to_string(d.join(LAUNCHER_FILENAME)).ok()).map(|t| LaunchOptions::parse(&t)).unwrap_or_default();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window { title: "Stick 'Em Up 2 Launcher".into(), resolution: WindowResolution::new(1280, 800), ..default() }),
                    ..default()
                })
                .set(AssetPlugin { file_path: assets_dir.to_string_lossy().into_owned(), meta_check: AssetMetaCheck::Never, ..default() }),
        )
        .insert_resource(ClearColor(Color::srgb(CLEAR[0], CLEAR[1], CLEAR[2])))
        .insert_resource(Choices { launch, settings: Settings::load(), editing_name: None, status: None, export_root: export_root.clone().unwrap_or_default() })
        .insert_resource(ExportRoot(export_root))
        .add_systems(Startup, setup)
        .add_systems(Update, (pointer, typing, refresh_labels, refresh_status).chain())
        .run();
}

#[derive(Resource)]
struct ExportRoot(Option<PathBuf>);

fn material(materials: &mut Assets<ColorMaterial>, texture: &Handle<Image>) -> Handle<ColorMaterial> {
    materials.add(ColorMaterial { texture: Some(texture.clone()), ..default() })
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    root: Res<ExportRoot>,
    asset_server: Res<AssetServer>,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<ColorMaterial>>,
    mut choices: ResMut<Choices>,
) {
    let meshes = meshes.into_inner();
    let materials = materials.into_inner();
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection { scaling_mode: ScalingMode::AutoMin { min_width: VIEW.x, min_height: VIEW.y }, ..OrthographicProjection::default_2d() }),
    ));
    let loaded = match &root.0 {
        Some(r) => Art::load(r, &asset_server),
        None => Err("The exported game (assets_exported/ExportedProject) was not found.\nSet STICK_EXPORT_ROOT to it.".to_owned()),
    };
    let art = match loaded {
        Ok(art) => art,
        Err(e) => {
            error!("{e}");
            commands.spawn((Text::new(format!("The launcher cannot show the game's art:\n{e}\n\nClose this window.")), Node { margin: UiRect::all(Val::Px(24.0)), ..default() }));
            choices.status = Some(e);
            return;
        }
    };

    // The cardboard and the title.
    if let Some(bg) = &art.background {
        commands.spawn((Mesh2d(meshes.add(mesh(&bg.geometry))), MeshMaterial2d(material(materials, &bg.texture)), Transform::from_xyz(0.0, 0.0, -10.0)));
    }
    if let Some(title) = &art.title {
        let (min, max) = title.geometry.positions.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| (lo.min(Vec2::new(p[0], p[1])), hi.max(Vec2::new(p[0], p[1]))));
        let size = (max - min).max(Vec2::splat(1e-3));
        let k = (34.0 / size.x).min(9.0 / size.y);
        let g = scaled(&title.geometry, Vec2::splat(k));
        let mat = material(materials, &title.texture);
        let at = Vec2::new(0.0, 11.6);
        if let Some((offset, color)) = art.title_shadow {
            let o = offset * k;
            commands.spawn((Mesh2d(meshes.add(mesh(&shadow_of(&g, None, Some(color))))), MeshMaterial2d(mat.clone()), Transform::from_xyz(at.x + o.x, at.y + o.y, -1.0)));
        }
        commands.spawn((Mesh2d(meshes.add(mesh(&g))), MeshMaterial2d(mat), Transform::from_xyz(at.x, at.y, -0.9)));
    }

    let text_material = material(materials, &art.font_texture);
    let mut paper_index = 0usize;
    let mut button = |commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<ColorMaterial>, action: Action, at: Vec2, size: Vec2, big: bool| {
        let paper = (!art.papers.is_empty()).then(|| &art.papers[paper_index % art.papers.len()]);
        paper_index += 1;
        let paper = paper.map(|p| (scaled(&p.geometry, size), material(materials, &p.texture)));
        let text = meshes.add(mesh(&sticks_sim::draw::Geometry::default()));
        let text_shadow = meshes.add(mesh(&sticks_sim::draw::Geometry::default()));
        let scale = art.fit_scale("Unbreakable Helis: Off", size.x * 0.9, size.y * if big { 0.8 } else { 1.0 });
        let shadow_offset = art.text_shadow.0 * (scale / art.text.scale[1].abs().max(1e-3));
        commands
            .spawn((
                LauncherButton { action, size, big, hovered: false, shown: None, text: text.clone(), text_shadow: text_shadow.clone() },
                Transform::from_xyz(at.x, at.y, 1.0),
                Visibility::default(),
            ))
            .with_children(|b| {
                if let Some((g, mat)) = paper {
                    b.spawn((Mesh2d(meshes.add(mesh(&shadow_of(&g, None, None)))), MeshMaterial2d(mat.clone()), Transform::from_xyz(PAPER_SHADOW_OFFSET.x, PAPER_SHADOW_OFFSET.y, 0.0)));
                    b.spawn((Mesh2d(meshes.add(mesh(&g))), MeshMaterial2d(mat), Transform::from_xyz(0.0, 0.0, 0.1)));
                }
                b.spawn((Mesh2d(text_shadow), MeshMaterial2d(text_material.clone()), Transform::from_xyz(shadow_offset.x, shadow_offset.y, 0.2)));
                b.spawn((Mesh2d(text), MeshMaterial2d(text_material.clone()), Transform::from_xyz(0.0, 0.0, 0.3)));
            });
    };
    if art.papers.is_empty() {
        warn!("no paper backgrounds found in the Lobby");
    }

    // Headings.
    let heading = |commands: &mut Commands, meshes: &mut Assets<Mesh>, text: &str, at: Vec2| {
        let scale = art.text_per_height * 2.9;
        let g = art.text_geometry(text, scale, WHITE);
        let o = art.text_shadow.0 * (scale / art.text.scale[1].abs().max(1e-3));
        commands.spawn((Mesh2d(meshes.add(mesh(&shadow_of(&g, None, Some(art.text_shadow.1))))), MeshMaterial2d(text_material.clone()), Transform::from_xyz(at.x + o.x, at.y + o.y, 0.5)));
        commands.spawn((Mesh2d(meshes.add(mesh(&g))), MeshMaterial2d(text_material.clone()), Transform::from_xyz(at.x, at.y, 0.6)));
    };
    let note = |commands: &mut Commands, meshes: &mut Assets<Mesh>, text: &str, at: Vec2| {
        let scale = art.text_per_height * 1.45;
        let g = art.text_geometry(text, scale, WHITE);
        let o = art.text_shadow.0 * (scale / art.text.scale[1].abs().max(1e-3));
        commands.spawn((Mesh2d(meshes.add(mesh(&shadow_of(&g, None, Some(art.text_shadow.1))))), MeshMaterial2d(text_material.clone()), Transform::from_xyz(at.x + o.x, at.y + o.y, 0.5)));
        commands.spawn((Mesh2d(meshes.add(mesh(&g))), MeshMaterial2d(text_material.clone()), Transform::from_xyz(at.x, at.y, 0.6)));
    };

    let row = |i: usize| FIRST_ROW_Y - i as f32 * ROW_STEP;
    let size = Vec2::new(COLUMN_WIDTH, ROW_HEIGHT);
    heading(&mut commands, meshes, "Cheats", Vec2::new(COLUMNS[0], HEADING_Y));
    for (i, &a) in CHEAT_ROWS.iter().enumerate() {
        button(&mut commands, meshes, materials, a, Vec2::new(COLUMNS[0], row(i)), size, false);
    }
    heading(&mut commands, meshes, "Settings", Vec2::new(COLUMNS[1], HEADING_Y));
    for (i, &a) in SETTING_ROWS.iter().enumerate() {
        button(&mut commands, meshes, materials, a, Vec2::new(COLUMNS[1], row(i)), size, false);
    }
    heading(&mut commands, meshes, "Online", Vec2::new(COLUMNS[1], row(4) - 0.4));
    button(&mut commands, meshes, materials, Action::Name, Vec2::new(COLUMNS[1], row(5) - 0.4), size, false);
    note(
        &mut commands,
        meshes,
        "Host: Start Network Game, then Send Link.\nFriends copy the link and open\nthe network screen to join.",
        Vec2::new(COLUMNS[1], row(7) + 0.2),
    );
    button(&mut commands, meshes, materials, Action::Play, Vec2::new(COLUMNS[2], row(0) - 1.0), Vec2::new(COLUMN_WIDTH, ROW_HEIGHT * 2.1), true);
    button(&mut commands, meshes, materials, Action::Quit, Vec2::new(COLUMNS[2], row(2) - 1.0), size, false);
    let cheat_note = if CHEATS_BUILT_IN {
        "Cheats are on from the start\nof every level. In a level,\nF2 lists the cheat keys."
    } else {
        "This game was built without\nits cheats; only Unlock All\nLevels works."
    };
    note(&mut commands, meshes, cheat_note, Vec2::new(COLUMNS[2], row(4) - 0.6));
    let status = (meshes.add(mesh(&sticks_sim::draw::Geometry::default())), meshes.add(mesh(&sticks_sim::draw::Geometry::default())));
    let at = Vec2::new(COLUMNS[2], row(6) - 0.6);
    let o = art.text_shadow.0 * 0.5;
    commands.spawn((Mesh2d(status.1.clone()), MeshMaterial2d(text_material.clone()), Transform::from_xyz(at.x + o.x, at.y + o.y, 0.5)));
    commands.spawn((Mesh2d(status.0.clone()), MeshMaterial2d(text_material.clone()), Transform::from_xyz(at.x, at.y, 0.6)));
    commands.spawn(StatusText { text: status.0, shadow: status.1, shown: None });
    commands.insert_resource(art);
}

/// Hover and clicks.
fn pointer(
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut buttons: Query<(&mut LauncherButton, &mut Transform)>,
    mut choices: ResMut<Choices>,
    mut exit: MessageWriter<AppExit>,
) {
    let cursor = windows.single().ok().and_then(|w| w.cursor_position()).and_then(|c| {
        let (camera, transform) = cameras.single().ok()?;
        camera.viewport_to_world_2d(transform, c).ok()
    });
    let clicked = mouse.just_pressed(MouseButton::Left);
    let mut pressed = None;
    for (mut b, mut transform) in &mut buttons {
        let centre = transform.translation.truncate();
        let hovered = cursor.is_some_and(|c| ((c - centre).abs() * 2.0).cmple(b.size).all()) && b.action.available();
        if b.hovered != hovered {
            b.hovered = hovered;
            transform.scale = Vec3::splat(if hovered { HOVER_SCALE } else { 1.0 });
        }
        if hovered && clicked {
            pressed = Some(b.action);
        }
    }
    let Some(action) = pressed else {
        if clicked {
            choices.finish_editing();
        }
        return;
    };
    match action {
        Action::Play => {
            choices.finish_editing();
            choices.save();
            match choices.launch() {
                Ok(()) => {
                    exit.write(AppExit::Success);
                }
                Err(e) => {
                    error!("{e}");
                    choices.status = Some(e);
                }
            }
        }
        Action::Quit => {
            choices.finish_editing();
            choices.save();
            exit.write(AppExit::Success);
        }
        other => choices.press(other),
    }
}

/// Typing the player name.
fn typing(mut keys: MessageReader<KeyboardInput>, mut choices: ResMut<Choices>) {
    for key in keys.read() {
        if key.state != ButtonState::Pressed {
            continue;
        }
        let Some(name) = choices.editing_name.as_mut() else { continue };
        match &key.logical_key {
            Key::Character(s) => {
                if name.chars().count() < 25 {
                    name.push_str(s.as_str());
                }
            }
            Key::Space => name.push(' '),
            Key::Backspace => {
                name.pop();
            }
            Key::Enter => choices.finish_editing(),
            Key::Escape => choices.editing_name = None,
            _ => {}
        }
    }
}

/// Labels follow the choices and the hover.
fn refresh_labels(time: Res<Time>, art: Option<Res<Art>>, choices: Res<Choices>, mut buttons: Query<&mut LauncherButton>, mut meshes: ResMut<Assets<Mesh>>) {
    let Some(art) = art else { return };
    let blink = (time.elapsed_secs() * 2.0) as i64 % 2 == 0;
    for mut b in &mut buttons {
        let text = choices.label(b.action, blink);
        let color = if !b.action.available() {
            UNAVAILABLE
        } else if b.hovered || (b.action == Action::Name && choices.editing_name.is_some()) {
            HIGHLIGHTED
        } else {
            WHITE
        };
        if b.shown.as_ref().is_some_and(|(t, c)| *t == text && *c == color) {
            continue;
        }
        let height = b.size.y * if b.big { 0.8 } else { 1.0 };
        // Every row shares one size (the longest label's), as on the Lobby's pages; a longer
        // label shrinks to fit.
        let scale = art.fit_scale("Unbreakable Helis: Off", b.size.x * 0.9, height).min(art.fit_scale(&text, b.size.x * 0.9, height));
        let g = art.text_geometry(&text, scale, color);
        set_mesh(&mut meshes, &b.text, mesh(&g));
        set_mesh(&mut meshes, &b.text_shadow, mesh(&shadow_of(&g, None, Some(art.text_shadow.1))));
        b.shown = Some((text, color));
    }
}

fn refresh_status(art: Option<Res<Art>>, choices: Res<Choices>, mut status: Query<&mut StatusText>, mut meshes: ResMut<Assets<Mesh>>) {
    let (Some(art), Ok(mut status)) = (art, status.single_mut()) else { return };
    if status.shown == choices.status {
        return;
    }
    let text = choices.status.clone().unwrap_or_default();
    let g = art.text_geometry(&text, art.text_per_height * 1.4, HIGHLIGHTED);
    set_mesh(&mut meshes, &status.text, mesh(&g));
    set_mesh(&mut meshes, &status.shadow, mesh(&shadow_of(&g, None, Some(art.text_shadow.1))));
    status.shown = choices.status.clone();
}

fn set_mesh(meshes: &mut Assets<Mesh>, handle: &Handle<Mesh>, new: Mesh) {
    let _ = meshes.insert(handle.id(), new);
}
