//! `PlayerSlotAssignments`: which machine plays which of the four player slots in a network
//! game. Every peer claims the slots its local inputs sit in; the host grants a claim unless
//! another machine holds the slot, and tells each client which slots are theirs.

use crate::transport::PeerId;

pub const SLOTS: usize = 4;

/// A machine of the game: the host itself or one of its clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    Host,
    Client(PeerId),
}

/// The table every peer keeps (`slotUsed`, `slotIDs`, `slotAssignedToUs`; the host also knows
/// `slotClient`).
#[derive(Clone, Debug, PartialEq)]
pub struct SlotTable<Id> {
    pub used: [bool; SLOTS],
    pub ids: [Option<Id>; SLOTS],
    pub assigned_to_us: [bool; SLOTS],
    /// Host only: who holds each used slot.
    pub holder: [Option<Machine>; SLOTS],
}

impl<Id> Default for SlotTable<Id> {
    fn default() -> Self {
        SlotTable { used: [false; SLOTS], ids: std::array::from_fn(|_| None), assigned_to_us: [false; SLOTS], holder: [None; SLOTS] }
    }
}

/// What the host tells one client (`ReceiveAssignments`).
#[derive(Clone, Debug, PartialEq)]
pub struct Assignments<Id> {
    pub used: [bool; SLOTS],
    pub ours: [bool; SLOTS],
    pub ids: [Option<Id>; SLOTS],
}

impl<Id: Clone + PartialEq> SlotTable<Id> {
    /// `SlotAssignedToUs`.
    pub fn assigned_to_us(&self, slot: usize) -> bool {
        self.used[slot] && self.assigned_to_us[slot]
    }

    /// `SlotAssignedToAnotherClient`.
    pub fn assigned_elsewhere(&self, slot: usize) -> bool {
        self.used[slot] && !self.assigned_to_us[slot]
    }

    pub fn elsewhere(&self) -> [bool; SLOTS] {
        std::array::from_fn(|i| self.assigned_elsewhere(i))
    }

    pub fn ours(&self) -> [bool; SLOTS] {
        std::array::from_fn(|i| self.assigned_to_us(i))
    }

    /// `RemotePlayerCount`.
    pub fn remote_count(&self) -> usize {
        (0..SLOTS).filter(|&i| self.assigned_elsewhere(i)).count()
    }

    /// `MakeClaims` on this peer's own table (before the claims go to the server).
    pub fn make_claims(&mut self, claims: &[Option<Id>; SLOTS]) {
        for i in 0..SLOTS {
            let has = claims[i].is_some();
            if self.assigned_to_us[i] && !has {
                self.used[i] = false;
            }
            if has {
                self.used[i] = true;
                self.ids[i] = claims[i].clone();
            }
            self.assigned_to_us[i] = has;
        }
    }

    /// `ClaimedSlots` on the host: a slot nobody holds, or that the claimer already holds, follows
    /// the claim. Afterwards the host's own "assigned to us" flags are recomputed (`Broadcast`).
    pub fn claimed(&mut self, claims: &[Option<Id>; SLOTS], claimer: Machine) {
        for i in 0..SLOTS {
            // An unheld slot belongs to the host (`default(NetworkPlayer)` is the server's).
            let held_by_claimer = self.holder[i].unwrap_or(Machine::Host) == claimer;
            if !self.used[i] || held_by_claimer {
                let has = claims[i].is_some();
                self.used[i] = has;
                self.holder[i] = has.then_some(claimer);
                self.ids[i] = claims[i].clone();
            }
        }
        self.refresh_host_view();
    }

    /// `OnPlayerDisconnected` on the host: the client's slots are free again.
    pub fn disconnected(&mut self, peer: PeerId) {
        for i in 0..SLOTS {
            if self.used[i] && self.holder[i] == Some(Machine::Client(peer)) {
                self.used[i] = false;
                self.holder[i] = None;
                self.ids[i] = None;
            }
        }
        self.refresh_host_view();
    }

    fn refresh_host_view(&mut self) {
        for i in 0..SLOTS {
            self.assigned_to_us[i] = self.used[i] && self.holder[i] == Some(Machine::Host);
        }
    }

    /// `SendAssignmentsTo(client)`.
    pub fn assignments_for(&self, peer: PeerId) -> Assignments<Id> {
        Assignments { used: self.used, ours: std::array::from_fn(|i| self.used[i] && self.holder[i] == Some(Machine::Client(peer))), ids: self.ids.clone() }
    }

    /// `ReceiveAssignments` on a client.
    pub fn receive(&mut self, a: Assignments<Id>) {
        self.used = a.used;
        self.assigned_to_us = a.ours;
        self.ids = a.ids;
        self.holder = [None; SLOTS];
    }

    /// The machine playing a slot (host view).
    pub fn holder(&self, slot: usize) -> Option<Machine> {
        if self.used[slot] { self.holder[slot] } else { None }
    }

    /// `ResetSlotsToDefaults`.
    pub fn reset(&mut self) {
        *self = SlotTable::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(slots: &[usize]) -> [Option<&'static str>; SLOTS] {
        std::array::from_fn(|i| slots.contains(&i).then_some(["a", "b", "c", "d"][i]))
    }

    #[test]
    fn the_host_grants_free_slots_and_keeps_held_ones() {
        let mut host: SlotTable<&str> = SlotTable::default();
        host.make_claims(&claims(&[0]));
        host.claimed(&claims(&[0]), Machine::Host);
        assert!(host.assigned_to_us(0));
        // A client claims 0 (taken) and 1 (free).
        host.claimed(&claims(&[0, 1]), Machine::Client(7));
        assert_eq!(host.holder(0), Some(Machine::Host));
        assert_eq!(host.holder(1), Some(Machine::Client(7)));
        assert!(host.assigned_elsewhere(1));
        assert_eq!(host.remote_count(), 1);
        let a = host.assignments_for(7);
        assert_eq!(a.used, [true, true, false, false]);
        assert_eq!(a.ours, [false, true, false, false]);

        // The client sees slot 0 as elsewhere, slot 1 as its own.
        let mut client: SlotTable<&str> = SlotTable::default();
        client.receive(a);
        assert!(client.assigned_elsewhere(0));
        assert!(client.assigned_to_us(1));

        // Dropping its claim frees the slot; a disconnection frees all of them.
        host.claimed(&claims(&[2]), Machine::Client(7));
        assert_eq!(host.holder(1), None);
        assert_eq!(host.holder(2), Some(Machine::Client(7)));
        host.disconnected(7);
        assert_eq!(host.holder(2), None);
        assert!(host.assigned_to_us(0));
    }
}
