//! Network games: the original's `NetworkSetup` / `ServerDiscovery` / `PlayerSlotAssignments` /
//! `LevelLoader` RPCs and the gameplay state streams, over `sticks_net` — on the local network as
//! in the original, and over the internet peer to peer (`sticks_p2p`, not in the original).
//!
//! The model is the original's:
//!
//! * One machine hosts (`Network.InitializeServer`, at most four clients, announced on the LAN
//!   every second); the others join from the connection screen. A host is also reachable by its
//!   game code over the internet; the Lobby's "Send Link" button copies an invitation with it,
//!   and a machine with an invitation on its clipboard lists that game while it is open. Every machine claims the player
//!   slots its own inputs are in; the host grants them (`PlayerSlotAssignments`).
//! * Lobby settings (mode, level, difficulty, teams, colours) are shared: whoever changes one
//!   sends it, the host relays it.
//! * The host loads levels for everybody (`LevelLoader.LoadLevel`) and starts the game once every
//!   machine has finished loading (`AllFinishedLoadingLevel`); until then nothing moves.
//! * Each object has an owner that simulates it: a player's avatar (and the helicopter it flies)
//!   belongs to the machine that player sits at; enemies and unpiloted helicopters to the host.
//!   Owners stream their objects' input and position every frame (`CharacterInput` /
//!   `CharacterControl2D.OnSerializeNetworkView`, `SetWeapon`); everybody else simulates the same
//!   object from that input, pulled towards the owner's position.
//! * The host decides what hurts (`DamageOtherOnCollision` runs on the server): health, scores,
//!   level endings and the end of versus games are the host's and travel as events
//!   (`SyncServerDamageToClients`, `ReceivePlayerScore`, `DestroyAllEnemiesLevelEnder.Activate`,
//!   `VersusGameEnder`).
//!
//! Deviations, by necessity: the transport is TCP on the LAN (see `sticks_net::transport`) and
//! QUIC through iroh on the internet; a whole level's
//! random draws share one seed the host sends with the level (the original never needed that,
//! its enemies' faces were synchronised objects). Breakable scenery and letters are simulated
//! by every machine on its own (TODO(UNCERTAIN): they are server objects in the original).

use std::collections::HashMap;
use std::time::Instant;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use sticks_net::discovery::{Broadcaster, Listener, version_mismatch_message};
use sticks_net::{AvatarState, Client, ClientEvent, GameAddress, HelicopterState, Host, HostEvent, LoadKind, LobbySettings, Machine, Message, NETWORK_VERSION, NetId, PeerId, SERVER_PORT, ServerListing, SlotTable, WireControls, WirePlayerId, WireScore};
use sticks_p2p::P2p;
use sticks_sim::character::DEFAULT_Z;
use sticks_sim::level_end::PlayerId;
use sticks_sim::lobby::CharacterColors;
use unity_compat::Vec2 as SimVec2;

use crate::enemy::{DrawZ, Enemy};
use crate::level::LevelSource;
use crate::life::DeathRng;
use crate::lobby::LobbyMemory;
use crate::state::{Session, SessionPlayer};
use crate::helicopter::Helicopter;
use crate::level_end::SceneObject;
use crate::life::{Mortal, PlayerScore};
use crate::player::{Avatar, ControlledBy, Controls, EnterVehiclePressed, Player};
use crate::state::{AppState, GameMode};
use crate::weapon::Armament;
use crate::FrameSet;

/// `Network.peerType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetRole {
    #[default]
    Offline,
    Connecting,
    Client,
    Host,
}

/// What the lobby and the game react to.
#[derive(Clone, Debug, PartialEq)]
pub enum NetNotice {
    /// `OnDisconnectedFromServer` on a client (or a failed connection attempt).
    LostHost { deliberate: bool },
    /// The host turned us away.
    Refused(String),
    /// A client left the host's game (`OnPlayerDisconnected`).
    ClientLeft(PeerId),
}

/// A level load every machine performs (`LevelLoader.LoadLevel`).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadCommand {
    /// The scene file (relative to the export root), or "Lobby" / "Complete".
    pub level: String,
    pub mode: GameMode,
    pub difficulty: i32,
    pub player_count: usize,
    pub prefix: u32,
    pub seed: u32,
}

pub const LOBBY_LEVEL: &str = "Lobby";
pub const COMPLETE_LEVEL: &str = "Complete";

/// The latest stream of an object owned elsewhere.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReceivedAvatar {
    pub state: AvatarState,
    /// Not applied yet.
    pub fresh: bool,
    /// `EnterVehicle` went down in any of the frames received since the last apply.
    pub enter: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReceivedHelicopter {
    pub state: HelicopterState,
    pub fresh: bool,
    pub exit: bool,
}

