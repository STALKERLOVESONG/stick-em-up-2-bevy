//! `Goals`: the game's achievements and each player's progress towards them.
//!
//! Twelve goals in three categories (the Lobby's three achievements screens). "All level" goals
//! count campaign levels done (`AllLevelGoalProgress`); the rest are done once
//! (`StandardGoalProgress`). Progress is kept per `PlayerID` and saved in `Goals.dat`
//! (`crate::save_data`).

use std::collections::HashMap;

use crate::level_end::PlayerId;

/// `GoalSpec`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoalSpec {
    /// `GoalCategory.ID`.
    pub category: &'static str,
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// `IsAllLevelGoal`.
    pub all_level: bool,
}

const fn goal(category: &'static str, id: &'static str, display_name: &'static str, description: &'static str, all_level: bool) -> GoalSpec {
    GoalSpec { category, id, display_name, description, all_level }
}

/// `Goals.Start`: the goals in the order they are listed.
pub const GOALS: [GoalSpec; 12] = [
    goal("standard", "complete -1", "Begun", "Complete all levels on beginner or higher", true),
    goal("standard", "office all destroyed", "Break room", "Destroy all the equipment that can be destroyed on Headquarters in 1P mode", false),
    goal("standard", "rover 720", "Dizzy", "Spin the moon rover through 720 degrees and land the right way up", false),
    goal("standard", "four frag shot", "Four frag shot", "Destroy four enemies with one spear", false),
    goal("standard", "complete 0", "Regulator", "Complete all levels on regular or higher", true),
    goal("tricky", "golden 0", "Regular master", "Get a golden score (destroy every enemy and don't get hit) on every level on regular or higher", true),
    goal(
        "tricky",
        "0 shot perfect health moon 1",
        "We came in peace for all mankind",
        "Finish Apollo in 1P mode on expert without firing any rockets or losing any health (it's okay if the rover gets destroyed, though)",
        false,
    ),
    goal("tricky", "four frag shot 1", "One chance", "Destroy four enemies with one spear on expert", false),
    goal("tricky", "complete 1", "One life ought to be enough for anybody", "Complete all levels on expert", true),
    goal("difficult", "rover 1440", "High flier", "Spin the moon rover through 1440 degrees (four turns) and land the right way up", false),
    goal("difficult", "0 shot perfect health japan", "Pacifist constitution", "Finish Japan in 1P mode on beginner without firing or losing any health", false),
    goal("difficult", "golden 1", "You are a super player!!", "Get a golden score (destroy every enemy and don't get hit) on every level on expert", true),
];

/// `Goals.GetGoalSpec`.
pub fn goal_spec(id: &str) -> Option<&'static GoalSpec> {
    GOALS.iter().find(|g| g.id == id)
}

/// `StandardGoalProgress` / `AllLevelGoalProgress`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoalProgress {
    Standard { done: bool },
    /// The campaign levels done, in the order they were done.
    AllLevel { levels: Vec<String> },
}

impl GoalProgress {
    /// `GoalSpec.GetNoProgress`.
    pub fn none_for(spec: &GoalSpec) -> GoalProgress {
        if spec.all_level { GoalProgress::AllLevel { levels: Vec::new() } } else { GoalProgress::Standard { done: false } }
    }

    pub fn is_started(&self) -> bool {
        match self {
            GoalProgress::Standard { done } => *done,
            GoalProgress::AllLevel { levels } => !levels.is_empty(),
        }
    }

    /// `IsDone` (`campaign_level_count` is `Goals.CampaignLevelCount`).
    pub fn is_done(&self, campaign_level_count: usize) -> bool {
        match self {
            GoalProgress::Standard { done } => *done,
            GoalProgress::AllLevel { levels } => levels.len() >= campaign_level_count,
        }
    }

    /// `GetLevelDoneCount` (0 for a standard goal).
    pub fn level_done_count(&self) -> usize {
        match self {
            GoalProgress::Standard { .. } => 0,
            GoalProgress::AllLevel { levels } => levels.len(),
        }
    }

    /// `Done(level)`: whether anything changed. The original throws when a level is given to a
    /// standard goal or none to an all-level goal; here nothing changes.
    pub fn done(&mut self, level: Option<&str>) -> bool {
        match (self, level) {
            (GoalProgress::Standard { done }, None) => !std::mem::replace(done, true),
            (GoalProgress::AllLevel { levels }, Some(level)) => {
                if levels.iter().any(|l| l == level) {
                    false
                } else {
                    levels.push(level.to_owned());
                    true
                }
            }
            _ => false,
        }
    }
}

/// `Goals.progress`: every player's progress by goal id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Goals {
    pub(crate) progress: Vec<(PlayerId, HashMap<String, GoalProgress>)>,
}

