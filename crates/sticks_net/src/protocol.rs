//! What host and clients say to each other. Each variant names the original's RPC or state
//! stream it stands for.

use crate::room::Room;
use crate::wire::{DecodeError, Reader, Writer};

/// Which networked object a state or event is about: a player slot's avatar
/// (`Player.PlayerIndex`), or an object of the level scene (enemies, helicopters), by its scene
/// file id, which every peer loads identically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NetId {
    Player(u8),
    Scene(u64),
}

impl NetId {
    fn write(&self, w: &mut Writer) {
        match *self {
            NetId::Player(i) => {
                w.u8(0);
                w.u8(i);
            }
            NetId::Scene(id) => {
                w.u8(1);
                w.u64(id);
            }
        }
    }

    fn read(r: &mut Reader) -> Result<NetId, DecodeError> {
        match r.u8()? {
            0 => Ok(NetId::Player(r.u8()?)),
            1 => Ok(NetId::Scene(r.u64()?)),
            _ => Err(DecodeError("bad net id")),
        }
    }
}

/// `PlayerID` on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WirePlayerId {
    pub platform: Option<String>,
    pub id: String,
    pub name: String,
}

impl WirePlayerId {
    fn write(&self, w: &mut Writer) {
        w.option(&self.platform, |w, s| w.str(s));
        w.str(&self.id);
        w.str(&self.name);
    }

    fn read(r: &mut Reader) -> Result<WirePlayerId, DecodeError> {
        Ok(WirePlayerId { platform: r.option(|r| r.str())?, id: r.str()?, name: r.str()? })
    }
}

/// `CharacterInput`'s stream (`OnSerializeNetworkView`) plus the button edges the owner's
/// `Update` saw (`NextWeapon`, `PreviousWeapon`, `EnterVehicle`), which the original sends as
/// the `SetWeapon` RPC and the control-slot requests.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WireControls {
    pub movement: f32,
    pub jump: bool,
    pub duck: bool,
    pub fire: bool,
    pub aim: [f32; 2],
    pub next_weapon: bool,
    pub previous_weapon: bool,
    pub selected_weapon: Option<u8>,
    pub enter_vehicle: bool,
}

impl WireControls {
    fn write(&self, w: &mut Writer) {
        w.f32(self.movement);
        let flags = u8::from(self.jump) | u8::from(self.duck) << 1 | u8::from(self.fire) << 2 | u8::from(self.next_weapon) << 3 | u8::from(self.previous_weapon) << 4 | u8::from(self.enter_vehicle) << 5;
        w.u8(flags);
        w.vec2(self.aim);
        w.option(&self.selected_weapon, |w, v| w.u8(*v));
    }

    fn read(r: &mut Reader) -> Result<WireControls, DecodeError> {
        let movement = r.f32()?;
        let flags = r.u8()?;
        let aim = r.vec2()?;
        let selected_weapon = r.option(|r| r.u8())?;
        Ok(WireControls {
            movement,
            jump: flags & 1 != 0,
            duck: flags & 2 != 0,
            fire: flags & 4 != 0,
            next_weapon: flags & 8 != 0,
            previous_weapon: flags & 16 != 0,
            enter_vehicle: flags & 32 != 0,
            aim,
            selected_weapon,
        })
    }
}

/// One character as its owner sends it every frame: the input stream and
/// `CharacterControl2D.OnSerializeNetworkView` (position, z, layer), the equipped weapon
/// (`SetWeapon`) and whether the object is active (piloting, dead or asleep off screen).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvatarState {
    pub id: NetId,
    pub controls: WireControls,
    pub position: [f32; 2],
    pub z: f32,
    pub layer: u8,
    pub weapon: Option<u8>,
    pub active: bool,
}

impl AvatarState {
    fn write(&self, w: &mut Writer) {
        self.id.write(w);
        self.controls.write(w);
        w.vec2(self.position);
        w.f32(self.z);
        w.u8(self.layer);
        w.option(&self.weapon, |w, v| w.u8(*v));
        w.bool(self.active);
    }

    fn read(r: &mut Reader) -> Result<AvatarState, DecodeError> {
        Ok(AvatarState {
            id: NetId::read(r)?,
            controls: WireControls::read(r)?,
            position: r.vec2()?,
            z: r.f32()?,
            layer: r.u8()?,
            weapon: r.option(|r| r.u8())?,
            active: r.bool()?,
        })
    }
}

