//! The Lobby's network screens and the lobby state every machine of a network game shares:
//!
//! * `NetworkSetup` ("Start Network Game", "Join Game: …", "Show Network Games (N found)") and
//!   `ServerList` (pages of hosts heard on the LAN), with `ServerList.ConnectToServerOrShowVersionError`;
//! * `LobbyEnder` (Back offline, Disconnect for a client, Stop Server for the host) and
//!   `LobbyClientDisconnection` (the "connection … was lost" error);
//! * `ControlAssignment` in a network game: columns played on other machines carry the
//!   "assigned elsewhere" label and take no cards, claims go to the host;
//! * `LobbyGameMode` / `LobbyDifficulty` / `LobbyLevelSelect` / `LobbyTeams` /
//!   `RemoteColorAssignments`: changes are sent, received ones applied;
//! * Rooms (not in the original): "Start Network Game" opens the create-room dialog (name,
//!   password, size; `super::room`), a locked room asks for its password before joining, and
//!   the join buttons show each room's name, size and lock;
//! * `SendStarterEditionLinkPrompt`: the original hides its "Send Link" button on desktop (it
//!   shared a store link to the free Starter Edition on phones); so does this.
//! * Room codes (not in the original): a room's code is its host's game code (`sticks_p2p`).
//!   The room panel shows it with Copy Room Code; "Join with Room Code" on the connection
//!   screen takes one (typed or pasted) and joins that room over the internet, peer to peer.
//!   A code on the clipboard is noticed too: while its host says the room is open, it is listed
//!   on the connection screen next to the games heard on the LAN.

use std::net::IpAddr;
use std::sync::Mutex;
use std::sync::mpsc::Receiver;

use bevy::prelude::*;
use sticks_net::{GameAddress, LobbySettings, ServerListing};

use super::{FLASHING_LABEL, Flash, Lobby, LobbyMemory};
use crate::net::{Net, NetNotice, mode_index, mode_of};

/// `LobbyClientDisconnection.Message`.
pub const CONNECTION_LOST: &str = "The connection to the device hosting the game was lost.\n\nSorry :(";
/// What the old invitation links started with (still understood).
const INVITE_SCHEME: &str = "seu2://";
/// How long Copy Room Code shows its confirmation.
const LINK_FEEDBACK_SECONDS: f32 = 2.5;
/// While the connection screen is up: how often the clipboard is read again, and how often an
/// invitation's host is asked whether its game is still open.
const CLIPBOARD_INTERVAL: f32 = 3.0;
const PROBE_INTERVAL: f32 = 4.0;

/// A room code to look for: found on the clipboard, or typed into "Join with Room Code".
struct Invite {
    code: String,
    /// Typed (join as soon as the room answers); a clipboard code is only listed.
    typed: bool,
    /// Probes the host did not answer.
    unanswered: u32,
    /// The host's answer to the last probe: its game, while it is open.
    listing: Option<ServerListing>,
    probe: Option<Mutex<Receiver<Option<Vec<u8>>>>>,
    /// Seconds until the next probe.
    next_probe: f32,
}

/// State of the network screens.
#[derive(Default)]
pub(super) struct NetworkScreens {
    /// `ControlAssignment.assignedElsewhereLabels` (one per player column).
    pub elsewhere_labels: Vec<usize>,
    /// `PlayerSlotAssignments.SlotAssignedToAnotherClient` this frame.
    pub elsewhere: [bool; 4],
    /// Not `NetworkUtility.Offline`.
    pub online: bool,
    /// `ServerList.page`.
    page: usize,
    /// The games listed this frame (LAN plus a clipboard invitation).
    listed: Vec<ServerListing>,
    /// A clipboard read in progress, and its result.
    clipboard: Option<Mutex<Receiver<Option<String>>>>,
    /// Seconds until the clipboard is read again.
    next_clipboard: f32,
    invite: Option<Invite>,
    /// Copy Room Code's confirmation text and how long it stays.
    pub(super) link_feedback: Option<(String, f32)>,
    /// The online page is open (the server button).
    pub show_room: bool,
    /// How joining with a typed room code is going (shown on the online page).
    pub join_status: Option<String>,
    /// A dialog over the screens.
    pub dialog: Option<Dialog>,
}

