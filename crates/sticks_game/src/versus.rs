//! How a Versus or Team Versus level ends (the rules are `sticks_sim::versus`):
//!
//! * `VersusGameEnder.LateUpdate` (the "Versus Game Ender" of the versus scenes): once a player
//!   (Versus) or a team (Team Versus) has 10 frags, `EndGame` freezes every player's frags
//!   against each player index ([`VersusState::definitive`]; the HUD's score labels and the end
//!   screen read these from then on), sets `GameEnded` (nobody respawns any more) and switches
//!   its "Ender" on.
//! * The Ender's `Start`s: `EndOfVersusGameUIActivator` (1 s of scaled time, then
//!   `CloseAllMenusImmediately` and "End of Versus Game" / "End of Team Versus Game", see
//!   `ingame_ui`), `EndOfLevelCameraMover`, `EndOfLevelTimeSlower`, the `EventSounds` start
//!   sound and `EndOfVersusGameLoserDestroyer` (everything a player of a losing team controls
//!   takes `float.MaxValue` damage).
//!
//! `RemoveOwnerAndServerOnlyParts` does nothing offline.

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use sticks_sim::level_end::{END_OF_LEVEL_UI_DELAY, TimeSlower};
use sticks_sim::versus::{DefinitiveFrags, VersusPlayer, game_should_end, winning_team};
use unity_compat::Vec3 as SimVec3;

use crate::FrameSet;
use crate::camera::{LevelCamera, Rig};
use crate::effects::PlaySound;
use crate::enemy::Enemy;
use crate::figure::{ManArt, Palette};
use crate::helicopter::HelicopterHit;
use crate::hud::{HudLayout, HudScene};
use crate::ingame_ui::{InGameUi, UiSet, VersusReport, VersusRow};
use crate::level::ActiveLevel;
use crate::life::{Damage, Mortal, OnKill, PlayerScore};
use crate::menu::TimeScale;
use crate::player::{ControlledBy, Player};
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::QueueSet;
use crate::vehicles::Piloting;

/// `VersusGameEnder`'s state for the level being played.
#[derive(Resource, Default)]
pub struct VersusState {
    /// `GameEnded`.
    pub ended: bool,
    /// `definitiveFragCounts`.
    pub definitive: DefinitiveFrags,
    /// `EndOfVersusGameUIActivator.Start`: scaled seconds left of `WaitForSeconds(1)`.
    ui_wait: Option<f32>,
    /// `EndOfLevelTimeSlower` of the Ender.
    time_slower: Option<TimeSlower>,
}

pub struct VersusPlugin;

impl Plugin for VersusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(VersusState::default()))
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| commands.remove_resource::<VersusState>())
            // `LateUpdate`.
            .add_systems(Update, versus_late_update.in_set(FrameSet::Present).run_if(resource_exists::<VersusState>))
            .add_systems(
                Update,
                unscaled_versus
                    .after(HudLayout)
                    .after(UiSet)
                    .before(QueueSet::Draw)
                    .run_if(in_state(AppState::InGame))
                    .run_if(resource_exists::<VersusState>),
            );
    }
}

