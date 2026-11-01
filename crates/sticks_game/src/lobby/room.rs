//! The network room above the Lobby's screens (not in the original), drawn with `sticks_paper`
//! on a camera of its own:
//!
//! * a bar at the top right while in a room ("Room: name 2/4"; new chat lines show under it for
//!   a few seconds) that opens the room panel;
//! * the panel: the room's name and size, its members (the host can remove one), Copy Invite,
//!   Ready (clients) or how many are ready (host), the chat, Hide and Leave;
//! * the create-room dialog ("Start Network Game": name, password, players) and the password
//!   dialog for a locked room.
//!
//! The host starts the game from the original's screens as before; it starts once every member
//! is ready (`super::start_game`).

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_net::RoomConfig;
use sticks_net::room::{MAX_CHAT, MAX_MACHINES, MAX_NAME};
use sticks_paper::{PaperArt, PaperButton, PaperClicked, PaperField, PaperLabel, PaperSubmitted, Spawner, WHITE};

use super::Lobby;
use super::network::{self, Dialog};
use crate::net::{Net, machine_name};

/// The render layer and camera order of the room's pieces (above the Lobby's camera).
const LAYER: usize = 7;
const CAMERA_ORDER: isize = 5;
/// The area the layout is made for (the camera shows at least this).
const VIEW: Vec2 = Vec2::new(58.0, 36.0);
/// How long new chat lines show under the bar.
const TOAST_SECONDS: f32 = 8.0;
const CHAT_LINES: usize = 8;
const PANEL_SIZE: Vec2 = Vec2::new(25.0, 34.5);
const PANEL_ROWS: usize = MAX_MACHINES as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Hidden,
    Bar,
    Panel,
    CreateRoom,
    Password,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RoomAction {
    Toggle,
    Hide,
    Leave,
    Invite,
    Ready,
    Kick(u32),
    Players,
    Create,
    Join,
    Cancel,
}

#[derive(Component)]
pub(super) struct RoomButton(RoomAction);

/// Labels whose text follows the room.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoomText {
    Title,
    Info,
    Chat(usize),
    Toast,
}

#[derive(Component)]
pub(super) struct ChatField;

#[derive(Component)]
pub(super) struct NameField;

#[derive(Component)]
pub(super) struct PasswordField;

/// What is built now.
#[derive(Resource)]
pub(super) struct RoomUi {
    built: Option<(View, u32)>,
    root: Option<Entity>,
    camera: Option<Entity>,
    members: Option<Entity>,
    members_key: Option<(bool, Vec<(u32, String, bool)>)>,
    /// The create-room dialog's size choice.
    max_machines: u8,
    seen_chat: usize,
    toast_left: f32,
}

impl Default for RoomUi {
    fn default() -> Self {
        RoomUi { built: None, root: None, camera: None, members: None, members_key: None, max_machines: MAX_MACHINES, seen_chat: 0, toast_left: 0.0 }
    }
}

/// Load the paper art once (the room's look).
pub(super) fn load_art(mut commands: Commands, config: Res<crate::config::GameConfig>, asset_server: Res<AssetServer>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<ColorMaterial>>) {
    match PaperArt::load(&config.export_root, &asset_server, &mut images, &mut materials) {
        Ok(art) => commands.insert_resource(art),
        Err(e) => warn!("the room panel cannot be drawn: {e}"),
    }
}

/// Half the visible area of the room camera.
fn half_view(window: Option<&Window>) -> Vec2 {
    let aspect = window.map_or(16.0 / 9.0, |w| w.width() / w.height().max(1.0));
    Vec2::new((VIEW.y * 0.5 * aspect).max(VIEW.x * 0.5), (VIEW.x * 0.5 / aspect).max(VIEW.y * 0.5))
}

