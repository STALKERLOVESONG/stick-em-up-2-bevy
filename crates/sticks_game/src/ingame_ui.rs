//! The Permanent System's `In-Game UI` screens as the original runs them: the pause menu
//! "Paused (Offline)", "Tips", "End of Level" and "Game Over" (drawn by `hud` with the HUD's
//! camera).
//!
//! * `InGameMenuHandler`: Escape (`KeyboardAndMouseMenuInput`: `Activate` = `Dismiss` =
//!   `GetKeyDown(Escape)`) or a gamepad (`XboxishGamepadMenuInput`: Start activates; B, Back or
//!   Start dismiss) fades out the open menu, or opens the offline menu when no tips,
//!   game-over, end-of-level or end-of-versus screen is up; its `closeButtons` ("Resume Game") fade their
//!   menu out; each menu's "Show Tips" fades the menu out and then shows the tips;
//!   `CloseAllMenusImmediately` switches every menu and the tips off and sets
//!   `Time.timeScale` to 1.
//! * `Pause` ("Paused (Offline)", and "Tips" via `EnableBehaviourWhenOffline`): `timeScale` 0
//!   while active, 1 when switched off.
//! * `FadeInUI` / `FadeOutUI` (`sticks_sim::level_end::Fade`) scale the alpha of every
//!   `tk2dSlicedSprite` and `tk2dTextMesh` below the screen (their alphas as loaded);
//!   `FadeOutUIOnButtonPress` ("Okay" on the tips).
//! * `Button` with mouse events on the `BoxCollider` `CopyBoxColliderFromChild` copied from
//!   "Background" (down, enter/exit, up as button), the pressed z tween and
//!   `ScaleProportionallyToLocalPositionZ`; `BackButton` (Escape) on the tips' "Okay";
//!   `RestartLevelButton`, `ReturnToLobbyButton`, `LevelCompleteButton` (→ `menu::LoadRequest`).
//! * `LevelTips.StartLevel` / `ShowTips` ("Tip Text", "View Again Later").
//! * `ScoreCalculator` on "End of Level" (rows, box resizing, tick-on; `sticks_sim::level_end`).
//! * "End of Versus Game" (`VersusScoreTable`: frags against each player, totals, winner) and
//!   "End of Team Versus Game" (`TeamVersusScoreTable`), shown by `versus`.
//!
//! Not reproduced: the server/client menus (networking), gamepad highlighting
//! (`Button.highlighted`, `LobbyInput` moves), `OnApplicationPause` / the Steam overlay opening
//! the menu, `MouseCursorManager`'s cursor hiding.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::level_end::{COLUMNS, Fade, PlayerId, PlayerResult, Score, ScoreBoard, button_z, calculate_scores, choose_tip, scale_for_local_z};
use sticks_sim::level_end::{CAMPAIGN_DEFAULT_TIP, VERSUS_DEFAULT_TIP, WORLD_UNITS_TO_BOX_PIXELS};

use crate::hud::{HudLayout, HudScene};
use crate::level::ActiveLevel;
use crate::lobby::LobbyMemory;
use crate::menu::{LoadRequest, TimeScale};
use crate::state::{AppState, GameMode, Session};

/// `Application.targetFrameRate` is never set offline on desktop.
/// TODO(UNCERTAIN): `TargetFPSSetter` may set it on some platforms; -1 (→ 60) is assumed.
const EXPECTED_FRAME_RATE: i32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// Enable the `FadeOutUI` of this object.
    FadeOut(usize),
    /// `InGameMenuHandler.ShowTips(menu)`.
    ShowTips(usize),
    Restart,
    ReturnToLobby,
    LevelCompleted,
    /// `InGameDisconnectButton`: leave the network game (or stop hosting it).
    Disconnect,
}

struct UiButton {
    object: usize,
    /// `CopyBoxColliderFromChild`: the child's `BoxCollider` (centre, size) on the button.
    collider: Option<([f32; 3], [f32; 3])>,
    /// `pressedBackgroundZOffset`.
    pressed_offset: f32,
    tween_velocity: f32,
    mouse_down: bool,
    mouse_entered: bool,
    /// `ScaleProportionallyToLocalPositionZ`.
    scales_with_z: bool,
    /// `BackButton`.
    back: bool,
    actions: Vec<Action>,
}

#[derive(Clone, Copy)]
enum Part {
    Sliced(usize),
    Text(usize),
}

/// An object with `FadeInUI` and/or `FadeOutUI` (one `ComponentCollection` each, alike).
struct Faded {
    object: usize,
    parts: Vec<(Part, f32)>,
    has_fade_in: bool,
    has_fade_out: bool,
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
    /// `FadeOutUI.enabled`.
    fade_out_enabled: bool,
}

