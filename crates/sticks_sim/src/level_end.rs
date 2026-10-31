//! How a level ends, and what the end-of-level screen shows.
//!
//! * `TriggeredByAllPlayers`: the players inside the trigger (added on `OnTriggerEnter2D`,
//!   removed on `OnTriggerExit2D`, by whatever root their `ControlSlot` sits on); triggered once
//!   every live player (`Lives.Remaining >= 0`) is inside, checked on every enter, on a
//!   player's game over and on a disconnect. [`AllPlayersTrigger`].
//! * `DestroyAllEnemiesLevelEnder`: `EnemyRegistrar.NewEnemy` counts up, each registered
//!   enemy's `Health.Exhausted` counts down; fires once the count is at or below zero after at
//!   least one enemy registered. [`EnemyCount`].
//! * `EndOfLevelTimeSlower`: `Time.timeScale` smooth-damped towards 0.4 over 0.25 s of unscaled
//!   time. [`TimeSlower`].
//! * `ScoreCalculator.OnEnable` and its tick-on coroutines. [`ScoreBoard`].
//! * `HighScores`, `ScoreTable`, `Score`, `ScoreSorter`, `PlayerID` (anonymous local ids).
//!   [`HighScores`].
//! * `LevelTips.StartLevel` tip choice. [`choose_tip`].
//!
//! TODO(UNCERTAIN): the order of `ScoreCalculator`'s coroutines inside a frame is not
//! recoverable from the decompiled code; the totals are taken to be summed before that frame's
//! ticks, so they trail the columns by one frame.

use std::cmp::Ordering;
use std::collections::HashMap;

use unity_compat::mathf;

/// `EndOfLevelTimeSlower`: `targetScale`, `changeTime`.
pub const SLOWED_TIME_SCALE: f32 = 0.4;
pub const TIME_SCALE_CHANGE_TIME: f32 = 0.25;
/// `EndOfLevelUIActivator.Start`: `WaitForSeconds(1)` (scaled time) before the screen shows.
pub const END_OF_LEVEL_UI_DELAY: f32 = 1.0;
/// `ScoreCalculator.OnEnable`: the level time scores `600 - seconds`, clamped to 0..=600.
pub const TIME_SCORE_SECONDS: i32 = 600;
/// `ScoreCalculator.worldUnitsToBoxPixels`.
pub const WORLD_UNITS_TO_BOX_PIXELS: f32 = 543.5;
/// `ScoreTable.MAXIMUM_SCORES`.
pub const MAXIMUM_SCORES: usize = 10;
/// `HighScoresDisplay.RegularScoreColor` / `GoldenScoreColor` (`Color32(255, 238, 34, 255)`).
pub const REGULAR_SCORE_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const GOLDEN_SCORE_COLOR: [f32; 4] = [1.0, 238.0 / 255.0, 34.0 / 255.0, 1.0];
/// `LevelTips` default messages.
pub const CAMPAIGN_DEFAULT_TIP: &str = "There are no special tips for this level. Take your time and good luck!\n\nYou can do it!";
pub const VERSUS_DEFAULT_TIP: &str = "Be kind.";

/// `TriggeredByAllPlayers`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AllPlayersTrigger {
    /// `triggeredPlayers` (a list: entering twice adds twice, leaving removes one).
    inside: Vec<usize>,
    triggered: bool,
}

impl AllPlayersTrigger {
    pub fn triggered(&self) -> bool {
        self.triggered
    }

    /// `OnTriggerEnter2D` by something controlled by `players` (usually one), then
    /// `CheckIfAllLivePlayersHaveTriggered` with the players that still have lives.
    pub fn enter(&mut self, players: &[usize], live_players: &[usize]) {
        self.inside.extend_from_slice(players);
        self.check(live_players);
    }