/// The network session.
#[derive(Resource, Default)]
pub struct Net {
    role: NetRole,
    host: Option<Host>,
    client: Option<Client>,
    /// The internet end point, started when first needed (`None` before, or when it failed).
    p2p: Option<P2p>,
    p2p_failed: bool,
    broadcaster: Broadcaster,
    listener: Listener,
    /// Whether a screen that lists games is up this frame (`ServerDiscovery.Client` runs).
    listening: bool,
    /// `ServerDiscovery.Client.GetServerList`.
    pub servers: Vec<ServerListing>,
    /// `PlayerSlotAssignments`.
    pub slots: SlotTable<PlayerId>,
    /// `ControlAssignment.lastLocallyLoggedInPlayerIndex`.
    pub logged_in: Option<usize>,
    /// `RemoteColorAssignments.colors` (every slot's, as its owner sent them).
    pub colors: [CharacterColors; 4],
    /// `RemoteLevelAvailability.HighestRemoteUnlockedLevelIndex`.
    pub remote_unlocked: Option<i32>,
    /// `UniqueClientNumber.MyNumber` (the host is 0).
    pub client_number: Option<u32>,
    numbers: HashMap<PeerId, u32>,
    /// The host this client joined.
    pub host_address: Option<GameAddress>,
    /// `ClientDisconnection.DeliberateDisconnection`.
    deliberate: bool,
    pub notices: Vec<NetNotice>,
    /// Lobby settings that arrived, for the lobby to apply.
    pub incoming_settings: Option<LobbySettings>,
    /// The settings as last sent or received (changes against these are sent).
    pub last_settings: Option<LobbySettings>,
    /// Colours as last sent for our slots.
    pub sent_colors: [Option<CharacterColors>; 4],
    /// Host: `RequestLoad`s from clients.
    pub load_requests: Vec<LoadKind>,
    /// A load to perform now.
    pub load: Option<LoadCommand>,
    prefix: u32,
    /// Host: who has finished loading the current level.
    loaded: Vec<(Machine, bool)>,
    /// `AllFinishedLoadingLevel` arrived (always true offline).
    ready: bool,
    /// Host: the machine playing each slot in the running game.
    pub game_owners: [Option<Machine>; 4],
    /// The random seed of the level being played (spawn points, faces, ...).
    pub level_seed: Option<u32>,

    pub(crate) avatars: HashMap<NetId, ReceivedAvatar>,
    pub(crate) helicopters: HashMap<NetId, ReceivedHelicopter>,
    /// Client: health from the host, applied in arrival order.
    pub(crate) health: Vec<(NetId, f32)>,
    pub(crate) helicopter_health: Vec<(NetId, f32)>,
    pub(crate) scores: Vec<(u8, WireScore)>,
    /// Level events from the host (kind, object, values).
    pub(crate) level_events: Vec<(String, u64, Vec<i32>)>,
    /// Host: what was last sent, to send changes only.
    sent_health: HashMap<NetId, f32>,
    sent_helicopter_health: HashMap<NetId, f32>,
    sent_scores: HashMap<u8, WireScore>,
}

/// The name the host announces: the launcher's player name, else `Environment.MachineName`
/// ("Game" when empty).
pub fn machine_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let launched = sticks_sim::options::LaunchOptions::from_environment().player_name;
        let name = launched.or_else(|| std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok()).unwrap_or_default();
        if name.is_empty() { "Game".to_owned() } else { name }
    })
    .clone()
}

pub(crate) fn wire_id(id: &PlayerId) -> WirePlayerId {
    WirePlayerId { platform: id.platform.clone(), id: id.id.clone(), name: id.name.clone() }
}

pub(crate) fn player_id(w: &WirePlayerId) -> PlayerId {
    PlayerId { platform: w.platform.clone(), id: w.id.clone(), name: w.name.clone() }
}

fn color3(c: Option<[f32; 4]>) -> Option<[f32; 3]> {
    c.map(|c| [c[0], c[1], c[2]])
}

fn color4(c: Option<[f32; 3]>) -> Option<[f32; 4]> {
    c.map(|c| [c[0], c[1], c[2], 1.0])
}

pub(crate) fn mode_index(mode: GameMode) -> u8 {
    match mode {
        GameMode::Campaign => 0,
        GameMode::Versus => 1,
        GameMode::TeamVersus => 2,
    }
}

pub(crate) fn mode_of(index: u8) -> Option<GameMode> {
    match index {
        0 => Some(GameMode::Campaign),
        1 => Some(GameMode::Versus),
        2 => Some(GameMode::TeamVersus),
        _ => None,
    }
}

impl Net {
    pub fn role(&self) -> NetRole {
        self.role
    }

    /// `NetworkUtility.Offline`.
    pub fn is_offline(&self) -> bool {
        self.role == NetRole::Offline
    }

    pub fn is_host(&self) -> bool {
        self.role == NetRole::Host
    }

    pub fn is_client(&self) -> bool {
        matches!(self.role, NetRole::Client | NetRole::Connecting)
    }

    /// The host decides (offline, every machine is its own server).
    pub fn decides(&self) -> bool {
        !self.is_client()
    }

    /// `AllFinishedLoadingLevel` has arrived (or nothing to wait for).
    pub fn ready(&self) -> bool {
        self.is_offline() || self.ready
    }

    /// The internet end point, started on first use.
    pub fn p2p(&mut self) -> Option<&P2p> {
        if self.p2p.is_none() && !self.p2p_failed {
            match P2p::start() {
                Ok(p) => self.p2p = Some(p),
                Err(e) => {
                    warn!("no internet play: {e}");
                    self.p2p_failed = true;
                }
            }
        }
        self.p2p.as_ref()
    }

    /// This host's game code (what an invitation carries), once its end point is up.
    pub fn game_code(&mut self) -> Option<String> {
        self.p2p().and_then(P2p::code)
    }

    /// Whether other machines can find this one on the internet yet.
    pub fn is_online(&self) -> bool {
        self.p2p.as_ref().is_some_and(P2p::is_online)
    }

