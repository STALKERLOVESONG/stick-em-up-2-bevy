//! `LobbyTeams`: the two Team Versus teams of player slots 0..3, kept for the whole session
//! (static lists in the original), and the HUD position a player takes when the HUD groups the
//! players by team (`GetPlayerPositionWhenGroupedByTeams`).
//!
//! The team arrangement screen (`TeamArrangement`) moves players with drag and drop or
//! left/right; it only appears in Team Versus, which needs more local players than the one
//! keyboard-and-mouse controller this port supports, so only the model is used for now.

/// `LobbyTeams.MAX_TEAMS`.
pub const MAX_TEAMS: usize = 2;
/// Player slots.
pub const MAX_PLAYERS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LobbyTeams {
    teams: [Vec<usize>; MAX_TEAMS],
}

impl Default for LobbyTeams {
    /// Team A holds players 0 and 1, team B players 2 and 3.
    fn default() -> Self {
        Self { teams: [vec![0, 1], vec![2, 3]] }
    }
}

impl LobbyTeams {
    /// `GetTeam`: the team's players in ascending order.
    pub fn team(&self, team: usize) -> &[usize] {
        &self.teams[team]
    }

    /// `MoveToTeam`: out of every team, into `team`, which is kept sorted.
    pub fn move_to_team(&mut self, player: usize, team: usize) {
        for t in &mut self.teams {
            t.retain(|&p| p != player);
        }
        self.teams[team].push(player);
        self.teams[team].sort_unstable();
    }

    /// `TeamIndexForPlayer` (`None` where the original throws).
    pub fn team_of(&self, player: usize) -> Option<usize> {
        self.teams.iter().position(|t| t.contains(&player))
    }

    /// `GetPlayerPositionWhenGroupedByTeams` offline: count the players with controls listed
    /// before this one, and after each team skip the slots of absent players
    /// (`4 - LevelLoader.PlayerCount`).
    pub fn player_position_when_grouped(&self, player: usize, has_controls: impl Fn(usize) -> bool, player_count: usize) -> Option<usize> {
        let mut position = 0;
        for team in &self.teams {
            for &p in team {
                if p == player {
                    return Some(position);
                }
                if has_controls(p) {
                    position += 1;
                }
            }
            position += MAX_PLAYERS.saturating_sub(player_count);
        }
        None
    }

    /// `TeamArrangement.Update`'s left/right: one step towards team A (left) or B (right).
    pub fn step(&mut self, player: usize, left: bool, right: bool) {
        let Some(current) = self.team_of(player) else { return };
        let mut wanted = current as i32;
        if left {
            wanted -= 1;
        }
        if right {
            wanted += 1;
        }
        let wanted = wanted.clamp(0, MAX_TEAMS as i32 - 1) as usize;
        if wanted != current {
            self.move_to_team(player, wanted);
        }
    }
}

/// `PlayerSlotAssignments.OnLoadedLevelEverywhere` offline: which child of "Player Spawn
/// Points" each player slot spawns at (`-1`: the slot has no local controls).
///
/// The child indices are shuffled outside the campaign (or with `RandomizeSpawnPoints`) by
/// `Utility.Shuffle` — a lazy Fisher–Yates over `System.Random.Next(i, count)` — and the first
/// four handed to the active slots in slot order. `next(i, count)` stands for
/// `rng.Next(i, count)` (time-seeded in the original, so any uniform source is faithful).
/// TODO(UNCERTAIN): with more active slots than spawn points the exhausted enumerator keeps
/// returning its last value (0 when there are none); `PlayerSpawner.SpawnAll` then stops at the
/// first index that is out of range.
pub fn assign_spawn_points(child_count: usize, active: [bool; MAX_PLAYERS], shuffle: bool, mut next: impl FnMut(usize, usize) -> usize) -> [i32; MAX_PLAYERS] {
    let mut order: Vec<usize> = (0..child_count).collect();
    if shuffle {
        for i in 0..order.len().min(MAX_PLAYERS) {
            let j = next(i, order.len());
            order.swap(i, j);
        }
    }
    order.truncate(MAX_PLAYERS);
    let mut out = [-1; MAX_PLAYERS];
    let mut taken = 0;
    for (slot, &is_active) in active.iter().enumerate() {
        if is_active {
            let index = order.get(taken).or(order.last()).copied().unwrap_or(0);
            out[slot] = index as i32;
            taken += 1;
        }
    }
    out
}