    /// `OnTriggerExit2D`.
    pub fn exit(&mut self, players: &[usize]) {
        for p in players {
            if let Some(i) = self.inside.iter().position(|q| q == p) {
                self.inside.remove(i);
            }
        }
    }

    /// `CheckIfAllLivePlayersHaveTriggered` (also on `Respawner.PlayerGotGameOver`).
    pub fn check(&mut self, live_players: &[usize]) {
        if !live_players.is_empty() && live_players.iter().all(|p| self.inside.contains(p)) {
            self.triggered = true;
        }
    }
}

/// `DestroyAllEnemiesLevelEnder.remaining`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnemyCount {
    remaining: Option<i32>,
}

impl EnemyCount {
    /// `HandleNewEnemy`.
    pub fn registered(&mut self) {
        self.remaining = Some(self.remaining.unwrap_or(0) + 1);
    }

    /// A registered enemy's `Health.Exhausted`.
    pub fn exhausted(&mut self) {
        if let Some(r) = self.remaining.as_mut() {
            *r -= 1;
        }
    }

    /// What the countdown label shows (`remaining ?? 0`).
    pub fn label(&self) -> i32 {
        self.remaining.unwrap_or(0)
    }

    /// `remaining.HasValue && remaining.Value <= 0`.
    pub fn all_gone(&self) -> bool {
        self.remaining.is_some_and(|r| r <= 0)
    }
}

/// `EndOfLevelTimeSlower.Update`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimeSlower {
    velocity: f32,
}

impl TimeSlower {
    /// The new `Time.timeScale` from the current one.
    pub fn update(&mut self, time_scale: f32, unscaled_delta: f32) -> f32 {
        mathf::smooth_damp(time_scale, SLOWED_TIME_SCALE, &mut self.velocity, TIME_SCALE_CHANGE_TIME, f32::INFINITY, unscaled_delta)
    }
}

/// `Math.Round` / `Mathf.Round`: halves go to the even neighbour.
pub fn round_half_even(v: f32) -> i32 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 && (r as i64) % 2 != 0 {
        (r - v.signum()) as i32
    } else {
        r as i32
    }
}

/// `ScoreCalculator.CalculateDamageScore`.
pub fn damage_score(damage: f32) -> i32 {
    round_half_even(damage * 10.0)
}

/// `ScoreCalculator.CalculateHealthScore`.
pub fn health_score(lives_excluding_current: i32, health_remaining: f32) -> i32 {
    let lives = lives_excluding_current.max(0);
    round_half_even((lives as f32 * 100.0 + health_remaining) * 10.0)
}

/// What `ScoreCalculator` reads of one active player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerResult {
    pub player_index: usize,
    /// `PlayerScore.DamageDealt`.
    pub damage_dealt: f32,
    /// `Lives.DifficultyAdjustedStartingLives`, `Lives.Remaining`.
    pub starting_lives: i32,
    pub lives_remaining: i32,
    /// The avatar's `Health.startingHealth`, `Health.Remaining`.
    pub starting_health: f32,
    pub health_remaining: f32,
}

/// `ScoreCalculator.CalculatedScore`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalculatedScore {
    pub damage: i32,
    pub health: i32,
    pub time: i32,
    pub damage_is_golden: bool,
    pub health_is_golden: bool,
    /// -1 for the team row.
    pub player_index: i32,
}

/// The score columns, in `ScoreComponent` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Damage,
    Health,
    Time,
    Total,
}

pub const COLUMNS: [Column; 4] = [Column::Damage, Column::Health, Column::Time, Column::Total];

impl CalculatedScore {
    pub fn total(&self) -> i32 {
        self.damage + self.health + self.time
    }

    pub fn is_golden(&self) -> bool {
        self.damage_is_golden & self.health_is_golden
    }

    pub fn get(&self, column: Column) -> i32 {
        match column {
            Column::Damage => self.damage,
            Column::Health => self.health,
            Column::Time => self.time,
            Column::Total => self.total(),
        }
    }

