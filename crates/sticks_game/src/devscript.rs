//! Development aid, not part of the original game: drive the running game from a script so it
//! can be checked unattended (screenshots of what is really drawn, scripted keys and aim).
//!
//! `STICK_SCRIPT` holds `;`-separated steps `<frame> <command> [args]`, frames counted from
//! application start:
//!
//! * `shot <name>` – save a screenshot to `STICK_CAPTURE_DIR/<name>.png` (default `captures`);
//! * `key <KeyName> down|up` – press or release a keyboard key (`KeyD`, `Space`, `Enter`, ...);
//! * `aim <x> <y>` – aim the player at this offset from its weapon pivot instead of the mouse;
//!   `aim off` gives the mouse back;
//! * `mouse <x> <y>` – put the (scripted) cursor at this window position (logical pixels) for the
//!   Lobby's mouse picking; `click` presses and, the next frame, releases the left button;
//!   `press` / `release` hold and let go of it (dragging);
//! * `damage <amount>` – damage the local player's avatar (`Health.Damage` through the normal
//!   damage path, as an Abyss trigger would), to check deaths and explosions on screen;
//! * `teleport <x> <y>` – put the local player's avatar at this position (game units);
//! * `exit` – quit.
//!
//! Example: `STICK_SCRIPT="200 shot start; 210 aim 0 8; 240 shot up; 250 aim 0 -8; 280 shot down; 300 exit"`.

use std::path::PathBuf;

use bevy::app::AppExit;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

/// The aim the script holds, as an offset from the player's weapon pivot (game units).
#[derive(Resource, Default)]
pub struct ScriptedAim(pub Option<Vec2>);

/// The cursor position the script holds (window logical pixels), instead of the real one.
#[derive(Resource, Default)]
pub struct ScriptedCursor(pub Option<Vec2>);

#[derive(Debug, Clone)]
enum Command {
    Shot(String),
    Key(KeyCode, bool),
    Aim(Option<Vec2>),
    Mouse(Vec2),
    Click,
    Press,
    Release,
    Damage(f32),
    Teleport(Vec2),
    Exit,
}

#[derive(Resource)]
struct DevScript {
    steps: Vec<(u64, Command)>,
    next: usize,
    frame: u64,
    dir: PathBuf,
}

pub struct DevScriptPlugin;

impl Plugin for DevScriptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScriptedAim>().init_resource::<ScriptedCursor>();
        let Ok(text) = std::env::var("STICK_SCRIPT") else { return };
        let steps = parse(&text);
        let dir = std::env::var_os("STICK_CAPTURE_DIR").map_or_else(|| PathBuf::from("captures"), PathBuf::from);
        let _ = std::fs::create_dir_all(&dir);
        info!("dev script: {} steps, captures in {}", steps.len(), dir.display());
        app.insert_resource(DevScript { steps, next: 0, frame: 0, dir })
            .add_systems(PreUpdate, run_script.after(InputSystems));
    }
}

fn key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Space" => KeyCode::Space,
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "F1" => KeyCode::F1,
        n if n.len() == 4 && n.starts_with("Key") => match n.as_bytes()[3] {
            b'A' => KeyCode::KeyA,
            b'D' => KeyCode::KeyD,
            b'S' => KeyCode::KeyS,
            b'W' => KeyCode::KeyW,
            b'Z' => KeyCode::KeyZ,
            b'X' => KeyCode::KeyX,
            b'C' => KeyCode::KeyC,
            b'E' => KeyCode::KeyE,
            b'Q' => KeyCode::KeyQ,
            _ => return None,
        },
        _ => return None,
    })
}

fn parse(text: &str) -> Vec<(u64, Command)> {
    let mut steps = Vec::new();
    for step in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let words: Vec<&str> = step.split_whitespace().collect();
        let Some(frame) = words.first().and_then(|w| w.parse::<u64>().ok()) else {
            warn!("dev script: bad step {step:?}");
            continue;
        };
        let command = match words.get(1..).unwrap_or_default() {
            ["shot", name] => Command::Shot((*name).to_owned()),
            ["key", key, state] => match key_code(key) {
                Some(code) => Command::Key(code, *state == "down"),
                None => {
                    warn!("dev script: unknown key {key}");
                    continue;
                }
            },
            ["aim", "off"] => Command::Aim(None),
            ["aim", x, y] => match (x.parse(), y.parse()) {
                (Ok(x), Ok(y)) => Command::Aim(Some(Vec2::new(x, y))),
                _ => continue,
            },
            ["mouse", x, y] => match (x.parse(), y.parse()) {
                (Ok(x), Ok(y)) => Command::Mouse(Vec2::new(x, y)),
                _ => continue,
            },
            ["click"] => Command::Click,
            ["press"] => Command::Press,
            ["release"] => Command::Release,
            ["teleport", x, y] => match (x.parse(), y.parse()) {
                (Ok(x), Ok(y)) => Command::Teleport(Vec2::new(x, y)),
                _ => continue,
            },
            ["damage", amount] => match amount.parse() {
                Ok(amount) => Command::Damage(amount),
                _ => continue,
            },
            ["exit"] => Command::Exit,
            other => {
                warn!("dev script: unknown command {other:?}");
                continue;
            }
        };
        steps.push((frame, command));
    }
    steps.sort_by_key(|(frame, _)| *frame);
    steps
}

fn run_script(
    mut commands: Commands,
    mut script: ResMut<DevScript>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut aim: ResMut<ScriptedAim>,
    mut cursor: ResMut<ScriptedCursor>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut exit: MessageWriter<AppExit>,
    mut damage: MessageWriter<crate::life::Damage>,
    mut players: Query<(Entity, &mut crate::player::Avatar), (With<crate::player::ControlledBy>, Without<crate::enemy::Enemy>)>,
    mut release: Local<bool>,
) {
    script.frame += 1;
    if *release {
        *release = false;
        mouse.release(MouseButton::Left);
    }
    while let Some((frame, command)) = script.steps.get(script.next).cloned() {
        if frame > script.frame {
            break;
        }
        script.next += 1;
        info!("dev script frame {}: {command:?}", script.frame);
        match command {
            Command::Shot(name) => {
                let path = script.dir.join(format!("{name}.png"));
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            }
            Command::Key(code, true) => keys.press(code),
            Command::Key(code, false) => keys.release(code),
            Command::Aim(value) => aim.0 = value,
            Command::Mouse(p) => cursor.0 = Some(p),
            Command::Press => mouse.press(MouseButton::Left),
            Command::Release => mouse.release(MouseButton::Left),
            Command::Click => {
                mouse.press(MouseButton::Left);
                *release = true;
            }
            Command::Damage(amount) => {
                for (target, _) in &players {
                    damage.write(crate::life::Damage { target, amount, on_kill: crate::life::OnKill::Nothing, responsible: None });
                }
            }
            Command::Teleport(p) => {
                for (_, mut avatar) in &mut players {
                    avatar.body.position = unity_compat::Vec2::new(p.x, p.y);
                    avatar.body.velocity = unity_compat::Vec2::ZERO;
                }
            }
            Command::Exit => {
                exit.write(AppExit::Success);
            }
        }
    }
}
