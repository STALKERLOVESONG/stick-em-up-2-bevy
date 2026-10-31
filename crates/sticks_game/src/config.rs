use std::path::PathBuf;

use bevy::prelude::*;

/// Where the recovered game data lives and which level to play.
#[derive(Resource, Clone, Debug)]
pub struct GameConfig {
    /// The `ExportedProject` directory (contains `Assets/` and `ProjectSettings/`).
    pub export_root: PathBuf,
    /// Level scene, relative to `export_root`.
    pub level_asset: String,
    /// Development path: a level was named on the command line, so the game skips the Lobby and
    /// loads it at once (as a single-player campaign game).
    pub level_from_command_line: bool,
}

const DEFAULT_LEVEL: &str = "Assets/Game/Levels/Platforms/Platforms.unity";

impl GameConfig {
    /// Resolve the export directory from, in order: `STICK_EXPORT_ROOT`, the first command-line
    /// argument, or an `assets_exported/ExportedProject` directory found by walking up from the
    /// working directory and the executable. A second argument selects the level scene.
    ///
    /// # Panics
    /// If no export directory can be found; the game cannot run without its data.
    pub fn discover() -> GameConfig {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let export_root = sticks_sim::options::find_export_root(args.first().map(String::as_str)).unwrap_or_else(|| {
            panic!(
                "could not find the exported Unity project. Set STICK_EXPORT_ROOT (or pass the path as the first \
                 argument) to the `ExportedProject` directory inside `assets_exported`."
            )
        });
        let level_from_command_line = args.get(1).is_some();
        let level_asset = args.get(1).cloned().unwrap_or_else(|| DEFAULT_LEVEL.to_owned());
        GameConfig { export_root, level_asset, level_from_command_line }
    }

    /// The export's `Assets/` directory; also the Bevy asset root.
    pub fn assets_dir(&self) -> PathBuf {
        self.export_root.join("Assets")
    }
}
