//! Boarding and leaving vehicles: the offline rules of `ControlSlot`, the vehicle search in
//! `CharacterControl2D.Update`, the exit handshake of `HelicopterControl.Update` /
//! `HelicopterGunnerControl.Update`, and `Player.Update`'s disembarkment.
//!
//! What the original does, offline:
//!
//! * Every `ControlSlot` without a `CharacterControl2D` is a candidate vehicle (listed once in
//!   `CharacterControl2D.Awake`). Each frame an active avatar picks the nearest candidate (2D
//!   squared distance between the transforms, strictly smaller wins, so the first of equals is
//!   kept) whose object is active, which `CanBeControlledBy(player)` and which the player does not
//!   already control. `E` (`Keyboard.EnterVehiclePressedThisFrame`) requests it.
//! * `CanBeControlledBy`: the slot is free or already the player's; the slot the player controls
//!   now (the avatar) is within `controlRadius` of this slot's transform (3D squared distance,
//!   `highlight` is never set — `Highlight.SetControlRadius` throws); every eligibility checker
//!   agrees (gunner and passenger seats want the pilot seat taken).
//! * Taking an exclusive slot deactivates the avatar (`RelinquishControlExceptFor`), records the
//!   vehicle root's "Disembarkment Point", and sends `SetController` to the vehicle.
//! * The vehicle's `Update` relinquishes control when its input's `Exit` is set, except on the
//!   frame the controller was set (`controllerJustSet`), so the `E` that entered does not leave.
//! * `Player.Update` then moves the avatar to the disembarkment point's x/y (keeping the avatar's
//!   z), reactivates it and hands control back to it.
//!
//! The networked branches (RPCs, ownership transfer) are not reproduced: the game is offline.

use unity_compat::{Vec2, Vec3};

/// `Utility.Layers.TeamVehicles(team)`: the layer `Player.SetUp` puts a controlled vehicle on.
pub fn team_vehicles_layer(team: u8) -> u8 {
    20 + team
}

/// `HelicopterControl.SetController`: engine volume while somebody flies it.
pub const OCCUPIED_ENGINE_VOLUME: f32 = 1.0;
/// `HelicopterControl.SetUnoccupiedEngineVolume`.
pub const UNOCCUPIED_ENGINE_VOLUME: f32 = 0.5;

/// `HelicopterControl.FixedUpdate`: `pitch = min(1, |movementControl|) * 0.20000005 + 0.9`.
pub fn helicopter_engine_pitch(movement_control: Vec2) -> f32 {
    movement_control.magnitude().min(1.0) * 0.200_000_05 + 0.9
}

/// A `ControlSlot` (exclusive, offline). `P` identifies a player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlSlot<P> {
    pub control_radius: f32,
    controller: Option<P>,
}

impl<P: Copy + PartialEq> ControlSlot<P> {
    pub fn new(control_radius: f32) -> Self {
        Self { control_radius, controller: None }
    }

    /// `AvailableForControl`.
    pub fn available(&self) -> bool {
        self.controller.is_none()
    }

    /// `Controller`.
    pub fn controller(&self) -> Option<P> {
        self.controller
    }

    pub fn is_controlled_by(&self, player: P) -> bool {
        self.controller == Some(player)
    }

    /// `CloseEnoughForControlByPlayerAt`: 3D squared distance against `controlRadius²`.
    pub fn close_enough(&self, slot_position: Vec3, player_position: Vec3) -> bool {
        (slot_position - player_position).sqr_magnitude() <= self.control_radius * self.control_radius
    }

    /// `CanBeControlledBy(player)`. `controlled_slot_position` is the position of the slot the
    /// player controls now (`None` when it controls nothing); `eligible` is the conjunction of the
    /// slot's eligibility checkers.
    pub fn can_be_controlled_by(&self, player: P, slot_position: Vec3, controlled_slot_position: Option<Vec3>, eligible: bool) -> bool {
        if !self.available() && self.controller != Some(player) {
            return false;
        }
        if let Some(position) = controlled_slot_position {
            if !self.close_enough(slot_position, position) {
                return false;
            }
        }
        eligible
    }

    /// `SendSetControllerMessage` (the slot's half).
    pub fn set_controller(&mut self, player: P) {
        self.controller = Some(player);
    }

    /// `ClearController`: returns who controlled it.
    pub fn clear_controller(&mut self) -> Option<P> {
        self.controller.take()
    }
}

/// A candidate for `CharacterControl2D`'s vehicle search.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Enterable {
    /// The slot's transform position.
    pub position: Vec3,
    /// `gameObject.activeInHierarchy && CanBeControlledBy(player) && Controller != player`.
    pub selectable: bool,
}

