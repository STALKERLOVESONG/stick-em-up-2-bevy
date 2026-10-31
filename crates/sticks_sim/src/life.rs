//! Health, lives, score and the death → explosion → respawn cycle.
//!
//! Recovered from `Health`, `Lives`, `PlayerScore`, `Respawner`, `Explodable`,
//! `ExplodeSpritesAndLines`/`Explosion` (timing only), `TellOwnerToExplodeOnHealthExhausted`,
//! `StopRespawningWhenControllerLivesDepleted` and `DamageOtherOnTriggerEnter`. Offline play
//! only: the RPC plumbing around each of those is not modelled.
//!
//! The original wires these together with C# events. Here every operation returns the events it
//! raised, in the order the original raised them, and the Bevy layer turns them into messages.

use crate::rng::GameRng;

pub const MAN_STARTING_HEALTH: f32 = 100.0;
pub const PLAYER_STARTING_LIVES: i32 = 2;
/// `ExplodeSpritesAndLines.fadeWaitTime` on `Man.prefab`.
pub const MAN_EXPLOSION_FADE_WAIT: f32 = 2.0;
/// `Explosion.fadeFadeTime`.
const EXPLOSION_FADE_TIME: f32 = 1.0;

/// `Mathf.Max(a, b)`: `a > b ? a : b` (so a NaN `a` yields `b`).
fn unity_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

// ---------------------------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HealthEvent {
    /// `Health.Damaged(amount)`.
    Damaged(f32),
    /// `Health.Changed`.
    Changed,
    /// The linked `Lives.LifeLost()` call made from inside `Damage`, just before `Exhausted`.
    LifeLost,
    /// `Health.Exhausted`.
    Exhausted,
}

/// Events raised by one `Health` call (at most four).
#[derive(Clone, Copy, Debug)]
pub struct HealthEvents {
    items: [HealthEvent; 4],
    len: usize,
}

impl HealthEvents {
    fn new() -> Self {
        Self { items: [HealthEvent::Changed; 4], len: 0 }
    }

    fn push(&mut self, event: HealthEvent) {
        self.items[self.len] = event;
        self.len += 1;
    }