    pub fn is_column_golden(&self, column: Column) -> bool {
        match column {
            Column::Damage => self.damage_is_golden,
            Column::Health => self.health_is_golden,
            Column::Time => false,
            Column::Total => self.is_golden(),
        }
    }
}

/// `ScoreCalculator.OnEnable`'s numbers: one row per active player (by player index) and the
/// team row. `level_time` is `LevelTimer.LevelTime`, `maximum_damage`
/// `MaximumDamageCalculator.MaximumDamage`, `player_count` `LevelLoader.PlayerCount`.
pub fn calculate_scores(level_time: Option<f32>, maximum_damage: f32, players: &[PlayerResult], player_count: usize) -> (Vec<CalculatedScore>, CalculatedScore) {
    let seconds = level_time.unwrap_or(TIME_SCORE_SECONDS as f32);
    let time = (TIME_SCORE_SECONDS - seconds as i32).clamp(0, TIME_SCORE_SECONDS);
    let maximum = damage_score(maximum_damage);
    let mut sorted: Vec<&PlayerResult> = players.iter().collect();
    sorted.sort_by_key(|p| p.player_index);
    let scores: Vec<CalculatedScore> = sorted
        .into_iter()
        .map(|p| {
            let damage = damage_score(p.damage_dealt);
            let best_health = health_score(p.starting_lives, p.starting_health);
            let health = health_score(p.lives_remaining, p.health_remaining);
            CalculatedScore { damage, health, time, damage_is_golden: damage == maximum, health_is_golden: health == best_health, player_index: p.player_index as i32 }
        })
        .collect();
    let damage: i32 = scores.iter().map(|s| s.damage).sum();
    let team = CalculatedScore {
        damage,
        health: scores.iter().map(|s| s.health).sum(),
        time: scores.iter().map(|s| s.time).sum(),
        damage_is_golden: damage == maximum,
        health_is_golden: scores.iter().all(|s| s.health_is_golden) && scores.len() == player_count,
        player_index: -1,
    };
    (scores, team)
}

/// `ScoreCalculator.TickOn`: the numbers count up from zero each frame by
/// `24000 / fps`, `1980 / fps` and `180 / fps` (damage, health, time; integer division with the
/// expected frame rate, 60 when `Application.targetFrameRate` is -1); a finished golden cell
/// turns gold. Totals are the sums of what is displayed.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreBoard {
    pub scores: Vec<CalculatedScore>,
    pub team: CalculatedScore,
    /// `lastDisplayedScore[column][row]`.
    displayed: [Vec<i32>; 4],
    increments: [i32; 3],
    /// The team row's labels as `UpdateTotals` last set them (text value, golden).
    team_cells: [(i32, bool); 4],
    /// Player rows' cells: (value, golden) per column.
    cells: Vec<[(i32, bool); 4]>,
}

impl ScoreBoard {
    pub fn new(scores: Vec<CalculatedScore>, team: CalculatedScore, expected_frame_rate: i32) -> ScoreBoard {
        let rows = scores.len();
        let fps = expected_frame_rate.max(1);
        let mut board = ScoreBoard {
            displayed: [vec![0; rows], vec![0; rows], vec![0; rows], vec![0; rows]],
            increments: [24000 / fps, 1980 / fps, 180 / fps],
            team_cells: [(0, false); 4],
            cells: vec![[(0, false); 4]; rows],
            scores,
            team,
        };
        // `TickOn`: every cell reset, then `UpdateTotals` runs to its first `yield`, then each
        // `TickUpComponent` does its first step.
        board.update_totals();
        board.tick();
        board
    }

    /// One later frame: `UpdateTotals`, then the column ticks.
    pub fn frame(&mut self) {
        self.update_totals();
        self.tick();
    }