/// `PlayerSpawner.SpawnAll`: the slots that spawn and at which child, in slot order; it stops at
/// the first index past the last child.
pub fn spawns(indices: [i32; MAX_PLAYERS], child_count: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (slot, &index) in indices.iter().enumerate() {
        if index >= child_count as i32 {
            break;
        }
        if index >= 0 {
            out.push((slot, index as usize));
        }
    }
    out
}

/// `PlayerSpawner.Spawn`: the `Player.TeamIndex` a slot plays for.
pub fn team_index(mode_is_versus: bool, mode_is_team_versus: bool, slot: usize, teams: &LobbyTeams) -> usize {
    if mode_is_versus {
        slot
    } else if mode_is_team_versus {
        teams.team_of(slot).unwrap_or(0)
    } else {
        0
    }
}

/// `CustomizeColor.bodyColors` (the colour picker's swatches, `Color32`).
pub const BODY_COLORS: [[u8; 4]; 15] = [
    [230, 89, 140, 255],
    [239, 66, 66, 255],
    [255, 96, 70, 255],
    [255, 125, 90, 255],
    [248, 223, 111, 255],
    [70, 198, 95, 255],
    [61, 176, 201, 255],
    [50, 181, 255, 255],
    [48, 102, 205, 255],
    [119, 108, 202, 255],
    [73, 80, 105, 255],
    [175, 158, 134, 255],
    [174, 129, 96, 255],
    [39, 39, 39, 255],
    [236, 236, 236, 255],
];

/// `CustomizeColor.DefaultBodyColorForPlayer`: players 0..3 get swatches 1, 5, 4, 8; in Team
/// Versus the n-th member of team A gets 1, 2, 4, 0 and of team B 8, 7, 10, 9. Slots outside
/// these tables index -1 in the original (an exception); `None` here.
pub fn default_body_color(player: usize, team_versus: Option<&LobbyTeams>) -> Option<[u8; 4]> {
    let swatch = match team_versus {
        Some(teams) => {
            let team = teams.team_of(player)?;
            let place = teams.team(team).iter().position(|&p| p == player)?;
            *[[1, 2, 4, 0], [8, 7, 10, 9]][team].get(place)?
        }
        None => *[1, 5, 4, 8].get(player)?,
    };
    Some(BODY_COLORS[swatch])
}

