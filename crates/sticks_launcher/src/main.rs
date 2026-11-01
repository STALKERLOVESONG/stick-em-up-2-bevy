//! The launcher (not in the original): one cardboard screen in the Lobby's style where the cheats,
//! the settings and the name other players see are picked, then the game is started with them.
//! It is drawn with `sticks_paper`, from the game's own Lobby art.
//!
//! * Cheats travel to the game in `STICK_LAUNCH_OPTIONS` (`sticks_sim::options::LaunchOptions`);
//!   they need a game built with its local debug hacks, except "Unlock All Levels" (the
//!   original's own `AllLevelUnlockCheat`).
//! * Settings are written to `Settings.txt`, which the game reads; the game has no settings
//!   screen of its own.
//! * "Original Game Only" starts the game with none of the additions (cheats, internet play,
//!   rooms, the working Send Link): the original as it was.
//! * Music, sound and game speed are sliders (drag along them); on/off choices light up when on.
//! * The launcher remembers its choices in `Launcher.txt` next to the save files.
//! * Started by Steam (the game's launch options set to `"<path>\sticks_launcher.exe" %command%`),
//!   it gets the original game's command line and offers "Play Original" to run that instead.

// No console window behind the game or the launcher on Windows (the launcher writes the
// game's log to `SEU2_log.txt` instead).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::ffi::OsString;
use std::path::PathBuf;

use bevy::asset::AssetMetaCheck;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::WindowResolution;
use sticks_paper::{HIGHLIGHTED, PaperArt, PaperClicked, PaperField, PaperInput, PaperLabel, PaperPlugin, PaperSet, Spawner, WHITE};
use sticks_sim::options::{GAME_SPEEDS, LAUNCH_OPTIONS_VARIABLE, LaunchOptions, Settings, clean_name, find_export_root, save_dir};

const LAUNCHER_FILENAME: &str = "Launcher.txt";
/// Where the game's log is written when the launcher starts it.
const GAME_LOG_FILENAME: &str = "SEU2_log.txt";
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
const NAME_LENGTH: usize = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Original,
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
    Play,
    PlayOriginal,
    Quit,
}

const CHEAT_ROWS: [Action; 8] = [Action::God, Action::InfiniteLives, Action::RapidFire, Action::AllWeapons, Action::HelicopterGod, Action::Speed, Action::ShowHotkeys, Action::UnlockLevels];
const SETTING_ROWS: [Action; 5] = [Action::Original, Action::Music, Action::Sound, Action::Fullscreen, Action::VSync];

impl Action {
    /// Needs the game's debug hacks.
    fn is_hack(self) -> bool {
        matches!(self, Action::God | Action::InfiniteLives | Action::RapidFire | Action::AllWeapons | Action::HelicopterGod | Action::Speed | Action::ShowHotkeys)
    }

    fn available(self, choices: &Choices) -> bool {
        match self {
            Action::UnlockLevels => !choices.launch.original_only,
            a if a.is_hack() => CHEATS_BUILT_IN && !choices.launch.original_only,
            _ => true,
        }
    }
}

/// A launcher button and what it does.
#[derive(Component)]
struct LauncherButton(Action);

/// The player-name field.
#[derive(Component)]
struct NameField;

/// A button that is also a slider: dragging along it sets the value (`fraction`).
#[derive(Component)]
struct Slider {
    /// The filled part of the track and the knob.
    fill: Entity,
    knob: Entity,
}

/// The slider being dragged (the left button went down on it).
#[derive(Resource, Default)]
struct Dragging(Option<Entity>);

/// Track geometry inside a slider button.
const TRACK_Y: f32 = -0.62;
const TRACK_HEIGHT: f32 = 0.16;
const TRACK_MARGIN: f32 = 1.2;
const KNOB: Vec2 = Vec2::new(0.42, 0.62);

/// The status line under the Play button.
#[derive(Component)]
struct StatusLine;