    fn tick(&mut self) {
        for (c, column) in [Column::Damage, Column::Health, Column::Time].into_iter().enumerate() {
            for row in 0..self.scores.len() {
                let score = self.scores[row];
                let target = score.get(column);
                let shown = (self.displayed[c][row] + self.increments[c]).min(target);
                self.displayed[c][row] = shown;
                let golden = shown == target && score.is_column_golden(column);
                // A cell keeps its gold once it turned.
                let was_golden = self.cells[row][c].1;
                self.cells[row][c] = (shown, golden || was_golden);
            }
        }
    }

    fn update_totals(&mut self) {
        for row in 0..self.scores.len() {
            let total: i32 = (0..3).map(|c| self.displayed[c][row]).sum();
            self.displayed[3][row] = total;
            let score = self.scores[row];
            let golden = total == score.total() && score.is_golden();
            let was = self.cells[row][3].1;
            self.cells[row][3] = (total, golden || was);
        }
        for (c, column) in COLUMNS.into_iter().enumerate() {
            let shown: i32 = self.displayed[c].iter().sum();
            let golden = shown == self.team.get(column) && self.team.is_column_golden(column);
            let was = self.team_cells[c].1;
            self.team_cells[c] = (shown, golden || was);
        }
    }

    /// The text and colour of a player row's cell.
    pub fn cell(&self, row: usize, column: Column) -> (String, [f32; 4]) {
        let (value, golden) = self.cells[row][column as usize];
        (value.to_string(), if golden { GOLDEN_SCORE_COLOR } else { REGULAR_SCORE_COLOR })
    }

    /// The text and colour of the team row's cell.
    pub fn team_cell(&self, column: Column) -> (String, [f32; 4]) {
        let (value, golden) = self.team_cells[column as usize];
        (value.to_string(), if golden { GOLDEN_SCORE_COLOR } else { REGULAR_SCORE_COLOR })
    }
}

/// `PlayerID` for players without a platform account (`GetAnonymousLocal`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerId {
    /// `Platform` (`None` for local anonymous players).
    pub platform: Option<String>,
    pub id: String,
    pub name: String,
}

impl PlayerId {
    pub fn anonymous_local(player_index: usize) -> PlayerId {
        PlayerId { platform: None, id: player_index.to_string(), name: format!("Player {}", player_index + 1) }
    }

    /// `PlayerID.CompareTo`: ids without a platform sort after those with one.
    fn compare(&self, other: &PlayerId) -> Ordering {
        match (&self.platform, &other.platform) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b).then_with(|| self.id.cmp(&other.id)),
        }
    }

    fn same(&self, other: &PlayerId) -> bool {
        self.platform == other.platform && self.id == other.id
    }
}

/// `Score`.
#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    /// Sorted (`PlayerID.CompareTo`).
    pub ids: Vec<PlayerId>,
    pub value: i32,
    pub golden: bool,
    /// `DateTime.UtcNow` as seconds since the Unix epoch.
    pub time: f64,
}

impl Score {
    pub fn new(mut ids: Vec<PlayerId>, value: i32, golden: bool, time: f64) -> Score {
        ids.sort_by(PlayerId::compare);
        Score { ids, value, golden, time }
    }
}

/// `ScoreSorter.Compare`: golden first, then higher scores, then earlier, then fewer players,
/// then by ids.
pub fn compare_scores(a: &Score, b: &Score) -> Ordering {
    match (a.golden, b.golden) {
        (false, true) => return Ordering::Greater,
        (true, false) => return Ordering::Less,
        _ => {}
    }
    b.value
        .cmp(&a.value)
        .then_with(|| a.time.partial_cmp(&b.time).unwrap_or(Ordering::Equal))
        .then_with(|| b.ids.len().cmp(&a.ids.len()))
        .then_with(|| a.ids.iter().zip(&b.ids).map(|(x, y)| x.compare(y)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal))
}

/// `ScoreTable`: the best ten scores and each player group's personal best.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreTable {
    pub(crate) scores: Vec<Score>,
    pub(crate) best: Vec<Score>,
}