/// Build, rebuild or remove the room's pieces as the network state changes.
#[allow(clippy::too_many_arguments)]
pub(super) fn room_view(
    mut commands: Commands,
    art: Option<Res<PaperArt>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ui: ResMut<RoomUi>,
    lobby: Res<Lobby>,
    net: Res<Net>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    let Some(art) = art else { return };
    let view = match &lobby.network.dialog {
        Some(Dialog::CreateRoom { .. }) => View::CreateRoom,
        Some(Dialog::Password { .. }) => View::Password,
        None if net.room.is_some() => {
            if lobby.network.show_room {
                View::Panel
            } else {
                View::Bar
            }
        }
        None => View::Hidden,
    };
    // New chat lines show under the bar for a while.
    if net.chat.len() != ui.seen_chat {
        if net.chat.len() > ui.seen_chat {
            ui.toast_left = TOAST_SECONDS;
        }
        ui.seen_chat = net.chat.len();
    }
    ui.toast_left = (ui.toast_left - time.delta_secs()).max(0.0);

    let half = half_view(windows.single().ok());
    let key = (view, (half.x * 100.0) as u32);
    let layer = RenderLayers::layer(LAYER);
    if ui.built != Some(key) {
        if let Some(root) = ui.root.take() {
            commands.entity(root).despawn();
        }
        ui.members = None;
        ui.members_key = None;
        ui.built = Some(key);
        if view == View::Hidden {
            if let Some(camera) = ui.camera.take() {
                commands.entity(camera).despawn();
            }
            return;
        }
        if ui.camera.is_none() {
            ui.camera = Some(commands.spawn(sticks_paper::camera(VIEW, CAMERA_ORDER, false, layer.clone())).id());
        }
        let mut s = Spawner { commands: &mut commands, art: &art, meshes: &mut meshes, layer: layer.clone() };
        let root = s.node(None, Vec3::ZERO);
        match view {
            View::Bar => build_bar(&mut s, root, half),
            View::Panel => build_panel(&mut s, root, half, &net),
            View::CreateRoom => build_create(&mut s, root, half, ui.max_machines),
            View::Password => {
                let name = match &lobby.network.dialog {
                    Some(Dialog::Password { listing, .. }) => listing.room().name,
                    _ => String::new(),
                };
                build_password(&mut s, root, half, &name);
            }
            View::Hidden => {}
        }
        ui.root = Some(root);
    }
    // The member rows follow the room.
    if view == View::Panel {
        let members: Vec<(u32, String, bool)> = net.room.as_ref().map(|r| r.members.iter().map(|m| (m.number, m.name.clone(), m.ready)).collect()).unwrap_or_default();
        let key = (net.is_host(), members);
        if ui.members_key.as_ref() != Some(&key) {
            if let Some(old) = ui.members.take() {
                commands.entity(old).despawn();
            }
            let mut s = Spawner { commands: &mut commands, art: &art, meshes: &mut meshes, layer };
            let node = s.node(ui.root, Vec3::new(half.x - PANEL_SIZE.x * 0.5 - 0.6, 0.0, 2.0));
            let me = net.client_number.unwrap_or(u32::MAX);
            for (i, (number, name, ready)) in key.1.iter().enumerate().take(PANEL_ROWS) {
                let y = 10.6 - i as f32 * 2.3;
                let what = if *number == 0 {
                    "host"
                } else if *ready {
                    "ready"
                } else {
                    "not ready"
                };
                let you = if *number == me { " (you)" } else { "" };
                s.label(Some(node), Vec3::new(-11.3, y, 0.0), &format!("{name}{you} - {what}"), 1.8, 16.0, if *ready { WHITE } else { [0.85, 0.85, 0.85, 1.0] }, true);
                if key.0 && *number != 0 {
                    let kick = s.button(Some(node), Vec3::new(8.6, y, 0.0), Vec2::new(5.0, 1.8), i, "Kick");
                    s.commands.entity(kick).insert(RoomButton(RoomAction::Kick(*number)));
                }
            }
            ui.members = Some(node);
            ui.members_key = Some(key);
        }
    }
}

