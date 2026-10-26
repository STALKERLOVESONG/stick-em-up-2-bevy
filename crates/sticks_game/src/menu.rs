//! The loading screen, the pause menu and the clean-up when a game ends. The game's menus are the
//! Lobby scene (`lobby`).
//!
//! TODO(UNCERTAIN): the original's "Level Loading" scene and `InGameMenuHandler` pause menu are
//! not reconstructed yet; these two screens are functional Bevy UI with the same flow (pause with
//! Escape, back to the Lobby), not a visual copy.

use bevy::app::AppExit;
use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;

use crate::camera::LevelCamera;
use crate::hud::Hud;
use crate::level::LevelGeometry;
use crate::player::{Avatar, Player};
use crate::state::AppState;
use crate::weapon::Projectile;

const OVERLAY: Color = Color::srgba(0.0, 0.0, 0.0, 0.65);
const BUTTON_NORMAL: Color = Color::srgb(0.18, 0.2, 0.26);
const BUTTON_HOVERED: Color = Color::srgb(0.28, 0.32, 0.42);
const BUTTON_PRESSED: Color = Color::srgb(0.9, 0.35, 0.3);

/// `Time.timeScale == 0` while a menu is open: the gameplay sets do not run.
#[derive(Resource, Default)]
pub struct Paused(pub bool);

/// Run condition for the gameplay sets.
pub fn not_paused(paused: Res<Paused>) -> bool {
    !paused.0
}

#[derive(Component)]
struct PauseScreen;

#[derive(Component, Clone, Copy)]
enum MenuAction {
    Resume,
    MainMenu,
    Quit,
}

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Paused>()
            .add_systems(OnExit(AppState::InGame), cleanup_game)
            .add_systems(Update, (menu_buttons, toggle_pause.run_if(in_state(AppState::InGame))));
    }
}

/// A full-screen column of centred items.
fn screen(background: Color) -> (Node, BackgroundColor) {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            position_type: PositionType::Absolute,
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            row_gap: Val::Px(16.0),
            ..default()
        },
        BackgroundColor(background),
    )
}

fn spawn_label(commands: &mut Commands, parent: Entity, text: &str, size: f32) {
    commands.spawn((Text::new(text), TextFont::from_font_size(size), TextColor(Color::WHITE), ChildOf(parent)));
}

fn spawn_button(commands: &mut Commands, parent: Entity, label: &str, action: MenuAction) {
    let button = commands
        .spawn((
            Button,
            action,
            Node { width: Val::Px(260.0), height: Val::Px(54.0), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(BUTTON_NORMAL),
            ChildOf(parent),
        ))
        .id();
    spawn_label(commands, button, label, 28.0);
}

fn spawn_pause_screen(commands: &mut Commands) {
    let root = commands.spawn((Name::new("Pause menu"), PauseScreen, screen(OVERLAY))).id();
    spawn_label(commands, root, "PAUSED", 56.0);
    spawn_button(commands, root, "Resume", MenuAction::Resume);
    spawn_button(commands, root, "Main menu", MenuAction::MainMenu);
    spawn_button(commands, root, "Quit", MenuAction::Quit);
}

/// Button colours and what a press does.
fn menu_buttons(
    mut commands: Commands,
    mut buttons: Query<(&Interaction, &MenuAction, &mut BackgroundColor), Changed<Interaction>>,
    mut next: ResMut<NextState<AppState>>,
    mut paused: ResMut<Paused>,
) {
    for (interaction, action, mut color) in &mut buttons {
        color.0 = match interaction {
            Interaction::Pressed => BUTTON_PRESSED,
            Interaction::Hovered => BUTTON_HOVERED,
            Interaction::None => BUTTON_NORMAL,
        };
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            MenuAction::Resume => paused.0 = false,
            MenuAction::MainMenu => next.set(AppState::Lobby),
            MenuAction::Quit => {
                commands.write_message(AppExit::Success);
            }
        }
    }
}

/// Escape opens and closes the pause menu; the menu follows `Paused`.
fn toggle_pause(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut paused: ResMut<Paused>,
    screens: Query<Entity, With<PauseScreen>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        paused.0 = !paused.0;
    }
    let showing = !screens.is_empty();
    if paused.0 && !showing {
        spawn_pause_screen(&mut commands);
    } else if !paused.0 && showing {
        for entity in &screens {
            commands.entity(entity).despawn();
        }
    }
}

/// Leaving a game removes everything it spawned; the next game builds it all again.
#[allow(clippy::type_complexity)]
fn cleanup_game(
    mut commands: Commands,
    mut paused: ResMut<Paused>,
    // `Allow<Disabled>`: enemies asleep off screen are disabled entities and must go too.
    doomed: Query<
        Entity,
        (
            Or<(With<LevelGeometry>, With<LevelCamera>, With<Player>, With<Avatar>, With<Projectile>, With<Hud>, With<PauseScreen>, With<crate::unity_render::QueueRun>)>,
            Allow<Disabled>,
        ),
    >,
) {
    paused.0 = false;
    for entity in &doomed {
        commands.entity(entity).despawn();
    }
}