    /// `Network.InitializeServer(4, 25790, false)`: on the LAN port, and by game code on the
    /// internet. Either is enough (a second copy of the game on one machine cannot have the LAN
    /// port).
    pub fn start_host(&mut self) -> std::io::Result<()> {
        self.disconnect(true);
        let mut host = match Host::start(SERVER_PORT) {
            Ok(host) => {
                info!("hosting a network game on port {SERVER_PORT}");
                host
            }
            Err(e) if self.p2p().is_some() => {
                warn!("LAN port {SERVER_PORT}: {e}; hosting for the internet only");
                Host::links_only()
            }
            Err(e) => return Err(e),
        };
        let announcement = sticks_net::discovery::announcement(NETWORK_VERSION, SERVER_PORT, Some(&machine_name()));
        if let Some(p2p) = self.p2p() {
            host.accept_links(p2p.host(announcement));
            info!("hosting on the internet too (the game code is on the invitation)");
        }
        self.host = Some(host);
        self.role = NetRole::Host;
        self.reset_session();
        self.client_number = Some(0);
        self.ready = true;
        self.deliberate = false;
        Ok(())
    }

    /// `NetworkUtility.ConnectTo`.
    pub fn connect(&mut self, address: GameAddress) {
        self.disconnect(true);
        info!("joining the network game at {address}");
        let client = match &address {
            GameAddress::Lan(a) => Client::connect(*a),
            GameAddress::Online(code) => match self.p2p() {
                Some(p2p) => Client::with_link(p2p.connect(code)),
                None => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let _ = tx.send(Err(std::io::Error::other("internet play is not available")));
                    Client::with_link(rx)
                }
            },
        };
        self.client = Some(client);
        self.role = NetRole::Connecting;
        self.host_address = Some(address);
        self.reset_session();
        self.deliberate = false;
    }

    /// `Network.Disconnect` (deliberate: the player chose to leave).
    pub fn disconnect(&mut self, deliberate: bool) {
        if let Some(mut host) = self.host.take() {
            host.broadcast(&Message::Goodbye.encode(), None);
            host.flush();
            host.shut_down();
            self.broadcaster.stop();
            if let Some(p2p) = &self.p2p {
                p2p.stop_hosting();
            }
            info!("stopped hosting");
        }
        if let Some(client) = self.client.take() {
            client.disconnect();
            info!("left the network game");
            if deliberate {
                self.deliberate = true;
            }
            self.notices.push(NetNotice::LostHost { deliberate });
        }
        self.role = NetRole::Offline;
        self.reset_session();
    }

    fn reset_session(&mut self) {
        self.slots.reset();
        self.logged_in = None;
        self.colors = Default::default();
        self.remote_unlocked = None;
        self.client_number = None;
        self.numbers.clear();
        self.incoming_settings = None;
        self.last_settings = None;
        self.sent_colors = Default::default();
        self.load_requests.clear();
        self.loaded.clear();
        self.ready = false;
        self.game_owners = [None; 4];
        self.clear_game_streams();
    }

    fn clear_game_streams(&mut self) {
        self.avatars.clear();
        self.helicopters.clear();
        self.health.clear();
        self.helicopter_health.clear();
        self.scores.clear();
        self.level_events.clear();
        self.sent_health.clear();
        self.sent_helicopter_health.clear();
        self.sent_scores.clear();
    }

    /// To everybody else: a client tells the host (which relays where the original's
    /// `RPCMode.All` / `Others` would reach the other clients); the host tells every client.
    pub fn send(&mut self, message: &Message) {
        let bytes = message.encode();
        if let Some(host) = &mut self.host {
            host.broadcast(&bytes, None);
        } else if let Some(client) = &mut self.client {
            client.send(&bytes);
        }
    }

    fn send_to(&mut self, peer: PeerId, message: &Message) {
        if let Some(host) = &mut self.host {
            host.send(peer, &message.encode());
        }
    }

    fn relay(&mut self, from: PeerId, message: &Message) {
        if let Some(host) = &mut self.host {
            host.broadcast(&message.encode(), Some(from));
        }
    }

    /// Run the LAN listener this frame (a screen that lists games is up).
    pub fn listen(&mut self) {
        self.listening = true;
    }

    /// `ServerList.ConnectToServerOrShowVersionError`: `Err` is the original's message.
    pub fn join(&mut self, listing: &ServerListing) -> Result<(), &'static str> {
        if let Some(message) = version_mismatch_message(NETWORK_VERSION, listing.version) {
            return Err(message);
        }
        self.connect(listing.address.clone());
        Ok(())
    }

    /// `PlayerSlotAssignments.MakeClaims`.
    pub fn make_claims(&mut self, claims: [Option<PlayerId>; 4]) {
        self.slots.make_claims(&claims);
        match self.role {
            NetRole::Host => {
                self.slots.claimed(&claims, Machine::Host);
                self.broadcast_assignments(None);
            }
            NetRole::Client => {
                let claims = claims.map(|c| c.as_ref().map(wire_id));
                self.send(&Message::ClaimSlots { claims });
            }
            _ => {}
        }
    }

    /// `PlayerSlotAssignments.Broadcast`.
    fn broadcast_assignments(&mut self, except: Option<PeerId>) {
        let peers: Vec<PeerId> = self.host.as_ref().map(|h| h.peers().collect()).unwrap_or_default();
        for peer in peers {
            if Some(peer) != except {
                self.send_assignments(peer);
            }
        }
    }

    fn send_assignments(&mut self, peer: PeerId) {
        let a = self.slots.assignments_for(peer);
        let ids = a.ids.map(|id| id.as_ref().map(wire_id));
        self.send_to(peer, &Message::Assignments { used: a.used, ours: a.ours, ids, spawn: None });
    }

    /// `LevelLoader.Load` on the host: everybody loads `level`.
    pub fn host_load(&mut self, level: String, mode: GameMode, difficulty: i32, player_count: usize) {
        self.prefix += 1;
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.subsec_nanos() ^ d.as_secs() as u32);
        let command = LoadCommand { level, mode, difficulty, player_count, prefix: self.prefix, seed };
        // `Network.maxConnections = 0`.
        if let Some(host) = &mut self.host {
            host.max_connections = 0;
        }
        self.loaded = std::iter::once(Machine::Host).chain(self.host.iter().flat_map(|h| h.peers().map(Machine::Client))).map(|m| (m, false)).collect();
        self.ready = false;
        self.game_owners = std::array::from_fn(|i| self.slots.holder(i).or(self.slots.assigned_to_us(i).then_some(Machine::Host)));
        self.send(&Message::LoadLevel {
            level: command.level.clone(),
            mode: mode_index(mode),
            difficulty,
            player_count: player_count as u8,
            prefix: command.prefix,
            seed,
        });
        self.load = Some(command);
    }

    /// `LevelLoader.SendRequestToLoad` from a client.
    pub fn request_load(&mut self, kind: LoadKind) {
        self.send(&Message::RequestLoad(kind));
    }

    /// `LevelLoader.Update` once the level is there: `FinishedLoadingLevel`.
    pub fn finished_loading(&mut self) {
        self.clear_game_streams();
        match self.role {
            NetRole::Host => self.mark_loaded(Machine::Host, self.prefix),
            NetRole::Client => {
                let prefix = self.prefix;
                self.send(&Message::FinishedLoading { prefix });
            }
            _ => {}
        }
    }

    fn mark_loaded(&mut self, machine: Machine, prefix: u32) {
        if prefix != self.prefix {
            return;
        }
        if let Some(entry) = self.loaded.iter_mut().find(|(m, _)| *m == machine) {
            entry.1 = true;
        }
        self.check_all_loaded();
    }

    /// `CheckIfAllPlayersHaveFinishedLoading`.
    fn check_all_loaded(&mut self) {
        if !self.loaded.is_empty() && self.loaded.iter().all(|(_, done)| *done) {
            self.loaded.clear();
            let prefix = self.prefix;
            self.send(&Message::AllFinishedLoading { prefix });
            self.ready = true;
            info!("every machine has loaded the level");
        }
    }

    /// The host has entered the Lobby again: `ConnectionLimitSetter.Awake`.
    pub fn back_in_lobby(&mut self) {
        if let Some(host) = &mut self.host {
            host.max_connections = sticks_net::transport::MAX_CONNECTIONS;
        }
        self.ready = true;
        self.clear_game_streams();
    }

    /// A level event the host decided; clients act on it when it arrives.
    pub fn send_level_event(&mut self, kind: &str, object: u64, values: Vec<i32>) {
        if self.is_host() {
            self.send(&Message::LevelEvent { kind: kind.to_owned(), object, values });
        }
    }

    /// Take the received events of `kind`.
    pub fn take_level_events(&mut self, kind: &str) -> Vec<(u64, Vec<i32>)> {
        let mut out = Vec::new();
        self.level_events.retain(|(k, object, values)| {
            if k == kind {
                out.push((*object, values.clone()));
                false
            } else {
                true
            }
        });
        out
    }

    /// This peer's colours for its own slots, sent when they change
    /// (`RemoteColorAssignments.SendAllColors`).
    pub fn share_colors(&mut self, slot: usize, colors: CharacterColors) {
        if self.is_offline() || slot >= 4 {
            return;
        }
        self.colors[slot] = colors;
        if self.sent_colors[slot] == Some(colors) {
            return;
        }
        self.sent_colors[slot] = Some(colors);
        self.send(&Message::Colors { slot: slot as u8, body: color3(colors.body), helmet: color3(colors.helmet), face: color3(colors.face) });
    }

    /// `ServerBroadcaster.Update`, the connections and every message that arrived.
    fn poll(&mut self) {
        let now = Instant::now();
        if self.host.as_ref().is_some_and(Host::on_lan) {
            let name = machine_name();
            self.broadcaster.broadcast(now, SERVER_PORT, &name);
        }
        if self.listening {
            self.servers = self.listener.servers(now);
        } else {
            self.listener.stop();
            self.servers.clear();
        }
        self.listening = false;

        if let Some(host) = &mut self.host {
            let events = host.poll();
            for event in events {
                match event {
                    HostEvent::Connected(peer) => info!("client {peer} connected ({})", self.host.as_ref().map(|h| h.describe(peer)).unwrap_or_default()),
                    HostEvent::Message(peer, bytes) => match Message::decode(&bytes) {
                        Ok(message) => self.host_message(peer, message),
                        Err(e) => warn!("client {peer}: {e}"),
                    },
                    HostEvent::Disconnected(peer) => self.client_left(peer),
                }
            }
        }
        if let Some(client) = &mut self.client {
            let events = client.poll();
            for event in events {
                match event {
                    ClientEvent::Connected => {
                        let name = machine_name();
                        self.send(&Message::Hello { version: NETWORK_VERSION, name });
                    }
                    ClientEvent::Failed(reason) => {
                        warn!("could not connect: {reason}");
                        self.client = None;
                        self.role = NetRole::Offline;
                        self.reset_session();
                        self.notices.push(NetNotice::LostHost { deliberate: false });
                    }
                    ClientEvent::Message(bytes) => match Message::decode(&bytes) {
                        Ok(message) => self.client_message(message),
                        Err(e) => warn!("host: {e}"),
                    },
                    ClientEvent::Disconnected => {
                        info!("disconnected from the host");
                        self.client = None;
                        self.role = NetRole::Offline;
                        self.reset_session();
                        let deliberate = std::mem::take(&mut self.deliberate);
                        self.notices.push(NetNotice::LostHost { deliberate });
                    }
                }
            }
        }
    }

    fn client_left(&mut self, peer: PeerId) {
        info!("client {peer} left");
        self.numbers.remove(&peer);
        self.slots.disconnected(peer);
        self.broadcast_assignments(None);
        // `LevelLoader.OnPlayerDisconnected`.
        if !self.loaded.is_empty() {
            self.loaded.retain(|(m, _)| *m != Machine::Client(peer));
            self.check_all_loaded();
        }
        self.notices.push(NetNotice::ClientLeft(peer));
    }

    fn host_message(&mut self, peer: PeerId, message: Message) {
        match message {
            Message::Hello { version, name } => {
                if version != NETWORK_VERSION {
                    let reason = version_mismatch_message(NETWORK_VERSION, version).unwrap_or_default().to_owned();
                    self.send_to(peer, &Message::Refused { reason });
                    if let Some(host) = &mut self.host {
                        host.flush();
                        host.kick(peer);
                    }
                    return;
                }
                info!("client {peer} is {name}");
                // `UniqueClientNumber.OnPlayerConnected`: one more than the highest.
                let number = self.numbers.values().copied().max().unwrap_or(0) + 1;
                self.numbers.insert(peer, number);
                self.send_to(peer, &Message::Welcome { client_number: number });
                // Every `OnPlayerConnected`: the assignments, the settings, everybody's colours,
                // the unlocked levels.
                self.send_assignments(peer);
                if let Some(s) = self.last_settings {
                    self.send_to(peer, &Message::Settings(s));
                }
                for slot in 0..4 {
                    if self.slots.used[slot] {
                        let c = self.colors[slot];
                        self.send_to(peer, &Message::Colors { slot: slot as u8, body: color3(c.body), helmet: color3(c.helmet), face: color3(c.face) });
                    }
                }
            }
            Message::Settings(s) => {
                self.incoming_settings = Some(s);
                self.last_settings = Some(s);
                self.relay(peer, &Message::Settings(s));
            }
            Message::ClaimSlots { claims } => {
                let claims = claims.map(|c| c.as_ref().map(player_id));
                self.slots.claimed(&claims, Machine::Client(peer));
                self.broadcast_assignments(None);
            }
            Message::Colors { slot, body, helmet, face } => {
                if let Some(c) = self.colors.get_mut(slot as usize) {
                    *c = CharacterColors { body: color4(body), helmet: color4(helmet), face: color4(face) };
                }
                self.relay(peer, &Message::Colors { slot, body, helmet, face });
            }
            Message::Unlocked { highest_campaign_level } => {
                self.remote_unlocked = Some(self.remote_unlocked.unwrap_or(-1).max(highest_campaign_level));
                self.relay(peer, &message_unlocked(highest_campaign_level));
            }
            Message::RequestLoad(kind) => self.load_requests.push(kind),
            Message::FinishedLoading { prefix } => self.mark_loaded(Machine::Client(peer), prefix),
            Message::Frame { avatars, helicopters } => {
                self.receive_frame(&avatars, &helicopters);
                self.relay(peer, &Message::Frame { avatars, helicopters });
            }
            other => warn!("client {peer} sent {other:?}, which only the host sends"),
        }
    }

    fn client_message(&mut self, message: Message) {
        match message {
            Message::Welcome { client_number } => {
                info!("joined as client {client_number}");
                self.client_number = Some(client_number);
                self.role = NetRole::Client;
                self.ready = true;
            }
            Message::Refused { reason } => {
                warn!("the host refused: {reason}");
                self.notices.push(NetNotice::Refused(reason));
                if let Some(client) = self.client.take() {
                    client.disconnect();
                }
                self.role = NetRole::Offline;
                self.reset_session();
            }
            Message::Settings(s) => {
                self.incoming_settings = Some(s);
                self.last_settings = Some(s);
            }
            Message::Assignments { used, ours, ids, .. } => {
                let ids = ids.map(|id| id.as_ref().map(player_id));
                self.slots.receive(sticks_net::Assignments { used, ours, ids });
            }
            Message::Colors { slot, body, helmet, face } => {
                if let Some(c) = self.colors.get_mut(slot as usize) {
                    *c = CharacterColors { body: color4(body), helmet: color4(helmet), face: color4(face) };
                }
            }
            Message::Unlocked { highest_campaign_level } => {
                self.remote_unlocked = Some(self.remote_unlocked.unwrap_or(-1).max(highest_campaign_level));
            }
            Message::LoadLevel { level, mode, difficulty, player_count, prefix, seed } => {
                let Some(mode) = mode_of(mode) else { return };
                if !(-1..=1).contains(&difficulty) || player_count > 4 {
                    return;
                }
                self.prefix = prefix;
                self.ready = false;
                self.load = Some(LoadCommand { level, mode, difficulty, player_count: player_count as usize, prefix, seed });
            }
            Message::AllFinishedLoading { prefix } => {
                if prefix == self.prefix {
                    self.ready = true;
                    info!("every machine has loaded the level");
                }
            }
            Message::Frame { avatars, helicopters } => self.receive_frame(&avatars, &helicopters),
            Message::Health { id, remaining } => self.health.push((id, remaining)),
            Message::HelicopterHealth { id, remaining } => self.helicopter_health.push((id, remaining)),
            Message::Score { slot, score } => self.scores.push((slot, score)),
            Message::LevelEvent { kind, object, values } => self.level_events.push((kind, object, values)),
            Message::Lives { .. } | Message::Respawn { .. } => {}
            Message::Goodbye => {
                info!("the host stopped the game");
            }
            other => warn!("the host sent {other:?}, which only clients send"),
        }
    }

    fn receive_frame(&mut self, avatars: &[AvatarState], helicopters: &[HelicopterState]) {
        for a in avatars {
            let enter = a.controls.enter_vehicle || self.avatars.get(&a.id).is_some_and(|r| r.fresh && r.enter);
            self.avatars.insert(a.id, ReceivedAvatar { state: *a, fresh: true, enter });
        }
        for h in helicopters {
            let exit = h.exit || self.helicopters.get(&h.id).is_some_and(|r| r.fresh && r.exit);
            self.helicopters.insert(h.id, ReceivedHelicopter { state: *h, fresh: true, exit });
        }
    }

    /// Host: `SyncServerDamageToClients` (changes only).
    pub(crate) fn share_health(&mut self, id: NetId, remaining: f32) {
        if !self.is_host() || self.sent_health.get(&id) == Some(&remaining) {
            return;
        }
        let first = !self.sent_health.contains_key(&id);
        self.sent_health.insert(id, remaining);
        if !first {
            self.send(&Message::Health { id, remaining });
        }
    }

    pub(crate) fn share_helicopter_health(&mut self, id: NetId, remaining: f32) {
        if !self.is_host() || self.sent_helicopter_health.get(&id) == Some(&remaining) {
            return;
        }
        let first = !self.sent_helicopter_health.contains_key(&id);
        self.sent_helicopter_health.insert(id, remaining);
        if !first {
            self.send(&Message::HelicopterHealth { id, remaining });
        }
    }

    /// Host: `PlayerScore.Update` → `ReceivePlayerScore`.
    pub(crate) fn share_score(&mut self, slot: u8, score: WireScore) {
        if !self.is_host() || self.sent_scores.get(&slot) == Some(&score) {
            return;
        }
        self.sent_scores.insert(slot, score);
        self.send(&Message::Score { slot, score });
    }
}