fn tag_button(s: &mut Spawner, parent: Entity, at: Vec3, size: Vec2, which: usize, text: &str, action: RoomAction) -> Entity {
    let b = s.button(Some(parent), at, size, which, text);
    s.commands.entity(b).insert(RoomButton(action));
    b
}

fn tag_label(s: &mut Spawner, parent: Entity, at: Vec3, height: f32, width: f32, left: bool, what: RoomText) {
    let l = s.label(Some(parent), at, "", height, width, WHITE, left);
    s.commands.entity(l).insert(what);
}

fn build_bar(s: &mut Spawner, root: Entity, half: Vec2) {
    let at = Vec3::new(half.x - 9.6, half.y - 1.7, 1.0);
    tag_button(s, root, at, Vec2::new(17.5, 2.2), 0, "", RoomAction::Toggle);
    tag_label(s, root, Vec3::new(half.x - 18.0, half.y - 5.2, 1.0), 1.25, 17.5, true, RoomText::Toast);
}

fn build_panel(s: &mut Spawner, root: Entity, half: Vec2, net: &Net) {
    let cx = half.x - PANEL_SIZE.x * 0.5 - 0.6;
    s.panel(Some(root), Vec3::new(cx, 0.0, 1.0), PANEL_SIZE, 2);
    let at = |x: f32, y: f32| Vec3::new(cx + x, y, 2.0);
    tag_label(s, root, at(-11.3, 15.3), 2.4, 13.5, true, RoomText::Title);
    tag_button(s, root, at(5.4, 15.4), Vec2::new(5.0, 1.9), 1, "Hide", RoomAction::Hide);
    tag_button(s, root, at(10.4, 15.4), Vec2::new(5.0, 1.9), 3, "Leave", RoomAction::Leave);
    tag_label(s, root, at(-11.3, 13.0), 1.4, 22.0, true, RoomText::Info);
    tag_button(s, root, at(-5.8, 0.9), Vec2::new(10.8, 2.2), 0, "Copy Invite", RoomAction::Invite);
    let ready = tag_button(s, root, at(5.8, 0.9), Vec2::new(10.8, 2.2), 1, "", RoomAction::Ready);
    if net.is_host() {
        // The host's column shows how many are ready; it starts the game itself.
        s.commands.entity(ready).insert(PaperButton { size: Vec2::new(10.8, 2.2), enabled: false, toggled: false, hovered: false });
    }
    for i in 0..CHAT_LINES {
        tag_label(s, root, at(-11.3, -2.6 - i as f32 * 1.4), 1.25, 22.5, true, RoomText::Chat(i));
    }
    let chat = s.field(Some(root), at(0.0, -15.0), Vec2::new(23.0, 2.2), 2, "Type here, Enter sends", false, MAX_CHAT);
    s.commands.entity(chat).insert(ChatField);
}

/// A dialog's shade (it takes the pointer from everything below) and paper.
fn dialog_base(s: &mut Spawner, root: Entity, half: Vec2, size: Vec2) {
    s.shade(Some(root), Vec3::new(0.0, 0.0, 5.0), half * 2.0, [0.0, 0.0, 0.0, 0.45]);
    s.panel(Some(root), Vec3::new(0.0, 0.0, 6.0), size, 1);
}