/// A helicopter's aim: a point in the world (mouse) or a direction from the helicopter
/// (gamepad), as `HelicopterInput` streams `absoluteAim` / `relativeAim`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WireAim {
    Absolute([f32; 2]),
    Relative([f32; 2]),
}

/// `HelicopterInput.OnSerializeNetworkView` plus `SynchronizeRigidBody2D` of the hull.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HelicopterState {
    pub id: NetId,
    pub movement: [f32; 2],
    pub fire: bool,
    pub aim: Option<WireAim>,
    pub exit: bool,
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub rotation: f32,
    pub angular_velocity: f32,
    pub active: bool,
}

impl HelicopterState {
    fn write(&self, w: &mut Writer) {
        self.id.write(w);
        w.vec2(self.movement);
        w.bool(self.fire);
        w.option(&self.aim, |w, a| match a {
            WireAim::Absolute(v) => {
                w.u8(0);
                w.vec2(*v);
            }
            WireAim::Relative(v) => {
                w.u8(1);
                w.vec2(*v);
            }
        });
        w.bool(self.exit);
        w.vec2(self.position);
        w.vec2(self.velocity);
        w.f32(self.rotation);
        w.f32(self.angular_velocity);
        w.bool(self.active);
    }

    fn read(r: &mut Reader) -> Result<HelicopterState, DecodeError> {
        Ok(HelicopterState {
            id: NetId::read(r)?,
            movement: r.vec2()?,
            fire: r.bool()?,
            aim: r.option(|r| match r.u8()? {
                0 => Ok(WireAim::Absolute(r.vec2()?)),
                1 => Ok(WireAim::Relative(r.vec2()?)),
                _ => Err(DecodeError("bad aim")),
            })?,
            exit: r.bool()?,
            position: r.vec2()?,
            velocity: r.vec2()?,
            rotation: r.f32()?,
            angular_velocity: r.f32()?,
            active: r.bool()?,
        })
    }
}

/// `PlayerScore` (`ReceivePlayerScore` sends damage and frags; the frags against each player
/// travel with them so the end-of-game tables agree everywhere).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WireScore {
    pub damage_dealt: f32,
    pub frags: i32,
    pub frags_against: [i32; 4],
}

/// `LevelLoader.LoadType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadKind {
    Normal,
    Lobby,
    Complete,
}

/// Lobby settings every peer shows (`LobbyGameMode`, `LobbyDifficulty`, `LobbyLevelSelect`,
/// `LobbyTeams`): whoever changes one sends them all, the host applies and relays them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LobbySettings {
    pub mode: u8,
    pub difficulty: i32,
    pub level_index: u32,
    /// `TeamIndexForPlayer(0..4)`.
    pub teams: [u8; 4],
}