/// `ScoreCalculator`'s references and its standard layout (taken in `Awake`).
struct ScoreScreen {
    rows: Vec<usize>,
    row_labels: Vec<[Option<usize>; 5]>,
    team_row: Option<usize>,
    team_labels: [Option<usize>; 4],
    contents: Option<usize>,
    bottom: Option<usize>,
    border: Option<usize>,
    standard_border_height: f32,
    board: Option<ScoreBoard>,
}

/// A player row of the end-of-versus tables (`Player.ActivePlayers` sorted by index).
pub struct VersusRow {
    pub index: usize,
    pub team: usize,
    /// `PlayerSlotAssignments.SlotPlayerID(i).GetName()`.
    pub name: String,
    /// `Player.Colors.Body`.
    pub color: [f32; 4],
}

/// What `VersusScoreTable.OnEnable` / `TeamVersusScoreTable.OnEnable` read.
pub struct VersusReport {
    pub rows: Vec<VersusRow>,
    pub definitive: sticks_sim::versus::DefinitiveFrags,
    pub teams: sticks_sim::lobby::LobbyTeams,
}

/// What `ScoreCalculator.OnEnable` reads from the level.
pub struct EndOfLevelReport {
    /// One per active player: the numbers, the name label text and the body colour.
    pub players: Vec<(PlayerResult, String, [f32; 4])>,
    /// `LevelTimer.LevelTime`.
    pub level_time: Option<f32>,
    /// `MaximumDamageCalculator.MaximumDamage`.
    pub maximum_damage: f32,
}

#[derive(Resource)]
pub(crate) struct InGameUi {
    offline_menu: Option<usize>,
    /// `InGameMenuHandler.serverMenu` / `clientMenu` (network games).
    server_menu: Option<usize>,
    client_menu: Option<usize>,
    tips: Option<usize>,
    end_of_level: Option<usize>,
    game_over: Option<usize>,
    /// `EndOfVersusGameUIActivator.EndOfVersusGameUI` / `EndOfTeamVersusGameUI`.
    end_of_versus: Option<usize>,
    end_of_team_versus: Option<usize>,
    /// `InGameMenuHandler.allMenus` (offline, server, client).
    all_menus: Vec<usize>,
    faded: Vec<Faded>,
    /// Objects with an enabled `Pause`.
    pauses: Vec<usize>,
    buttons: Vec<UiButton>,
    /// The button the left mouse button went down on.
    pressed: Option<usize>,
    /// `ShowTips` waiting for this menu's fade-out.
    tips_after: Option<usize>,
    tip_text: Option<usize>,
    view_again: Option<usize>,
    score: Option<ScoreScreen>,
    /// This frame's unscaled delta (the first coroutine step runs inside `OnEnable`).
    dt: f32,
}

pub struct InGameUiPlugin;

/// The screens' per-frame update.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiSet;

impl Plugin for InGameUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), (setup_ui, start_level).chain().after(crate::hud::load_hud))
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| commands.remove_resource::<InGameUi>())
            .add_systems(
                Update,
                update_ui.in_set(UiSet).after(HudLayout).before(crate::unity_render::QueueSet::Draw).run_if(in_state(AppState::InGame)).run_if(resource_exists::<InGameUi>).run_if(resource_exists::<HudScene>),
            );
    }
}

fn find_named(hud: &HudScene, root: usize, name: &str) -> Option<usize> {
    hud.subtree(root).into_iter().find(|&o| hud.scene.objects[o].name == name)
}