fn build_create(s: &mut Spawner, root: Entity, half: Vec2, max_machines: u8) {
    dialog_base(s, root, half, Vec2::new(26.0, 21.0));
    s.label(Some(root), Vec3::new(0.0, 7.6, 7.0), "Create a Room", 2.8, 22.0, WHITE, false);
    let mut name = machine_name();
    name.push_str("'s Room");
    let name = sticks_net::room::clean(&name, MAX_NAME);
    let field = s.field(Some(root), Vec3::new(0.0, 3.8, 7.0), Vec2::new(22.0, 2.3), 0, "Room name", false, MAX_NAME);
    s.commands.entity(field).insert((NameField, PaperField { text: name, placeholder: "Room name".to_owned(), secret: false, max: MAX_NAME, focused: true }));
    let pw = s.field(Some(root), Vec3::new(0.0, 0.8, 7.0), Vec2::new(22.0, 2.3), 1, "No password (anyone can join)", true, MAX_NAME);
    s.commands.entity(pw).insert(PasswordField);
    tag_button(s, root, Vec3::new(0.0, -2.3, 7.0), Vec2::new(22.0, 2.2), 2, &format!("Players: {max_machines}"), RoomAction::Players);
    tag_button(s, root, Vec3::new(-5.8, -6.6, 7.0), Vec2::new(10.5, 2.5), 3, "Create", RoomAction::Create);
    tag_button(s, root, Vec3::new(5.8, -6.6, 7.0), Vec2::new(10.5, 2.5), 0, "Cancel", RoomAction::Cancel);
}

fn build_password(s: &mut Spawner, root: Entity, half: Vec2, room: &str) {
    dialog_base(s, root, half, Vec2::new(26.0, 15.0));
    s.label(Some(root), Vec3::new(0.0, 4.4, 7.0), &format!("Password for {room}"), 2.4, 22.0, WHITE, false);
    let pw = s.field(Some(root), Vec3::new(0.0, 0.6, 7.0), Vec2::new(22.0, 2.3), 1, "Password", true, MAX_NAME);
    s.commands.entity(pw).insert((PasswordField, PaperField { text: String::new(), placeholder: "Password".to_owned(), secret: true, max: MAX_NAME, focused: true }));
    tag_button(s, root, Vec3::new(-5.8, -3.6, 7.0), Vec2::new(10.5, 2.5), 3, "Join", RoomAction::Join);
    tag_button(s, root, Vec3::new(5.8, -3.6, 7.0), Vec2::new(10.5, 2.5), 0, "Cancel", RoomAction::Cancel);
}

/// The texts that follow the room, the chat and the buttons' states.
pub(super) fn room_labels(
    ui: Res<RoomUi>,
    lobby: Res<Lobby>,
    net: Res<Net>,
    texts: Query<(Entity, &RoomText)>,
    buttons: Query<(&RoomButton, &Children)>,
    mut labels: Query<&mut PaperLabel>,
) {
    let mut set = |entity: Entity, text: String| {
        if let Ok(mut label) = labels.get_mut(entity) {
            if label.text != text {
                label.text = text;
            }
        }
    };
    let room = net.room.as_ref();
    let line = |l: &crate::net::ChatLine| match &l.from {
        Some(from) => format!("{from}: {}", l.text),
        None => format!("* {}", l.text),
    };
    for (entity, what) in &texts {
        let text = match *what {
            RoomText::Title => room.map(|r| r.name.clone()).unwrap_or_default(),
            RoomText::Info => room
                .map(|r| format!("{}/{} players{}{}", r.members.len(), r.max_machines, if r.locked { ", password" } else { "" }, if net.is_host() { " - you host" } else { "" }))
                .unwrap_or_default(),
            RoomText::Chat(i) => {
                let first = net.chat.len().saturating_sub(CHAT_LINES);
                net.chat.get(first + i).map(line).unwrap_or_default()
            }
            RoomText::Toast => {
                if ui.toast_left > 0.0 {
                    let first = net.chat.len().saturating_sub(3);
                    net.chat[first..].iter().map(line).collect::<Vec<_>>().join("\n")
                } else {
                    String::new()
                }
            }
        };
        set(entity, text);
    }
    for (RoomButton(action), children) in &buttons {
        let text = match action {
            RoomAction::Toggle => room.map(|r| format!("Room: {} {}/{}", r.name, r.members.len(), r.max_machines)),
            RoomAction::Ready => Some(if net.is_host() {
                let ready = room.map_or(0, |r| r.members.iter().filter(|m| m.ready).count());
                format!("{ready}/{} ready", room.map_or(1, |r| r.members.len()))
            } else if net.is_ready() {
                "Ready!".to_owned()
            } else {
                "Not Ready".to_owned()
            }),
            RoomAction::Invite => Some(lobby.network.link_feedback.as_ref().map_or_else(|| "Copy Invite".to_owned(), |(t, _)| t.clone())),
            RoomAction::Players => Some(format!("Players: {}", ui.max_machines)),
            _ => None,
        };
        if let Some(text) = text {
            for child in children.to_vec() {
                set(child, text.clone());
            }
        }
    }
}

