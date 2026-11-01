//! The Lobby screens beyond the start-a-game path: colour customisation (`CustomizeColor`,
//! `ColorModels`), the error screen (`LobbyError`), high scores (`HighScoresDisplay`),
//! achievements (`GoalsDisplay`, `OverallGoalProgressDisplay`), the credits' links and cheats
//! (`WebLink`, `VersionNumberLabel`, `AllLevelUnlockCheat`, `TrivialFPSEnabler`,
//! `ShowEndingCheat`) and the summary's `FollowX` / `FollowActivation` team labels.
//!
//! Scripts on inactive objects get their `Awake`/`Start` the first time their object is active
//! in the hierarchy, and `OnEnable` every time it becomes so ([`Screens::enabled`]).

use std::collections::HashMap;

use sticks_sim::goals::{GOALS, goal_row_height, progress_mark_lines};
use sticks_sim::lobby::{CharacterColors, ColorType, MAX_PLAYERS, closest_color_index, label_position, samples_start_x, swatches, SAMPLE_WIDTH};
use sticks_sim::lobby_models::{arrange_models, model_aim};
use sticks_sim::text::format_text;
use unity_compat::Vec2;
use unity_data::interactive::ScriptInstance;

use super::{Lobby, LobbyMemory, level_display_name};
use crate::state::GameMode;

/// `HighScoresDisplay.RegularScoreColor` / `GoldenScoreColor`.
pub(super) const REGULAR_SCORE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub(super) const GOLDEN_SCORE: [f32; 4] = [1.0, 238.0 / 255.0, 34.0 / 255.0, 1.0];
/// `HighScoresDisplay.ROW_COUNT`.
const HIGH_SCORE_ROWS: usize = 10;
/// Presses that trigger a credits cheat.
const CHEAT_PRESS_COUNT: u32 = 10;

/// `CustomizeColor` after `Awake`.
struct Customize {
    object: usize,
    kind: ColorType,
    swatches: Vec<[f32; 4]>,
    /// Each swatch button (the "Default" button is -1) and its colour index.
    buttons: Vec<(usize, i32)>,
    start_x: f32,
    label_start_y: f32,
    label_height: f32,
    label_z: f32,
    player_labels: Vec<usize>,
    single_label: Option<usize>,
    left_arrow: Option<usize>,
    right_arrow: Option<usize>,
    selected: [Option<i32>; MAX_PLAYERS],
    pressed: Option<i32>,
    left_pressed: bool,
    right_pressed: bool,
    first_update: bool,
}

/// `ColorModels` after `Start`.
struct ColorModels {
    object: usize,
    models: Vec<Option<usize>>,
    remove_gaps: bool,
    versus_label: Option<usize>,
    start_x: f32,
    offset_x: f32,
}

/// `HighScoresDisplay` after `Awake`.
struct HighScores {
    object: usize,
    /// Each row's root, name label and score label.
    rows: Vec<(usize, Option<usize>, Option<usize>)>,
    last: Option<(usize, i32, usize)>,
}