/// Dialogs of the network screens (drawn by `super::room`).
#[derive(Clone, Debug)]
pub enum Dialog {
    /// "Start Network Game": the room's name, password and size; then the original's next screen.
    CreateRoom { next: Option<usize>, failed_label: Option<usize> },
    /// Joining a locked room.
    Password { listing: ServerListing, success: Option<usize>, error: Option<usize>, return_to: usize },
}

/// Status lines of the "Join with Room Code" dialog.
pub(super) const LOOKING: &str = "Looking for the room...";
const NOT_FOUND: &str = "No open room with that code.\nCheck it, or ask the host to make the room again.";
const NOT_A_CODE: &str = "That is not a room code.";
/// Unanswered probes before a typed code is given up on.
const GIVE_UP_AFTER: u32 = 2;

/// A room code in `text`: the whole text without spaces and dashes (a code as shown, in
/// groups), a word of it, or an old invitation link (`seu2://<code>/DESKTOP-1`).
pub fn parse_code(text: &str) -> Option<String> {
    if let Some(start) = text.find(INVITE_SCHEME).map(|i| i + INVITE_SCHEME.len()) {
        let code = text[start..].split_whitespace().next()?.split('/').next()?;
        return sticks_p2p::is_game_code(code).then(|| code.to_owned());
    }
    let joined: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    if sticks_p2p::is_game_code(&joined) {
        return Some(joined);
    }
    text.split(|c: char| !c.is_ascii_alphanumeric()).find(|w| w.len() >= 32 && sticks_p2p::is_game_code(w)).map(str::to_owned)
}

/// A code as people read it: groups of eight.
pub fn grouped(code: &str) -> String {
    code.as_bytes().chunks(8).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

#[cfg(windows)]
fn hidden(command: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    // `CREATE_NO_WINDOW`: no console flashes up.
    command.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
fn hidden(command: &mut std::process::Command) -> &mut std::process::Command {
    command
}

/// Put `text` on the system clipboard with the platform's own tool.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let candidates: &[(&str, &[&str])] = if cfg!(windows) {
        &[("clip", &[])]
    } else if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
    };
    for (program, args) in candidates {
        let mut command = Command::new(program);
        command.args(*args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        let Ok(mut child) = hidden(&mut command).spawn() else { continue };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }
    false
}


impl Lobby {
    pub(super) fn set_button_text(&mut self, button: usize, text: &str) {
        let label = self.scene.child(button, "Text").unwrap_or(button);
        self.set_text(label, text);
    }

    pub(super) fn flash_label(&mut self, object: usize) {
        if let Some(&text) = self.text_of_object.get(&object) {
            let from = self.texts[text].color;
            self.flashes.push(Flash { text, from, to: FLASHING_LABEL, time: 0.0 });
        }
    }
}

/// The players of a network game: this machine's plus the other machines'
/// (`ControlAssignment.PlayerCount`).
pub fn total_players(memory: &LobbyMemory, net: &Net) -> usize {
    memory.player_count() + if net.is_offline() { 0 } else { net.slots.remote_count() }
}

/// Everything the network screens do in a frame, before the other scripts.
pub(super) fn update(lobby: &mut Lobby, memory: &mut LobbyMemory, net: &mut Net, dt: f32) {
    lobby.network.elsewhere = if net.is_offline() { [false; 4] } else { net.slots.elsewhere() };
    lobby.network.online = !net.is_offline();
    notices(lobby, net);
    connection_screen(lobby, net, dt);
    server_list(lobby, net);
    lobby_ender(lobby, net);
    send_link_prompt(lobby, net, dt);
    share_settings(memory, net);
}

/// `LobbyClientDisconnection.OnDisconnectedFromServer`.
fn notices(lobby: &mut Lobby, net: &mut Net) {
    let notices: Vec<NetNotice> = net.notices.drain(..).collect();
    for notice in notices {
        let message = match notice {
            NetNotice::LostHost { deliberate: false } => CONNECTION_LOST.to_owned(),
            NetNotice::Refused(reason) => reason,
            _ => continue,
        };
        let Some(s) = lobby.named_script("LobbyClientDisconnection").cloned() else { continue };
        let (Some(error), Some(setup)) = (lobby.scene.field_object(&s, "accidentalDisconnectionErrorScreen"), lobby.scene.field_object(&s, "networkSetupScreen")) else { continue };
        lobby.show_error(error, &message, setup);
    }
}