/// `PlayerSpawner.Spawn` with no customised colours: the default body for the slot, the
/// matching helmet (`MatchingHelmetColorForBodyColor`: body × 0.8980392, alpha kept) and a
/// random face; `CharacterColor.SetColors` stores body and face as `Color32`.
pub fn player_palette(player: usize, team_versus: Option<&LobbyTeams>, rng: &mut crate::rng::GameRng) -> Option<crate::visual::CharacterPalette> {
    let body32 = default_body_color(player, team_versus)?;
    let body = body32.map(|c| f32::from(c) / 255.0);
    let k = 0.898_039_2;
    let helmet = [body[0] * k, body[1] * k, body[2] * k, body[3]];
    Some(crate::visual::CharacterPalette { body: crate::visual::to_color32(body), helmet, face: crate::visual::random_face_color(rng) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_colours_per_slot_and_team() {
        assert_eq!(default_body_color(0, None), Some([239, 66, 66, 255]));
        assert_eq!(default_body_color(1, None), Some([70, 198, 95, 255]));
        assert_eq!(default_body_color(3, None), Some([48, 102, 205, 255]));
        assert_eq!(default_body_color(4, None), None);
        let mut teams = LobbyTeams::default();
        assert_eq!(default_body_color(1, Some(&teams)), Some(BODY_COLORS[2]));
        assert_eq!(default_body_color(2, Some(&teams)), Some(BODY_COLORS[8]));
        teams.move_to_team(3, 0);
        assert_eq!(default_body_color(3, Some(&teams)), Some(BODY_COLORS[4]));
        let mut rng = crate::rng::GameRng::from_seed(7);
        let palette = player_palette(0, None, &mut rng).unwrap();
        // The Color32 → Color → Color32 round trip keeps the swatch.
        assert_eq!(palette.body, [239, 66, 66, 255]);
        assert!((palette.helmet[0] - 239.0 / 255.0 * 0.898_039_2).abs() < 1e-6);
        assert_eq!(palette.helmet[3], 1.0);
    }

    #[test]
    fn campaign_spawns_take_children_in_order() {
        let a = assign_spawn_points(3, [false, true, false, true], false, |_, _| unreachable!());
        assert_eq!(a, [-1, 0, -1, 1]);
        assert_eq!(spawns(a, 3), vec![(1, 0), (3, 1)]);
    }

    #[test]
    fn shuffled_spawns_are_a_lazy_fisher_yates() {
        // next always picks the last remaining element.
        let a = assign_spawn_points(5, [true, true, false, false], true, |_, n| n - 1);
        // i=0: swap 0<->4 → [4,1,2,3,0]; i=1: swap 1<->4 → [4,0,2,3,1].
        assert_eq!(a, [4, 0, -1, -1]);
    }

    #[test]
    fn spawn_all_stops_past_the_last_child() {
        assert_eq!(spawns([0, 5, 1, -1], 2), vec![(0, 0)]);
    }

    #[test]
    fn team_index_by_mode() {
        let teams = LobbyTeams::default();
        assert_eq!(team_index(false, false, 3, &teams), 0);
        assert_eq!(team_index(true, false, 3, &teams), 3);
        assert_eq!(team_index(false, true, 3, &teams), 1);
    }

    #[test]
    fn default_teams_and_moves_stay_sorted() {
        let mut teams = LobbyTeams::default();
        assert_eq!(teams.team(0), &[0, 1]);
        assert_eq!(teams.team_of(3), Some(1));
        teams.move_to_team(3, 0);
        assert_eq!(teams.team(0), &[0, 1, 3]);
        assert_eq!(teams.team(1), &[2]);
        teams.move_to_team(0, 1);
        assert_eq!(teams.team(1), &[0, 2]);
        assert_eq!(teams.team_of(0), Some(1));
    }

    #[test]
    fn left_right_steps_clamp_to_the_two_teams() {
        let mut teams = LobbyTeams::default();
        teams.step(0, true, false);
        assert_eq!(teams.team_of(0), Some(0));
        teams.step(0, false, true);
        assert_eq!(teams.team_of(0), Some(1));
        teams.step(0, false, true);
        assert_eq!(teams.team_of(0), Some(1));
        // Both pressed cancel out.
        teams.step(0, true, true);
        assert_eq!(teams.team_of(0), Some(1));
    }

    #[test]
    fn grouped_positions_skip_absent_players_after_each_team() {
        let teams = LobbyTeams::default();
        // Players 0 and 2 have controls: 2 players, so each team is followed by 2 empty slots.
        let controls = |p: usize| p == 0 || p == 2;
        assert_eq!(teams.player_position_when_grouped(0, controls, 2), Some(0));
        assert_eq!(teams.player_position_when_grouped(1, controls, 2), Some(1));
        assert_eq!(teams.player_position_when_grouped(2, controls, 2), Some(3));
        assert_eq!(teams.player_position_when_grouped(3, controls, 2), Some(4));
        // Four players: no gaps.
        assert_eq!(teams.player_position_when_grouped(3, |_| true, 4), Some(3));
        assert_eq!(teams.player_position_when_grouped(5, |_| true, 4), None);
    }
}