/// Clicks and Enter in the room's pieces.
#[allow(clippy::too_many_arguments)]
pub(super) fn room_actions(
    mut clicked: MessageReader<PaperClicked>,
    mut submitted: MessageReader<PaperSubmitted>,
    buttons: Query<&RoomButton>,
    mut fields: Query<(Entity, &mut PaperField, Has<NameField>, Has<PasswordField>, Has<ChatField>)>,
    mut ui: ResMut<RoomUi>,
    mut lobby: ResMut<Lobby>,
    mut net: ResMut<Net>,
) {
    let lobby = &mut *lobby;
    let net = &mut *net;
    let mut actions: Vec<RoomAction> = clicked.read().filter_map(|PaperClicked(e)| buttons.get(*e).ok().map(|b| b.0)).collect();
    for PaperSubmitted(entity) in submitted.read() {
        let Ok((_, mut field, name, password, chat)) = fields.get_mut(*entity) else { continue };
        if chat {
            let text = std::mem::take(&mut field.text);
            net.send_chat(&text);
        } else if name || password {
            actions.push(match lobby.network.dialog {
                Some(Dialog::Password { .. }) => RoomAction::Join,
                _ => RoomAction::Create,
            });
        }
    }
    let mut typed_name = String::new();
    let mut typed_password = String::new();
    for (_, field, name, password, _) in fields.iter() {
        if name {
            typed_name = field.text.clone();
        }
        if password {
            typed_password = field.text.clone();
        }
    }
    for action in actions {
        match action {
            RoomAction::Toggle => lobby.network.show_room = !lobby.network.show_room,
            RoomAction::Hide => lobby.network.show_room = false,
            RoomAction::Leave => network::leave(lobby, net),
            RoomAction::Invite => network::send_link(lobby, net),
            RoomAction::Ready => {
                let ready = net.is_ready();
                net.set_ready(!ready);
            }
            RoomAction::Kick(number) => net.kick(number),
            RoomAction::Players => {
                ui.max_machines = if ui.max_machines >= MAX_MACHINES { 2 } else { ui.max_machines + 1 };
            }
            RoomAction::Create => {
                if let Some(Dialog::CreateRoom { next, failed_label }) = lobby.network.dialog.take_if(|d| matches!(d, Dialog::CreateRoom { .. })) {
                    let config = RoomConfig::new(&typed_name, &typed_password, ui.max_machines);
                    network::create_room(lobby, net, config, next, failed_label);
                    lobby.network.show_room = true;
                }
            }
            RoomAction::Join => {
                if let Some(Dialog::Password { listing, success, error, return_to }) = lobby.network.dialog.take_if(|d| matches!(d, Dialog::Password { .. })) {
                    network::join_with(lobby, net, &listing, &typed_password, success, error, return_to);
                }
            }
            RoomAction::Cancel => lobby.network.dialog = None,
        }
    }
}

/// Leaving the Lobby (a level loads): the room's pieces go; they are built again on return.
pub(super) fn hide(mut commands: Commands, mut ui: ResMut<RoomUi>) {
    for e in [ui.root.take(), ui.camera.take()].into_iter().flatten() {
        commands.entity(e).despawn();
    }
    ui.members = None;
    ui.members_key = None;
    ui.built = None;
}