fn message_unlocked(level: i32) -> Message {
    Message::Unlocked { highest_campaign_level: level }
}

/// Which object of the network game an entity is, and whether this machine owns (simulates) it.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetObject {
    pub id: NetId,
    pub local: bool,
}

/// A player slot played on this machine.
#[derive(Component, Clone, Copy, Debug)]
pub struct LocalPlayer;

/// Run condition: the gameplay may run (`Network.isMessageQueueRunning` and the level has
/// loaded everywhere).
pub fn gameplay_ready(net: Res<Net>) -> bool {
    net.ready()
}

pub fn is_offline(net: Res<Net>) -> bool {
    net.is_offline()
}

/// The host decides (offline every machine does): AI, damage, level endings.
pub fn decides(net: Res<Net>) -> bool {
    net.decides()
}

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Net>()
            .add_systems(PreUpdate, (poll_network, perform_loads).chain())
            .add_systems(Last, flush_network)
            .add_systems(OnEnter(AppState::InGame), |mut net: ResMut<Net>| net.finished_loading())
            .add_systems(OnEnter(AppState::Lobby), |mut net: ResMut<Net>| net.back_in_lobby())
            .add_systems(Update, (tag_net_objects, leave_with_the_host, remove_departed_players, in_game_load_requests).run_if(in_state(AppState::InGame)))
            .add_systems(
                Update,
                apply_streams
                    .in_set(FrameSet::RoleInput)
                    .after(crate::player::read_player_input)
                    .after(crate::enemy::copy_enemy_input)
                    .after(crate::vehicles::read_helicopter_input)
                    .after(crate::helicopter::copy_helicopter_input)
                    .run_if(not(is_offline)),
            )
            .add_systems(Update, (send_streams, share_outcomes, receive_scores).in_set(FrameSet::Present).run_if(not(is_offline)));
    }
}

