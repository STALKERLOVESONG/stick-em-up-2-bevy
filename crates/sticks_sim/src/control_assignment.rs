//! `ControlAssignment`'s model: which lobby inputs (keyboard and mouse, each gamepad) control
//! which of the four player slots, kept for the whole session (static lists in the original).
//!
//! * [`ControlAssignment::start`] (`Start`, once): the keyboard and mouse go to Player 1, and a
//!   gamepad too when it is the only one.
//! * [`ControlAssignment::update`] (`Update`, while the screen is up): unavailable inputs drop
//!   out; left/right move an input one column (right of Player 4 is the "unused" column, left
//!   from there brings it back to Player 4); an input not seen before goes to the first empty
//!   column (else Player 4), or to the unused column on the first frame after the screen opens.
//! * [`ControlAssignment::drop_into`]: a card dropped on another column (`UpdateGUI`).
//!
//! The network half (`PlayerSlotAssignments`, columns claimed by other clients) is not
//! reproduced: offline no column is assigned elsewhere.

use crate::lobby::MAX_PLAYERS;

/// The unused-controllers column (`deactivatedInputs`), after the four player columns.
pub const DEACTIVATED_COLUMN: usize = MAX_PLAYERS;

/// What a lobby input reports this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputFrame<T> {
    pub input: T,
    /// `LobbyInput.IsAvailable`: it has a current processor.
    pub available: bool,
    /// Its processor is a gamepad's (`LobbyInput.IsGamepadProcessor`).
    pub gamepad: bool,
    pub move_left: bool,
    pub move_right: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlAssignment<T> {
    /// `allPlayerInputs` (`None` until `Start` ran: `IsInitialized`).
    columns: Option<[Vec<T>; MAX_PLAYERS]>,
    /// `deactivatedInputs`.
    deactivated: Vec<T>,
}

impl<T> Default for ControlAssignment<T> {
    fn default() -> Self {
        Self { columns: None, deactivated: Vec::new() }
    }
}

impl<T: Copy + PartialEq> ControlAssignment<T> {
    pub fn is_initialized(&self) -> bool {
        self.columns.is_some()
    }

    /// `Start` (only the first time): the available keyboard (any non-gamepad input) in
    /// Player 1, and the gamepad there too when exactly one is available.
    pub fn start(&mut self, all: &[InputFrame<T>]) {
        if self.is_initialized() {
            return;
        }
        let mut columns: [Vec<T>; MAX_PLAYERS] = Default::default();
        let gamepads = all.iter().filter(|i| i.available && i.gamepad).count();
        for i in all.iter().filter(|i| i.available) {
            if !i.gamepad || gamepads == 1 {
                columns[0].push(i.input);
            }
        }
        self.columns = Some(columns);
    }

    /// `Update` (before `UpdateGUI`). `first_update` is `firstUpdateSinceEnabled`. Returns
    /// whether anything moved.
    pub fn update(&mut self, all: &[InputFrame<T>], first_update: bool) -> bool {
        self.update_beside(all, first_update, [false; MAX_PLAYERS])
    }

    /// [`ControlAssignment::update`] in a network game: `elsewhere[i]` is
    /// `PlayerSlotAssignments.SlotAssignedToAnotherClient(i)`. Moves skip those columns and new
    /// inputs never land in them.
    pub fn update_beside(&mut self, all: &[InputFrame<T>], first_update: bool, elsewhere: [bool; MAX_PLAYERS]) -> bool {
        let Some(columns) = self.columns.as_mut() else { return false };
        let available = |input: &T| all.iter().any(|i| i.input == *input && i.available);
        for column in columns.iter_mut() {
            column.retain(available);
        }
        self.deactivated.retain(available);
        let frame = |input: T| all.iter().find(|i| i.input == input).copied();
        let mut changed = false;
        let mut moved: Vec<T> = Vec::new();
        for c in 0..MAX_PLAYERS {
            let mut k = columns[c].len();
            while k > 0 {
                k -= 1;
                let input = columns[c][k];
                let Some(f) = frame(input) else { continue };
                if moved.contains(&input) || !(f.move_left || f.move_right) {
                    continue;
                }
                // Step over the columns another client plays in.
                let mut target = Some(c);
                loop {
                    target = match target {
                        Some(t) if f.move_left => t.checked_sub(1),
                        Some(t) => Some(t + 1),
                        None => None,
                    };
                    match target {
                        Some(t) if t < MAX_PLAYERS && elsewhere[t] => continue,
                        _ => break,
                    }
                }
                match target {
                    // Left of Player 1: nothing happens.
                    None => {}
                    Some(t) if t >= MAX_PLAYERS => {
                        columns[c].remove(k);
                        self.deactivated.push(input);
                        changed = true;
                    }
                    Some(t) => {
                        columns[c].remove(k);
                        columns[t].push(input);
                        moved.push(input);
                        changed = true;
                    }
                }
            }
        }
        let mut reactivated = Vec::new();
        let mut k = self.deactivated.len();
        while k > 0 {
            k -= 1;
            let input = self.deactivated[k];
            if frame(input).is_some_and(|f| f.move_left) {
                self.deactivated.remove(k);
                reactivated.push(input);
            }
        }
        for f in all {
            if !f.available || columns.iter().any(|c| c.contains(&f.input)) || self.deactivated.contains(&f.input) {
                continue;
            }
            if first_update {
                self.deactivated.push(f.input);
                continue;
            }
            // The first empty column; the last one when none is empty or the input came back
            // from the unused column (columns another client plays in do not count).
            let last = (0..MAX_PLAYERS).rev().find(|&c| !elsewhere[c]);
            let mut target = (0..MAX_PLAYERS).find(|&c| !elsewhere[c] && columns[c].is_empty()).or(last);
            if reactivated.contains(&f.input) {
                target = last;
            }
            match target {
                Some(t) => {
                    columns[t].push(f.input);
                    changed = true;
                }
                None => self.deactivated.push(f.input),
            }
        }
        changed
    }

