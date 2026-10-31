//! Stick 'Em Up 2 as a Bevy application.
//!
//! Architecture: the rules live in the engine-independent `sticks_sim` crate (collision,
//! character controller, animator, camera rig). This crate owns what Bevy is good at: entities,
//! scheduling, assets, input devices and presentation. Simulation values are wrapped in
//! components; one rendered frame runs the sets in [`FrameSet`] order, which mirrors the
//! original's order — fixed steps (`FixedUpdate` + physics), `Update` with the animator, then
//! `LateUpdate` for the camera.

pub mod audio;
pub mod vehicles;
/// Local debug hacks (git-ignored file; `build.rs` sets `debug_hacks` when it exists).
#[cfg(debug_hacks)]
mod hacks;
pub mod camera;
pub mod config;
pub mod debug;
pub mod devscript;
pub mod effects;
pub mod enemy;
pub mod figure;
pub mod helicopter;
pub mod hud;
pub mod ingame_ui;
pub mod input;
pub mod letters;
pub mod loading;
pub mod level;
pub mod level_end;
pub mod life;
pub mod lobby;
pub mod menu;
pub mod net;
pub mod player;
pub mod save;
pub mod scenery;
pub mod settings;
pub mod state;
pub mod unity_render;
pub mod versus;
pub mod weapon;

use bevy::prelude::*;

pub use config::GameConfig;
pub use state::AppState;

/// Ordering of one rendered frame (see crate docs). Everything runs in `Update`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum FrameSet {
    /// Frame clock and device input.
    Input,
    /// Zero or more fixed steps: character `FixedUpdate` then the physics step.
    FixedSteps,
    /// `CharacterInput.Update` (script execution order -100, the first `Update`): every
    /// character copies its processor (devices or enemy AI) into the input it acts on. The fixed
    /// steps above therefore use what the previous frame copied, as in the original.
    RoleInput,
    /// Character `Update`, animator evaluation and `OnAnimatorMove`.
    Characters,
    /// Camera `LateUpdate`.
    Camera,
    /// Copy simulation results into Bevy transforms.
    Present,
}

pub struct GamePlugin {
    config: GameConfig,
}

impl GamePlugin {
    pub fn new(config: GameConfig) -> Self {
        Self { config }
    }
}

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.config.clone())
            .insert_resource(level::LevelSource(self.config.level_asset.clone()))
            .insert_state(if self.config.level_from_command_line { AppState::Loading } else { AppState::Lobby })
            .configure_sets(
                Update,
                (FrameSet::Input, FrameSet::FixedSteps, FrameSet::RoleInput, FrameSet::Characters, FrameSet::Camera, FrameSet::Present)
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            )
            // `Time.timeScale == 0` in a `Pause` menu: only the presentation and the screens keep
            // running.
            .configure_sets(Update, (FrameSet::Input, FrameSet::FixedSteps, FrameSet::RoleInput, FrameSet::Characters, FrameSet::Camera).run_if(menu::not_paused))
            // A network game starts once every machine has loaded the level.
            .configure_sets(Update, (FrameSet::Input, FrameSet::FixedSteps, FrameSet::RoleInput, FrameSet::Characters, FrameSet::Camera).run_if(net::gameplay_ready))
            .add_plugins((level::LevelPlugin, life::LifePlugin, player::PlayerPlugin, camera::CameraPlugin, weapon::WeaponPlugin, enemy::EnemyPlugin, figure::FigurePlugin, menu::MenuPlugin, hud::HudPlugin, letters::LettersPlugin, helicopter::HelicopterPlugin, loading::LoadingPlugin, debug::DebugPlugin, devscript::DevScriptPlugin))
            .add_plugins((unity_render::UnityRenderPlugin, lobby::LobbyPlugin, effects::EffectsPlugin, audio::GameAudioPlugin, vehicles::VehiclePlugin, ingame_ui::InGameUiPlugin, level_end::LevelEndPlugin, save::SavePlugin, scenery::SceneryPlugin, input::InputPlugin, versus::VersusPlugin, net::NetPlugin, settings::SettingsPlugin));
        #[cfg(debug_hacks)]
        app.add_plugins(hacks::HacksPlugin);
    }
}