/// The games to list: the LAN's, then the clipboard invitation's while its host says it is open.
fn listed(lobby: &Lobby, net: &Net) -> Vec<ServerListing> {
    let mut out = net.servers.clone();
    if let Some(listing) = lobby.network.invite.as_ref().and_then(|i| i.listing.clone()) {
        out.push(listing);
    }
    out
}

/// Read the clipboard now and then, and ask the host of an invitation found there whether its
/// game is open (so a stale invitation, or one to this machine's own game, is never listed).
fn watch_invitations(lobby: &mut Lobby, net: &mut Net, dt: f32) {
    let screens = &mut lobby.network;
    screens.next_clipboard -= dt;
    if screens.clipboard.is_none() && screens.next_clipboard <= 0.0 {
        screens.clipboard = Some(Mutex::new(sticks_paper::read_clipboard()));
        screens.next_clipboard = CLIPBOARD_INTERVAL;
    }
    if let Some(rx) = &screens.clipboard {
        let received = rx.lock().ok().and_then(|rx| rx.try_recv().ok());
        if let Some(text) = received {
            screens.clipboard = None;
            let code = text.as_deref().and_then(parse_code);
            // Our own game code is never "a game on the network".
            let code = code.filter(|c| net.p2p().and_then(sticks_p2p::P2p::code).as_deref() != Some(c.as_str()));
            // A typed code stays until its search ends, whatever the clipboard holds.
            if !screens.invite.as_ref().is_some_and(|i| i.typed) {
                match code {
                    None => screens.invite = None,
                    Some(code) if screens.invite.as_ref().is_some_and(|i| i.code == code) => {}
                    Some(code) => {
                        info!("room code on the clipboard: {code}");
                        screens.invite = Some(Invite { code, typed: false, unanswered: 0, listing: None, probe: None, next_probe: 0.0 });
                    }
                }
            }
        }
    }
    let Some(invite) = &mut lobby.network.invite else { return };
    let mut typed_answered = false;
    if let Some(rx) = &invite.probe {
        let answer = rx.lock().ok().and_then(|rx| rx.try_recv().ok());
        if let Some(answer) = answer {
            invite.probe = None;
            invite.listing = answer.and_then(|bytes| sticks_net::discovery::parse_announcement(&bytes, IpAddr::from([0, 0, 0, 0]))).map(|mut l| {
                l.address = GameAddress::Online(invite.code.clone());
                l
            });
            if invite.listing.is_none() {
                invite.unanswered += 1;
            }
            typed_answered = invite.typed;
        }
        if typed_answered {
            typed_code_answered(lobby, net);
        }
        return;
    }
    invite.next_probe -= dt;
    if invite.next_probe <= 0.0 {
        invite.next_probe = PROBE_INTERVAL;
        let code = invite.code.clone();
        if let Some(p2p) = net.p2p() {
            invite.probe = Some(Mutex::new(p2p.probe(&code)));
        }
    }
}

/// "Join with Room Code": look for the room of `text`'s code; it is joined when its host answers
/// (`typed_code_answered`).
pub(super) fn start_code_search(lobby: &mut Lobby, net: &Net, text: &str) {
    let Some(code) = parse_code(text) else {
        lobby.network.join_status = Some(NOT_A_CODE.to_owned());
        return;
    };
    if net.current_game_code().as_deref() == Some(code.as_str()) {
        lobby.network.join_status = Some("That is this computer's own code.".to_owned());
        return;
    }
    info!("looking for room {code}");
    lobby.network.invite = Some(Invite { code, typed: true, unanswered: 0, listing: None, probe: None, next_probe: 0.0 });
    lobby.network.join_status = Some(LOOKING.to_owned());
}