    /// `ResolveWithServerAssignments(allowChanges)`: a column with inputs that the server gave
    /// to another client moves to the first free column (or its inputs go unused); without
    /// `allow_changes` (a level is loading) every such column is emptied. Returns the slots this
    /// peer now claims (`MakeClaims`: every column with inputs) when the claims changed, else
    /// `None`. `logged_in` tracks `lastLocallyLoggedInPlayerIndex`.
    pub fn resolve_with_server_assignments(
        &mut self,
        assigned_to_us: [bool; MAX_PLAYERS],
        assigned_elsewhere: [bool; MAX_PLAYERS],
        allow_changes: bool,
        logged_in: &mut Option<usize>,
    ) -> Option<[bool; MAX_PLAYERS]> {
        let mut deactivate = Vec::new();
        let columns = self.columns.as_mut()?;
        let mut changed = false;
        for i in 0..MAX_PLAYERS {
            if columns[i].is_empty() {
                if assigned_to_us[i] {
                    changed = true;
                }
                continue;
            }
            if assigned_to_us[i] {
                continue;
            }
            if !allow_changes {
                deactivate.push(i);
            } else if assigned_elsewhere[i] {
                match (0..MAX_PLAYERS).find(|&j| columns[j].is_empty() && !assigned_elsewhere[j]) {
                    Some(j) => {
                        columns.swap(i, j);
                        changed = true;
                    }
                    None => deactivate.push(i),
                }
            } else {
                changed = true;
            }
        }
        for i in deactivate {
            let inputs = std::mem::take(&mut columns[i]);
            self.deactivated.extend(inputs);
        }
        // `GetLocallyLoggedInPlayerIndex`: the first column with inputs or not played elsewhere.
        let columns = self.columns.as_ref()?;
        let now = (0..MAX_PLAYERS).find(|&i| !columns[i].is_empty() || !assigned_elsewhere[i]);
        if *logged_in != now {
            changed = true;
            *logged_in = now;
        }
        if !changed || !allow_changes {
            return None;
        }
        Some(std::array::from_fn(|i| !columns[i].is_empty()))
    }

    /// A card dropped on `column` (0..=3, or [`DEACTIVATED_COLUMN`]): the input leaves its
    /// column for that one. Returns whether it moved.
    pub fn drop_into(&mut self, input: T, column: usize) -> bool {
        let Some(from) = self.column_of(input) else { return false };
        if from == column || column > DEACTIVATED_COLUMN {
            return false;
        }
        self.list_mut(from).retain(|&i| i != input);
        self.list_mut(column).push(input);
        true
    }

    fn list_mut(&mut self, column: usize) -> &mut Vec<T> {
        match (column, self.columns.as_mut()) {
            (c, Some(columns)) if c < MAX_PLAYERS => &mut columns[c],
            _ => &mut self.deactivated,
        }
    }

    /// The inputs of a player column (`LobbyInputsForPlayer`), or of the unused column.
    pub fn column(&self, column: usize) -> &[T] {
        match (column, self.columns.as_ref()) {
            (c, Some(columns)) if c < MAX_PLAYERS => &columns[c],
            (DEACTIVATED_COLUMN, _) => &self.deactivated,
            _ => &[],
        }
    }

    /// The column an input sits in ([`DEACTIVATED_COLUMN`] for the unused one).
    pub fn column_of(&self, input: T) -> Option<usize> {
        if let Some(columns) = &self.columns {
            if let Some(c) = columns.iter().position(|c| c.contains(&input)) {
                return Some(c);
            }
        }
        self.deactivated.contains(&input).then_some(DEACTIVATED_COLUMN)
    }

    /// `IsPlayerActiveLocally`: one of the column's inputs is available.
    pub fn is_player_active(&self, column: usize, available: impl Fn(T) -> bool) -> bool {
        column < MAX_PLAYERS && self.column(column).iter().any(|&i| available(i))
    }