/// `InGameMenuHandler.Awake`, the screens' `Awake`s (collections, buttons, score references).
fn setup_ui(mut commands: Commands, hud: Option<Res<HudScene>>, net: Res<crate::net::Net>) {
    let Some(hud) = hud else { return };
    let Some(root) = hud.in_game_ui else {
        warn!("Permanent System has no In-Game UI: no pause menu");
        return;
    };
    let child = |name: &str| hud.scene.child(root, name);
    let handler = hud.scene.scripts_named("InGameMenuHandler").next();
    let menu = |field: &str| handler.and_then(|h| hud.scene.field_object(h, field));
    let offline_menu = menu("offlineMenu").or_else(|| child("Paused (Offline)"));
    let (server_menu, client_menu) = (menu("serverMenu"), menu("clientMenu"));
    let all_menus: Vec<usize> = [offline_menu, server_menu, client_menu].into_iter().flatten().collect();
    let tips = child("Tips");
    let end_of_level = child("End of Level");
    let game_over = child("Game Over");
    let end_of_versus = child("End of Versus Game");
    let end_of_team_versus = child("End of Team Versus Game");
    let ui_subtree = hud.subtree(root);

    // `FadeInUI.ComponentCollection`: every sliced sprite and text below, alpha as loaded.
    let mut faded = Vec::new();
    for &object in &ui_subtree {
        let has_fade_in = hud.scene.script(object, "FadeInUI").is_some_and(|s| s.enabled);
        let fade_out = hud.scene.script(object, "FadeOutUI");
        if !has_fade_in && fade_out.is_none() {
            continue;
        }
        let mut parts = Vec::new();
        for o in hud.subtree(object) {
            if let Some(&i) = hud.sliced_of.get(&o) {
                parts.push((Part::Sliced(i), hud.sliced[i].color[3]));
            }
        }
        for o in hud.subtree(object) {
            if let Some(&i) = hud.text_of.get(&o) {
                parts.push((Part::Text(i), hud.texts[i].color[3]));
            }
        }
        faded.push(Faded { object, parts, has_fade_in, has_fade_out: fade_out.is_some(), fade_in: None, fade_out: None, fade_out_enabled: fade_out.is_some_and(|s| s.enabled) });
    }
    // `Pause` (enabled offline on the tips by `EnableBehaviourWhenOffline`: a network game keeps
    // running behind them).
    let offline = net.is_offline();
    let pauses: Vec<usize> = ui_subtree
        .iter()
        .copied()
        .filter(|&o| hud.scene.script(o, "Pause").is_some_and(|s| s.enabled || (offline && hud.scene.script(o, "EnableBehaviourWhenOffline").is_some())))
        .collect();

    // Buttons and what pressing them does.
    let mut buttons = Vec::new();
    for s in hud.scene.scripts_named("Button") {
        let object = s.object;
        if !ui_subtree.contains(&object) {
            continue;
        }
        let background = hud.scene.child(object, "Background").unwrap_or(object);
        let collider = hud
            .subtree(object)
            .into_iter()
            .find_map(|o| hud.scene.boxes.iter().find(|b| b.object == o))
            .map(|b| (b.center, b.size));
        let has = |class: &str| hud.scene.script(object, class).is_some();
        let mut actions = Vec::new();
        if has("RestartLevelButton") {
            actions.push(Action::Restart);
        }
        if has("ReturnToLobbyButton") {
            actions.push(Action::ReturnToLobby);
        }
        if has("LevelCompleteButton") {
            actions.push(Action::LevelCompleted);
        }
        if has("InGameDisconnectButton") {
            actions.push(Action::Disconnect);
        }
        buttons.push(UiButton {
            object,
            collider,
            pressed_offset: -0.04 - hud.scene.objects[background].local_position[2],
            tween_velocity: 0.0,
            mouse_down: false,
            mouse_entered: false,
            scales_with_z: has("ScaleProportionallyToLocalPositionZ"),
            back: has("BackButton"),
            actions,
        });
    }
    let mut add_action = |button: usize, action: Action| {
        if let Some(b) = buttons.iter_mut().find(|b| b.object == button) {
            b.actions.push(action);
        }
    };
    // `closeButtons`: fade out `button.transform.parent.parent`.
    if let Some(h) = handler {
        for button in hud.scene.field_objects(h, "closeButtons").into_iter().flatten() {
            let menu = hud.scene.objects[button].parent.and_then(|p| hud.scene.objects[p].parent);
            if let Some(menu) = menu {
                add_action(button, Action::FadeOut(menu));
            }
        }
    }
    for &menu in &all_menus {
        if let Some(show) = find_named(&hud, menu, "Show Tips") {
            add_action(show, Action::ShowTips(menu));
        }
    }
    for s in hud.scene.scripts_named("FadeOutUIOnButtonPress") {
        if !ui_subtree.contains(&s.object) {
            continue;
        }
        let button = hud.scene.field_object(s, "button").unwrap_or(s.object);
        let target = hud.scene.field_object(s, "fadeOutUI").unwrap_or(s.object);
        add_action(button, Action::FadeOut(target));
    }

    // `ScoreCalculator.Awake`.
    let score = end_of_level.and_then(|eol| hud.scene.script(eol, "ScoreCalculator")).map(|s| {
        let rows: Vec<usize> = hud.scene.field_objects(s, "playerRows").into_iter().flatten().collect();
        let labels = |row: usize, names: &[&str]| names.iter().map(|n| hud.scene.child(row, n)).collect::<Vec<_>>();
        let row_labels = rows
            .iter()
            .map(|&r| {
                let l = labels(r, &["Name", "Damage", "Health", "Time", "Total"]);
                [l[0], l[1], l[2], l[3], l[4]]
            })
            .collect();
        let team_row = hud.scene.field_object(s, "teamRow");
        let team_labels = team_row.map_or([None; 4], |t| {
            let l = labels(t, &["Damage", "Health", "Time", "Total"]);
            [l[0], l[1], l[2], l[3]]
        });
        let border = hud.scene.field_object(s, "boxBorder").and_then(|o| hud.sliced_of.get(&o).copied());
        ScoreScreen {
            standard_border_height: border.map_or(0.0, |b| hud.sliced[b].dimensions[1]),
            rows,
            row_labels,
            team_row,
            team_labels,
            contents: hud.scene.field_object(s, "boxContents"),
            bottom: hud.scene.field_object(s, "bottomStuff"),
            border,
            board: None,
        }
    });

    let tip_text = tips.and_then(|t| find_named(&hud, t, "Tip Text"));
    let view_again = tips.and_then(|t| find_named(&hud, t, "View Again Later"));
    info!("in-game UI: {} menus, {} buttons, {} fading screens", all_menus.len(), buttons.len(), faded.len());
    commands.insert_resource(InGameUi {
        offline_menu,
        server_menu,
        client_menu,
        tips,
        end_of_level,
        game_over,
        end_of_versus,
        end_of_team_versus,
        all_menus,
        faded,
        pauses,
        buttons,
        pressed: None,
        tips_after: None,
        tip_text,
        view_again,
        score,
        dt: 0.0,
    });
}