/// The host of a typed code answered a probe (or did not): join its room (asking for the
/// password of a locked one), or say it was not found.
fn typed_code_answered(lobby: &mut Lobby, net: &mut Net) {
    let Some(invite) = &lobby.network.invite else { return };
    if !lobby.network.show_room {
        // Closed: the code is no longer looked for.
        lobby.network.invite = None;
        lobby.network.join_status = None;
        return;
    }
    match invite.listing.clone() {
        Some(listing) => {
            let Some(s) = lobby.named_script("NetworkSetup").cloned() else { return };
            let next = lobby.scene.field_object(&s, "nextScreen");
            let error = lobby.scene.field_object(&s, "serverVersionMismatchErrorScreen");
            lobby.network.invite = None;
            lobby.network.join_status = None;
            lobby.network.show_room = false;
            join(lobby, net, &listing, next, error, s.object);
        }
        None if invite.unanswered >= GIVE_UP_AFTER => {
            lobby.network.invite = None;
            lobby.network.join_status = Some(NOT_FOUND.to_owned());
        }
        None => {}
    }
}

/// The online page was closed: stop looking for a typed code.
pub(super) fn cancel_code_search(lobby: &mut Lobby) {
    if lobby.network.invite.as_ref().is_some_and(|i| i.typed) {
        lobby.network.invite = None;
    }
    lobby.network.join_status = None;
}

/// The online page's "Create a Room": the original's "Start Network Game" (the create-room
/// dialog, then the screen after the title).
pub(super) fn open_create_room(lobby: &mut Lobby) {
    let Some(s) = lobby.named_script("NetworkSetup").cloned() else { return };
    let (next, failed_label) = (lobby.scene.field_object(&s, "nextScreen"), lobby.scene.field_object(&s, "connectToWiFiLabel"));
    lobby.network.dialog = Some(Dialog::CreateRoom { next, failed_label });
}

/// `NetworkSetup.Update`.
fn connection_screen(lobby: &mut Lobby, net: &mut Net, dt: f32) {
    let Some(s) = lobby.named_script("NetworkSetup").cloned() else { return };
    let listing_screen_up = lobby.active_in_hierarchy(s.object) || lobby.named_script("ServerList").is_some_and(|l| lobby.active_in_hierarchy(l.object));
    // The title and server list watch the clipboard for codes; the online page looks for a typed one.
    if (listing_screen_up || lobby.network.show_room) && net.is_offline() && !crate::settings::original_only() {
        watch_invitations(lobby, net, dt);
    } else {
        // Read again as soon as a listing screen comes up.
        lobby.network.next_clipboard = 0.0;
    }
    if !lobby.active_in_hierarchy(s.object) {
        return;
    }
    net.listen();
    let servers = listed(lobby, net);
    lobby.network.listed = servers.clone();
    let field = |lobby: &Lobby, name: &str| lobby.scene.field_object(&s, name);
    // The Steam build is the full version: no lock on "Start Network Game".
    if let Some(lock) = field(lobby, "startNetworkGameButton").and_then(|b| lobby.scene.child(b, "Lock")) {
        lobby.active[lock] = false;
    }
    let join = field(lobby, "joinNetworkGameButton");
    let show = field(lobby, "showNetworkGamesButton");
    let labels = ["noNetworkGamesFoundLabel", "connectToWiFiLabel", "connectToWiFiStarterEditionlabel"].map(|f| field(lobby, f));
    // TODO(UNCERTAIN): `Application.internetReachability` is taken to be the local network
    // (there is no portable check): "No Network Games Found", never "Connect to Wi-Fi".
    let shown_label = if servers.is_empty() { labels[0] } else { None };
    for label in labels.into_iter().flatten() {
        lobby.active[label] = Some(label) == shown_label;
    }
    if let Some(join) = join {
        lobby.active[join] = servers.len() == 1;
        if let Some(listing) = servers.first().filter(|_| servers.len() == 1) {
            lobby.set_button_text(join, &format!("Join Game: {}", listing_label(listing)));
        }
    }
    if let Some(show) = show {
        lobby.active[show] = servers.len() > 1;
        if servers.len() > 1 {
            lobby.set_button_text(show, &format!("Show Network Games ({} found)", servers.len()));
        }
    }
}