impl ScoreTable {
    /// `AddScore`.
    pub fn add(&mut self, score: Score) {
        self.scores.push(score.clone());
        self.scores.sort_by(compare_scores);
        self.scores.truncate(MAXIMUM_SCORES);
        let same_group = |s: &Score| s.ids.len() == score.ids.len() && s.ids.iter().zip(&score.ids).all(|(a, b)| a.same(b));
        match self.best.iter().position(same_group) {
            Some(i) if compare_scores(&score, &self.best[i]) == Ordering::Less => self.best[i] = score,
            Some(_) => {}
            None => self.best.push(score),
        }
    }

    pub fn scores(&self) -> &[Score] {
        &self.scores
    }

    /// `GetBest`.
    pub fn best(&self) -> Option<&Score> {
        self.scores.first()
    }
}

/// `HighScores.scoreLists`: one table per level, player count and difficulty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HighScores {
    pub(crate) tables: HashMap<(String, usize, i32), ScoreTable>,
}

impl HighScores {
    /// `GetScoreTable` (created on first use).
    pub fn table(&mut self, level: &str, players: usize, difficulty: i32) -> &mut ScoreTable {
        self.tables.entry((level.to_owned(), players, difficulty)).or_default()
    }

    fn has_scores(&self, level: &str) -> bool {
        (1..=4).any(|p| (-1..=1).any(|d| self.tables.get(&(level.to_owned(), p, d)).is_some_and(|t| !t.scores.is_empty())))
    }

    /// `HighScores.IsLevelUnlocked` offline without cheats or the starter edition: the first
    /// level and every versus level are open; a campaign level opens once the one before it has
    /// a score at any player count and difficulty.
    pub fn is_level_unlocked(&self, index: usize, campaign: bool, campaign_levels: &[String]) -> bool {
        if index == 0 || !campaign {
            return true;
        }
        campaign_levels.get(index - 1).is_some_and(|previous| self.has_scores(previous))
    }
}

/// `LevelTips.StartLevel`: the first tip all of whose control types are in use (a tip listing
/// none always applies).
pub fn choose_tip<'a>(tips: &'a [unity_data::level::LevelTip], control_types_in_use: &[&str]) -> Option<&'a unity_data::level::LevelTip> {
    tips.iter().find(|t| t.applicable_control_types.iter().all(|c| control_types_in_use.contains(&c.as_str())))
}

/// `LevelTips.tipsSeenByPlayerIDs`: which players saw a tip (by level and its control types).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TipsSeen {
    pub(crate) seen: HashMap<(String, Vec<String>), Vec<PlayerId>>,
}

impl TipsSeen {
    /// Records the players and says whether any of them had not seen the tip (it is then shown
    /// with the "View Again Later" hint).
    pub fn mark(&mut self, level: &str, control_types: &[String], players: &[PlayerId]) -> bool {
        let seen = self.seen.entry((level.to_owned(), control_types.to_vec())).or_default();
        let unseen: Vec<&PlayerId> = players.iter().filter(|p| !seen.iter().any(|s| s.same(p))).collect();
        if unseen.is_empty() {
            return false;
        }
        let new: Vec<PlayerId> = unseen.into_iter().cloned().collect();
        seen.extend(new);
        true
    }
}

/// `FadeInUI.FadeIn` / `FadeOutUI.FadeOut`: the UI's visibility smooth-damped towards 1 over
/// 0.25 s (until ≥ 0.99, then exactly 1) or towards 0 over 0.1 s (until < 0.05, then the object
/// is switched off), in unscaled time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub visibility: f32,
    velocity: f32,
    pub fading_in: bool,
    pub finished: bool,
}

pub const FADE_IN_TIME: f32 = 0.25;
pub const FADE_OUT_TIME: f32 = 0.1;

impl Fade {
    pub fn fade_in() -> Fade {
        Fade { visibility: 0.0, velocity: 0.0, fading_in: true, finished: false }
    }

