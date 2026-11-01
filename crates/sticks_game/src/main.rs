// No console window behind the game or the launcher on Windows: the game's log goes to
// `SEU2_log.txt` next to `SEU2.exe` instead (see `log_to_file`).
#![cfg_attr(windows, windows_subsystem = "windows")]

use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;
use bevy::window::WindowPlugin;
use sticks_game::{GameConfig, GamePlugin};

/// Set on the copy of the game whose output already goes to the log file.
const LOGGING_VARIABLE: &str = "SEU2_LOGGING";

/// Without a console the log would be lost: the game starts itself again with its output in
/// `SEU2_log.txt` next to the program (or in the working folder), waits for that copy and
/// returns its exit code. `None` when it cannot (the game then runs here without a log).
#[cfg(windows)]
fn log_to_file() -> Option<i32> {
    let exe = std::env::current_exe().ok()?;
    let file = [exe.parent().map(std::path::Path::to_path_buf), std::env::current_dir().ok()]
        .into_iter()
        .flatten()
        .find_map(|dir| std::fs::File::create(dir.join("SEU2_log.txt")).ok())?;
    let err = file.try_clone().ok()?;
    let status = std::process::Command::new(&exe).args(std::env::args_os().skip(1)).env(LOGGING_VARIABLE, "1").stdout(file).stderr(err).status().ok()?;
    Some(status.code().unwrap_or(1))
}

fn main() {
    #[cfg(windows)]
    if std::env::var_os(LOGGING_VARIABLE).is_none() {
        if let Some(code) = log_to_file() {
            std::process::exit(code);
        }
    }
    let config = GameConfig::discover();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window { title: "Stick 'Em Up 2".into(), ..default() }),
                    ..default()
                })
                // All game assets are read straight from the exported Unity project.
                // The `.meta` files next to the assets are Unity's, not Bevy's; never read them.
                .set(AssetPlugin {
                    file_path: config.assets_dir().to_string_lossy().into_owned(),
                    meta_check: AssetMetaCheck::Never,
                    ..default()
                }),
        )
        .add_plugins(GamePlugin::new(config))
        .run();
}
