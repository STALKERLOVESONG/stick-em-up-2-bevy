//! How a Versus or Team Versus game ends and what its end screen shows.
//!
//! * [`game_should_end`] (`VersusGameEnder.LateUpdate`): in Versus a player with
//!   [`FRAG_LIMIT`] frags ends it; in Team Versus a team whose frags add up to it.
//! * [`DefinitiveFrags`] (`EndGame` → `ReceiveDefinitiveFragCounts`): every active player's
//!   frags against each player index, frozen when the game ends; the score labels and tables
//!   read these from then on.
//! * [`winning_team`] (`EndOfVersusGameLoserDestroyer.Start`): the team with the most frags
//!   (the first such team in player order on a tie); everyone else's character is destroyed.
//! * [`ScoreTableLayout`] (`VersusScoreTable.OnEnable`): which rows and columns of the frag
//!   table show and how the box shrinks around them.

/// `VersusGameEnder.FRAG_LIMIT`.
pub const FRAG_LIMIT: i32 = 10;
/// `VersusScoreTable.OnEnable`: world units to sliced-sprite pixels of the box border, and the
/// narrowest the box gets.
pub const TABLE_UNITS_TO_PIXELS: f32 = 543.5;
pub const TABLE_MINIMUM_WIDTH: f32 = 917.9;

/// A player as the end-of-game rules see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersusPlayer {
    pub index: usize,
    pub team: usize,
    pub frags: i32,
}

/// `VersusGameEnder.LateUpdate`'s test (players in `Player.ActivePlayers` order).
pub fn game_should_end(team_versus: bool, players: &[VersusPlayer]) -> bool {
    if !team_versus {
        return players.iter().any(|p| p.frags >= FRAG_LIMIT);
    }
    let mut totals = [0i32; 2];
    for p in players {
        if let Some(t) = totals.get_mut(p.team) {
            *t += p.frags;
        }
    }
    totals.iter().any(|&t| t >= FRAG_LIMIT)
}

/// `definitiveFragCounts`: per player index, the frags against each player index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefinitiveFrags {
    counts: [Option<[i32; 4]>; 4],
}

impl DefinitiveFrags {
    pub fn set(&mut self, player: usize, against: [i32; 4]) {
        if let Some(slot) = self.counts.get_mut(player) {
            *slot = Some(against);
        }
    }

    /// `GetDefinitiveFrags(fragger, fragged)` (0 for a player without counts, where the
    /// original would throw).
    pub fn frags(&self, fragger: usize, fragged: usize) -> i32 {
        self.counts.get(fragger).copied().flatten().map_or(0, |c| c[fragged])
    }

    /// `GetDefinitiveFragTotal`: the sum, if the player had counts.
    pub fn total(&self, player: usize) -> Option<i32> {
        self.counts.get(player).copied().flatten().map(|c| c.iter().sum())
    }
}

/// `EndOfVersusGameLoserDestroyer.Start`: the winning team (`GroupBy` keeps the order in which
/// teams first appear; `OrderByDescending` is stable).
pub fn winning_team(players: &[VersusPlayer]) -> Option<usize> {
    let mut teams: Vec<(usize, i32)> = Vec::new();
    for p in players {
        match teams.iter_mut().find(|(t, _)| *t == p.team) {
            Some((_, score)) => *score += p.frags,
            None => teams.push((p.team, p.frags)),
        }
    }
    let mut best: Option<(usize, i32)> = None;
    for (team, score) in teams {
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((team, score));
        }
    }
    best.map(|(t, _)| t)
}

/// `VersusScoreTable`'s references as `Awake` stores them (local positions).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreTableStandard {
    /// x of the "0".."3" cells of the first player row.
    pub column_x: [f32; 4],
    /// `standardTotalCellX`: x of the first row's "Total".
    pub total_x: f32,
    /// y of the four player rows.
    pub row_y: [f32; 4],
    /// y of the heading row and of the rows' parent.
    pub heading_y: f32,
    pub rows_parent_y: f32,
    pub table: [f32; 2],
    pub bottom_stuff_y: f32,
    pub box_contents_y: f32,
    pub border: [f32; 2],
}

/// What `VersusScoreTable.OnEnable` sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreTableLayout {
    /// Player rows shown.
    pub rows: [bool; 4],
    /// The frags-against columns "0".."3" (in every row and the heading row).
    pub columns: [bool; 4],
    /// The heading row (more than two players).
    pub heading: bool,
    /// x of every "Total" cell.
    pub total_x: f32,
    pub table: [f32; 2],
    pub bottom_stuff_y: f32,
    pub box_contents_y: f32,
    pub border: [f32; 2],
}