/// `CharacterControl2D.Update`'s search: the selectable candidate nearest to the avatar in 2D
/// (`Vector2.SqrMagnitude`), the first one on ties.
pub fn nearest_enterable(avatar_position: Vec3, candidates: &[Enterable]) -> Option<usize> {
    let mut best = None;
    let mut best_distance = f32::MAX;
    for (i, c) in candidates.iter().enumerate() {
        if !c.selectable {
            continue;
        }
        let d = Vec2::new(c.position.x - avatar_position.x, c.position.y - avatar_position.y).sqr_magnitude();
        if d < best_distance {
            best = Some(i);
            best_distance = d;
        }
    }
    best
}

/// The `controllerJustSet` handshake of `HelicopterControl.Update` (and the gunner seat's).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExitLatch {
    controller_just_set: bool,
}

impl ExitLatch {
    /// `SetController`.
    pub fn on_set_controller(&mut self) {
        self.controller_just_set = true;
    }

    /// One `Update` with an input present: whether control is relinquished this frame.
    pub fn update(&mut self, exit: bool) -> bool {
        if exit && !self.controller_just_set {
            true
        } else {
            self.controller_just_set = false;
            false
        }
    }
}

/// `Player.MoveAvatarTo(disembarkmentPoint ?? avatar)`: x/y from the point, z kept.
pub fn disembark_position(disembarkment_point: Option<Vec3>, avatar_position: Vec3) -> Vec3 {
    let destination = disembarkment_point.unwrap_or(avatar_position);
    Vec3::new(destination.x, destination.y, avatar_position.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_needs_the_player_within_the_radius_in_3d() {
        let slot: ControlSlot<u8> = ControlSlot::new(10.0);
        let at = Vec3::new(546.0, 10.0, 0.0);
        assert!(slot.can_be_controlled_by(1, at, Some(Vec3::new(540.0, 2.0, 0.0)), true));
        // 6² + 8² = 100: exactly at the radius is close enough.
        assert!(slot.can_be_controlled_by(1, at, Some(Vec3::new(540.0, 2.0, 0.0)), true));
        // z counts: (6, 8, 1) is beyond 10.
        assert!(!slot.can_be_controlled_by(1, at, Some(Vec3::new(540.0, 2.0, -1.0)), true));
        // A player controlling nothing is not distance-checked.
        assert!(slot.can_be_controlled_by(1, at, None, true));
        assert!(!slot.can_be_controlled_by(1, at, None, false));
    }

    #[test]
    fn a_taken_slot_is_only_the_controllers() {
        let mut slot: ControlSlot<u8> = ControlSlot::new(10.0);
        slot.set_controller(1);
        let at = Vec3::new(0.0, 0.0, 0.0);
        assert!(!slot.can_be_controlled_by(2, at, Some(at), true));
        assert!(slot.can_be_controlled_by(1, at, Some(at), true));
        assert_eq!(slot.clear_controller(), Some(1));
        assert!(slot.available());
    }

    #[test]
    fn nearest_candidate_uses_2d_distance_and_keeps_the_first_tie() {
        let avatar = Vec3::new(0.0, 0.0, -0.1);
        let candidates = [
            Enterable { position: Vec3::new(5.0, 0.0, 0.0), selectable: true },
            Enterable { position: Vec3::new(0.0, 3.0, 50.0), selectable: true }, // z ignored
            Enterable { position: Vec3::new(-3.0, 0.0, 0.0), selectable: true }, // tie with #1
            Enterable { position: Vec3::new(1.0, 0.0, 0.0), selectable: false },
        ];
        assert_eq!(nearest_enterable(avatar, &candidates), Some(1));
        assert_eq!(nearest_enterable(avatar, &candidates[3..]), None);
    }

    #[test]
    fn the_entering_press_does_not_exit() {
        let mut latch = ExitLatch::default();
        latch.on_set_controller();
        // Same frame (or the vehicle's next Update while E is still "pressed this frame").
        assert!(!latch.update(true));
        assert!(!latch.update(false));
        assert!(latch.update(true));
        // Without an exit the latch just stays clear.
        let mut latch = ExitLatch::default();
        latch.on_set_controller();
        assert!(!latch.update(false));
        assert!(latch.update(true));
    }

    #[test]
    fn disembarking_keeps_the_avatar_depth() {
        let p = disembark_position(Some(Vec3::new(546.0, 11.43, 0.0)), Vec3::new(540.0, 2.0, -0.1));
        assert_eq!(p, Vec3::new(546.0, 11.43, -0.1));
        let p = disembark_position(None, Vec3::new(540.0, 2.0, -0.1));
        assert_eq!(p, Vec3::new(540.0, 2.0, -0.1));
    }

    #[test]
    fn engine_pitch_and_layers() {
        assert!((helicopter_engine_pitch(Vec2::ZERO) - 0.9).abs() < 1e-6);
        assert!((helicopter_engine_pitch(Vec2::new(1.0, 1.0)) - 1.100_000_05).abs() < 1e-6);
        assert!((helicopter_engine_pitch(Vec2::new(0.5, 0.0)) - 1.0).abs() < 1e-6);
        assert_eq!(team_vehicles_layer(0), 20);
        assert_eq!(team_vehicles_layer(1), 21);
    }
}