/// `VersusGameEnder.LateUpdate` → `EndGame` → `toActivate.SetActive(true)`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn versus_late_update(
    mut state: ResMut<VersusState>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    players: Query<(Entity, &Player, &PlayerScore)>,
    avatars: Query<(Entity, &ControlledBy, &Mortal, Option<&Piloting>), (Without<Enemy>, Allow<Disabled>)>,
    mut rigs: Query<&mut Rig, With<LevelCamera>>,
    mut damage: MessageWriter<Damage>,
    mut helicopter_hits: MessageWriter<HelicopterHit>,
    mut sounds: MessageWriter<PlaySound>,
    mut net: ResMut<crate::net::Net>,
) {
    if state.ended {
        return;
    }
    let Some(ender) = &level.0.versus_ender else { return };
    // `if (!Network.isServer && !NetworkUtility.Offline) return`: a client ends the game when the
    // host's `EndGame` arrives with the frags it froze (`ReceiveDefinitiveFragCounts`).
    let told = net.take_level_events("VersusEnded").into_iter().next().map(|(_, values)| values);
    // `Player.ActivePlayers` (enabled in spawn order, which is player-index order).
    let mut active: Vec<(Entity, VersusPlayer, [i32; 4])> = players
        .iter()
        .map(|(entity, p, score)| (entity, VersusPlayer { index: p.index, team: p.team, frags: score.0.frags() }, [0, 1, 2, 3].map(|i| score.0.frags_against(i))))
        .collect();
    active.sort_by_key(|(_, p, _)| p.index);
    if net.is_client() {
        let Some(values) = told else { return };
        // Per player: index, then the frags against players 0..3.
        for chunk in values.chunks_exact(5) {
            if let Some((_, p, against)) = active.iter_mut().find(|(_, p, _)| p.index as i32 == chunk[0]) {
                *against = [chunk[1], chunk[2], chunk[3], chunk[4]];
                p.frags = against.iter().sum();
            }
        }
    } else {
        let list: Vec<VersusPlayer> = active.iter().map(|(_, p, _)| *p).collect();
        if !game_should_end(session.mode != GameMode::Versus, &list) {
            return;
        }
        let values = active.iter().flat_map(|(_, p, against)| std::iter::once(p.index as i32).chain(against.iter().copied())).collect();
        net.send_level_event("VersusEnded", 0, values);
    }
    let list: Vec<VersusPlayer> = active.iter().map(|(_, p, _)| *p).collect();
    // `EndGame` (offline: `ReceiveDefinitiveFragCounts`, `AllDefinitiveFragCountsReceived`).
    for (_, p, against) in &active {
        state.definitive.set(p.index, *against);
    }
    state.ended = true;
    info!("versus game over: {:?}", list.iter().map(|p| (p.index + 1, p.frags)).collect::<Vec<_>>());
    let Some(spec) = &ender.ender else { return };
    if spec.versus_ui {
        state.ui_wait = Some(END_OF_LEVEL_UI_DELAY);
    }
    if let Some(distance) = spec.camera_minimum_distance {
        for mut rig in &mut rigs {
            rig.settings.screen_edge_margin = 0.0;
            rig.settings.minimum_on_screen_width = 0.0;
            rig.settings.minimum_distance = distance;
        }
    }
    if spec.time_slower {
        state.time_slower = Some(TimeSlower::default());
    }
    let at = SimVec3::new(spec.position[0], spec.position[1], spec.position[2]);
    for group in &spec.start_sounds {
        sounds.write(PlaySound { group: group.clone(), at: Some(at), owner: None });
    }
    if spec.loser_destroyer {
        // `EndOfVersusGameLoserDestroyer.Start`: `Player.Controlling` of every other team, when
        // its object is active.
        let winner = winning_team(&list);
        for (slot, p, _) in &active {
            if Some(p.team) == winner {
                continue;
            }
            let Some((avatar, _, mortal, piloting)) = avatars.iter().find(|(_, c, _, _)| c.0 == *slot) else { continue };
            match piloting {
                Some(piloting) => {
                    helicopter_hits.write(HelicopterHit { helicopter: piloting.vehicle, amount: f32::MAX, responsible: None });
                }
                None if mortal.avatar_active() => {
                    damage.write(Damage { target: avatar, amount: f32::MAX, on_kill: OnKill::Nothing, responsible: None });
                }
                None => {}
            }
        }
    }
}

/// `EndOfLevelTimeSlower.Update` (unscaled) and `EndOfVersusGameUIActivator`'s coroutine.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn unscaled_versus(
    mut state: ResMut<VersusState>,
    mut ui: Option<ResMut<InGameUi>>,
    mut hud: Option<ResMut<HudScene>>,
    mut time_scale: ResMut<TimeScale>,
    real: Res<Time<Real>>,
    scaled: Res<Time>,
    session: Res<Session>,
    art: Option<Res<ManArt>>,
    players: Query<(Entity, &Player)>,
    avatars: Query<(&ControlledBy, Option<&Palette>), (Without<Enemy>, Allow<Disabled>)>,
) {
    let state = &mut *state;
    if let Some(slower) = state.time_slower.as_mut() {
        time_scale.scale = slower.update(time_scale.scale, real.delta_secs());
    }
    let Some(wait) = state.ui_wait.as_mut() else { return };
    *wait -= scaled.delta_secs();
    if *wait > 0.0 {
        return;
    }
    state.ui_wait = None;
    let (Some(ui), Some(hud)) = (ui.as_deref_mut(), hud.as_deref_mut()) else { return };
    let mut rows: Vec<VersusRow> = players
        .iter()
        .map(|(slot, p)| {
            let palette = avatars.iter().find(|(c, _)| c.0 == slot).and_then(|(_, palette)| palette);
            let body = palette.map_or_else(|| art.as_ref().map_or([255; 4], |a| a.body_color()), |p| p.0.body);
            VersusRow { index: p.index, team: p.team, name: format!("Player {}", p.index + 1), color: body.map(|v| f32::from(v) / 255.0) }
        })
        .collect();
    rows.sort_by_key(|r| r.index);
    let report = VersusReport { rows, definitive: state.definitive.clone(), teams: session.teams.clone() };
    ui.show_end_of_versus(hud, &mut time_scale, session.mode == GameMode::TeamVersus, &report);
}