impl ScoreTableLayout {
    /// The table for `count` players: the frags-against matrix only with more than two.
    pub fn new(s: &ScoreTableStandard, count: usize) -> ScoreTableLayout {
        let matrix = count > 2;
        let (mut vx, mut vy, mut heading_shift) = (0.0f32, 0.0f32, 0.0f32);
        let mut rows = [false; 4];
        let mut columns = [false; 4];
        for i in 0..4 {
            let row_shown = i < count;
            rows[i] = row_shown;
            columns[i] = matrix && row_shown;
            if !columns[i] && i > 0 {
                vx += s.column_x[i] - s.column_x[i - 1];
            }
            if !row_shown && i != 0 {
                vy += s.row_y[i - 1] - s.row_y[i];
            }
        }
        if !matrix {
            vx += s.total_x - s.column_x[3];
            heading_shift += s.heading_y - s.rows_parent_y;
        }
        let mut border = [s.border[0] - vx * TABLE_UNITS_TO_PIXELS, s.border[1] - (vy + heading_shift) * TABLE_UNITS_TO_PIXELS];
        border[0] = border[0].max(TABLE_MINIMUM_WIDTH);
        ScoreTableLayout {
            rows,
            columns,
            heading: matrix,
            total_x: s.total_x - vx,
            table: [s.table[0] + vx / 2.0, s.table[1] + heading_shift],
            bottom_stuff_y: s.bottom_stuff_y + vy + heading_shift,
            box_contents_y: s.box_contents_y - (vy + heading_shift) / 2.0,
            border,
        }
    }
}

/// `"{name} Wins!"` for the first of the highest totals.
pub fn winner_header<'a>(rows: impl IntoIterator<Item = (&'a str, i32)>) -> String {
    let mut best: Option<(&str, i32)> = None;
    for (name, total) in rows {
        if best.is_none_or(|(_, b)| total > b) {
            best = Some((name, total));
        }
    }
    format!("{} Wins!", best.map_or("", |(n, _)| n))
}

/// `TeamVersusScoreUI.TeamColor`: the average of the team's body colours.
pub fn team_color(bodies: &[[f32; 4]]) -> Option<[f32; 4]> {
    if bodies.is_empty() {
        return None;
    }
    let mut c = [0.0f32; 4];
    for b in bodies {
        for k in 0..4 {
            c[k] += b[k];
        }
    }
    Some(c.map(|v| v / bodies.len() as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(index: usize, team: usize, frags: i32) -> VersusPlayer {
        VersusPlayer { index, team, frags }
    }

    #[test]
    fn ten_frags_end_the_game() {
        assert!(!game_should_end(false, &[p(0, 0, 9), p(1, 1, 9)]));
        assert!(game_should_end(false, &[p(0, 0, 9), p(1, 1, 10)]));
        assert!(!game_should_end(true, &[p(0, 0, 5), p(1, 1, 4), p(2, 1, 5)]));
        assert!(game_should_end(true, &[p(0, 0, 5), p(1, 1, 5), p(2, 1, 5)]));
    }

    #[test]
    fn definitive_counts_and_winners() {
        let mut d = DefinitiveFrags::default();
        d.set(1, [3, -1, 2, 0]);
        assert_eq!(d.total(1), Some(4));
        assert_eq!(d.total(0), None);
        assert_eq!(d.frags(1, 2), 2);
        assert_eq!(winning_team(&[p(0, 0, 3), p(1, 1, 6), p(2, 0, 2)]), Some(1));
        // A tie goes to the team listed first.
        assert_eq!(winning_team(&[p(0, 1, 3), p(1, 0, 3)]), Some(1));
        assert_eq!(winner_header([("Player 1", 3), ("Player 2", 3)]), "Player 1 Wins!");
        assert_eq!(team_color(&[[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]]), Some([0.5, 0.0, 0.5, 1.0]));
    }

    fn standard() -> ScoreTableStandard {
        ScoreTableStandard {
            column_x: [-0.15, 0.15, 0.45, 0.75],
            total_x: 1.05,
            row_y: [0.0, -0.12, -0.24, -0.36],
            heading_y: -0.30165648,
            rows_parent_y: -0.45165646,
            table: [-0.06, 0.0],
            bottom_stuff_y: -1.0,
            box_contents_y: 0.535,
            border: [1500.0, 600.0],
        }
    }

    #[test]
    fn two_players_hide_the_matrix() {
        let l = ScoreTableLayout::new(&standard(), 2);
        assert_eq!(l.rows, [true, true, false, false]);
        assert_eq!(l.columns, [false; 4]);
        assert!(!l.heading);
        // Columns 1..3 (0.9) and the gap to "Total" (0.3) go.
        assert!((l.total_x - (1.05 - 1.2)).abs() < 1e-5);
        assert!((l.table[0] - (-0.06 + 0.6)).abs() < 1e-5);
        let shift = 0.24 + 0.15;
        assert!((l.bottom_stuff_y - (-1.0 + shift)).abs() < 1e-5);
        assert!((l.box_contents_y - (0.535 - shift / 2.0)).abs() < 1e-5);
        assert_eq!(l.border[0], TABLE_MINIMUM_WIDTH);
        assert!((l.border[1] - (600.0 - shift * TABLE_UNITS_TO_PIXELS)).abs() < 1e-2);
    }

    #[test]
    fn three_players_show_three_columns() {
        let l = ScoreTableLayout::new(&standard(), 3);
        assert_eq!(l.columns, [true, true, true, false]);
        assert!(l.heading);
        assert!((l.total_x - 0.75).abs() < 1e-5);
        assert!((l.bottom_stuff_y - (-1.0 + 0.12)).abs() < 1e-5);
    }
}