impl InGameUi {
    fn faded_index(&self, object: usize) -> Option<usize> {
        self.faded.iter().position(|f| f.object == object)
    }

    fn apply_alpha(&self, hud: &mut HudScene, index: usize, visibility: f32) {
        for &(part, target) in &self.faded[index].parts {
            match part {
                Part::Sliced(i) => hud.sliced[i].color[3] = visibility * target,
                Part::Text(i) => hud.texts[i].color[3] = visibility * target,
            }
        }
    }

    /// The pause menu of a network game: the host's or a client's.
    fn network_menu(&self, host: bool) -> Option<usize> {
        if host { self.server_menu } else { self.client_menu }
    }

    /// `GameObject.SetActive`: `OnEnable` / `OnDisable` of the scripts on that object.
    pub fn set_active(&mut self, hud: &mut HudScene, time: &mut TimeScale, object: usize, active: bool) {
        if hud.active[object] == active {
            return;
        }
        hud.active[object] = active;
        let index = self.faded_index(object);
        if active {
            if let Some(i) = index {
                // `FadeInUI.OnEnable` → `StartCoroutine(FadeIn())` runs to its first `yield`.
                if self.faded[i].has_fade_in {
                    let mut fade = Fade::fade_in();
                    let v = fade.step(self.dt);
                    self.apply_alpha(hud, i, v);
                    self.faded[i].fade_in = Some(fade);
                }
                if self.faded[i].fade_out_enabled {
                    self.start_fade_out(hud, i);
                }
            }
            if self.pauses.contains(&object) {
                time.scale = 0.0;
            }
        } else {
            // Coroutines stop with their object.
            if let Some(i) = index {
                self.faded[i].fade_in = None;
                self.faded[i].fade_out = None;
            }
            if self.pauses.contains(&object) {
                time.scale = 1.0;
            }
        }
    }

    fn start_fade_out(&mut self, hud: &mut HudScene, i: usize) {
        let mut fade = Fade::fade_out();
        let v = fade.step(self.dt);
        self.apply_alpha(hud, i, v);
        self.faded[i].fade_out = Some(fade);
    }

    /// `fadeOutUI.enabled = true`.
    fn enable_fade_out(&mut self, hud: &mut HudScene, object: usize) {
        let Some(i) = self.faded_index(object) else { return };
        if !self.faded[i].has_fade_out || self.faded[i].fade_out_enabled {
            return;
        }
        self.faded[i].fade_out_enabled = true;
        if hud.active_in_hierarchy(object) {
            self.start_fade_out(hud, i);
        }
    }

    /// `InGameMenuHandler.CloseAllMenusImmediately`.
    pub fn close_all_menus_immediately(&mut self, hud: &mut HudScene, time: &mut TimeScale) {
        let menus: Vec<usize> = self.all_menus.iter().copied().chain(self.tips).collect();
        for menu in menus {
            if let Some(i) = self.faded_index(menu) {
                self.faded[i].fade_out_enabled = false;
                self.faded[i].fade_out = None;
            }
            self.set_active(hud, time, menu, false);
        }
        time.scale = 1.0;
    }

    /// `LevelTips.ShowTips`.
    fn show_tips(&mut self, hud: &mut HudScene, time: &mut TimeScale, include_menu_hint: bool) {
        if let Some(hint) = self.view_again {
            hud.active[hint] = include_menu_hint;
        }
        if let Some(tips) = self.tips {
            self.set_active(hud, time, tips, true);
        }
    }

    /// `GameOverUIActivator.GameOver`.
    pub fn game_over(&mut self, hud: &mut HudScene, time: &mut TimeScale) {
        self.close_all_menus_immediately(hud, time);
        if let Some(o) = self.game_over {
            self.set_active(hud, time, o, true);
        }
    }

    /// `GameOverUIActivator.GameOverUI.SetActive(false)` (`DisableGameOvering`).
    pub fn hide_game_over(&mut self, hud: &mut HudScene, time: &mut TimeScale) {
        if let Some(o) = self.game_over {
            self.set_active(hud, time, o, false);
        }
    }