#[derive(Resource)]
struct Choices {
    launch: LaunchOptions,
    settings: Settings,
    status: Option<String>,
    export_root: Option<PathBuf>,
    /// The original game's command line, when Steam started the launcher (`steam_command`).
    original: Option<Vec<OsString>>,
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

fn percent(v: f32) -> String {
    if v <= 0.0 { "Off".to_owned() } else { format!("{}%", (v * 100.0).round() as i32) }
}

impl Choices {
    fn label(&self, action: Action) -> String {
        let c = &self.launch.cheats;
        if self.launch.original_only && (action.is_hack() || action == Action::UnlockLevels) {
            return match action {
                Action::God => "Cheats: off (original game)".to_owned(),
                _ => "(original game)".to_owned(),
            };
        }
        if action.is_hack() && !CHEATS_BUILT_IN {
            return match action {
                Action::God => "God Mode: not built in".to_owned(),
                _ => "(cheats not built in)".to_owned(),
            };
        }
        match action {
            Action::Original => format!("Original Game Only: {}", on_off(self.launch.original_only)),
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
            Action::Fullscreen => format!("Display: {}", self.settings.display.name()),
            Action::VSync => format!("Vertical Sync: {}", on_off(self.settings.vsync)),
            Action::Play => "Play".to_owned(),
            Action::PlayOriginal => "Play Original (Unity)".to_owned(),
            Action::Quit => "Quit".to_owned(),
        }
    }

    /// A slider's value from 0 to 1.
    fn fraction(&self, action: Action) -> Option<f32> {
        match action {
            Action::Music => Some(self.settings.music_volume.clamp(0.0, 1.0)),
            Action::Sound => Some(self.settings.sound_volume.clamp(0.0, 1.0)),
            Action::Speed => Some(self.launch.cheats.speed as f32 / (GAME_SPEEDS.len() - 1).max(1) as f32),
            _ => None,
        }
    }

    /// Set a slider from 0 to 1 (volumes in steps of 5%, the game speed to its nearest choice).
    fn set_fraction(&mut self, action: Action, f: f32) {
        let f = f.clamp(0.0, 1.0);
        match action {
            Action::Music => self.settings.music_volume = (f * 20.0).round() / 20.0,
            Action::Sound => self.settings.sound_volume = (f * 20.0).round() / 20.0,
            Action::Speed => self.launch.cheats.speed = (f * (GAME_SPEEDS.len() - 1) as f32).round() as usize,
            _ => {}
        }
    }

    /// On/off choices that are on (drawn highlighted).
    fn is_on(&self, action: Action) -> bool {
        let c = &self.launch.cheats;
        match action {
            Action::Original => self.launch.original_only,
            Action::God => c.god,
            Action::InfiniteLives => c.infinite_lives,
            Action::RapidFire => c.rapid_fire,
            Action::AllWeapons => c.all_weapons,
            Action::HelicopterGod => c.helicopter_god,
            Action::ShowHotkeys => c.show_hotkeys,
            Action::UnlockLevels => self.launch.unlock_all_levels,
            Action::VSync => self.settings.vsync,
            _ => false,
        }
    }