    pub fn fade_out() -> Fade {
        Fade { visibility: 1.0, velocity: 0.0, fading_in: false, finished: false }
    }

    /// One step of the coroutine; the visibility to apply this frame.
    pub fn step(&mut self, unscaled_delta: f32) -> f32 {
        if self.finished {
            return self.visibility;
        }
        if self.fading_in {
            if self.visibility < 0.99 {
                self.visibility = mathf::smooth_damp(self.visibility, 1.0, &mut self.velocity, FADE_IN_TIME, f32::INFINITY, unscaled_delta);
            } else {
                // The resume after the frame that reached 0.99 leaves the loop:
                // `UpdateAllAlphas(1)`.
                self.finished = true;
                self.visibility = 1.0;
            }
        } else if self.visibility >= 0.05 {
            self.visibility = mathf::smooth_damp(self.visibility, 0.0, &mut self.velocity, FADE_OUT_TIME, f32::INFINITY, unscaled_delta);
        } else {
            self.finished = true;
        }
        self.visibility
    }
}

/// `Button.Update`'s pressed tween: the button's local z smooth-damped towards the pressed
/// offset (`-0.04 - background.localPosition.z`) while held, else 0, over 0.025 s unscaled;
/// `ScaleProportionallyToLocalPositionZ` then scales it by `1 - (z / -0.04) * 0.05`.
pub fn button_z(z: f32, pressed: bool, pressed_offset: f32, velocity: &mut f32, unscaled_delta: f32) -> f32 {
    mathf::smooth_damp(z, if pressed { pressed_offset } else { 0.0 }, velocity, 0.025, f32::INFINITY, unscaled_delta)
}