    /// `EndOfLevelUIActivator.Start` after its waits: `CloseAllMenusImmediately`, then the
    /// screen (`FadeInUI.OnEnable`, `ScoreCalculator.OnEnable`). Returns the score to record.
    pub fn show_end_of_level(&mut self, hud: &mut HudScene, time: &mut TimeScale, report: &EndOfLevelReport, player_count: usize) -> Option<Score> {
        self.close_all_menus_immediately(hud, time);
        let eol = self.end_of_level?;
        self.set_active(hud, time, eol, true);
        let screen = self.score.as_mut()?;
        let results: Vec<PlayerResult> = report.players.iter().map(|p| p.0).collect();
        let (scores, team) = calculate_scores(report.level_time, report.maximum_damage, &results, player_count);
        // Player names in score order (by player index), coloured like the players.
        let mut named: Vec<&(PlayerResult, String, [f32; 4])> = report.players.iter().collect();
        named.sort_by_key(|p| p.0.player_index);
        for (row, (_, name, color)) in named.iter().enumerate() {
            if let Some(label) = screen.row_labels.get(row).and_then(|l| l[0]) {
                hud.set_text(label, name);
                hud.set_text_color(label, *color);
            }
        }
        // Rows: one per player, the team row with more than one; the box shrinks around them.
        let objects = &hud.scene.objects;
        let mut shift = 0.0f32;
        for (i, &row) in screen.rows.iter().enumerate() {
            let shown = i < scores.len();
            hud.active[row] = shown;
            if !shown && i != 0 {
                shift += objects[screen.rows[i - 1]].local_position[1] - objects[row].local_position[1];
            }
        }
        let team_shown = scores.len() > 1;
        if let Some(team_row) = screen.team_row {
            hud.active[team_row] = team_shown;
            let standard = objects[team_row].local_position[1];
            if team_shown {
                hud.local_position[team_row][1] = standard + shift;
            } else if let Some(&last) = screen.rows.last() {
                shift += objects[last].local_position[1] - standard;
            }
        }
        if let Some(bottom) = screen.bottom {
            hud.local_position[bottom][1] = hud.scene.objects[bottom].local_position[1] + shift;
        }
        if let Some(contents) = screen.contents {
            hud.local_position[contents][1] = hud.scene.objects[contents].local_position[1] - shift / 2.0;
        }
        if let Some(border) = screen.border {
            hud.sliced[border].dimensions[1] = screen.standard_border_height - shift * WORLD_UNITS_TO_BOX_PIXELS;
        }
        let ids: Vec<PlayerId> = scores.iter().map(|s| PlayerId::anonymous_local(s.player_index.max(0) as usize)).collect();
        let score = Score::new(ids, team.total(), team.is_golden(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()));
        let board = ScoreBoard::new(scores, team, EXPECTED_FRAME_RATE);
        screen.board = Some(board);
        self.write_score_labels(hud, true);
        Some(score)
    }

    /// `EndOfVersusGameUIActivator.Start` after its wait: `CloseAllMenusImmediately`, then the
    /// screen (`FadeInUI.OnEnable`, then `VersusScoreTable.OnEnable` or
    /// `TeamVersusScoreTable.OnEnable`).
    pub fn show_end_of_versus(&mut self, hud: &mut HudScene, time: &mut TimeScale, team_versus: bool, report: &VersusReport) {
        self.close_all_menus_immediately(hud, time);
        let Some(screen) = (if team_versus { self.end_of_team_versus } else { self.end_of_versus }) else { return };
        self.set_active(hud, time, screen, true);
        if team_versus {
            team_versus_table(hud, screen, report);
        } else {
            versus_table(hud, screen, report);
        }
    }

    /// The cells' texts; their colours on `Reset` (`first`) and when a cell turns gold, at full
    /// alpha (the fade scales the alpha again on the frames it runs).
    fn write_score_labels(&self, hud: &mut HudScene, first: bool) {
        let Some(screen) = &self.score else { return };
        let Some(board) = &screen.board else { return };
        for row in 0..board.scores.len() {
            for (c, column) in COLUMNS.into_iter().enumerate() {
                let Some(label) = screen.row_labels.get(row).and_then(|l| l[c + 1]) else { continue };
                let (text, color) = board.cell(row, column);
                hud.set_text(label, &text);
                set_cell_color(hud, label, color, first);
            }
        }
        for (c, column) in COLUMNS.into_iter().enumerate() {
            let Some(label) = screen.team_labels[c] else { continue };
            let (text, color) = board.team_cell(column);
            hud.set_text(label, &text);
            set_cell_color(hud, label, color, first);
        }
    }
}