/// `ServerList.Update`.
fn server_list(lobby: &mut Lobby, net: &mut Net) {
    let Some(s) = lobby.named_script("ServerList").cloned() else { return };
    if !lobby.active_in_hierarchy(s.object) {
        return;
    }
    net.listen();
    let servers = listed(lobby, net);
    lobby.network.listed = servers.clone();
    if servers.is_empty() {
        if let Some(back) = lobby.scene.field_object(&s, "backScreen") {
            lobby.activate(back);
        }
        return;
    }
    let buttons: Vec<Option<usize>> = lobby.scene.field_objects(&s, "gameButtons");
    let per_page = buttons.len().max(1);
    let pages = (servers.len() - 1) / per_page + 1;
    lobby.network.page = lobby.network.page.min(pages - 1);
    let first = lobby.network.page * per_page;
    for (i, button) in buttons.iter().enumerate() {
        let Some(button) = *button else { continue };
        let listing = servers.get(first + i);
        lobby.active[button] = listing.is_some();
        if let Some(listing) = listing {
            lobby.set_button_text(button, &listing_label(listing));
        }
    }
    let page = lobby.network.page;
    if let Some(previous) = lobby.scene.field_object(&s, "previousPageButton") {
        lobby.active[previous] = page != 0;
    }
    if let Some(next) = lobby.scene.field_object(&s, "nextPageButton") {
        lobby.active[next] = servers.len() > (page + 1) * per_page;
    }
}

/// `LobbyEnder.Update`: Back offline, Stop Server on the host, Disconnect on a client.
fn lobby_ender(lobby: &mut Lobby, net: &Net) {
    for s in lobby.scene.scripts_named("LobbyEnder").cloned().collect::<Vec<_>>() {
        for (field, on) in [("backButton", net.is_offline()), ("stopServerButton", net.is_host()), ("disconnectButton", net.is_client())] {
            if let Some(o) = lobby.scene.field_object(&s, field) {
                lobby.active[o] = on;
            }
        }
    }
}

/// `SendStarterEditionLinkPrompt.Update`: the labels follow `NetworkUtility.Offline`; the
/// button stays hidden, as the original hides it on desktop (it linked to the phones' Starter
/// Edition). Rooms are shared with their code instead (`copy_room_code`).
fn send_link_prompt(lobby: &mut Lobby, net: &Net, dt: f32) {
    let Some(s) = lobby.named_script("SendStarterEditionLinkPrompt").cloned() else { return };
    let offline = net.is_offline();
    if let Some(o) = lobby.scene.field_object(&s, "offlineLabel") {
        lobby.active[o] = offline;
    }
    if let Some(o) = lobby.scene.field_object(&s, "networkGameLabel") {
        lobby.active[o] = !offline;
    }
    if let Some(button) = lobby.scene.field_object(&s, "sendLinkButton") {
        lobby.active[button] = false;
    }
    if let Some((_, left)) = &mut lobby.network.link_feedback {
        *left -= dt;
        if *left <= 0.0 {
            lobby.network.link_feedback = None;
        }
    }
}

/// This room's code: the host's game code (a client knows it when it joined with one).
pub(super) fn room_code(net: &Net) -> Option<String> {
    if net.is_host() {
        net.current_game_code()
    } else {
        match &net.host_address {
            Some(GameAddress::Online(code)) => Some(code.clone()),
            _ => None,
        }
    }
}

/// Copy Room Code: the code on the clipboard, for a friend anywhere.
pub(super) fn copy_room_code(lobby: &mut Lobby, net: &mut Net) {
    let feedback = match room_code(net) {
        None if net.is_host() => "Not online yet, try again".to_owned(),
        None => "Joined on the LAN: no code".to_owned(),
        Some(code) => {
            if copy_to_clipboard(&code) {
                info!("room code copied: {code}");
                if net.is_host() && !net.is_online() { "Copied (getting online...)".to_owned() } else { "Code copied!".to_owned() }
            } else {
                warn!("no clipboard tool; the room code is: {code}");
                "No clipboard: see the log".to_owned()
            }
        }
    };
    lobby.network.link_feedback = Some((feedback, LINK_FEEDBACK_SECONDS));
}

