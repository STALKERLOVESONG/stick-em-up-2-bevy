use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;
use bevy::window::WindowPlugin;
use sticks_game::{GameConfig, GamePlugin};

fn main() {
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