fn poll_network(mut net: ResMut<Net>) {
    net.poll();
}

fn flush_network(mut net: ResMut<Net>) {
    if let Some(host) = &mut net.host {
        host.flush();
    }
    if let Some(client) = &mut net.client {
        client.flush();
    }
}

/// Owner tags for the level's networked objects: enemies and helicopters belong to the host.
#[allow(clippy::type_complexity)]
fn tag_net_objects(
    mut commands: Commands,
    net: Res<Net>,
    enemies: Query<(Entity, &SceneObject), (With<Enemy>, Without<NetObject>, Allow<Disabled>)>,
    helicopters: Query<(Entity, &Helicopter), (Without<NetObject>, Allow<Disabled>)>,
    others: Query<Entity, (With<Enemy>, Without<SceneObject>, Without<NetObject>, Allow<Disabled>)>,
) {
    let local = net.decides();
    for (entity, object) in &enemies {
        commands.entity(entity).insert(NetObject { id: NetId::Scene(object.0 as u64), local });
    }
    // The test target of a level without enemies (one at most).
    for entity in &others {
        commands.entity(entity).insert(NetObject { id: NetId::Scene(u64::MAX), local });
    }
    for (entity, h) in &helicopters {
        commands.entity(entity).insert(NetObject { id: NetId::Scene(h.spec.root as u64), local });
    }
}