/// The settings everybody shares: received ones replace ours, our changes are sent.
fn share_settings(memory: &mut LobbyMemory, net: &mut Net) {
    if net.is_offline() {
        return;
    }
    if let Some(s) = net.incoming_settings.take() {
        if let Some(mode) = mode_of(s.mode) {
            memory.mode = mode;
        }
        memory.difficulty = s.difficulty.clamp(-1, 1);
        memory.level_index = s.level_index as usize;
        // `LobbyTeams.ReceiveTeams`.
        if s.teams.iter().all(|&t| (t as usize) < sticks_sim::lobby::MAX_TEAMS) {
            let mut teams = sticks_sim::lobby::LobbyTeams::default();
            for (player, &team) in s.teams.iter().enumerate() {
                teams.move_to_team(player, team as usize);
            }
            memory.teams = teams;
        }
        return;
    }
    // A client keeps quiet until the host's settings have arrived (`OnPlayerConnected` sends
    // them); otherwise joining would overwrite the host's choices.
    if net.is_client() && net.last_settings.is_none() {
        return;
    }
    let teams = std::array::from_fn(|p| memory.teams.team_of(p).unwrap_or(0) as u8);
    let mine = LobbySettings { mode: mode_index(memory.mode), difficulty: memory.difficulty, level_index: memory.level_index as u32, teams };
    if net.last_settings != Some(mine) {
        net.last_settings = Some(mine);
        net.send(&sticks_net::Message::Settings(mine));
    }
    // `RemoteColorAssignments.SendAllColors` for the slots this machine plays.
    for slot in 0..4 {
        if net.slots.assigned_to_us(slot) {
            net.share_colors(slot, memory.player_colors(slot));
        }
    }
}

/// `ControlAssignment.ResolveWithServerAssignments` (every frame in a network game).
pub(super) fn resolve_assignments(memory: &mut LobbyMemory, net: &mut Net) {
    if net.is_offline() || !(net.is_host() || net.role() == crate::net::NetRole::Client) {
        return;
    }
    let ours = net.slots.ours();
    let elsewhere = net.slots.elsewhere();
    let mut logged_in = net.logged_in;
    let claims = memory.assignment.resolve_with_server_assignments(ours, elsewhere, true, &mut logged_in);
    net.logged_in = logged_in;
    if let Some(claims) = claims {
        let ids = std::array::from_fn(|i| claims[i].then(|| LobbyMemory::player_id(i)));
        net.make_claims(ids);
    }
}

/// Buttons of the network screens. Returns whether `object` was one of them.
pub(super) fn press(lobby: &mut Lobby, net: &mut Net, object: usize) -> bool {
    if let Some(s) = lobby.named_script("NetworkSetup").cloned() {
        let field = |lobby: &Lobby, name: &str| lobby.scene.field_object(&s, name);
        if field(lobby, "startNetworkGameButton") == Some(object) {
            let (next, failed_label) = (field(lobby, "nextScreen"), field(lobby, "connectToWiFiLabel"));
            if crate::settings::original_only() {
                // `NetworkSetup.StartNetworkGame`: host at once.
                create_room(lobby, net, sticks_net::RoomConfig::new(&crate::net::machine_name(), "", sticks_net::room::MAX_MACHINES), next, failed_label);
            } else {
                lobby.network.dialog = Some(Dialog::CreateRoom { next, failed_label });
            }
            return true;
        }
        if field(lobby, "joinNetworkGameButton") == Some(object) {
            if lobby.network.listed.len() == 1 {
                let listing = lobby.network.listed[0].clone();
                let next = field(lobby, "nextScreen");
                let error = field(lobby, "serverVersionMismatchErrorScreen");
                join(lobby, net, &listing, next, error, s.object);
            }
            return true;
        }
        if field(lobby, "showNetworkGamesButton") == Some(object) {
            if let Some(screen) = field(lobby, "networkGamesListScreen") {
                lobby.activate(screen);
            }
            return true;
        }
    }
    if let Some(s) = lobby.named_script("ServerList").cloned() {
        let field = |lobby: &Lobby, name: &str| lobby.scene.field_object(&s, name);
        if field(lobby, "backButton") == Some(object) {
            if let Some(screen) = field(lobby, "backScreen") {
                lobby.activate(screen);
            }
            return true;
        }
        if field(lobby, "previousPageButton") == Some(object) {
            lobby.network.page = lobby.network.page.saturating_sub(1);
            return true;
        }
        if field(lobby, "nextPageButton") == Some(object) {
            lobby.network.page += 1;
            return true;
        }
        let buttons = lobby.scene.field_objects(&s, "gameButtons");
        if let Some(i) = buttons.iter().position(|b| *b == Some(object)) {
            let index = lobby.network.page * buttons.len() + i;
            if let Some(listing) = lobby.network.listed.get(index).cloned() {
                let next = field(lobby, "postConnectionScreen");
                let error = field(lobby, "errorScreen");
                join(lobby, net, &listing, next, error, s.object);
            }
            return true;
        }
    }
    for s in lobby.scene.scripts_named("LobbyEnder").cloned().collect::<Vec<_>>() {
        let pressed = |field: &str| lobby.scene.field_object(&s, field) == Some(object);
        if pressed("disconnectButton") || pressed("stopServerButton") {
            leave(lobby, net);
            return true;
        }
    }
    false
}