pub fn scale_for_local_z(z: f32) -> f32 {
    1.0 - z / -0.04 * 0.05
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_live_players_must_be_inside() {
        let mut t = AllPlayersTrigger::default();
        t.enter(&[0], &[0, 1]);
        assert!(!t.triggered());
        t.exit(&[0]);
        t.enter(&[1], &[0, 1]);
        assert!(!t.triggered());
        // Player 0 got game over: the check runs again with only player 1 alive.
        t.check(&[1]);
        assert!(t.triggered());
        // Nobody alive: never.
        let mut t = AllPlayersTrigger::default();
        t.enter(&[0], &[]);
        assert!(!t.triggered());
    }

    #[test]
    fn enemy_count_needs_a_registration() {
        let mut c = EnemyCount::default();
        assert!(!c.all_gone());
        c.exhausted();
        assert!(!c.all_gone(), "no enemy registered yet");
        c.registered();
        c.registered();
        c.exhausted();
        assert_eq!(c.label(), 1);
        c.exhausted();
        assert!(c.all_gone());
    }

    #[test]
    fn time_slows_to_forty_percent() {
        let mut s = TimeSlower::default();
        let mut scale = 1.0;
        for _ in 0..120 {
            scale = s.update(scale, 1.0 / 60.0);
        }
        assert!((scale - SLOWED_TIME_SCALE).abs() < 1e-3, "{scale}");
    }

    #[test]
    fn rounding_is_bankers() {
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(3.5), 4);
        assert_eq!(round_half_even(-2.5), -2);
        assert_eq!(round_half_even(2.4), 2);
    }

    #[test]
    fn scores_of_a_flawless_solo_run() {
        let p = PlayerResult { player_index: 0, damage_dealt: 1000.0, starting_lives: 2, lives_remaining: 2, starting_health: 100.0, health_remaining: 100.0 };
        let (rows, team) = calculate_scores(Some(100.7), 1000.0, &[p], 1);
        assert_eq!(rows[0], CalculatedScore { damage: 10000, health: 3000, time: 500, damage_is_golden: true, health_is_golden: true, player_index: 0 });
        assert!(team.is_golden());
        assert_eq!(team.total(), 13500);
        // Not finished: 600 s.
        let (rows, _) = calculate_scores(None, 1000.0, &[p], 1);
        assert_eq!(rows[0].time, 0);
    }

    #[test]
    fn the_board_ticks_up_and_turns_gold() {
        let p = PlayerResult { player_index: 0, damage_dealt: 100.0, starting_lives: 2, lives_remaining: 1, starting_health: 100.0, health_remaining: 50.0 };
        let (rows, team) = calculate_scores(Some(590.0), 100.0, &[p], 1);
        let mut board = ScoreBoard::new(rows, team, 60);
        assert_eq!(board.cell(0, Column::Damage).0, "400");
        assert_eq!(board.cell(0, Column::Total).0, "0");
        for _ in 0..200 {
            board.frame();
        }
        assert_eq!(board.cell(0, Column::Damage), ("1000".to_owned(), GOLDEN_SCORE_COLOR));
        assert_eq!(board.cell(0, Column::Health), ("1500".to_owned(), REGULAR_SCORE_COLOR));
        assert_eq!(board.cell(0, Column::Time).0, "10");
        assert_eq!(board.cell(0, Column::Total).0, "2510");
        assert_eq!(board.team_cell(Column::Total).0, "2510");
    }

    #[test]
    fn a_score_unlocks_the_next_campaign_level() {
        let levels: Vec<String> = ["Skyline", "Japan", "Moon"].map(String::from).to_vec();
        let mut scores = HighScores::default();
        assert!(scores.is_level_unlocked(0, true, &levels));
        assert!(!scores.is_level_unlocked(1, true, &levels));
        assert!(scores.is_level_unlocked(2, false, &levels));
        scores.table("Skyline", 1, 0).add(Score::new(vec![PlayerId::anonymous_local(0)], 1234, false, 0.0));
        assert!(scores.is_level_unlocked(1, true, &levels));
        assert!(!scores.is_level_unlocked(2, true, &levels));
    }

    #[test]
    fn golden_scores_rank_first() {
        let mut t = ScoreTable::default();
        let id = || vec![PlayerId::anonymous_local(0)];
        t.add(Score::new(id(), 500, false, 0.0));
        t.add(Score::new(id(), 100, true, 1.0));
        t.add(Score::new(id(), 900, false, 2.0));
        let values: Vec<i32> = t.scores().iter().map(|s| s.value).collect();
        assert_eq!(values, [100, 900, 500]);
        for i in 0..20 {
            t.add(Score::new(id(), i, false, 3.0));
        }
        assert_eq!(t.scores().len(), MAXIMUM_SCORES);
    }

    #[test]
    fn a_tip_is_shown_once_per_player() {
        let tips = vec![
            unity_data::level::LevelTip { applicable_control_types: vec!["TouchProcessor".into()], message: "touch".into() },
            unity_data::level::LevelTip { applicable_control_types: vec![], message: "any".into() },
        ];
        let tip = choose_tip(&tips, &["KeyboardAndMouseProcessor"]).unwrap();
        assert_eq!(tip.message, "any");
        let mut seen = TipsSeen::default();
        let me = [PlayerId::anonymous_local(0)];
        assert!(seen.mark("Pirates", &tip.applicable_control_types, &me));
        assert!(!seen.mark("Pirates", &tip.applicable_control_types, &me));
    }

    #[test]
    fn fades_finish() {
        let mut f = Fade::fade_in();
        let mut frames = 0;
        while !f.finished {
            f.step(1.0 / 60.0);
            frames += 1;
            assert!(frames < 200);
        }
        assert_eq!(f.visibility, 1.0);
        let mut f = Fade::fade_out();
        while !f.finished {
            f.step(1.0 / 60.0);
        }
        assert!(f.visibility < 0.05);
        assert!((scale_for_local_z(-0.04) - 0.95).abs() < 1e-6);
    }
}