/// `ClientDisconnection`: a client that lost its host goes back to the Lobby.
fn leave_with_the_host(mut net: ResMut<Net>, mut next: ResMut<NextState<AppState>>, mut was_online: Local<bool>) {
    // The Lobby's `LobbyClientDisconnection` is not in a level: no error screen follows.
    net.notices.retain(|n| !matches!(n, NetNotice::LostHost { .. } | NetNotice::Refused(_)));
    let online = !net.is_offline();
    if *was_online && !online {
        info!("the network game ended: back to the Lobby");
        next.set(AppState::Lobby);
    }
    *was_online = online;
}

/// `RemoveDisconnectedPlayers` (host) and the clients' copy of it: a machine that left takes its
/// players with it.
fn remove_departed_players(
    mut commands: Commands,
    mut net: ResMut<Net>,
    players: Query<(Entity, &Player)>,
    avatars: Query<(Entity, &ControlledBy), Allow<Disabled>>,
) {
    let mut gone: Vec<usize> = Vec::new();
    let notices: Vec<NetNotice> = net.notices.iter().filter(|n| matches!(n, NetNotice::ClientLeft(_))).cloned().collect();
    net.notices.retain(|n| !matches!(n, NetNotice::ClientLeft(_)));
    for notice in notices {
        if let NetNotice::ClientLeft(peer) = notice {
            for slot in 0..4 {
                if net.game_owners[slot] == Some(Machine::Client(peer)) {
                    net.game_owners[slot] = None;
                    gone.push(slot);
                    net.send_level_event("PlayerLeft", slot as u64, Vec::new());
                }
            }
        }
    }
    for (object, _) in net.take_level_events("PlayerLeft") {
        gone.push(object as usize);
    }
    for slot in gone {
        for (entity, player) in &players {
            if player.index != slot {
                continue;
            }
            for (avatar, controlled) in &avatars {
                if controlled.0 == entity {
                    commands.entity(avatar).try_despawn();
                }
            }
            commands.entity(entity).try_despawn();
            info!("player {} left the game", slot + 1);
        }
    }
}

