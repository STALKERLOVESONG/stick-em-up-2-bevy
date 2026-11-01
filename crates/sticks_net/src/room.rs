//! Lobby rooms (not in the original, whose host was just "the device hosting the game"): a
//! hosted network game has a name, an optional password and a size; the host keeps the list of
//! who is in it, who is ready, and relays the room's chat.
//!
//! What a listing shows about a room travels in the LAN announcement's info (and in the answer
//! to an internet probe): `name\0players/max\0open|locked`. The original reads the info up to its
//! first NUL as the host's name, so it lists a room by its name.

use crate::wire::{DecodeError, Reader, Writer};

/// Longest room name, player name and password (the original's "Join Game: …" label holds 25
/// characters of name).
pub const MAX_NAME: usize = 25;
/// Longest chat line.
pub const MAX_CHAT: usize = 120;
/// Machines in a room, the host's included: each needs a player slot of its own, and there are
/// four.
pub const MAX_MACHINES: u8 = 4;

/// `text` as it may travel: one line without control characters or NULs, at most `limit`
/// characters, trimmed.
pub fn clean(text: &str, limit: usize) -> String {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).take(limit).collect();
    cleaned.trim().to_owned()
}

/// What the host sets when it opens a room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomConfig {
    pub name: String,
    /// Empty: anybody may join.
    pub password: String,
    /// Machines allowed in, the host's included (2..=`MAX_MACHINES`).
    pub max_machines: u8,
}

impl RoomConfig {
    pub fn new(name: &str, password: &str, max_machines: u8) -> RoomConfig {
        let name = clean(name, MAX_NAME);
        RoomConfig {
            name: if name.is_empty() { "Stick 'Em Up 2".to_owned() } else { name },
            password: clean(password, MAX_NAME),
            max_machines: max_machines.clamp(2, MAX_MACHINES),
        }
    }

    pub fn locked(&self) -> bool {
        !self.password.is_empty()
    }
}

/// One machine in a room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomMember {
    /// `UniqueClientNumber` (the host is 0).
    pub number: u32,
    pub name: String,
    /// The host counts as ready: it starts the game.
    pub ready: bool,
}

/// The room as the host sends it to everybody whenever it changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    pub name: String,
    pub locked: bool,
    pub max_machines: u8,
    /// The host first.
    pub members: Vec<RoomMember>,
}

impl Room {
    pub fn everyone_ready(&self) -> bool {
        self.members.iter().all(|m| m.ready)
    }

    /// The members who are not ready, by name.
    pub fn waiting_for(&self) -> Vec<String> {
        self.members.iter().filter(|m| !m.ready).map(|m| m.name.clone()).collect()
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        w.str(&self.name);
        w.bool(self.locked);
        w.u8(self.max_machines);
        w.list(&self.members, |w, m| {
            w.u32(m.number);
            w.str(&m.name);
            w.bool(m.ready);
        });
    }

    pub(crate) fn read(r: &mut Reader) -> Result<Room, DecodeError> {
        Ok(Room {
            name: r.str()?,
            locked: r.bool()?,
            max_machines: r.u8()?,
            members: r.list(|r| Ok(RoomMember { number: r.u32()?, name: r.str()?, ready: r.bool()? }))?,
        })
    }

    /// The announcement info for this room.
    pub fn info(&self) -> String {
        format!("{}\0{}/{}\0{}", self.name, self.members.len(), self.max_machines, if self.locked { "locked" } else { "open" })
    }
}

/// What a listing says about a room (from the announcement info).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomSummary {
    pub name: String,
    /// `None` for hosts that do not say (the original).
    pub machines: Option<(u8, u8)>,
    pub locked: bool,
}

impl RoomSummary {
    pub fn parse(info: &str) -> RoomSummary {
        let mut parts = info.split('\0');
        let name = parts.next().unwrap_or_default().to_owned();
        let machines = parts.next().and_then(|p| {
            let (n, m) = p.split_once('/')?;
            Some((n.parse().ok()?, m.parse().ok()?))
        });
        let locked = parts.next() == Some("locked");
        RoomSummary { name, machines, locked }
    }

    pub fn is_full(&self) -> bool {
        self.machines.is_some_and(|(n, m)| n >= m)
    }

    /// "Name 2/4" with a lock note: what a join button shows.
    pub fn label(&self) -> String {
        let mut s = self.name.clone();
        if let Some((n, m)) = self.machines {
            s.push_str(&format!(" {n}/{m}"));
        }
        if self.locked {
            s.push_str(" (locked)");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_summarise_in_the_announcement_info() {
        let room = Room {
            name: "Dan's Room".into(),
            locked: true,
            max_machines: 4,
            members: vec![RoomMember { number: 0, name: "Dan".into(), ready: true }, RoomMember { number: 1, name: "Sam".into(), ready: false }],
        };
        let summary = RoomSummary::parse(&room.info());
        assert_eq!(summary, RoomSummary { name: "Dan's Room".into(), machines: Some((2, 4)), locked: true });
        assert_eq!(summary.label(), "Dan's Room 2/4 (locked)");
        assert!(!summary.is_full());
        assert_eq!(room.waiting_for(), vec!["Sam".to_owned()]);
        // The original's info is a bare machine name.
        assert_eq!(RoomSummary::parse("DESKTOP-1"), RoomSummary { name: "DESKTOP-1".into(), machines: None, locked: false });
    }

    #[test]
    fn configs_and_text_are_cleaned() {
        let c = RoomConfig::new("  \n ", "pw\u{0}", 9);
        assert_eq!(c.name, "Stick 'Em Up 2");
        assert_eq!(c.password, "pw");
        assert_eq!(c.max_machines, MAX_MACHINES);
        assert!(c.locked());
        assert_eq!(clean(&"x".repeat(200), MAX_CHAT).len(), MAX_CHAT);
    }
}