    pub fn as_slice(&self) -> &[HealthEvent] {
        &self.items[..self.len]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<'a> IntoIterator for &'a HealthEvents {
    type Item = &'a HealthEvent;
    type IntoIter = std::slice::Iter<'a, HealthEvent>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    starting: f32,
    remaining: f32,
    /// `Health.lives != null` (set by `Player.SetUp` in campaign mode).
    lives_linked: bool,
}

impl Health {
    /// `Awake` → `Reset`.
    pub fn new(starting: f32) -> Health {
        Health { starting, remaining: starting, lives_linked: false }
    }

    pub fn starting(&self) -> f32 {
        self.starting
    }

    pub fn remaining(&self) -> f32 {
        self.remaining
    }

    pub fn is_exhausted(&self) -> bool {
        self.remaining <= 0.0
    }

    pub fn normalized(&self) -> f32 {
        self.remaining / self.starting
    }

    pub fn is_linked_to_lives(&self) -> bool {
        self.lives_linked
    }

    /// `Health.LinkTo(lives)`.
    pub fn link_to_lives(&mut self) {
        self.lives_linked = true;
    }

    /// `Health.Damage`. Health never goes below zero (it may exceed the starting value after
    /// negative damage). Damaging an exhausted `Health` raises nothing.
    pub fn damage(&mut self, amount: f32) -> HealthEvents {
        let mut events = HealthEvents::new();
        let was_exhausted = self.is_exhausted();
        self.remaining = unity_max(self.remaining - amount, 0.0);
        if was_exhausted && self.is_exhausted() {
            return events;
        }
        events.push(HealthEvent::Damaged(amount));
        events.push(HealthEvent::Changed);
        if self.is_exhausted() && !was_exhausted {
            if self.lives_linked {
                events.push(HealthEvent::LifeLost);
            }
            events.push(HealthEvent::Exhausted);
        }
        events
    }

    /// `Health.Reset` (also `OnSpawned`): back to `startingHealth`; always raises `Changed`.
    pub fn reset(&mut self) -> HealthEvents {
        self.remaining = self.starting;
        let mut events = HealthEvents::new();
        events.push(HealthEvent::Changed);
        events
    }

    /// `SetStartingHealth.Awake`: replace `startingHealth`, then reset.
    pub fn set_starting_health(&mut self, starting: f32) -> HealthEvents {
        self.starting = starting;
        self.reset()
    }
}

// ---------------------------------------------------------------------------------------------
// Lives
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LivesEvent {
    Changed,
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lives {
    remaining: i32,
}

impl Lives {
    /// `Lives.Awake`: `startingLives - 2 * LevelLoader.Difficulty` (difficulty is -1, 0 or 1).
    pub fn new(starting_lives: i32, difficulty: i32) -> Lives {
        Lives { remaining: starting_lives - 2 * difficulty }
    }

    /// May go negative: below zero is game over.
    /// Development hook (not in the original): set the lives left.
    pub fn set_remaining(&mut self, remaining: i32) {
        self.remaining = remaining;
    }

    pub fn remaining(&self) -> i32 {
        self.remaining
    }

    /// `Lives.LifeLost`.
    pub fn life_lost(&mut self) -> [LivesEvent; 2] {
        self.remaining -= 1;
        [LivesEvent::Changed, LivesEvent::Lost]
    }
}

// ---------------------------------------------------------------------------------------------
// Score
// ---------------------------------------------------------------------------------------------

/// `PlayerScore` (frags only; damage dealt is a plain public field in the original too).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub damage_dealt: f32,
    frags: i32,
    frags_against: [i32; 4],
}

impl Score {
    pub fn frags(&self) -> i32 {
        self.frags
    }

    pub fn frags_against(&self, player_index: usize) -> i32 {
        self.frags_against[player_index]
    }

    /// The frags against every player index.
    pub fn all_frags_against(&self) -> [i32; 4] {
        self.frags_against
    }

    /// `PlayerScore.ReceivePlayerScore`: the server's figures replace this peer's.
    pub fn receive(&mut self, damage_dealt: f32, frags: i32, frags_against: [i32; 4]) {
        self.damage_dealt = damage_dealt;
        self.frags = frags;
        self.frags_against = frags_against;
    }

    pub fn fragged(&mut self, player_index: usize) {
        self.frags += 1;
        self.frags_against[player_index] += 1;
    }

    /// `PlayerScore.SelfFragged`: a suicide costs one frag.
    pub fn self_fragged(&mut self, own_player_index: usize) {
        self.frags -= 1;
        self.frags_against[own_player_index] -= 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Death cycle
// ---------------------------------------------------------------------------------------------

/// What the outside world looks like to the respawner right now.
pub struct RespawnEnv<'a> {
    /// `LevelLoader.Mode == GameMode.Campaign`.
    pub campaign: bool,
    /// `VersusGameEnder.GameEnded` (always false in campaign).
    pub versus_ended: bool,
    /// Children of `/Player Spawn Points`.
    pub spawn_point_count: usize,
    /// `Time.deltaTime` where the call is made (the fixed step inside physics callbacks).
    pub time_delta: f32,
    pub rng: &'a mut GameRng,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LifeEvent {
    Damaged(f32),
    HealthChanged,
    LivesChanged,
    LivesLost,
    Exhausted,
    /// `Respawner.EnablePlaceholder`: the camera keeps framing the spot where the avatar died.
    CameraPlaceholderEnabled,
    /// `Respawner.PlayerGotGameOver`: the controller has no lives left and will not respawn.
    GameOver,
    /// `Explodable.Exploded`: explosion visuals start; the avatar object is deactivated in the
    /// `LateUpdate` of this frame.
    Exploded,
    /// `Explodable.ExplosionFinished`.
    ExplosionFinished,
    /// `Respawner.Respawn`: the avatar object is active again and `OnRespawned` was broadcast.
    /// `spawn_point` is the randomly chosen child of `/Player Spawn Points`; only non-campaign
    /// modes move the avatar to it. Followed by `HealthChanged` (`Health.Reset`).
    Respawned { spawn_point: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Alive,
    /// Dead; the explosion is running and will end in a respawn.
    Exploding,
    /// Dead for good (game over, or the versus game ended while exploding).
    Gone,
}

/// `Explosion`'s timeline: parts drift for `fadeWaitTime`, fade for one second, then it finishes.
#[derive(Clone, Copy, Debug)]
struct ExplosionTimer {
    run_time: f32,
    fade_wait_time: f32,
}

impl ExplosionTimer {
    /// `Explosion.Update`; true once the fade is over (`num > 1`).
    fn update(&mut self, delta_time: f32) -> bool {
        self.run_time += delta_time;
        let num = unity_max(self.run_time - self.fade_wait_time, 0.0) / EXPLOSION_FADE_TIME;
        num > 1.0
    }
}

/// Health plus the `Respawner`'s state machine for one respawnable avatar.
#[derive(Clone, Debug)]
pub struct LifeCycle {
    pub health: Health,
    stage: Stage,
    explosion: Option<ExplosionTimer>,
    /// `Respawner`'s `ExplosionFinished` handler is subscribed (removed on game over).
    respawn_armed: bool,
    /// `StopRespawningWhenControllerLivesDepleted` is present.
    stops_when_lives_depleted: bool,
    fade_wait_time: f32,
    /// `ExplodeSpritesAndLines.explosion != null`: the explosion object is built one frame after
    /// `Start`; before that an explosion finishes instantly without hiding anything.
    explosion_ready: bool,
    deactivate_pending: bool,
    avatar_active: bool,
}

impl LifeCycle {
    pub fn new(health: Health, stops_when_lives_depleted: bool, fade_wait_time: f32) -> LifeCycle {
        LifeCycle {
            health,
            stage: Stage::Alive,
            explosion: None,
            respawn_armed: true,
            stops_when_lives_depleted,
            fade_wait_time,
            explosion_ready: false,
            deactivate_pending: false,
            avatar_active: true,
        }
    }

    /// The `Man` as authored in `Man.prefab`.
    pub fn man() -> LifeCycle {
        LifeCycle::new(Health::new(MAN_STARTING_HEALTH), true, MAN_EXPLOSION_FADE_WAIT)
    }

    /// A character without a `Respawner` (enemies): once its explosion finishes it is gone.
    pub fn without_respawner(mut self) -> LifeCycle {
        self.respawn_armed = false;
        self.stops_when_lives_depleted = false;
        self
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Whether the avatar's `GameObject` is active (it runs, collides, is drawn, influences the camera).
    pub fn avatar_active(&self) -> bool {
        self.avatar_active
    }

    /// The frame after `Start`: `ExplodeSpritesAndLines.Run` has built the explosion.
    pub fn explosion_prepared(&mut self) {
        self.explosion_ready = true;
    }

    /// `Health.Damage` plus every subscriber the avatar has on it, in the order the original ran
    /// them. `lives` is the controller's `Lives` (`ControlSlot.Controller.GetComponent<Lives>()`),
    /// which is also what a linked `Health` charges.
    pub fn damage(&mut self, amount: f32, mut lives: Option<&mut Lives>, env: &mut RespawnEnv<'_>) -> Vec<LifeEvent> {
        let mut out = Vec::new();
        for event in &self.health.damage(amount) {
            match *event {
                HealthEvent::Damaged(a) => out.push(LifeEvent::Damaged(a)),
                HealthEvent::Changed => out.push(LifeEvent::HealthChanged),
                HealthEvent::LifeLost => {
                    if let Some(l) = lives.as_deref_mut() {
                        for e in l.life_lost() {
                            out.push(match e {
                                LivesEvent::Changed => LifeEvent::LivesChanged,
                                LivesEvent::Lost => LifeEvent::LivesLost,
                            });
                        }
                    }
                }
                HealthEvent::Exhausted => {
                    out.push(LifeEvent::Exhausted);
                    self.on_exhausted(lives.as_deref(), env, &mut out);
                }
            }
        }
        out
    }

    /// The two `Health.Exhausted` subscribers: the respawner's and the explosion trigger.
    /// TODO(UNCERTAIN): their relative order depends on `Start` order of two components; nothing
    /// observable depends on it.
    fn on_exhausted(&mut self, lives: Option<&Lives>, env: &mut RespawnEnv<'_>, out: &mut Vec<LifeEvent>) {
        if self.stops_when_lives_depleted && lives.is_some_and(|l| l.remaining() < 0) {
            self.respawn_armed = false;
            out.push(LifeEvent::GameOver);
        } else {
            out.push(LifeEvent::CameraPlaceholderEnabled);
        }
        out.push(LifeEvent::Exploded);
        if self.explosion_ready {
            let mut timer = ExplosionTimer { run_time: 0.0, fade_wait_time: self.fade_wait_time };
            // `Explosion.Go` evaluates `Update()` once immediately.
            timer.update(env.time_delta);
            self.explosion = Some(timer);
            self.stage = Stage::Exploding;
            self.deactivate_pending = true;
        } else {
            // `explodable.Finished()` straight away.
            self.stage = Stage::Exploding;
            self.finish_explosion(env, out);
        }
    }

    /// `Explosion.Update` for this frame (call once per rendered frame, after the fixed steps).
    pub fn tick(&mut self, delta_time: f32, env: &mut RespawnEnv<'_>) -> Vec<LifeEvent> {
        let mut out = Vec::new();
        if self.stage != Stage::Exploding {
            return out;
        }
        if let Some(timer) = self.explosion.as_mut() {
            if timer.update(delta_time) {
                self.explosion = None;
                self.finish_explosion(env, &mut out);
            }
        }
        out
    }

    /// `ExplodeSpritesAndLines.LateUpdate`: hide the avatar if an explosion started this frame.
    pub fn late_update(&mut self) {
        if self.deactivate_pending {
            self.avatar_active = false;
            self.deactivate_pending = false;
        }
    }

    /// `Explodable.ExplosionFinished` → the respawner's handler (if still subscribed).
    fn finish_explosion(&mut self, env: &mut RespawnEnv<'_>, out: &mut Vec<LifeEvent>) {
        out.push(LifeEvent::ExplosionFinished);
        if !self.respawn_armed || !(env.campaign || !env.versus_ended) {
            self.stage = Stage::Gone;
            return;
        }
        let spawn_point = env.rng.range_int(0, env.spawn_point_count as i32).max(0) as usize;
        self.avatar_active = true;
        self.deactivate_pending = false;
        self.stage = Stage::Alive;
        out.push(LifeEvent::Respawned { spawn_point });
        // `Health.Reset` always raises `Changed`.
        self.health.reset();
        out.push(LifeEvent::HealthChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(rng: &mut GameRng) -> RespawnEnv<'_> {
        RespawnEnv { campaign: true, versus_ended: false, spawn_point_count: 3, time_delta: 1.0 / 60.0, rng }
    }

    // --- Health -------------------------------------------------------------------------------

    #[test]
    fn damage_raises_damaged_then_changed() {
        let mut h = Health::new(100.0);
        let e = h.damage(30.0);
        assert_eq!(e.as_slice(), &[HealthEvent::Damaged(30.0), HealthEvent::Changed]);
        assert_eq!(h.remaining(), 70.0);
        assert!((h.normalized() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn lethal_damage_clamps_at_zero_and_raises_exhausted_last() {
        let mut h = Health::new(100.0);
        let e = h.damage(250.0);
        assert_eq!(e.as_slice(), &[HealthEvent::Damaged(250.0), HealthEvent::Changed, HealthEvent::Exhausted]);
        assert_eq!(h.remaining(), 0.0);
        assert!(h.is_exhausted());
    }

    #[test]
    fn linked_health_loses_a_life_between_changed_and_exhausted() {
        let mut h = Health::new(100.0);
        h.link_to_lives();
        let e = h.damage(100.0);
        assert_eq!(
            e.as_slice(),
            &[HealthEvent::Damaged(100.0), HealthEvent::Changed, HealthEvent::LifeLost, HealthEvent::Exhausted]
        );
    }

    #[test]
    fn damaging_exhausted_health_is_silent() {
        let mut h = Health::new(10.0);
        h.damage(10.0);
        assert!(h.damage(5.0).is_empty());
        assert!(h.damage(f32::MAX).is_empty());
        assert_eq!(h.remaining(), 0.0);
    }

    #[test]
    fn non_lethal_hits_never_exhaust_and_zero_damage_still_notifies() {
        let mut h = Health::new(100.0);
        h.link_to_lives();
        assert_eq!(h.damage(0.0).as_slice(), &[HealthEvent::Damaged(0.0), HealthEvent::Changed]);
        assert_eq!(h.damage(99.0).as_slice(), &[HealthEvent::Damaged(99.0), HealthEvent::Changed]);
        assert!(!h.is_exhausted());
    }

    #[test]
    fn negative_damage_heals_without_an_upper_clamp() {
        let mut h = Health::new(100.0);
        h.damage(-50.0);
        assert_eq!(h.remaining(), 150.0);
    }

    #[test]
    fn nan_damage_exhausts_like_mathf_max() {
        // `Mathf.Max(NaN, 0)` is `a > b ? a : b` → 0.
        let mut h = Health::new(100.0);
        let e = h.damage(f32::NAN);
        assert!(h.is_exhausted());
        assert_eq!(e.as_slice().len(), 3);
    }

    #[test]
    fn float_max_damage_is_the_abyss_kill() {
        let mut h = Health::new(100.0);
        assert!(h.damage(f32::MAX).as_slice().contains(&HealthEvent::Exhausted));
    }

    #[test]
    fn reset_restores_and_always_notifies() {
        let mut h = Health::new(100.0);
        h.damage(100.0);
        assert_eq!(h.reset().as_slice(), &[HealthEvent::Changed]);
        assert_eq!(h.remaining(), 100.0);
        assert_eq!(h.reset().as_slice(), &[HealthEvent::Changed]);
    }

    #[test]
    fn set_starting_health_resets_to_the_new_value() {
        let mut h = Health::new(100.0);
        h.set_starting_health(40.0);
        assert_eq!(h.remaining(), 40.0);
        assert_eq!(h.starting(), 40.0);
    }

    // --- Lives / score ------------------------------------------------------------------------

    #[test]
    fn lives_scale_with_difficulty() {
        assert_eq!(Lives::new(2, 0).remaining(), 2);
        assert_eq!(Lives::new(2, -1).remaining(), 4);
        assert_eq!(Lives::new(2, 1).remaining(), 0);
    }

    #[test]
    fn losing_a_life_raises_changed_then_lost() {
        let mut l = Lives::new(2, 0);
        assert_eq!(l.life_lost(), [LivesEvent::Changed, LivesEvent::Lost]);
        assert_eq!(l.remaining(), 1);
    }

    #[test]
    fn suicide_costs_a_frag() {
        let mut s = Score::default();
        s.fragged(1);
        s.self_fragged(0);
        s.self_fragged(0);
        assert_eq!(s.frags(), -1);
        assert_eq!(s.frags_against(0), -2);
        assert_eq!(s.frags_against(1), 1);
    }

    // --- Death cycle --------------------------------------------------------------------------

    fn ready_man() -> LifeCycle {
        let mut man = LifeCycle::man();
        man.explosion_prepared();
        man
    }

    fn run_until_respawn(man: &mut LifeCycle, rng: &mut GameRng, dt: f32) -> (f32, Vec<LifeEvent>) {
        let mut t = 0.0;
        loop {
            t += dt;
            let events = man.tick(dt, &mut env(rng));
            man.late_update();
            if !events.is_empty() {
                return (t, events);
            }
            assert!(t < 10.0, "explosion never finished");
        }
    }

    #[test]
    fn abyss_death_event_order_with_lives_remaining() {
        let mut rng = GameRng::from_seed(1);
        let mut lives = Lives::new(2, 0);
        let mut man = ready_man();
        man.health.link_to_lives();
        let events = man.damage(f32::MAX, Some(&mut lives), &mut env(&mut rng));
        assert_eq!(
            events,
            vec![
                LifeEvent::Damaged(f32::MAX),
                LifeEvent::HealthChanged,
                LifeEvent::LivesChanged,
                LifeEvent::LivesLost,
                LifeEvent::Exhausted,
                LifeEvent::CameraPlaceholderEnabled,
                LifeEvent::Exploded,
            ]
        );
        assert_eq!(lives.remaining(), 1);
        assert_eq!(man.stage(), Stage::Exploding);
        // Still active until the end of the frame (`ExplodeSpritesAndLines.LateUpdate`).
        assert!(man.avatar_active());
        man.late_update();
        assert!(!man.avatar_active());
    }

    #[test]
    fn respawn_comes_after_wait_plus_fade_and_resets_health() {
        let mut rng = GameRng::from_seed(2);
        let mut lives = Lives::new(2, 0);
        let mut man = ready_man();
        man.health.link_to_lives();
        man.damage(f32::MAX, Some(&mut lives), &mut env(&mut rng));
        man.late_update();
        let dt = 1.0 / 60.0;
        let (t, events) = run_until_respawn(&mut man, &mut rng, dt);
        // 2 s wait + 1 s fade, minus the fixed step the explosion already consumed in `Go`.
        assert!((t - 3.0).abs() < 0.05, "respawned after {t} s");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], LifeEvent::ExplosionFinished);
        assert!(matches!(events[1], LifeEvent::Respawned { spawn_point } if spawn_point < 3));
        assert_eq!(events[2], LifeEvent::HealthChanged);
        assert!(man.avatar_active());
        assert_eq!(man.stage(), Stage::Alive);
        assert_eq!(man.health.remaining(), 100.0);
    }

    #[test]
    fn no_respawn_before_the_explosion_ends() {
        let mut rng = GameRng::from_seed(3);
        let mut man = ready_man();
        man.damage(f32::MAX, None, &mut env(&mut rng));
        man.late_update();
        for _ in 0..170 {
            assert!(man.tick(1.0 / 60.0, &mut env(&mut rng)).is_empty());
        }
        assert!(!man.avatar_active());
        assert!(man.health.is_exhausted());
    }

    #[test]
    fn running_out_of_lives_is_game_over_with_no_respawn() {
        let mut rng = GameRng::from_seed(4);
        let mut lives = Lives::new(2, 0);
        let mut man = ready_man();
        man.health.link_to_lives();
        for death in 0..3 {
            let events = man.damage(f32::MAX, Some(&mut lives), &mut env(&mut rng));
            man.late_update();
            let last = death == 2;
            assert_eq!(events.contains(&LifeEvent::GameOver), last, "death {death}");
            assert_eq!(events.contains(&LifeEvent::CameraPlaceholderEnabled), !last, "death {death}");
            if !last {
                let (_, e) = run_until_respawn(&mut man, &mut rng, 1.0 / 60.0);
                assert!(e.iter().any(|e| matches!(e, LifeEvent::Respawned { .. })));
            }
        }
        assert_eq!(lives.remaining(), -1);
        for _ in 0..400 {
            assert!(!man.tick(1.0 / 60.0, &mut env(&mut rng)).iter().any(|e| matches!(e, LifeEvent::Respawned { .. })));
        }
        assert!(!man.avatar_active());
    }

    #[test]
    fn without_the_stop_component_lives_are_ignored() {
        let mut rng = GameRng::from_seed(5);
        let mut lives = Lives::new(2, 1); // 0 lives; one loss → -1
        let mut man = LifeCycle::new(Health::new(100.0), false, 2.0);
        man.explosion_prepared();
        man.health.link_to_lives();
        let events = man.damage(100.0, Some(&mut lives), &mut env(&mut rng));
        assert!(!events.contains(&LifeEvent::GameOver));
        assert!(events.contains(&LifeEvent::CameraPlaceholderEnabled));
    }

    #[test]
    fn versus_respawn_is_suppressed_once_the_game_has_ended() {
        let mut rng = GameRng::from_seed(6);
        let mut man = ready_man();
        let mut versus = RespawnEnv { campaign: false, versus_ended: false, spawn_point_count: 4, time_delta: 1.0 / 60.0, rng: &mut rng };
        man.damage(100.0, None, &mut versus);
        man.late_update();
        versus.versus_ended = true;
        let mut finished = false;
        for _ in 0..400 {
            let e = man.tick(1.0 / 60.0, &mut versus);
            assert!(!e.iter().any(|e| matches!(e, LifeEvent::Respawned { .. })));
            finished |= e.contains(&LifeEvent::ExplosionFinished);
        }
        assert!(finished);
        assert_eq!(man.stage(), Stage::Gone);
        assert!(!man.avatar_active());
    }

    #[test]
    fn explosion_before_the_explosion_object_exists_respawns_instantly_without_hiding() {
        let mut rng = GameRng::from_seed(7);
        let mut man = LifeCycle::man(); // `explosion_prepared` never called
        let events = man.damage(100.0, None, &mut env(&mut rng));
        assert!(events.iter().any(|e| matches!(e, LifeEvent::Respawned { .. })));
        man.late_update();
        assert!(man.avatar_active());
        assert_eq!(man.health.remaining(), 100.0);
    }

    #[test]
    fn a_second_death_after_respawn_runs_the_whole_cycle_again() {
        let mut rng = GameRng::from_seed(8);
        let mut lives = Lives::new(2, 0);
        let mut man = ready_man();
        man.health.link_to_lives();
        for _ in 0..2 {
            man.damage(f32::MAX, Some(&mut lives), &mut env(&mut rng));
            man.late_update();
            let (t, e) = run_until_respawn(&mut man, &mut rng, 1.0 / 30.0);
            assert!((t - 3.0).abs() < 0.1, "{t}");
            assert!(e.iter().any(|e| matches!(e, LifeEvent::Respawned { .. })));
        }
        assert_eq!(lives.remaining(), 0);
    }
}