/// `VersusScoreTable.OnEnable`: the frags-against matrix (with more than two players), the
/// totals and the winner, in a box shrunk around what shows.
fn versus_table(hud: &mut HudScene, screen: usize, report: &VersusReport) {
    use sticks_sim::versus::{ScoreTableLayout, ScoreTableStandard, winner_header};
    let Some(s) = hud.scene.script(screen, "VersusScoreTable").cloned() else { return };
    let rows: Vec<usize> = hud.scene.field_objects(&s, "playerRows").into_iter().flatten().collect();
    let field = |name: &str| hud.scene.field_object(&s, name);
    let (Some(heading), Some(contents), Some(bottom), Some(border), Some(header), Some(table)) =
        (field("headingRow"), field("boxContents"), field("bottomStuff"), field("boxBorder"), field("header"), field("table"))
    else {
        return;
    };
    if rows.len() < 4 {
        return;
    }
    // `GetTableCell(column, row)`: row -1 is the heading row; column -1 "Name", 4 "Total".
    let cell = |hud: &HudScene, column: i32, row: i32| -> Option<usize> {
        let parent = if row == -1 { heading } else { *rows.get(row as usize)? };
        let name = match column {
            -1 => "Name".to_owned(),
            4 => "Total".to_owned(),
            c => c.to_string(),
        };
        hud.scene.child(parent, &name)
    };
    // `Awake`'s standard layout (the scene's values).
    let initial = |o: usize| hud.scene.objects[o].local_position;
    let column_x = [0, 1, 2, 3].map(|c| cell(hud, c, 0).map_or(0.0, |o| initial(o)[0]));
    let rows_parent_y = hud.scene.objects[rows[0]].parent.map_or(0.0, |p| initial(p)[1]);
    let border_sliced = hud.sliced_of.get(&border).copied();
    let standard = ScoreTableStandard {
        column_x,
        total_x: cell(hud, 4, 0).map_or(0.0, |o| initial(o)[0]),
        row_y: [0, 1, 2, 3].map(|r| initial(rows[r])[1]),
        heading_y: initial(heading)[1],
        rows_parent_y,
        table: [initial(table)[0], initial(table)[1]],
        bottom_stuff_y: initial(bottom)[1],
        box_contents_y: initial(contents)[1],
        // Nothing resizes the box before this screen (the HUD is loaded with each level).
        border: border_sliced.map_or([0.0, 0.0], |b| hud.sliced[b].dimensions),
    };
    let count = report.rows.len();
    let layout = ScoreTableLayout::new(&standard, count);
    for (i, &row) in rows.iter().enumerate() {
        hud.active[row] = layout.rows[i];
        for j in -1..4 {
            if let Some(o) = cell(hud, i as i32, j) {
                hud.active[o] = layout.columns[i];
            }
        }
    }
    hud.active[heading] = layout.heading;
    for k in -1..4 {
        if let Some(o) = cell(hud, 4, k) {
            hud.local_position[o][0] = layout.total_x;
        }
    }
    hud.local_position[table] = [layout.table[0], layout.table[1], 0.0];
    hud.local_position[bottom][1] = layout.bottom_stuff_y;
    hud.local_position[contents][1] = layout.box_contents_y;
    if let Some(b) = border_sliced {
        hud.sliced[b].dimensions = layout.border;
    }
    let order: Vec<usize> = report.rows.iter().map(|r| r.index).collect();
    let mut totals = Vec::new();
    for (r, row) in report.rows.iter().enumerate() {
        for o in [cell(hud, -1, r as i32), cell(hud, r as i32, -1)].into_iter().flatten() {
            hud.set_text(o, &row.name);
            hud.set_text_color(o, row.color);
        }
        for (c, &against) in order.iter().enumerate() {
            if let Some(o) = cell(hud, c as i32, r as i32) {
                hud.set_text(o, &report.definitive.frags(row.index, against).to_string());
            }
        }
        let total = report.definitive.total(row.index).unwrap_or(0);
        if let Some(o) = cell(hud, 4, r as i32) {
            hud.set_text(o, &total.to_string());
        }
        totals.push((row.name.as_str(), total));
    }
    hud.set_text(header, &winner_header(totals));
}

/// `TeamVersusScoreTable.OnEnable`: each team's frags (every player slot's definitive total),
/// its name coloured like its players, and the winner.
fn team_versus_table(hud: &mut HudScene, screen: usize, report: &VersusReport) {
    use sticks_sim::versus::{team_color, winner_header};
    let Some(s) = hud.scene.script(screen, "TeamVersusScoreTable").cloned() else { return };
    let rows: Vec<usize> = hud.scene.field_objects(&s, "teamRows").into_iter().flatten().collect();
    let Some(header) = hud.scene.field_object(&s, "header") else { return };
    let mut totals = [0i32; 2];
    for player in 0..4 {
        if let Some(team) = report.teams.team_of(player) {
            totals[team] += report.definitive.total(player).unwrap_or(0);
        }
    }
    let mut names: Vec<(String, i32)> = Vec::new();
    for (team, &row) in rows.iter().enumerate().take(2) {
        let name = hud.scene.child(row, "Name");
        let bodies: Vec<[f32; 4]> = report.rows.iter().filter(|r| r.team == team).map(|r| r.color).collect();
        if let (Some(label), Some(color)) = (name, team_color(&bodies)) {
            hud.set_text_color(label, color);
        }
        if let Some(total) = hud.scene.child(row, "Total") {
            hud.set_text(total, &totals[team].to_string());
        }
        let text = name.and_then(|l| hud.text_of.get(&l)).map_or_else(String::new, |&t| hud.texts[t].text.clone());
        names.push((text, totals[team]));
    }
    hud.set_text(header, &winner_header(names.iter().map(|(n, t)| (n.as_str(), *t))));
}