/// A colour of a player's figure (`RemoteColorAssignments.ReceiveColorAssignment`); `None` is
/// the default.
pub type WireColor = Option<[f32; 3]>;

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// Client → host on connecting: who it is, and the room's password if it has one.
    Hello { version: u32, name: String, password: String },
    /// `UniqueClientNumber.ReceiveNumber`.
    Welcome { client_number: u32 },
    /// The host turned the client away (full, playing, other version).
    Refused { reason: String },

    Settings(LobbySettings),
    /// `PlayerSlotAssignments.ClaimedSlots` (client → host).
    ClaimSlots { claims: [Option<WirePlayerId>; 4] },
    /// `ReceiveAssignments` / `ReceiveAssignmentsAndSpawn` (host → client): `ours` is "assigned
    /// to you".
    Assignments { used: [bool; 4], ours: [bool; 4], ids: [Option<WirePlayerId>; 4], spawn: Option<[i32; 4]> },
    /// `ReceiveColorAssignment` for all three colours of a slot.
    Colors { slot: u8, body: WireColor, helmet: WireColor, face: WireColor },
    /// `RemoteLevelAvailability.ReceiveAvailability`.
    Unlocked { highest_campaign_level: i32 },

    /// `LevelLoader.RequestLoad` (client → host).
    RequestLoad(LoadKind),
    /// `LevelLoader.LoadLevel` (host → clients). `seed` starts every peer's shared random stream
    /// so level set-up draws (enemy faces) agree.
    LoadLevel { level: String, mode: u8, difficulty: i32, player_count: u8, prefix: u32, seed: u32 },
    /// `FinishedLoadingLevel` (client → host).
    FinishedLoading { prefix: u32 },
    /// `AllFinishedLoadingLevel` (host → clients).
    AllFinishedLoading { prefix: u32 },

    /// The owner's state streams for this frame (relayed by the host).
    Frame { avatars: Vec<AvatarState>, helicopters: Vec<HelicopterState> },
    /// `SyncServerDamageToClients.ChangedRemotely` (host → clients).
    Health { id: NetId, remaining: f32 },
    /// `PlayerScore.ReceivePlayerScore` (host → clients).
    Score { slot: u8, score: WireScore },
    /// The player's lives (host → clients).
    Lives { slot: u8, remaining: i32 },
    /// `Respawner.Respawn` (host → clients).
    Respawn { id: NetId, position: [f32; 2] },
    /// A helicopter's health (`HelicopterControl`'s health on the server).
    HelicopterHealth { id: NetId, remaining: f32 },
    /// Level scripts whose outcome the host decides (`DestroyAllEnemiesLevelEnder.Activate`,
    /// `TimeLimit`, `VersusGameEnder.EndGame`, ...): a named event about a scene object.
    LevelEvent { kind: String, object: u64, values: Vec<i32> },
    /// The host left the game deliberately (`ClientDisconnection.DeliberateDisconnection`).
    Goodbye,

    /// The room as it is now (host → clients, whenever it changes).
    RoomInfo(Room),
    /// A client is (not) ready to play (client → host).
    Ready(bool),
    /// A chat line: a client sends it with an empty `from`; the host sends everybody's (its own
    /// too) with the sender's name filled in.
    Chat { from: String, text: String },
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let ids = |w: &mut Writer, ids: &[Option<WirePlayerId>; 4]| {
            for id in ids {
                w.option(id, |w, id| id.write(w));
            }
        };
        let color = |w: &mut Writer, c: &WireColor| w.option(c, |w, c| {
            w.f32(c[0]);
            w.f32(c[1]);
            w.f32(c[2]);
        });
        match self {
            Message::Hello { version, name, password } => {
                w.u8(0);
                w.u32(*version);
                w.str(name);
                w.str(password);
            }
            Message::Welcome { client_number } => {
                w.u8(1);
                w.u32(*client_number);
            }
            Message::Refused { reason } => {
                w.u8(2);
                w.str(reason);
            }
            Message::Settings(s) => {
                w.u8(3);
                w.u8(s.mode);
                w.i32(s.difficulty);
                w.u32(s.level_index);
                for t in s.teams {
                    w.u8(t);
                }
            }
            Message::ClaimSlots { claims } => {
                w.u8(4);
                ids(&mut w, claims);
            }
            Message::Assignments { used, ours, ids: slot_ids, spawn } => {
                w.u8(5);
                for i in 0..4 {
                    w.bool(used[i]);
                    w.bool(ours[i]);
                }
                ids(&mut w, slot_ids);
                w.option(spawn, |w, s| {
                    for v in s {
                        w.i32(*v);
                    }
                });
            }
            Message::Colors { slot, body, helmet, face } => {
                w.u8(6);
                w.u8(*slot);
                color(&mut w, body);
                color(&mut w, helmet);
                color(&mut w, face);
            }
            Message::Unlocked { highest_campaign_level } => {
                w.u8(7);
                w.i32(*highest_campaign_level);
            }
            Message::RequestLoad(kind) => {
                w.u8(8);
                w.u8(match kind {
                    LoadKind::Normal => 0,
                    LoadKind::Lobby => 1,
                    LoadKind::Complete => 2,
                });
            }
            Message::LoadLevel { level, mode, difficulty, player_count, prefix, seed } => {
                w.u8(9);
                w.str(level);
                w.u8(*mode);
                w.i32(*difficulty);
                w.u8(*player_count);
                w.u32(*prefix);
                w.u32(*seed);
            }
            Message::FinishedLoading { prefix } => {
                w.u8(10);
                w.u32(*prefix);
            }
            Message::AllFinishedLoading { prefix } => {
                w.u8(11);
                w.u32(*prefix);
            }
            Message::Frame { avatars, helicopters } => {
                w.u8(12);
                w.list(avatars, |w, a| a.write(w));
                w.list(helicopters, |w, h| h.write(w));
            }
            Message::Health { id, remaining } => {
                w.u8(13);
                id.write(&mut w);
                w.f32(*remaining);
            }
            Message::Score { slot, score } => {
                w.u8(14);
                w.u8(*slot);
                w.f32(score.damage_dealt);
                w.i32(score.frags);
                for v in score.frags_against {
                    w.i32(v);
                }
            }
            Message::Lives { slot, remaining } => {
                w.u8(15);
                w.u8(*slot);
                w.i32(*remaining);
            }
            Message::Respawn { id, position } => {
                w.u8(16);
                id.write(&mut w);
                w.vec2(*position);
            }
            Message::HelicopterHealth { id, remaining } => {
                w.u8(17);
                id.write(&mut w);
                w.f32(*remaining);
            }
            Message::LevelEvent { kind, object, values } => {
                w.u8(18);
                w.str(kind);
                w.u64(*object);
                w.list(values, |w, v| w.i32(*v));
            }
            Message::Goodbye => w.u8(19),
            Message::RoomInfo(room) => {
                w.u8(20);
                room.write(&mut w);
            }
            Message::Ready(ready) => {
                w.u8(21);
                w.bool(*ready);
            }
            Message::Chat { from, text } => {
                w.u8(22);
                w.str(from);
                w.str(text);
            }
        }
        w.bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Message, DecodeError> {
        let mut r = Reader::new(bytes);
        let ids = |r: &mut Reader| -> Result<[Option<WirePlayerId>; 4], DecodeError> {
            Ok([r.option(WirePlayerId::read)?, r.option(WirePlayerId::read)?, r.option(WirePlayerId::read)?, r.option(WirePlayerId::read)?])
        };
        let color = |r: &mut Reader| -> Result<WireColor, DecodeError> { r.option(|r| Ok([r.f32()?, r.f32()?, r.f32()?])) };
        let message = match r.u8()? {
            0 => Message::Hello { version: r.u32()?, name: r.str()?, password: r.str()? },
            1 => Message::Welcome { client_number: r.u32()? },
            2 => Message::Refused { reason: r.str()? },
            3 => Message::Settings(LobbySettings { mode: r.u8()?, difficulty: r.i32()?, level_index: r.u32()?, teams: [r.u8()?, r.u8()?, r.u8()?, r.u8()?] }),
            4 => Message::ClaimSlots { claims: ids(&mut r)? },
            5 => {
                let mut used = [false; 4];
                let mut ours = [false; 4];
                for i in 0..4 {
                    used[i] = r.bool()?;
                    ours[i] = r.bool()?;
                }
                let slot_ids = ids(&mut r)?;
                let spawn = r.option(|r| Ok([r.i32()?, r.i32()?, r.i32()?, r.i32()?]))?;
                Message::Assignments { used, ours, ids: slot_ids, spawn }
            }
            6 => Message::Colors { slot: r.u8()?, body: color(&mut r)?, helmet: color(&mut r)?, face: color(&mut r)? },
            7 => Message::Unlocked { highest_campaign_level: r.i32()? },
            8 => Message::RequestLoad(match r.u8()? {
                0 => LoadKind::Normal,
                1 => LoadKind::Lobby,
                2 => LoadKind::Complete,
                _ => return Err(DecodeError("bad load kind")),
            }),
            9 => Message::LoadLevel { level: r.str()?, mode: r.u8()?, difficulty: r.i32()?, player_count: r.u8()?, prefix: r.u32()?, seed: r.u32()? },
            10 => Message::FinishedLoading { prefix: r.u32()? },
            11 => Message::AllFinishedLoading { prefix: r.u32()? },
            12 => Message::Frame { avatars: r.list(AvatarState::read)?, helicopters: r.list(HelicopterState::read)? },
            13 => Message::Health { id: NetId::read(&mut r)?, remaining: r.f32()? },
            14 => {
                let slot = r.u8()?;
                let damage_dealt = r.f32()?;
                let frags = r.i32()?;
                let frags_against = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
                Message::Score { slot, score: WireScore { damage_dealt, frags, frags_against } }
            }
            15 => Message::Lives { slot: r.u8()?, remaining: r.i32()? },
            16 => Message::Respawn { id: NetId::read(&mut r)?, position: r.vec2()? },
            17 => Message::HelicopterHealth { id: NetId::read(&mut r)?, remaining: r.f32()? },
            18 => Message::LevelEvent { kind: r.str()?, object: r.u64()?, values: r.list(|r| r.i32())? },
            19 => Message::Goodbye,
            20 => Message::RoomInfo(Room::read(&mut r)?),
            21 => Message::Ready(r.bool()?),
            22 => Message::Chat { from: r.str()?, text: r.str()? },
            _ => return Err(DecodeError("unknown message")),
        };
        if !r.is_empty() {
            return Err(DecodeError("trailing bytes"));
        }
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(m: Message) {
        assert_eq!(Message::decode(&m.encode()), Ok(m));
    }

    #[test]
    fn every_message_round_trips() {
        let id = WirePlayerId { platform: None, id: "1".into(), name: "Player 2".into() };
        let controls = WireControls { movement: -0.2, jump: true, fire: true, aim: [0.6, 0.8], selected_weapon: Some(3), enter_vehicle: true, ..WireControls::default() };
        round_trip(Message::Hello { version: 1005, name: "PC".into(), password: "pw".into() });
        round_trip(Message::RoomInfo(Room {
            name: "Room".into(),
            locked: false,
            max_machines: 3,
            members: vec![crate::room::RoomMember { number: 0, name: "Host".into(), ready: true }],
        }));
        round_trip(Message::Ready(true));
        round_trip(Message::Chat { from: "Dan".into(), text: "hi".into() });
        round_trip(Message::Welcome { client_number: 2 });
        round_trip(Message::Refused { reason: "full".into() });
        round_trip(Message::Settings(LobbySettings { mode: 2, difficulty: -1, level_index: 4, teams: [0, 1, 1, 0] }));
        round_trip(Message::ClaimSlots { claims: [Some(id.clone()), None, None, None] });
        round_trip(Message::Assignments { used: [true, true, false, false], ours: [false, true, false, false], ids: [None, Some(id), None, None], spawn: Some([2, 0, -1, -1]) });
        round_trip(Message::Colors { slot: 1, body: Some([1.0, 0.5, 0.0]), helmet: None, face: Some([0.2, 0.2, 0.2]) });
        round_trip(Message::Unlocked { highest_campaign_level: 3 });
        round_trip(Message::RequestLoad(LoadKind::Lobby));
        round_trip(Message::LoadLevel { level: "Japan VS".into(), mode: 1, difficulty: 0, player_count: 3, prefix: 7, seed: 99 });
        round_trip(Message::FinishedLoading { prefix: 7 });
        round_trip(Message::AllFinishedLoading { prefix: 7 });
        round_trip(Message::Frame {
            avatars: vec![AvatarState { id: NetId::Player(2), controls, position: [3.0, -1.0], z: -0.1, layer: 10, weapon: Some(1), active: true }],
            helicopters: vec![HelicopterState {
                id: NetId::Scene(123_456_789_012),
                movement: [1.0, 0.0],
                fire: false,
                aim: Some(WireAim::Relative([0.0, -1.0])),
                exit: false,
                position: [10.0, 5.0],
                velocity: [0.5, 0.0],
                rotation: 3.0,
                angular_velocity: -1.0,
                active: true,
            }],
        });
        round_trip(Message::Health { id: NetId::Scene(5), remaining: 0.5 });
        round_trip(Message::Score { slot: 3, score: WireScore { damage_dealt: 2.5, frags: 4, frags_against: [0, 1, 3, 0] } });
        round_trip(Message::Lives { slot: 0, remaining: -1 });
        round_trip(Message::Respawn { id: NetId::Player(1), position: [1.0, 2.0] });
        round_trip(Message::HelicopterHealth { id: NetId::Scene(9), remaining: 40.0 });
        round_trip(Message::LevelEvent { kind: "VersusGameEnded".into(), object: 0, values: vec![1, 2, 3] });
        round_trip(Message::Goodbye);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(Message::decode(&[]).is_err());
        assert!(Message::decode(&[200]).is_err());
        let mut bytes = Message::Goodbye.encode();
        bytes.push(0);
        assert!(Message::decode(&bytes).is_err());
    }
}