/// `LobbyEnder`'s Disconnect / Stop Server (and the room's Leave): back to the connection screen.
pub(super) fn leave(lobby: &mut Lobby, net: &mut Net) {
    // TODO(UNCERTAIN): the original's Disconnect leaves `DeliberateDisconnection` unset, which
    // shows the "connection lost" error to the client who chose to leave; leaving is treated as
    // deliberate here.
    net.disconnect(true);
    net.notices.retain(|n| !matches!(n, NetNotice::LostHost { .. }));
    lobby.network.show_room = false;
    let screen = lobby.scene.scripts_named("LobbyEnder").find_map(|s| lobby.scene.field_object(s, "connectionScreen"));
    if let Some(screen) = screen {
        lobby.activate(screen);
    }
}

/// "Start Network Game" after the create-room dialog: host, then the original's next screen
/// (or its "connect to Wi-Fi" label flashing when hosting failed).
pub(super) fn create_room(lobby: &mut Lobby, net: &mut Net, config: sticks_net::RoomConfig, next: Option<usize>, failed_label: Option<usize>) {
    match net.start_host(config) {
        Ok(()) => {
            if let Some(next) = next {
                lobby.activate(next);
            }
        }
        Err(e) => {
            warn!("cannot host a network game: {e}");
            if let Some(label) = failed_label {
                lobby.active[label] = true;
                lobby.flash_label(label);
            }
        }
    }
}

/// What a join button shows: the room's name, size and lock (the original: the host's name).
fn listing_label(listing: &ServerListing) -> String {
    if crate::settings::original_only() { listing.name() } else { listing.room().label() }
}

/// `ServerList.ConnectToServerOrShowVersionError`; a locked room asks for its password first.
fn join(lobby: &mut Lobby, net: &mut Net, listing: &ServerListing, success: Option<usize>, error: Option<usize>, return_to: usize) {
    if listing.room().locked {
        lobby.network.dialog = Some(Dialog::Password { listing: listing.clone(), success, error, return_to });
        return;
    }
    join_with(lobby, net, listing, "", success, error, return_to);
}

/// Join `listing` with `password`.
pub(super) fn join_with(lobby: &mut Lobby, net: &mut Net, listing: &ServerListing, password: &str, success: Option<usize>, error: Option<usize>, return_to: usize) {
    match net.join(listing, password) {
        Ok(()) => {
            if let Some(screen) = success {
                lobby.activate(screen);
            }
        }
        Err(message) => {
            if let Some(error) = error {
                lobby.show_error(error, message, return_to);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_codes() {
        // The Ed25519 base point: a valid public key.
        let code = "5866666666666666666666666666666666666666666666666666666666666666";
        assert_eq!(parse_code(code).as_deref(), Some(code));
        assert_eq!(parse_code(&format!("  {}\n", grouped(code))).as_deref(), Some(code));
        assert_eq!(parse_code(&format!("my code is {code} ok")).as_deref(), Some(code));
        assert_eq!(parse_code(&format!("seu2://{code}/MY%20PC")).as_deref(), Some(code));
        assert!(parse_code("seu2://192.168.1.20:25790/DESKTOP-1").is_none());
        assert!(parse_code("nothing here").is_none());
    }
}