/// `numberCell.color = ...` on `Reset`, and `label.color = GoldenScoreColor` once golden.
fn set_cell_color(hud: &mut HudScene, label: usize, color: [f32; 4], first: bool) {
    let changed = hud.text_color(label).is_some_and(|c| c[..3] != color[..3]);
    if first || changed {
        hud.set_text_color(label, color);
    }
}

/// `LevelLoader.Update` → `LevelTips.StartLevel` when the level has loaded.
fn start_level(
    ui: Option<ResMut<InGameUi>>,
    hud: Option<ResMut<HudScene>>,
    mut time: ResMut<TimeScale>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    mut memory: ResMut<LobbyMemory>,
) {
    let (Some(mut ui), Some(mut hud)) = (ui, hud) else { return };
    let tips = level.0.tips.as_deref().unwrap_or(&[]);
    let in_use = session.control_types_in_use();
    let tip = choose_tip(tips, &in_use);
    let default = if session.mode == GameMode::Campaign { CAMPAIGN_DEFAULT_TIP } else { VERSUS_DEFAULT_TIP };
    let text = tip.map_or(default, |t| t.message.as_str());
    if let Some(label) = ui.tip_text {
        hud.set_text(label, text);
    }
    let Some(tip) = tip else { return };
    // The players the tip applies to: those with one of its control types (any player when it
    // names none).
    let applicable = &tip.applicable_control_types;
    let ids: Vec<PlayerId> = session
        .players
        .iter()
        .filter(|p| !p.devices.is_empty())
        .filter(|p| applicable.is_empty() || p.devices.iter().any(|d| applicable.iter().any(|a| a == d.kind().control_type())))
        .map(|p| PlayerId::anonymous_local(p.index))
        .collect();
    if memory.tips_seen.mark(&level.0.name, &tip.applicable_control_types, &ids) {
        memory.save.tips = true;
        ui.show_tips(&mut hud, &mut time, true);
    }
}