impl Goals {
    fn player_mut(&mut self, player: &PlayerId) -> &mut HashMap<String, GoalProgress> {
        let index = match self.progress.iter().position(|(p, _)| p.platform == player.platform && p.id == player.id) {
            Some(i) => i,
            None => {
                self.progress.push((player.clone(), HashMap::new()));
                self.progress.len() - 1
            }
        };
        &mut self.progress[index].1
    }

    /// `GetProgress` (no progress when the player has none recorded).
    pub fn progress(&self, player: &PlayerId, goal: &GoalSpec) -> GoalProgress {
        self.progress
            .iter()
            .find(|(p, _)| p.platform == player.platform && p.id == player.id)
            .and_then(|(_, m)| m.get(goal.id))
            .cloned()
            .unwrap_or_else(|| GoalProgress::none_for(goal))
    }

    /// `GetPlayerGoalDoneCount`.
    pub fn done_count(&self, player: &PlayerId, campaign_level_count: usize) -> usize {
        self.progress
            .iter()
            .find(|(p, _)| p.platform == player.platform && p.id == player.id)
            .map_or(0, |(_, m)| m.values().filter(|g| g.is_done(campaign_level_count)).count())
    }

    /// `GoalDone` for an active local player: whether the progress changed (then the original
    /// saves `Goals.dat`).
    pub fn goal_done(&mut self, player: &PlayerId, goal_id: &str, level: Option<&str>) -> bool {
        let Some(spec) = goal_spec(goal_id) else { return false };
        self.player_mut(player).entry(spec.id.to_owned()).or_insert_with(|| GoalProgress::none_for(spec)).done(level)
    }
}

/// `GoalsDisplay.Awake`: a row's height for a description of `description_lines` lines.
pub fn goal_row_height(name_scale_y: f32, description_scale_y: f32, description_lines: usize) -> f32 {
    (name_scale_y + description_lines as f32 * description_scale_y) * 3.25 + 0.3
}

/// `GoalsDisplay.Update`: the lines that mark one player's progress on one goal, around
/// `center`: a dash when not started, a dot when done, otherwise a pie slice of the campaign
/// (four segments per level done, then back to the centre).
pub fn progress_mark_lines(progress: &GoalProgress, campaign_level_count: usize, center: [f32; 3]) -> Vec<([f32; 3], [f32; 3], f32)> {
    let add = |p: [f32; 3], d: [f32; 3]| [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
    if !progress.is_started() {
        return vec![(add(center, [-0.45, 0.0, 0.0]), add(center, [0.45, 0.0, 0.0]), 0.15)];
    }
    if progress.is_done(campaign_level_count) {
        return vec![(center, add(center, [0.001, 0.0, 0.0]), 1.05)];
    }
    let mut lines = Vec::new();
    let mut previous = [0.0f32; 3];
    let step = std::f32::consts::PI * 2.0 / (campaign_level_count * 4) as f32;
    for l in 0..=progress.level_done_count() * 4 {
        let f = l as f32 * step;
        let next = [f.sin() * 0.45, f.cos() * 0.45, 0.0];
        lines.push((add(center, previous), add(center, next), 0.15));
        previous = next;
    }
    lines.push((add(center, previous), center, 0.15));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goals_record_once() {
        let mut goals = Goals::default();
        let me = PlayerId::anonymous_local(0);
        assert!(goals.goal_done(&me, "rover 720", None));
        assert!(!goals.goal_done(&me, "rover 720", None));
        assert!(!goals.goal_done(&me, "no such goal", None));
        assert!(goals.goal_done(&me, "complete -1", Some("Skyline")));
        assert!(!goals.goal_done(&me, "complete -1", Some("Skyline")));
        assert_eq!(goals.done_count(&me, 10), 1);
        let progress = goals.progress(&me, goal_spec("complete -1").unwrap());
        assert!(progress.is_started() && !progress.is_done(10));
        assert_eq!(goals.done_count(&me, 1), 2);
        assert_eq!(GOALS.iter().filter(|g| g.category == "standard").count(), 5);
    }

    #[test]
    fn progress_marks() {
        let none = GoalProgress::Standard { done: false };
        assert_eq!(progress_mark_lines(&none, 10, [0.0; 3]).len(), 1);
        let done = GoalProgress::Standard { done: true };
        assert_eq!(progress_mark_lines(&done, 10, [0.0; 3])[0].2, 1.05);
        let some = GoalProgress::AllLevel { levels: vec!["A".into(), "B".into()] };
        // 2 levels × 4 + 1 segments, then the closing line.
        assert_eq!(progress_mark_lines(&some, 10, [0.0; 3]).len(), 10);
        assert!((goal_row_height(0.5, 0.4, 2) - ((0.5 + 0.8) * 3.25 + 0.3)).abs() < 1e-6);
    }
}