fn wire_controls(c: &sticks_sim::character::CharacterInput, enter: bool) -> WireControls {
    WireControls {
        movement: c.movement,
        jump: c.jump,
        duck: c.duck,
        fire: c.fire,
        aim: [c.aim.x, c.aim.y],
        next_weapon: c.next_weapon,
        previous_weapon: c.previous_weapon,
        selected_weapon: c.selected_weapon.map(|w| w as u8),
        enter_vehicle: enter,
    }
}

/// Every object this machine owns: its input and position this frame.
#[allow(clippy::type_complexity)]
fn send_streams(
    mut net: ResMut<Net>,
    avatars: Query<(&NetObject, &Avatar, &Controls, Option<&EnterVehiclePressed>, Option<&Armament>, Option<&DrawZ>, Has<Disabled>), Allow<Disabled>>,
    helicopters: Query<(&NetObject, &Helicopter)>,
    local_players: Query<(), With<LocalPlayer>>,
) {
    if !net.ready() {
        return;
    }
    let mut out = Vec::new();
    for (object, avatar, controls, enter, armament, z, disabled) in &avatars {
        if !object.local {
            continue;
        }
        let p = avatar.position();
        out.push(AvatarState {
            id: object.id,
            controls: wire_controls(&controls.0, enter.is_some_and(|e| e.0)),
            position: [p.x, p.y],
            z: z.map_or(DEFAULT_Z, |z| z.0),
            layer: avatar.body.layer,
            weapon: armament.and_then(Armament::equipped).map(|w| w as u8),
            // Asleep off screen or piloting (death travels as health).
            active: !disabled,
        });
    }
    let host = net.is_host();
    let mut helis = Vec::new();
    for (object, h) in &helicopters {
        // The pilot's machine flies it; otherwise the host.
        let mine = match h.pilot() {
            Some(pilot) => local_players.contains(pilot),
            None => host,
        };
        if mine {
            helis.push(h.net_state(object.id));
        }
    }
    if !out.is_empty() || !helis.is_empty() {
        net.send(&Message::Frame { avatars: out, helicopters: helis });
    }
}