/// One frame of the screens: coroutines, buttons, `BackButton`, `InGameMenuHandler.Update`,
/// the score tick-on.
#[allow(clippy::too_many_arguments)]
fn update_ui(
    mut ui: ResMut<InGameUi>,
    mut hud: ResMut<HudScene>,
    mut time: ResMut<TimeScale>,
    real: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut requests: MessageWriter<LoadRequest>,
    pads: Res<crate::input::Gamepads>,
    mut net: ResMut<crate::net::Net>,
) {
    let ui = &mut *ui;
    let hud = &mut *hud;
    let time = &mut *time;
    ui.dt = real.delta_secs();
    let dt = ui.dt;

    // `FadeIn` / `FadeOut` coroutines.
    for i in 0..ui.faded.len() {
        if let Some(mut fade) = ui.faded[i].fade_in.take() {
            let v = fade.step(dt);
            ui.apply_alpha(hud, i, v);
            if !fade.finished {
                ui.faded[i].fade_in = Some(fade);
            }
        }
        if let Some(mut fade) = ui.faded[i].fade_out.take() {
            let v = fade.step(dt);
            ui.apply_alpha(hud, i, v);
            if fade.finished {
                // `base.enabled = false; gameObject.SetActive(false)`.
                ui.faded[i].fade_out_enabled = false;
                let object = ui.faded[i].object;
                ui.set_active(hud, time, object, false);
            } else {
                ui.faded[i].fade_out = Some(fade);
            }
        }
    }
    // `InGameMenuHandler.ShowTips`: once the menu's `FadeOutUI` has switched itself off.
    if let Some(menu) = ui.tips_after {
        let fading = ui.faded_index(menu).is_some_and(|i| ui.faded[i].fade_out_enabled);
        if !fading {
            ui.tips_after = None;
            ui.show_tips(hud, time, false);
        }
    }

    // Mouse events on the buttons' box colliders (the UI camera is orthographic, looking +z).
    let cursor = windows.single().ok().and_then(|w| {
        let p = w.cursor_position()?;
        let (width, height) = (w.width(), w.height());
        (width > 0.0 && height > 0.0).then(|| {
            let x = hud.camera[0] + (p.x / width * 2.0 - 1.0) * hud.ortho * hud.aspect;
            let y = hud.camera[1] + (1.0 - p.y / height * 2.0) * hud.ortho;
            [x, y]
        })
    });
    let mut hit: Option<(usize, f32)> = None;
    for (k, b) in ui.buttons.iter().enumerate() {
        let (Some(point), Some((center, size))) = (cursor, b.collider) else { continue };
        if !hud.active_in_hierarchy(b.object) {
            continue;
        }
        let world = hud.world(b.object);
        let local = world.inverse().transform_point([point[0], point[1], world.t[2]]);
        let inside = (0..2).all(|a| (local[a] - center[a]).abs() <= size[a] * 0.5);
        if inside && hit.is_none_or(|(_, z)| world.t[2] < z) {
            hit = Some((k, world.t[2]));
        }
    }
    let hit = hit.map(|(k, _)| k);
    let mut presses: Vec<usize> = Vec::new();
    for (k, b) in ui.buttons.iter_mut().enumerate() {
        b.mouse_entered = hit == Some(k);
    }
    if mouse.just_pressed(MouseButton::Left) {
        if let Some(k) = hit {
            ui.buttons[k].mouse_down = true;
            ui.pressed = Some(k);
        }
    }
    if mouse.just_released(MouseButton::Left) {
        if let Some(k) = ui.pressed.take() {
            ui.buttons[k].mouse_down = false;
            // `OnMouseUpAsButton`: released over the collider it went down on.
            if hit == Some(k) && hud.active_in_hierarchy(ui.buttons[k].object) {
                presses.push(k);
            }
        }
    }
    let escape = keys.just_pressed(KeyCode::Escape);
    for (k, b) in ui.buttons.iter_mut().enumerate() {
        if !hud.active_in_hierarchy(b.object) {
            b.mouse_down = false;
            b.mouse_entered = false;
            continue;
        }
        // `Button.Update`: the pressed tween; `ScaleProportionallyToLocalPositionZ`.
        let z = hud.local_position[b.object][2];
        let down = b.mouse_down && b.mouse_entered;
        let new_z = button_z(z, down, b.pressed_offset, &mut b.tween_velocity, dt);
        hud.local_position[b.object][2] = new_z;
        if b.scales_with_z {
            let s = scale_for_local_z(new_z);
            hud.local_scale[b.object] = [s, s, 1.0];
        }
        // `BackButton.Update`: Escape presses a visible back button.
        if b.back && escape {
            presses.push(k);
        }
    }
    let mut actions: Vec<Action> = Vec::new();
    for k in presses {
        actions.extend(ui.buttons[k].actions.iter().copied());
    }

    // `InGameMenuHandler.Update`: `MenuInput.Activate` / `Dismiss` of every menu input (the
    // keyboard's Escape for both; a gamepad's Start, and B or Back to dismiss).
    let (mut activate, mut dismiss) = (escape, escape);
    for device in pads.devices() {
        if let crate::input::InputDevice::Gamepad(k) = device {
            if let Some(f) = pads.frame(k) {
                let (a, d) = sticks_sim::devices::pad_menu(&f.edges);
                activate |= a;
                dismiss |= d;
            }
        }
    }
    let open_menu = ui.all_menus.iter().copied().find(|&m| hud.active[m]);
    let shown = |o: Option<usize>| o.is_some_and(|o| hud.active[o]);
    match open_menu {
        Some(menu) if dismiss => actions.push(Action::FadeOut(menu)),
        _ if activate && !shown(ui.tips) && !shown(ui.game_over) && !shown(ui.end_of_level) && !shown(ui.end_of_versus) && !shown(ui.end_of_team_versus) => {
            // `NetworkUtility.Offline ? offlineMenu : (!Network.isServer ? clientMenu : serverMenu)`.
            let menu = if net.is_offline() { ui.offline_menu } else { ui.network_menu(net.is_host()) };
            if let Some(menu) = menu {
                ui.set_active(hud, time, menu, true);
            }
        }
        _ => {}
    }

    for action in actions {
        match action {
            Action::FadeOut(object) => ui.enable_fade_out(hud, object),
            Action::ShowTips(menu) => {
                ui.enable_fade_out(hud, menu);
                ui.tips_after = Some(menu);
            }
            Action::Restart => {
                requests.write(LoadRequest::Restart);
            }
            Action::ReturnToLobby => {
                requests.write(LoadRequest::ReturnToLobby);
            }
            Action::LevelCompleted => {
                requests.write(LoadRequest::LevelCompleted);
            }
            Action::Disconnect => {
                // `ClientDisconnection.DeliberateDisconnection = true; Network.Disconnect()`; a
                // client is then sent back to the Lobby (`ClientDisconnection.Update`).
                net.disconnect(true);
                requests.write(LoadRequest::ReturnToLobby);
            }
        }
    }

    // `ScoreCalculator`'s coroutines (after the fade on this frame).
    let ticking = ui.end_of_level.is_some_and(|o| hud.active[o]);
    if ticking {
        if let Some(board) = ui.score.as_mut().and_then(|s| s.board.as_mut()) {
            board.frame();
        }
        ui.write_score_labels(hud, false);
    }
}