/// `GoalsDisplay` after `Awake`.
struct GoalsScreen {
    object: usize,
    /// Each goal of the category: its row root and name label.
    rows: Vec<(usize, Option<usize>, &'static sticks_sim::goals::GoalSpec)>,
    lines_renderer: Option<usize>,
}

/// `WebLink` (colours taken in `Start`).
struct WebLink {
    object: usize,
    url: String,
    standard: [f32; 4],
}

#[derive(Default)]
pub(super) struct Screens {
    customize: Vec<Customize>,
    color_models: Vec<ColorModels>,
    high_scores: Option<HighScores>,
    goals: Vec<GoalsScreen>,
    web_links: Option<Vec<WebLink>>,
    /// `LobbyError.returnTo` of each error screen (by its object).
    error_return: HashMap<usize, usize>,
    /// `activeInHierarchy` of script objects last frame (for `OnEnable`).
    was_active: HashMap<usize, bool>,
    /// Buttons whose `SimulatePress` ran this frame; pressed after the screens' updates.
    pub(super) simulated: Vec<usize>,
    unlock_presses: u32,
    fps_presses: u32,
    ending_presses: u32,
    /// `ShowEndingCheat` asked for `LevelLoader.GameComplete()` this frame.
    pub(super) show_ending: bool,
    version_checked: bool,
    /// The progress marks of the achievements screens this frame (renderer world position, lines).
    pub(super) goal_marks: Vec<(unity_compat::Vec3, Vec<([f32; 3], [f32; 3], f32, [u8; 4])>)>,
}

impl Screens {
    /// Whether `object` became active in the hierarchy this frame (`OnEnable`); records it.
    fn enabled(&mut self, lobby_active: bool, object: usize) -> bool {
        let was = self.was_active.insert(object, lobby_active).unwrap_or(false);
        lobby_active && !was
    }
}

impl Lobby {
    fn mover_target(&mut self, object: usize, target: [f32; 3]) {
        if let Some(m) = self.movers.iter_mut().find(|m| m.object == object) {
            m.target = target;
        }
    }

    /// `transform.position = (x, ..)` of an object whose ancestors are not rotated.
    fn set_world_x(&mut self, object: usize, x: f32) {
        let now = self.world(object).t[0];
        self.local[object][0] += x - now;
    }

    fn text_color_of(&self, object: usize) -> Option<[f32; 4]> {
        self.text_of_object.get(&object).map(|&t| self.texts[t].color)
    }

