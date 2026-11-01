//! Online play above the Lobby's screens (not in the original), drawn with `sticks_paper` on a
//! camera of its own:
//!
//! * a small square server button in the bottom-left corner (with the room's size beside it
//!   while in a room, and new chat lines for a few seconds) that opens the online page;
//! * the online page: offline, "Create a Room" and joining a friend's room with its code (typed
//!   or pasted with Ctrl+V); in a room, its name, size and code (Copy Room Code), its members
//!   (the host can remove one), the chat, Leave and Close;
//! * the create-room dialog (name, password, players) and the password dialog for a locked room.
//!
//! The host starts the game from the original's screens as before.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_net::RoomConfig;
use sticks_net::room::{MAX_CHAT, MAX_MACHINES, MAX_NAME};
use sticks_paper::{PaperArt, PaperClicked, PaperField, PaperLabel, PaperSubmitted, Spawner, WHITE};

use super::Lobby;
use super::network::{self, Dialog};
use crate::net::{Net, machine_name};

/// The render layer and camera order of the room's pieces (above the Lobby's camera).
const LAYER: usize = 7;
const CAMERA_ORDER: isize = 5;
/// The area the layout is made for (the camera shows at least this).
const VIEW: Vec2 = Vec2::new(58.0, 36.0);
/// How long new chat lines show beside the button.
const TOAST_SECONDS: f32 = 8.0;
const CHAT_LINES: usize = 7;
const PAGE_SIZE: Vec2 = Vec2::new(40.0, 30.0);
const BUTTON_SIZE: f32 = 3.2;
const ROWS: usize = MAX_MACHINES as usize;
const GREY: [f32; 4] = [0.85, 0.85, 0.85, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Hidden,
    /// The server button alone.
    Button,
    /// The online page, offline (create or join).
    Offline,
    /// The online page in a room.
    Room,
    CreateRoom,
    Password,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RoomAction {
    Open,
    Close,
    Leave,
    CopyCode,
    Kick(u32),
    Players,
    Create,
    Join,
    Cancel,
    OpenCreate,
    JoinCode,
}

#[derive(Component)]
pub(super) struct RoomButton(RoomAction);

/// Labels whose text follows the room.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoomText {
    Title,
    Info,
    Code,
    Chat(usize),
    Toast,
    Badge,
    JoinStatus,
}

#[derive(Component)]
pub(super) struct ChatField;

#[derive(Component)]
pub(super) struct NameField;

#[derive(Component)]
pub(super) struct PasswordField;

#[derive(Component)]
pub(super) struct CodeField;