/// Objects owned elsewhere act on their owner's latest input and are pulled to its position.
#[allow(clippy::type_complexity)]
fn apply_streams(
    mut commands: Commands,
    mut net: ResMut<Net>,
    mut avatars: Query<(Entity, &NetObject, &mut Avatar, &mut Controls, &Mortal, Option<&mut EnterVehiclePressed>, Option<&mut Armament>, Has<Disabled>, Has<Enemy>), Allow<Disabled>>,
    mut helicopters: Query<(&NetObject, &mut Helicopter)>,
    local_players: Query<(), With<LocalPlayer>>,
) {
    let net = &mut *net;
    for (entity, object, mut avatar, mut controls, mortal, enter, armament, disabled, enemy) in &mut avatars {
        if object.local {
            continue;
        }
        let Some(r) = net.avatars.get_mut(&object.id) else { continue };
        let s = r.state;
        let c = &mut controls.0;
        c.movement = s.controls.movement;
        c.jump = s.controls.jump;
        c.duck = s.controls.duck;
        c.fire = s.controls.fire;
        c.aim = SimVec2::new(s.controls.aim[0], s.controls.aim[1]);
        // Weapon changes come as the equipped weapon (`SetWeapon`), not as button edges.
        c.next_weapon = false;
        c.previous_weapon = false;
        c.selected_weapon = None;
        if let Some(mut enter) = enter {
            enter.0 = r.enter;
        }
        r.enter = false;
        if let (Some(mut armament), Some(w)) = (armament, s.weapon) {
            armament.select_weapon(w as usize);
        }
        if enemy {
            // The host's `DeactivateOffScreen` / `OffScreenReactivator`.
            if !s.active && !disabled && mortal.avatar_active() {
                commands.entity(entity).insert(Disabled);
            } else if s.active && disabled {
                commands.entity(entity).remove::<Disabled>();
            }
            if (s.z - DEFAULT_Z).abs() > f32::EPSILON {
                commands.entity(entity).insert(DrawZ(s.z));
            } else {
                commands.entity(entity).remove::<DrawZ>();
            }
        }
        if r.fresh {
            r.fresh = false;
            avatar.body.layer = s.layer;
            if mortal.avatar_active() && s.active {
                avatar.set_network_target(SimVec2::new(s.position[0], s.position[1]));
            }
        }
    }
    let host = net.is_host();
    for (object, mut h) in &mut helicopters {
        let mine = match h.pilot() {
            Some(pilot) => local_players.contains(pilot),
            None => host,
        };
        if mine {
            continue;
        }
        let Some(r) = net.helicopters.get_mut(&object.id) else { continue };
        h.apply_net_input(&r.state, r.exit);
        r.exit = false;
        if r.fresh {
            r.fresh = false;
            h.apply_net_body(&r.state);
        }
    }
}

/// Host: health and scores as they change (`SyncServerDamageToClients`, `PlayerScore`).
fn share_outcomes(mut net: ResMut<Net>, avatars: Query<(&NetObject, &Mortal), Allow<Disabled>>, helicopters: Query<(&NetObject, &Helicopter)>, players: Query<(&Player, &PlayerScore)>) {
    if !net.is_host() || !net.ready() {
        return;
    }
    for (object, mortal) in &avatars {
        net.share_health(object.id, mortal.health.remaining());
    }
    for (object, h) in &helicopters {
        net.share_helicopter_health(object.id, h.health_remaining());
    }
    for (player, score) in &players {
        let s = &score.0;
        net.share_score(player.index as u8, WireScore { damage_dealt: s.damage_dealt, frags: s.frags(), frags_against: s.all_frags_against() });
    }
}

/// Client: `ReceivePlayerScore`.
fn receive_scores(mut net: ResMut<Net>, mut players: Query<(&Player, &mut PlayerScore)>) {
    for (slot, score) in std::mem::take(&mut net.scores) {
        for (player, mut s) in &mut players {
            if player.index == slot as usize {
                s.0.receive(score.damage_dealt, score.frags, score.frags_against);
            }
        }
    }
}

/// `LevelLoader.LoadLevel` on every machine: the session the host's lobby set up (this
/// machine's slots with their inputs, the others' as remote players), the shared seed, then the
/// Level Loading screen (or the Lobby).
#[allow(clippy::too_many_arguments)]
fn perform_loads(
    mut net: ResMut<Net>,
    memory: Res<LobbyMemory>,
    mut session: ResMut<Session>,
    mut source: ResMut<LevelSource>,
    mut rng: ResMut<DeathRng>,
    mut next: ResMut<NextState<AppState>>,
    state: Res<State<AppState>>,
) {
    let Some(command) = net.load.take() else { return };
    if command.level == LOBBY_LEVEL || command.level == COMPLETE_LEVEL {
        if command.level == COMPLETE_LEVEL {
            // TODO: the "Complete" ending scene is not reconstructed (see `menu`).
            warn!("campaign complete: the ending scene is not reconstructed; back to the Lobby");
        }
        net.level_seed = None;
        if *state.get() != AppState::Lobby {
            next.set(AppState::Lobby);
        }
        return;
    }
    let mut players = Vec::new();
    for index in 0..4 {
        if net.slots.assigned_to_us(index) {
            let devices = memory.player_devices(index);
            if !devices.is_empty() {
                players.push(SessionPlayer { index, devices, remote: false });
            }
        } else if net.slots.assigned_elsewhere(index) {
            players.push(SessionPlayer { index, devices: Vec::new(), remote: true });
        }
    }
    info!("loading {} ({:?}, difficulty {}, {} player(s))", command.level, command.mode, command.difficulty, command.player_count);
    *session = Session { mode: command.mode, difficulty: command.difficulty, player_count: command.player_count, players, teams: memory.teams.clone() };
    source.0 = command.level.clone();
    rng.0 = sticks_sim::rng::GameRng::from_seed(command.seed);
    net.level_seed = Some(command.seed);
    next.set(AppState::Loading);
}

/// Host, in a level: clients' `RequestLoad`s (`RestartLevelButton`, `ReturnToLobbyButton`,
/// `LevelCompleteButton`).
fn in_game_load_requests(mut net: ResMut<Net>, session: Res<Session>, source: Res<LevelSource>) {
    if !net.is_host() {
        return;
    }
    let requests = std::mem::take(&mut net.load_requests);
    // `RequestLoad` ignores requests while a load is under way: the first one wins.
    if let Some(kind) = requests.first() {
        let level = match kind {
            LoadKind::Normal => source.0.clone(),
            LoadKind::Lobby => LOBBY_LEVEL.to_owned(),
            LoadKind::Complete => COMPLETE_LEVEL.to_owned(),
        };
        let count = session.player_count;
        net.host_load(level, session.mode, session.difficulty, count);
    }
}