    fn press(&mut self, action: Action) {
        let c = &mut self.launch.cheats;
        match action {
            Action::Original => self.launch.original_only = !self.launch.original_only,
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
            Action::Fullscreen => self.settings.display = self.settings.display.next(),
            Action::VSync => self.settings.vsync = !self.settings.vsync,
            Action::Play | Action::PlayOriginal | Action::Quit => {}
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
        let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(if cfg!(windows) { "SEU2.exe" } else { "SEU2" });
        if !exe.is_file() {
            return Err("The game (SEU2.exe) was not found\nnext to the launcher. Build it first.".to_owned());
        }
        let mut command = std::process::Command::new(&exe);
        command.env(LAUNCH_OPTIONS_VARIABLE, self.launch.effective().to_environment());
        if let Some(root) = &self.export_root {
            command.env("STICK_EXPORT_ROOT", root);
        }
        if let Some(dir) = exe.parent() {
            command.current_dir(dir);
        }
        // The game has no console: its log goes to `SEU2_log.txt` next to it (or next to the
        // save files when that folder cannot be written).
        let log = [exe.parent().map(PathBuf::from), save_dir()].into_iter().flatten().find_map(|dir| {
            let file = std::fs::File::create(dir.join(GAME_LOG_FILENAME)).ok()?;
            Some((file.try_clone().ok()?, file))
        });
        if let Some((out, err)) = log {
            command.stdout(out).stderr(err);
        }
        command.spawn().map(|_| ()).map_err(|e| format!("The game did not start:\n{e}"))
    }

    /// Run the command Steam would have run (the original Unity game).
    fn launch_original(&self) -> Result<(), String> {
        let Some((exe, args)) = self.original.as_ref().and_then(|c| c.split_first()) else {
            return Err("Start the launcher from Steam\nto play the original.".to_owned());
        };
        let exe = PathBuf::from(exe);
        let mut command = std::process::Command::new(&exe);
        command.args(args);
        if let Some(dir) = exe.parent() {
            command.current_dir(dir);
        }
        command.spawn().map(|_| ()).map_err(|e| format!("The original did not start:\n{e}"))
    }
}

/// The original game's command line when Steam started the launcher in its place
/// (`%command%` in the launch options): it starts with the game's `.exe`.
fn steam_command(args: &[OsString]) -> Option<Vec<OsString>> {
    let first = args.first()?;
    let is_exe = std::path::Path::new(first).extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    (is_exe && std::path::Path::new(first).is_file()).then(|| args.to_vec())
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let original = steam_command(&args);
    // A folder argument is the exported project (not when Steam passes the original's command).
    let folder = if original.is_some() { None } else { args.first().and_then(|a| a.to_str()) };
    let export_root = find_export_root(folder);
    let assets_dir = export_root.as_ref().map_or_else(|| PathBuf::from("assets"), |r| r.join("Assets"));
    let launch = save_dir().and_then(|d| std::fs::read_to_string(d.join(LAUNCHER_FILENAME)).ok()).map(|t| LaunchOptions::parse(&t)).unwrap_or_default();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window { title: "Stick 'Em Up 2".into(), resolution: WindowResolution::new(1280, 800), ..default() }),
                    ..default()
                })
                .set(AssetPlugin { file_path: assets_dir.to_string_lossy().into_owned(), meta_check: AssetMetaCheck::Never, ..default() }),
        )
        .add_plugins(PaperPlugin)
        .insert_resource(ClearColor(Color::srgb(sticks_paper::CLEAR[0], sticks_paper::CLEAR[1], sticks_paper::CLEAR[2])))
        .insert_resource(Choices { launch, settings: Settings::load(), status: None, export_root, original })
        .add_systems(Startup, setup)
        .init_resource::<Dragging>()
        .add_systems(Update, (clicks, drag_sliders, name_field, refresh).chain().after(PaperSet))
        .run();
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut choices: ResMut<Choices>,
) {
    let layer = RenderLayers::layer(0);
    commands.spawn(sticks_paper::camera(VIEW, 0, true, layer.clone()));
    let loaded = match &choices.export_root {
        Some(r) => PaperArt::load(r, &asset_server, &mut images, &mut materials),
        None => Err("The game's data was not found.\nRun Setup.bat first: it exports it from your\nSteam copy of Stick 'Em Up 2 (or set STICK_EXPORT_ROOT).".to_owned()),
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
    let mut s = Spawner { commands: &mut commands, art: &art, meshes: &mut meshes, layer };
    s.background(None, Vec3::new(0.0, 0.0, -10.0));
    s.title(None, Vec3::new(0.0, 11.6, -1.0), Vec2::new(34.0, 9.0));

    let row = |i: usize| FIRST_ROW_Y - i as f32 * ROW_STEP;
    let size = Vec2::new(COLUMN_WIDTH, ROW_HEIGHT);
    let mut paper = 0usize;

    heading(&mut s, "Cheats", Vec2::new(COLUMNS[0], HEADING_Y));
    for (i, &a) in CHEAT_ROWS.iter().enumerate() {
        add_button(&mut s, &mut paper, a, Vec2::new(COLUMNS[0], row(i)), size);
    }
    heading(&mut s, "Settings", Vec2::new(COLUMNS[1], HEADING_Y));
    for (i, &a) in SETTING_ROWS.iter().enumerate() {
        add_button(&mut s, &mut paper, a, Vec2::new(COLUMNS[1], row(i)), size);
    }
    heading(&mut s, "Online Name", Vec2::new(COLUMNS[1], row(5) - 0.2));
    let name = s.field(None, Vec3::new(COLUMNS[1], row(6) - 0.2, 1.0), size, 3, "Click to type a name", false, NAME_LENGTH);
    s.commands.entity(name).insert((NameField, PaperField { text: choices.launch.player_name.clone().unwrap_or_default(), placeholder: "Click to type a name".to_owned(), secret: false, max: NAME_LENGTH, focused: false }));
    note(&mut s, "Start Network Game makes a room;\ngive friends its room code.", Vec2::new(COLUMNS[1], row(7) - 0.4));

    add_button(&mut s, &mut paper, Action::Play, Vec2::new(COLUMNS[2], row(0) - 1.0), Vec2::new(COLUMN_WIDTH, ROW_HEIGHT * 2.1));
    let mut next = 2;
    if choices.original.is_some() {
        add_button(&mut s, &mut paper, Action::PlayOriginal, Vec2::new(COLUMNS[2], row(next) - 1.0), size);
        next += 1;
    }
    add_button(&mut s, &mut paper, Action::Quit, Vec2::new(COLUMNS[2], row(next) - 1.0), size);
    let cheat_note = if CHEATS_BUILT_IN {
        "Cheats are on from the start\nof every level. In a level,\nF2 lists the cheat keys."
    } else {
        "This game was built without\nits cheats; only Unlock All\nLevels works."
    };
    note(&mut s, cheat_note, Vec2::new(COLUMNS[2], row(5) - 0.4));
    let status = s.label(None, Vec3::new(COLUMNS[2], row(7) - 0.4, 0.5), "", 1.4, COLUMN_WIDTH, HIGHLIGHTED, false);
    commands.entity(status).insert(StatusLine);
    commands.insert_resource(art);
}

/// A launcher button; `paper` counts them so neighbours wear different papers.
fn add_button(s: &mut Spawner, paper: &mut usize, action: Action, at: Vec2, size: Vec2) {
    *paper += 1;
    let e = s.button(None, at.extend(1.0), size, *paper, "");
    s.commands.entity(e).insert(LauncherButton(action));
    if matches!(action, Action::Music | Action::Sound | Action::Speed) {
        // A track along the bottom of the button, its filled part and a knob.
        let track = size.x - TRACK_MARGIN * 2.0;
        s.rect(Some(e), Vec3::new(0.0, TRACK_Y, 0.3), Vec2::new(track, TRACK_HEIGHT), [0.12, 0.11, 0.1, 0.85]);
        let fill = s.rect(Some(e), Vec3::new(-track * 0.5, TRACK_Y, 0.35), Vec2::new(1.0, TRACK_HEIGHT), [0.95, 0.85, 0.45, 1.0]);
        let knob = s.rect(Some(e), Vec3::new(0.0, TRACK_Y, 0.4), KNOB, [0.97, 0.96, 0.93, 1.0]);
        s.commands.entity(e).insert(Slider { fill, knob });
    }
}

fn heading(s: &mut Spawner, text: &str, at: Vec2) {
    s.label(None, at.extend(0.5), text, 2.9, COLUMN_WIDTH, WHITE, false);
}

fn note(s: &mut Spawner, text: &str, at: Vec2) {
    s.label(None, at.extend(0.5), text, 1.45, COLUMN_WIDTH, WHITE, false);
}

fn clicks(
    mut clicked: MessageReader<PaperClicked>,
    buttons: Query<(&LauncherButton, Has<Slider>)>,
    mut choices: ResMut<Choices>,
    mut dragging: ResMut<Dragging>,
    mut exit: MessageWriter<AppExit>,
) {
    for PaperClicked(entity) in clicked.read() {
        let Ok((&LauncherButton(action), slider)) = buttons.get(*entity) else { continue };
        if slider {
            dragging.0 = Some(*entity);
            continue;
        }
        match action {
            Action::Play => {
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
            Action::PlayOriginal => {
                choices.save();
                match choices.launch_original() {
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
                choices.save();
                exit.write(AppExit::Success);
            }
            other => choices.press(other),
        }
    }
}

/// While the left button is held after going down on a slider, its value follows the cursor
/// along the track (clicking anywhere on it jumps there).
fn drag_sliders(
    mouse: Res<ButtonInput<MouseButton>>,
    input: Res<PaperInput>,
    mut dragging: ResMut<Dragging>,
    sliders: Query<(&LauncherButton, &sticks_paper::PaperButton, &GlobalTransform), With<Slider>>,
    mut choices: ResMut<Choices>,
) {
    let Some(entity) = dragging.0 else { return };
    if !mouse.pressed(MouseButton::Left) {
        dragging.0 = None;
        return;
    }
    let (Ok((&LauncherButton(action), button, at)), Some(cursor)) = (sliders.get(entity), input.cursor) else { return };
    if !action.available(&choices) {
        return;
    }
    let half = (button.size.x * 0.5 - TRACK_MARGIN) * at.scale().x;
    let f = (cursor.x - (at.translation().x - half)) / (half * 2.0).max(1e-3);
    choices.set_fraction(action, f);
}

/// The typed name is the player name (the machine's name when empty).
fn name_field(fields: Query<&PaperField, (With<NameField>, Changed<PaperField>)>, mut choices: ResMut<Choices>) {
    for field in &fields {
        let name = clean_name(&field.text);
        if choices.launch.player_name != name {
            choices.launch.player_name = name;
        }
    }
}

/// Button labels and availability follow the choices; the status line shows the last problem.
fn refresh(
    choices: Res<Choices>,
    mut buttons: Query<(&LauncherButton, &mut sticks_paper::PaperButton, &Children, Option<&Slider>)>,
    mut labels: Query<&mut PaperLabel, Without<StatusLine>>,
    mut status: Query<&mut PaperLabel, With<StatusLine>>,
    mut parts: Query<&mut Transform, Without<sticks_paper::PaperButton>>,
) {
    for (&LauncherButton(action), mut button, children, slider) in &mut buttons {
        let available = action.available(&choices);
        if button.enabled != available {
            button.enabled = available;
        }
        let on = available && choices.is_on(action);
        if button.toggled != on {
            button.toggled = on;
        }
        if let (Some(slider), Some(f)) = (slider, choices.fraction(action)) {
            let track = button.size.x - TRACK_MARGIN * 2.0;
            let f = if available { f } else { 0.0 };
            if let Ok(mut fill) = parts.get_mut(slider.fill) {
                // The fill is a unit-wide bar stretched from the track's left end.
                let target = Transform { translation: Vec3::new(-track * 0.5 + track * f * 0.5, TRACK_Y, 0.35), scale: Vec3::new((track * f).max(0.001), 1.0, 1.0), ..default() };
                if *fill != target {
                    *fill = target;
                }
            }
            if let Ok(mut knob) = parts.get_mut(slider.knob) {
                let x = -track * 0.5 + track * f;
                if knob.translation.x != x {
                    knob.translation.x = x;
                }
            }
        }
        let text = choices.label(action);
        for child in children.to_vec() {
            if let Ok(mut label) = labels.get_mut(child) {
                if label.text != text {
                    label.text = text.clone();
                }
            }
        }
    }
    if let Ok(mut line) = status.single_mut() {
        let text = choices.status.clone().unwrap_or_default();
        if line.text != text {
            line.text = text;
        }
    }
}
