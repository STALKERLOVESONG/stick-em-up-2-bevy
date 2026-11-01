//! Player settings (not in the original, which had none on desktop): music and sound volume,
//! fullscreen, vertical sync. Chosen in the launcher (the game has no settings screen), read from
//! `Settings.txt` next to the save files (`sticks_sim::options`) and applied to the window and
//! the audio.
//!
//! Also the options a launcher started the game with ([`Launch`]).

use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
pub use sticks_sim::options::{Cheats, LaunchOptions, Settings};

use crate::lobby::LobbyMemory;

/// The options this run was launched with (`sticks_sim::options::LAUNCH_OPTIONS_VARIABLE`).
#[derive(Resource, Clone, Debug, Default)]
pub struct Launch(pub LaunchOptions);

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        let launch = LaunchOptions::from_environment();
        if launch != LaunchOptions::default() {
            info!("launched with {}", launch.to_environment());
        }
        app.insert_resource(Launch(launch)).add_systems(Startup, apply_launch_options).add_systems(Last, apply_settings);
    }
}

/// `AllLevelUnlockCheat.Enabled` from the launcher.
fn apply_launch_options(launch: Res<Launch>, mut memory: ResMut<LobbyMemory>) {
    if launch.0.unlock_all_levels {
        memory.all_levels_unlocked = true;
    }
}

/// The window follows the settings; changed settings are saved.
fn apply_settings(memory: Res<LobbyMemory>, mut windows: Query<&mut Window, With<PrimaryWindow>>, mut applied: Local<Option<Settings>>) {
    let settings = memory.settings;
    if *applied == Some(settings) {
        return;
    }
    let first = applied.is_none();
    *applied = Some(settings);
    if !first {
        if let Err(e) = settings.save() {
            warn!("{}: {e}", sticks_sim::options::SETTINGS_FILENAME);
        }
    }
    let Ok(mut window) = windows.single_mut() else { return };
    let mode = if settings.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    if window.mode != mode {
        window.mode = mode;
    }
    let present = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if window.present_mode != present {
        window.present_mode = present;
    }
}