    /// `LobbyError.ShowErrorAndReturnTo` of the error screen `error`.
    pub(super) fn show_error(&mut self, error: usize, message: &str, return_to: usize) {
        if let Some(s) = self.scene.script(error, "LobbyError").cloned() {
            if let Some(label) = self.scene.field_object(&s, "messageLabel") {
                self.set_text(label, message);
                if let Some(&t) = self.text_of_object.get(&label) {
                    let n = message.chars().count();
                    if n > self.texts[t].layout.max_chars {
                        self.texts[t].layout.max_chars = n;
                    }
                }
            }
        }
        self.screens.error_return.insert(error, return_to);
        self.activate(error);
    }
}

/// Mouse-down events (`OnMouseDown`) of the credits' hidden counters.
pub(super) fn mouse_down(lobby: &mut Lobby, memory: &mut LobbyMemory, object: usize) {
    if lobby.scene.script(object, "AllLevelUnlockCheat").is_some() {
        lobby.screens.unlock_presses += 1;
        if lobby.screens.unlock_presses >= CHEAT_PRESS_COUNT {
            lobby.screens.unlock_presses = 0;
            memory.all_levels_unlocked = true;
            let at = lobby.world_position(object);
            lobby.sounds.push(("Large Triumph".to_owned(), at));
        }
    }
    if lobby.scene.script(object, "TrivialFPSEnabler").is_some() {
        lobby.screens.fps_presses += 1;
        if lobby.screens.fps_presses >= CHEAT_PRESS_COUNT {
            lobby.screens.fps_presses = 0;
            memory.fps_display = !memory.fps_display;
        }
    }
    if lobby.scene.script(object, "ShowEndingCheat").is_some() {
        lobby.screens.ending_presses += 1;
        if lobby.screens.ending_presses >= CHEAT_PRESS_COUNT {
            lobby.screens.ending_presses = 0;
            // `LevelLoader.GameComplete()`: `lobby_update` loads the "Complete" scene.
            lobby.screens.show_ending = true;
        }
    }
}

/// `Button.Pressed` handlers of the scripts here. Returns whether one handled `object`.
pub(super) fn press(lobby: &mut Lobby, object: usize) -> bool {
    let mut handled = false;
    for c in &mut lobby.screens.customize {
        if let Some(&(_, index)) = c.buttons.iter().find(|(b, _)| *b == object) {
            c.pressed = Some(index);
            handled = true;
        }
        if c.left_arrow == Some(object) {
            c.left_pressed = true;
            handled = true;
        }
        if c.right_arrow == Some(object) {
            c.right_pressed = true;
            handled = true;
        }
    }
    // `LobbyError.Start`: its back button returns to where the error came from.
    for s in lobby.scene.scripts_named("LobbyError").cloned().collect::<Vec<_>>() {
        if lobby.scene.field_object(&s, "backButton") == Some(object) {
            if let Some(&back) = lobby.screens.error_return.get(&s.object) {
                lobby.activate(back);
            }
            handled = true;
        }
    }
    // `ShowAchievements.Start`.
    if let Some(s) = lobby.scene.script(object, "ShowAchievements").cloned() {
        if let Some(screen) = lobby.scene.field_object(&s, "achievementsScreen") {
            lobby.activate(screen);
        }
        handled = true;
    }
    // `WebLink.OnMouseUpAsButton`: `Application.OpenURL`.
    if let Some(link) = lobby.screens.web_links.as_ref().and_then(|l| l.iter().find(|w| w.object == object)) {
        open_url(&link.url);
        handled = true;
    }
    handled
}

/// `Application.OpenURL` with the platform's opener.
fn open_url(url: &str) {
    let result = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = result {
        bevy::log::warn!("cannot open {url}: {e}");
    }
}

/// One frame of every screen script here (after the start-a-game screens).
pub(super) fn update(lobby: &mut Lobby, memory: &mut LobbyMemory, player_moves: &[(i32, i32); MAX_PLAYERS], mouse_player: Option<usize>, dt: f32) {
    awake_and_start(lobby);
    color_models(lobby, memory, dt);
    customize_colors(lobby, memory, player_moves, mouse_player);
    overall_goal_progress(lobby, memory);
    high_scores(lobby, memory);
    goals_displays(lobby, memory);
    web_links(lobby);
    follow(lobby);
}

/// `Awake`/`Start` of the scripts on objects active for the first time.
fn awake_and_start(lobby: &mut Lobby) {
    for s in lobby.scene.scripts_named("CustomizeColor").cloned().collect::<Vec<_>>() {
        if lobby.active_in_hierarchy(s.object) && !lobby.screens.customize.iter().any(|c| c.object == s.object) {
            customize_awake(lobby, &s);
        }
    }
    for s in lobby.scene.scripts_named("ColorModels").cloned().collect::<Vec<_>>() {
        if lobby.active_in_hierarchy(s.object) && !lobby.screens.color_models.iter().any(|c| c.object == s.object) {
            let models = lobby.scene.field_objects(&s, "models");
            let x = |m: Option<&Option<usize>>| m.copied().flatten().map_or(0.0, |o| lobby.scene.objects[o].local_position[0]);
            let start_x = x(models.first());
            let offset_x = x(models.get(1)) - start_x;
            lobby.screens.color_models.push(ColorModels {
                object: s.object,
                remove_gaps: s.fields.i64_of("removeGapsAndGroupByTeams").unwrap_or(0) != 0,
                versus_label: lobby.scene.field_object(&s, "versusLabel"),
                models,
                start_x,
                offset_x,
            });
        }
    }
    if lobby.screens.high_scores.is_none() {
        if let Some(s) = lobby.named_script("HighScoresDisplay").cloned() {
            if lobby.active_in_hierarchy(s.object) {
                high_scores_awake(lobby, &s);
            }
        }
    }
    for s in lobby.scene.scripts_named("GoalsDisplay").cloned().collect::<Vec<_>>() {
        if lobby.active_in_hierarchy(s.object) && !lobby.screens.goals.iter().any(|g| g.object == s.object) {
            goals_awake(lobby, &s);
        }
    }
    if lobby.screens.web_links.is_none() {
        let mut links: Vec<WebLink> = lobby
            .scene
            .scripts_named("WebLink")
            .map(|s| WebLink { object: s.object, url: s.fields.str_of("url").unwrap_or_default().to_owned(), standard: [1.0; 4] })
            .collect();
        for w in &mut links {
            w.standard = lobby.text_color_of(w.object).unwrap_or([1.0; 4]);
        }
        lobby.screens.web_links = Some(links);
    }
}

/// `CustomizeColor.Awake`: the swatch row (the "Default"/"Match"/"Random" button, then one copy
/// of the prototype per colour), the arrows at its ends and the label layout.
fn customize_awake(lobby: &mut Lobby, s: &ScriptInstance) {
    let Some(kind) = s.fields.i64_of("type").and_then(ColorType::from_index) else { return };
    let colors = swatches(kind);
    let n = colors.len();
    let start_x = samples_start_x(n);
    let field = |name: &str| lobby.scene.field_object(s, name);
    let (Some(prototype), Some(default)) = (field("prototypeColorSample"), field("defaultButton")) else { return };
    let (left_arrow, right_arrow) = (field("leftArrow"), field("rightArrow"));
    let player_labels: Vec<usize> = lobby.scene.field_objects(s, "playerLabels").into_iter().flatten().collect();
    let single_label = field("singlePlayerLabel");
    let proto_local = lobby.local[prototype];
    let mut buttons = Vec::new();
    for i in -1..n as i32 {
        let place = [start_x + i as f32 * SAMPLE_WIDTH, proto_local[1], proto_local[2]];
        let object = if i == -1 {
            lobby.active[default] = true;
            lobby.local[default] = place;
            default
        } else {
            let copy = lobby.instantiate(prototype, None, place);
            // `GetComponentInChildren<tk2dSprite>().color`: the sample's "Background" sprite.
            if let Some(background) = lobby.scene.child(copy, "Background") {
                lobby.set_sprite_color(background, colors[i as usize]);
            }
            copy
        };
        buttons.push((object, i));
    }
    let total = n as f32 * SAMPLE_WIDTH;
    for (arrow, right) in [(left_arrow, false), (right_arrow, true)] {
        let Some(arrow) = arrow else { continue };
        let mut x = start_x - SAMPLE_WIDTH + (lobby.local[arrow][0] - proto_local[0]);
        if right {
            x += total;
        }
        lobby.local[arrow][0] = x;
    }
    let first = player_labels.first().map_or([0.0; 3], |&l| lobby.local[l]);
    let second_y = player_labels.get(1).map_or(first[1], |&l| lobby.local[l][1]);
    lobby.active[prototype] = false;
    if let Some(text) = lobby.scene.child(default, "Text") {
        lobby.set_text(text, kind.default_caption());
    }
    lobby.screens.customize.push(Customize {
        object: s.object,
        kind,
        swatches: colors,
        buttons,
        start_x,
        label_start_y: first[1],
        label_height: first[1] - second_y,
        label_z: first[2],
        player_labels,
        single_label,
        left_arrow,
        right_arrow,
        selected: [None; MAX_PLAYERS],
        pressed: None,
        left_pressed: false,
        right_pressed: false,
        first_update: true,
    });
}

/// `CustomizeColor.OnEnable` / `Update`: each player's label stands under its chosen swatch;
/// the player's inputs' left/right (A/D, a gamepad's stick or d-pad) change it, and the player
/// with the keyboard and mouse can click a swatch.
fn customize_colors(lobby: &mut Lobby, memory: &mut LobbyMemory, player_moves: &[(i32, i32); MAX_PLAYERS], mouse_player: Option<usize>) {
    let mut states = std::mem::take(&mut lobby.screens.customize);
    for c in &mut states {
        let active = lobby.active_in_hierarchy(c.object);
        if lobby.screens.enabled(active, c.object) {
            c.selected = [None; MAX_PLAYERS];
            c.first_update = true;
        }
        if !active {
            continue;
        }
        let n = c.swatches.len() as i32;
        let position = |c: &Customize, index: i32, player: usize| label_position(&c.selected, index, player, c.start_x, c.label_start_y, c.label_height, c.label_z);
        let mut actual_label: [Option<usize>; MAX_PLAYERS] = [None; MAX_PLAYERS];
        let mut players = 0;
        // The keyboard and mouse player (`KeyboardAndMouseProcessor`) clicks swatches; only a
        // touch player gets the arrows.
        for i in 0..MAX_PLAYERS {
            let has_input = memory.is_player_active(i);
            if has_input {
                players += 1;
            } else {
                c.selected[i] = None;
            }
            let Some(&label) = c.player_labels.get(i) else { continue };
            let was_active = lobby.active[label];
            lobby.active[label] = has_input;
            if has_input && (!was_active || c.first_update) {
                let color = memory.player_colors(i).get(c.kind);
                c.selected[i] = Some(closest_color_index(&c.swatches, color));
                let p = position(c, c.selected[i].unwrap_or(-1), i);
                lobby.local[label] = p;
                lobby.mover_target(label, p);
            }
            actual_label[i] = Some(label);
        }
        let single = players == 1;
        if let Some(single_label) = c.single_label {
            let was_active = lobby.active[single_label];
            lobby.active[single_label] = single;
            if single {
                for k in 0..c.player_labels.len().min(MAX_PLAYERS) {
                    let Some(index) = c.selected[k] else { continue };
                    lobby.active[c.player_labels[k]] = false;
                    actual_label[k] = Some(single_label);
                    let p = position(c, index, k);
                    if !was_active || c.first_update {
                        lobby.local[single_label] = p;
                    }
                    lobby.mover_target(single_label, p);
                }
            }
        }
        for arrow in [c.left_arrow, c.right_arrow].into_iter().flatten() {
            lobby.active[arrow] = false;
        }
        for i in 0..MAX_PLAYERS {
            let Some(value) = c.selected[i] else { continue };
            let mut index = value;
            // `LobbyInput.MoveLeft` / `MoveRight` of the player's inputs.
            if memory.is_player_active(i) {
                index += player_moves[i].1 - player_moves[i].0;
            }
            if mouse_player == Some(i) {
                if let Some(pressed) = c.pressed {
                    index = pressed;
                }
            }
            index = index.clamp(-1, n - 1);
            let p = position(c, index, i);
            if let Some(label) = actual_label[i] {
                lobby.mover_target(label, p);
            }
            if index != value {
                c.selected[i] = Some(index);
                let mut colors: CharacterColors = memory.player_colors(i);
                colors.set(c.kind, (index != -1).then(|| c.swatches[index as usize]));
                if memory.colors.set(&LobbyMemory::player_id(i), colors) {
                    memory.save.colors = true;
                }
            }
        }
        c.left_pressed = false;
        c.right_pressed = false;
        c.pressed = None;
        c.first_update = false;
    }
    lobby.screens.customize = states;
}

/// `ColorModels.OnEnable` / `Update`: one model per active player, fading to its colours,
/// aiming; on the summary screen side by side and grouped by team.
fn color_models(lobby: &mut Lobby, memory: &LobbyMemory, dt: f32) {
    let Some(mut figures) = lobby.figures.take() else { return };
    let states = std::mem::take(&mut lobby.screens.color_models);
    let team_versus = (memory.mode == GameMode::TeamVersus).then_some(&memory.teams);
    for c in &states {
        let active = lobby.active_in_hierarchy(c.object);
        if lobby.screens.enabled(active, c.object) {
            for root in c.models.iter().flatten() {
                let face = sticks_sim::visual::random_face_color(&mut figures.rng);
                if let Some(model) = figures.model_mut(*root) {
                    model.palette.face = face;
                }
            }
        }
        if !active {
            continue;
        }
        let mut shown = [false; MAX_PLAYERS];
        for i in 0..MAX_PLAYERS {
            let Some(Some(root)) = c.models.get(i).copied() else { continue };
            shown[i] = memory.is_player_active(i);
            lobby.active[root] = shown[i];
            if !shown[i] {
                continue;
            }
            let colors = memory.player_colors(i);
            let body = colors.body_or_default(i, team_versus).unwrap_or([1.0; 4]);
            let helmet = colors.helmet_or_matching(i, team_versus).unwrap_or([1.0; 4]);
            if let Some(model) = figures.model_mut(root) {
                model.fade_colors(body, helmet, colors.face, dt);
                model.set_aim(model_aim());
            }
        }
        if c.remove_gaps {
            let arrangement = arrange_models(shown, c.start_x, c.offset_x, team_versus);
            for i in 0..MAX_PLAYERS {
                let Some(Some(root)) = c.models.get(i).copied() else { continue };
                let Some(model) = figures.model_mut(root) else { continue };
                if let Some(x) = arrangement.x[i] {
                    model.set_x(x);
                    lobby.local[root][0] = model.character.position().x;
                }
                if arrangement.mirrored[i] {
                    let aim = model_aim();
                    model.set_aim(Vec2::new(-aim.x, aim.y));
                }
            }
            if let Some(label) = c.versus_label {
                lobby.active[label] = arrangement.versus_label_x.is_some();
                if let Some(x) = arrangement.versus_label_x {
                    lobby.set_world_x(label, x);
                }
            }
        } else if let Some(label) = c.versus_label {
            lobby.active[label] = false;
        }
    }
    lobby.screens.color_models = states;
    lobby.figures = Some(figures);
}

/// `OverallGoalProgressDisplay.OnEnable`: "n/12 Done", n the fewest goals any local player has.
fn overall_goal_progress(lobby: &mut Lobby, memory: &LobbyMemory) {
    for s in lobby.scene.scripts_named("OverallGoalProgressDisplay").cloned().collect::<Vec<_>>() {
        let active = lobby.active_in_hierarchy(s.object);
        if !lobby.screens.enabled(active, s.object) {
            continue;
        }
        let Some(label) = lobby.scene.field_object(&s, "label") else { continue };
        let done = (0..MAX_PLAYERS).filter(|&i| memory.is_player_active(i)).map(|i| memory.goals.done_count(&LobbyMemory::player_id(i), memory.campaign_level_count())).min();
        let text = done.map_or(String::new(), |n| format!("{n}/{} Done", GOALS.len()));
        lobby.set_text(label, &text);
    }
}

/// `HighScoresDisplay.Awake`: ten rows copied from the prototype, numbered; the prototype and
/// the second row (which only gave the spacing) go.
fn high_scores_awake(lobby: &mut Lobby, s: &ScriptInstance) {
    let (Some(prototype), Some(second)) = (lobby.scene.field_object(s, "rowPrototype"), lobby.scene.field_object(s, "secondRow")) else { return };
    let start = lobby.local[prototype];
    let step = [0, 1, 2].map(|k| lobby.local[second][k] - start[k]);
    let mut rows = Vec::new();
    for k in 0..HIGH_SCORE_ROWS {
        let at = [0, 1, 2].map(|a| start[a] + step[a] * k as f32);
        let row = lobby.instantiate(prototype, None, at);
        if let Some(position) = lobby.scene.child(row, "Position") {
            lobby.set_text(position, &format!("{}.", k + 1));
        }
        rows.push((row, lobby.scene.child(row, "Names"), lobby.scene.child(row, "Score")));
    }
    lobby.active[prototype] = false;
    lobby.active[second] = false;
    lobby.screens.high_scores = Some(HighScores { object: s.object, rows, last: None });
}

/// `HighScoresDisplay.Update`: the selected campaign level's table for this player count and
/// difficulty; outside the campaign (or without players) the screen backs out at once.
fn high_scores(lobby: &mut Lobby, memory: &mut LobbyMemory) {
    let Some(mut state) = lobby.screens.high_scores.take() else { return };
    let Some(s) = lobby.scene.script(state.object, "HighScoresDisplay").cloned() else {
        lobby.screens.high_scores = Some(state);
        return;
    };
    if lobby.active_in_hierarchy(state.object) {
        let players = memory.player_count();
        if memory.mode != GameMode::Campaign || players == 0 {
            if let Some(back) = lobby.scene.field_object(&s, "backButton") {
                lobby.screens.simulated.push(back);
            }
        } else if state.last != Some((memory.level_index, memory.difficulty, players)) {
            let level = memory.campaign_levels.get(memory.level_index).cloned().unwrap_or_default();
            if let Some(label) = lobby.scene.field_object(&s, "levelNameLabel") {
                lobby.set_text(label, level_display_name(&level));
            }
            let selected = (memory.difficulty + 1).max(0) as usize;
            for (i, star) in lobby.scene.field_objects(&s, "difficultyStars").into_iter().enumerate() {
                if let Some(star) = star {
                    lobby.active[star] = i == selected;
                }
            }
            if let Some(label) = lobby.scene.field_object(&s, "playerCountLabel") {
                lobby.set_text(label, &if players == 1 { format!("{players} Player") } else { format!("{players} Players") });
            }
            let scores = memory.high_scores.table(&level, players, memory.difficulty).scores().to_vec();
            if let Some(none) = lobby.scene.field_object(&s, "noScoresLabel") {
                lobby.active[none] = scores.is_empty();
            }
            for (j, &(row, names, score_label)) in state.rows.iter().enumerate() {
                let shown = j < scores.len();
                lobby.active[row] = shown;
                if !shown {
                    continue;
                }
                let score = &scores[j];
                if let Some(label) = score_label {
                    lobby.set_text(label, &score.value.to_string());
                    lobby.set_text_color(label, if score.golden { GOLDEN_SCORE } else { REGULAR_SCORE });
                }
                if let Some(label) = names {
                    let text = score.ids.iter().map(|id| if id.name.is_empty() { "Unknown" } else { id.name.as_str() }).collect::<Vec<_>>().join(", ");
                    lobby.set_text(label, &text);
                }
            }
            state.last = Some((memory.level_index, memory.difficulty, players));
        }
    }
    lobby.screens.high_scores = Some(state);
}

/// `GoalsDisplay.Awake`: a row per goal of the category, below one another by the height of
/// its (word-wrapped) description.
fn goals_awake(lobby: &mut Lobby, s: &ScriptInstance) {
    let category = s.fields.str_of("categoryID").unwrap_or_default().to_owned();
    let (Some(prototype), Some(start)) = (lobby.scene.field_object(s, "rowPrototype"), lobby.scene.field_object(s, "startPosition")) else { return };
    let parent = lobby.scene.objects[s.object].parent;
    let mut at = lobby.local[start];
    let mut rows = Vec::new();
    for goal in GOALS.iter().filter(|g| g.category == category) {
        let row = lobby.instantiate(prototype, parent, at);
        lobby.active[row] = true;
        let name = lobby.scene.child(row, "Name");
        let description = lobby.scene.child(row, "Description");
        if let Some(name) = name {
            lobby.set_text(name, goal.display_name);
        }
        let mut lines = 1;
        let (mut name_scale, mut description_scale) = (0.0, 0.0);
        if let Some(&t) = name.and_then(|n| lobby.text_of_object.get(&n)) {
            name_scale = lobby.texts[t].layout.scale[1];
        }
        if let Some(description) = description {
            lobby.set_text(description, goal.description);
            if let Some(&t) = lobby.text_of_object.get(&description) {
                description_scale = lobby.texts[t].layout.scale[1];
                let (font, layout) = (lobby.texts[t].font.clone(), lobby.texts[t].layout.clone());
                if let Some(f) = lobby.fonts.get(&font) {
                    lines = format_text(f, &layout, goal.description).matches('\n').count() + 1;
                }
            }
        }
        at[1] -= goal_row_height(name_scale, description_scale, lines);
        rows.push((row, name, goal));
    }
    lobby.active[prototype] = false;
    lobby.screens.goals.push(GoalsScreen { object: s.object, rows, lines_renderer: lobby.scene.field_object(s, "linesRenderer") });
}

/// `GoalsDisplay.Update`: names make room for one mark column per local player; each player's
/// mark (in its body colour) on every goal.
fn goals_displays(lobby: &mut Lobby, memory: &LobbyMemory) {
    lobby.screens.goal_marks.clear();
    let local_players = (0..MAX_PLAYERS).filter(|&i| memory.is_player_active(i)).count();
    let screens = std::mem::take(&mut lobby.screens.goals);
    for g in &screens {
        if !lobby.active_in_hierarchy(g.object) {
            continue;
        }
        for &(_, name, _) in &g.rows {
            if let Some(name) = name {
                lobby.local[name][0] = local_players as f32 * 1.2;
            }
        }
        let mut lines = Vec::new();
        let mut column = [0.6f32, 0.94, -0.1];
        for j in 0..MAX_PLAYERS {
            if !memory.is_player_active(j) {
                continue;
            }
            let team_versus = (memory.mode == GameMode::TeamVersus).then_some(&memory.teams);
            let body = memory.player_colors(j).body_or_default(j, team_versus).unwrap_or([1.0; 4]);
            let color = sticks_sim::visual::to_color32(body);
            for &(row, _, goal) in &g.rows {
                let r = lobby.local[row];
                let center = [r[0] + column[0], r[1] + column[1], r[2] + column[2]];
                let progress = memory.goals.progress(&LobbyMemory::player_id(j), goal);
                for (a, b, width) in progress_mark_lines(&progress, memory.campaign_level_count().max(1), center) {
                    lines.push((a, b, width, color));
                }
            }
            column[0] += 1.2;
        }
        if let Some(renderer) = g.lines_renderer.filter(|&r| lobby.active_in_hierarchy(r)) {
            let at = lobby.world_position(renderer);
            lobby.screens.goal_marks.push((at, lines));
        }
    }
    lobby.screens.goals = screens;
}

/// `WebLink.UpdateColor` (hovered ×1.2, pressed ×0.9, alpha kept) and `VersionNumberLabel.Awake`.
fn web_links(lobby: &mut Lobby) {
    let links: Vec<(usize, [f32; 4])> = lobby.screens.web_links.as_ref().map(|l| l.iter().map(|w| (w.object, w.standard)).collect()).unwrap_or_default();
    for (object, standard) in links {
        let k = if lobby.pressed_on == Some(object) {
            0.9
        } else if lobby.hovered == Some(object) {
            1.2
        } else {
            1.0
        };
        lobby.set_text_color(object, [standard[0] * k, standard[1] * k, standard[2] * k, standard[3]]);
    }
    if !lobby.screens.version_checked {
        lobby.screens.version_checked = true;
        for s in lobby.scene.scripts_named("VersionNumberLabel").cloned().collect::<Vec<_>>() {
            match lobby.versions.clone() {
                Some((version, unity)) if version != unity => lobby.set_text(s.object, &format!("Version {version}")),
                _ => lobby.active[s.object] = false,
            }
        }
    }
}

/// `FollowX.Update` and `FollowActivation.Update` (the summary's team name labels).
fn follow(lobby: &mut Lobby) {
    for s in lobby.scene.scripts_named("FollowX").cloned().collect::<Vec<_>>() {
        if !lobby.active_in_hierarchy(s.object) {
            continue;
        }
        if let Some(target) = lobby.scene.field_object(&s, "target") {
            let x = lobby.world(target).t[0];
            lobby.set_world_x(s.object, x);
        }
    }
    for s in lobby.scene.scripts_named("FollowActivation").cloned().collect::<Vec<_>>() {
        if !lobby.active_in_hierarchy(s.object) {
            continue;
        }
        if let (Some(source), Some(destination)) = (lobby.scene.field_object(&s, "source"), lobby.scene.field_object(&s, "destination")) {
            lobby.active[destination] = lobby.active[source];
        }
    }
}