    /// `LocalPlayerCount` (= `PlayerCount` offline): columns with an available input.
    pub fn player_count(&self, available: impl Fn(T) -> bool) -> usize {
        (0..MAX_PLAYERS).filter(|&c| self.is_player_active(c, &available)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KB: u8 = 0;

    fn frame(input: u8, gamepad: bool) -> InputFrame<u8> {
        InputFrame { input, available: true, gamepad, move_left: false, move_right: false }
    }

    #[test]
    fn start_puts_the_keyboard_and_a_lone_gamepad_in_player_one() {
        let mut a = ControlAssignment::default();
        a.start(&[frame(KB, false), frame(1, true)]);
        assert_eq!(a.column(0), &[KB, 1]);
        let mut b = ControlAssignment::default();
        b.start(&[frame(KB, false), frame(1, true), frame(2, true)]);
        assert_eq!(b.column(0), &[KB]);
        // The two gamepads are not seen until the first update, which sends them to "unused".
        b.update(&[frame(KB, false), frame(1, true), frame(2, true)], true);
        assert_eq!(b.column(DEACTIVATED_COLUMN), &[1, 2]);
        // `Start` runs once.
        b.start(&[]);
        assert_eq!(b.column(0), &[KB]);
    }

    #[test]
    fn new_inputs_take_the_first_empty_column() {
        let mut a = ControlAssignment::default();
        a.start(&[frame(KB, false)]);
        let all = [frame(KB, false), frame(1, true), frame(2, true)];
        assert!(a.update(&all, false));
        assert_eq!(a.column(1), &[1]);
        assert_eq!(a.column(2), &[2]);
        assert_eq!(a.player_count(|_| true), 3);
        assert!(a.is_player_active(2, |_| true));
        assert!(!a.is_player_active(2, |i| i != 2));
    }

    #[test]
    fn left_and_right_move_between_columns_and_the_unused_one() {
        let mut a = ControlAssignment::default();
        a.start(&[frame(KB, false)]);
        let right = |i: u8| InputFrame { move_right: true, ..frame(i, i != KB) };
        let left = |i: u8| InputFrame { move_left: true, ..frame(i, i != KB) };
        for expected in 1..4 {
            a.update(&[right(KB)], false);
            assert_eq!(a.column_of(KB), Some(expected));
        }
        a.update(&[right(KB)], false);
        assert_eq!(a.column_of(KB), Some(DEACTIVATED_COLUMN));
        assert_eq!(a.player_count(|_| true), 0);
        // Left from "unused" lands in Player 4.
        a.update(&[left(KB)], false);
        assert_eq!(a.column_of(KB), Some(3));
        // One step per frame, even though the input reaches a column processed later.
        a.update(&[left(KB)], false);
        assert_eq!(a.column_of(KB), Some(2));
        a.drop_into(KB, 0);
        a.update(&[left(KB)], false);
        assert_eq!(a.column_of(KB), Some(0));
    }

    #[test]
    fn network_games_skip_the_columns_other_clients_play_in() {
        let mut a = ControlAssignment::default();
        a.start(&[frame(KB, false)]);
        let elsewhere = [false, true, true, false];
        let right = |i: u8| InputFrame { move_right: true, ..frame(i, i != KB) };
        a.update_beside(&[right(KB)], false, elsewhere);
        assert_eq!(a.column_of(KB), Some(3));
        // A new gamepad takes the first column nobody else plays in.
        a.update_beside(&[frame(KB, false), frame(1, true)], false, elsewhere);
        assert_eq!(a.column_of(1), Some(0));
        // Everything taken: the last free column.
        a.update_beside(&[frame(KB, false), frame(1, true), frame(2, true)], false, elsewhere);
        assert_eq!(a.column_of(2), Some(3));
    }

    #[test]
    fn server_assignments_move_or_drop_local_columns() {
        let mut a = ControlAssignment::default();
        a.start(&[frame(KB, false)]);
        let mut logged_in = None;
        // Offline-style first claim: the keyboard in column 0.
        assert_eq!(a.resolve_with_server_assignments([false; 4], [false; 4], true, &mut logged_in), Some([true, false, false, false]));
        // The server says column 0 is another client's: the keyboard moves to column 1.
        assert_eq!(a.resolve_with_server_assignments([false; 4], [true, false, false, false], true, &mut logged_in), Some([false, true, false, false]));
        assert_eq!(a.column_of(KB), Some(1));
        // Granted: nothing more to claim.
        assert_eq!(a.resolve_with_server_assignments([false, true, false, false], [true, false, false, false], true, &mut logged_in), None);
        // While loading, a column the server did not grant loses its inputs.
        assert_eq!(a.resolve_with_server_assignments([false; 4], [true, false, false, false], false, &mut logged_in), None);
        assert_eq!(a.column_of(KB), Some(DEACTIVATED_COLUMN));
    }

    #[test]
    fn unavailable_inputs_drop_out_and_drops_move_cards() {
        let mut a = ControlAssignment::default();
        a.start(&[frame(KB, false), frame(1, true)]);
        assert!(a.drop_into(1, 2));
        assert_eq!(a.column(2), &[1]);
        assert!(!a.drop_into(1, 2));
        a.update(&[frame(KB, false), InputFrame { available: false, ..frame(1, true) }], false);
        assert_eq!(a.column_of(1), None);
        // Back again: a new input for the model, into the first empty column.
        a.update(&[frame(KB, false), frame(1, true)], false);
        assert_eq!(a.column_of(1), Some(1));
    }
}