/// What is built now.
#[derive(Resource)]
pub(super) struct RoomUi {
    built: Option<(View, u32)>,
    root: Option<Entity>,
    camera: Option<Entity>,
    members: Option<Entity>,
    members_key: Option<(bool, Vec<(u32, String)>)>,
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
        Err(e) => warn!("room art: {e}"),
    }
}

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
    let view = if crate::settings::original_only() {
        View::Hidden
    } else {
        match &lobby.network.dialog {
            Some(Dialog::CreateRoom { .. }) => View::CreateRoom,
            Some(Dialog::Password { .. }) => View::Password,
            None if !lobby.network.show_room => View::Button,
            None if net.room.is_some() => View::Room,
            None => View::Offline,
        }
    };
    // New chat lines show beside the button for a while.
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
            View::Button => build_button(&mut s, root, half),
            View::Offline => build_offline(&mut s, root, half),
            View::Room => build_room(&mut s, root, half),
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
    if view == View::Room {
        let members: Vec<(u32, String)> = net.room.as_ref().map(|r| r.members.iter().map(|m| (m.number, m.name.clone())).collect()).unwrap_or_default();
        let key = (net.is_host(), members);
        if ui.members_key.as_ref() != Some(&key) {
            if let Some(old) = ui.members.take() {
                commands.entity(old).despawn();
            }
            let mut s = Spawner { commands: &mut commands, art: &art, meshes: &mut meshes, layer };
            let node = s.node(ui.root, Vec3::new(-PAGE_SIZE.x * 0.25, 0.0, 2.0));
            let me = net.client_number.unwrap_or(u32::MAX);
            s.label(Some(node), Vec3::new(-8.2, 6.3, 0.0), "Players", 1.7, 16.0, WHITE, true);
            for (i, (number, name)) in key.1.iter().enumerate().take(ROWS) {
                let y = 4.2 - i as f32 * 2.2;
                let role = if *number == 0 { " (host)" } else { "" };
                let you = if *number == me || (*number == 0 && key.0) { " - you" } else { "" };
                s.label(Some(node), Vec3::new(-8.2, y, 0.0), &format!("{}. {name}{role}{you}", i + 1), 1.6, 13.0, WHITE, true);
                if key.0 && *number != 0 {
                    let kick = s.button(Some(node), Vec3::new(7.0, y, 0.0), Vec2::new(4.2, 1.7), i, "Kick");
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

/// The square server button: a paper square with a little rack of three server units.
fn build_button(s: &mut Spawner, root: Entity, half: Vec2) {
    let at = Vec3::new(-half.x + BUTTON_SIZE * 0.5 + 0.8, -half.y + BUTTON_SIZE * 0.5 + 0.8, 1.0);
    let button = tag_button(s, root, at, Vec2::splat(BUTTON_SIZE), 2, "", RoomAction::Open);
    for k in 0..3 {
        let y = 0.85 - k as f32 * 0.85;
        s.rect(Some(button), Vec3::new(0.0, y, 0.3), Vec2::new(2.1, 0.62), [0.12, 0.11, 0.1, 1.0]);
        s.rect(Some(button), Vec3::new(0.62, y, 0.35), Vec2::new(0.2, 0.2), [0.35, 0.9, 0.4, 1.0]);
        s.rect(Some(button), Vec3::new(-0.55, y, 0.35), Vec2::new(0.7, 0.1), [0.55, 0.55, 0.5, 1.0]);
    }
    // In a room: its size beside the button; new chat lines above it.
    tag_label(s, root, Vec3::new(at.x + BUTTON_SIZE * 0.5 + 0.5, at.y, 1.0), 1.4, 10.0, true, RoomText::Badge);
    tag_label(s, root, Vec3::new(at.x - BUTTON_SIZE * 0.5, at.y + BUTTON_SIZE * 0.5 + 2.4, 1.0), 1.2, 24.0, true, RoomText::Toast);
}

/// A page's shade (it takes the pointer from everything below) and paper.
fn page_base(s: &mut Spawner, root: Entity, half: Vec2, size: Vec2) {
    s.shade(Some(root), Vec3::new(0.0, 0.0, 0.5), half * 2.0, [0.0, 0.0, 0.0, 0.45]);
    s.panel(Some(root), Vec3::new(0.0, 0.0, 1.0), size, 1);
}

fn build_offline(s: &mut Spawner, root: Entity, half: Vec2) {
    page_base(s, root, half, Vec2::new(34.0, 24.0));
    let at = |x: f32, y: f32| Vec3::new(x, y, 2.0);
    s.label(Some(root), at(0.0, 9.6), "Online Play", 2.8, 26.0, WHITE, false);
    tag_button(s, root, at(14.0, 9.8), Vec2::new(4.6, 2.0), 3, "Close", RoomAction::Close);
    s.label(Some(root), at(0.0, 6.4), "Make a room and give friends its code:", 1.4, 30.0, GREY, false);
    tag_button(s, root, at(0.0, 3.8), Vec2::new(18.0, 2.6), 0, "Create a Room", RoomAction::OpenCreate);
    s.label(Some(root), at(0.0, -0.4), "Or join a friend's room with its code:", 1.4, 30.0, GREY, false);
    let field = s.field(Some(root), at(-4.0, -3.2), Vec2::new(22.0, 2.3), 1, "Paste the code (Ctrl+V)", false, 96);
    s.commands.entity(field).insert(CodeField);
    tag_button(s, root, at(11.0, -3.2), Vec2::new(7.0, 2.3), 2, "Join", RoomAction::JoinCode);
    tag_label(s, root, at(0.0, -7.2), 1.3, 30.0, false, RoomText::JoinStatus);
}

fn build_room(s: &mut Spawner, root: Entity, half: Vec2) {
    page_base(s, root, half, PAGE_SIZE);
    let at = |x: f32, y: f32| Vec3::new(x, y, 2.0);
    tag_label(s, root, at(-18.3, 12.6), 2.4, 24.0, true, RoomText::Title);
    tag_button(s, root, at(11.2, 12.7), Vec2::new(5.0, 1.9), 3, "Leave", RoomAction::Leave);
    tag_button(s, root, at(16.6, 12.7), Vec2::new(5.0, 1.9), 1, "Close", RoomAction::Close);
    tag_label(s, root, at(-18.3, 10.4), 1.4, 36.0, true, RoomText::Info);
    tag_label(s, root, at(-18.3, 8.4), 1.15, 26.0, true, RoomText::Code);
    tag_button(s, root, at(13.4, 8.6), Vec2::new(10.5, 2.0), 0, "Copy Room Code", RoomAction::CopyCode);
    // The chat on the right half (the members are built separately on the left).
    s.label(Some(root), at(1.5, 6.3), "Chat", 1.7, 16.0, WHITE, true);
    for i in 0..CHAT_LINES {
        tag_label(s, root, at(1.5, 4.2 - i as f32 * 1.45), 1.2, 17.5, true, RoomText::Chat(i));
    }
    let chat = s.field(Some(root), at(10.2, -11.6), Vec2::new(17.6, 2.2), 2, "Type here, Enter sends", false, MAX_CHAT);
    s.commands.entity(chat).insert(ChatField);
    s.label(Some(root), at(-9.5, -11.6), "The host starts the game from the usual screens.", 1.15, 17.0, GREY, false);
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
            RoomText::Code => match network::room_code(&net) {
                Some(code) => {
                    let groups = network::grouped(&code);
                    let words: Vec<&str> = groups.split(' ').collect();
                    let half = words.len().div_ceil(2);
                    format!("Room code: {}\n{}", words[..half].join(" "), words[half..].join(" "))
                }
                None if net.is_host() => "Room code: getting online...".to_owned(),
                None => "Joined on the local network.".to_owned(),
            },
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
            RoomText::Badge => room.map(|r| format!("{}/{}", r.members.len(), r.max_machines)).unwrap_or_default(),
            RoomText::JoinStatus => lobby.network.join_status.clone().unwrap_or_default(),
        };
        set(entity, text);
    }
    for (RoomButton(action), children) in &buttons {
        let text = match action {
            RoomAction::CopyCode => Some(lobby.network.link_feedback.as_ref().map_or_else(|| "Copy Room Code".to_owned(), |(t, _)| t.clone())),
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
    mut fields: Query<(Entity, &mut PaperField, Has<NameField>, Has<PasswordField>, Has<ChatField>, Has<CodeField>)>,
    mut ui: ResMut<RoomUi>,
    mut lobby: ResMut<Lobby>,
    mut net: ResMut<Net>,
) {
    let lobby = &mut *lobby;
    let net = &mut *net;
    let mut actions: Vec<RoomAction> = clicked.read().filter_map(|PaperClicked(e)| buttons.get(*e).ok().map(|b| b.0)).collect();
    for PaperSubmitted(entity) in submitted.read() {
        let Ok((_, mut field, name, password, chat, code)) = fields.get_mut(*entity) else { continue };
        if chat {
            let text = std::mem::take(&mut field.text);
            net.send_chat(&text);
        } else if code {
            actions.push(RoomAction::JoinCode);
        } else if name || password {
            actions.push(match lobby.network.dialog {
                Some(Dialog::Password { .. }) => RoomAction::Join,
                _ => RoomAction::Create,
            });
        }
    }
    let mut typed_name = String::new();
    let mut typed_password = String::new();
    let mut typed_code = String::new();
    for (_, field, name, password, _, code) in fields.iter() {
        if code {
            typed_code = field.text.clone();
        }
        if name {
            typed_name = field.text.clone();
        }
        if password {
            typed_password = field.text.clone();
        }
    }
    for action in actions {
        match action {
            RoomAction::Open => lobby.network.show_room = true,
            RoomAction::Close => {
                lobby.network.show_room = false;
                network::cancel_code_search(lobby);
            }
            RoomAction::Leave => network::leave(lobby, net),
            RoomAction::CopyCode => network::copy_room_code(lobby, net),
            RoomAction::Kick(number) => net.kick(number),
            RoomAction::Players => {
                ui.max_machines = if ui.max_machines >= MAX_MACHINES { 2 } else { ui.max_machines + 1 };
            }
            RoomAction::OpenCreate => {
                lobby.network.show_room = false;
                network::cancel_code_search(lobby);
                network::open_create_room(lobby);
            }
            RoomAction::Create => {
                if let Some(Dialog::CreateRoom { next, failed_label }) = lobby.network.dialog.take_if(|d| matches!(d, Dialog::CreateRoom { .. })) {
                    let config = RoomConfig::new(&typed_name, &typed_password, ui.max_machines);
                    network::create_room(lobby, net, config, next, failed_label);
                }
            }
            RoomAction::Join => {
                if let Some(Dialog::Password { listing, success, error, return_to }) = lobby.network.dialog.take_if(|d| matches!(d, Dialog::Password { .. })) {
                    network::join_with(lobby, net, &listing, &typed_password, success, error, return_to);
                }
            }
            RoomAction::Cancel => lobby.network.dialog = None,
            RoomAction::JoinCode => {
                // A search under way is not started again.
                if lobby.network.join_status.as_deref() != Some(network::LOOKING) {
                    network::start_code_search(lobby, net, &typed_code);
                }
            }
        }
    }
}

/// Leaving the Lobby (a level loads): the room's pieces go; they are built again on return.
pub(super) fn hide(mut commands: Commands, mut ui: ResMut<RoomUi>) {
    for e in [ui.root.take(), ui.camera.take()].into_iter().flatten() {
        commands.entity(e).despawn();
    }
    ui.built = None;
    ui.members = None;
    ui.members_key = None;
}
