//! Level scenery that moves or breaks (`unity_data::scenery`, `sticks_sim::scenery`):
//!
//! * `HorizontalScroller2D` bodies (Japan's clouds) drift with the fixed steps and wrap; their
//!   sprites follow through [`SpriteFollowOffsets`].
//! * `StarShower` (Moon): stars fall on the players and rovers between its x bounds (on the
//!   machine that decides; they are ordinary enemy projectiles from there on).
//! * `Whale`s (Pirates) rise and sink on their timers; their colliders are moving obstacles for
//!   the characters (`helicopter::publish_hulls` gathers them) and their sprites follow.
//! * Breakable props (Japan's hanging lamps, any `Health` + `ExplodeOnHealthExhausted` object):
//!   `SetLayerInCampaignMode` on load; a projectile hit (`DamageOtherOnCollision` →
//!   [`LevelColliderHit`]) takes health; when it is exhausted the prop explodes
//!   (`ExplodeSpritesAndLines`: its `SpriteShadower` sprites fly apart and fade), its sound plays
//!   (`PlaySoundOnExplode`), the object switches off (its colliders and static sprites go) and
//!   `DestroyOnExplosionFinished` removes it when the explosion is over.

use std::collections::HashMap;

use bevy::prelude::*;
use sticks_sim::explosion::Explosion;
use sticks_sim::life::{Health, HealthEvent};
use sticks_sim::scenery::{ScrollerState, prop_sprite};
use sticks_sim::visual::{CharacterVisual, VisualArt};
use sticks_sim::weapons::difficulty_modified;
use unity_compat::Vec3 as SimVec3;
use unity_data::sprites::SpriteLibrary;

use crate::config::GameConfig;
use crate::effects::PlaySound;
use crate::letters::LevelColliderHit;
use crate::level::{ActiveLevel, Collision};
use crate::life::DeathRng;
use crate::player::FrameClock;
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::{FrameQueue, HiddenSprites, QueueSet, SceneLight, SpriteFollowOffsets, TextureLoad, UnityLitMaterial, UnityTextures};

/// The level's `HorizontalScroller2D` bodies (index-matched with `Level::scrollers`).
#[derive(Resource, Default)]
pub struct Scrollers(pub Vec<ScrollerState>);

enum PropPhase {
    Alive,
    Exploding(Explosion),
    Gone,
}

struct Prop {
    /// Index into `Level::props`.
    spec: usize,
    health: Health,
    phase: PropPhase,
    /// The sprites that explode: their sprite names in the collection `collection`.
    parts: Vec<(usize, String)>,
    collection: Option<String>,
}

#[derive(Resource, Default)]
struct Props {
    items: Vec<Prop>,
    /// Exploding-sprite art and atlas texture by collection GUID.
    arts: HashMap<String, (VisualArt, u32)>,
    /// `DestroyAllForGoal.remaining`: the goal props still standing (counted with one player only).
    goal_remaining: usize,
}

pub struct SceneryPlugin;

impl Plugin for SceneryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Scrollers>()
            .add_systems(OnEnter(AppState::InGame), (start_scrollers, set_up_props))
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| {
                commands.remove_resource::<Props>();
                commands.insert_resource(Scrollers::default());
            })
            .init_resource::<Waves>()
            .add_systems(OnEnter(AppState::InGame), start_waves)
            .init_resource::<Whales>()
            .add_systems(OnEnter(AppState::InGame), start_whales)
            .add_systems(Update, star_showers.in_set(crate::FrameSet::Characters).run_if(crate::net::decides).run_if(in_state(AppState::InGame)))
            .add_systems(Update, (step_scrollers, step_waves, step_whales).chain().in_set(crate::FrameSet::FixedSteps).after(crate::player::run_fixed_steps))
            .add_systems(
                Update,
                (
                    hit_props.in_set(crate::FrameSet::FixedSteps).after(crate::weapon::step_projectiles),
                    tick_props.in_set(crate::FrameSet::Characters).after(crate::life::tick_lives),
                )
                    .run_if(resource_exists::<Props>),
            )
            .add_systems(Update, queue_props.in_set(QueueSet::Collect).run_if(resource_exists::<Props>));
    }
}

/// `HorizontalScroller2D.OnEnable`: the bodies start where the scene put them.
fn start_scrollers(mut commands: Commands, level: Res<ActiveLevel>) {
    commands.insert_resource(Scrollers(level.0.scrollers.iter().map(ScrollerState::of).collect()));
}

/// Every fixed step of the frame moves the bodies; their sprites follow.
fn step_scrollers(clock: Res<FrameClock>, mut scrollers: ResMut<Scrollers>, offsets: Option<ResMut<SpriteFollowOffsets>>) {
    for &delta in &clock.steps {
        for s in &mut scrollers.0 {
            s.fixed_step(delta);
        }
    }
    if let Some(mut offsets) = offsets {
        offsets.0 = scrollers.0.iter().map(ScrollerState::offset).collect();
    }
}

/// `Wave`s: the spec, `loop` (the sprite's untrimmed size) and the time so far.
#[derive(Resource, Default)]
struct Waves(Vec<(unity_data::scenery::WaveSpec, [f32; 2], f32)>);

/// `Wave.Start`: the loop is the sprite's untrimmed bounds size.
fn start_waves(mut commands: Commands, config: Res<GameConfig>, level: Res<ActiveLevel>) {
    let assets_dir = config.assets_dir();
    let mut libraries: HashMap<String, Option<SpriteLibrary>> = HashMap::new();
    let mut waves = Vec::new();
    for spec in &level.0.waves {
        let Some(sprite) = level.0.sprites.iter().find(|s| s.object == spec.object && s.shadow_of.is_none()) else { continue };
        let lib = libraries.entry(sprite.collection.clone()).or_insert_with(|| SpriteLibrary::load(&assets_dir, &sprite.collection).ok());
        let Some(def) = lib.as_ref().and_then(|l| l.collection.sprites.get(sprite.sprite_id)) else { continue };
        let size = def.untrimmed_bounds[1];
        waves.push((spec.clone(), [size[0] * sprite.scale[0].abs(), size[1] * sprite.scale[1].abs()], 0.0));
    }
    commands.insert_resource(Waves(waves));
}

/// `Wave.Update`: `startPosition + (velocity.x * time % loop.x, sin(time * velocity.y * 2π) *
/// amplitude % loop.y)` (C#'s `%` keeps the dividend's sign, as Rust's does).
fn step_waves(clock: Res<FrameClock>, mut waves: ResMut<Waves>, offsets: Option<ResMut<SpriteFollowOffsets>>) {
    let delta = clock.stepper.frame().delta;
    let Some(mut offsets) = offsets else { return };
    for (spec, size, time) in &mut waves.0 {
        *time += delta;
        let x = if size[0] != 0.0 { spec.velocity[0] * *time % size[0] } else { 0.0 };
        let wobble = (*time * spec.velocity[1] * std::f32::consts::PI * 2.0).sin() * spec.oscillation_amplitude;
        let y = if size[1] != 0.0 { wobble % size[1] } else { wobble };
        offsets.0.push([x * spec.parent_scale[0], y * spec.parent_scale[1]]);
    }
}

/// `StarShower.Update` (`ServerOnly`): one random target of its `Targeter` (no qualifiers: every
/// `Targetable`: the players, the rovers' and helicopters' "Target"s); in range, a star falls with
/// probability `Δt × rate × targets` this frame.
#[allow(clippy::too_many_arguments)]
fn star_showers(
    mut commands: Commands,
    clock: Res<FrameClock>,
    collision: Res<Collision>,
    session: Res<Session>,
    level: Res<ActiveLevel>,
    library: Option<Res<crate::weapon::ProjectileLibrary>>,
    mut rng: ResMut<DeathRng>,
    players: Query<(&crate::player::Avatar, &crate::life::Mortal), With<crate::player::ControlledBy>>,
    cars: Query<&crate::cars::Car>,
    helicopters: Query<&crate::helicopter::Helicopter>,
) {
    let Some(library) = library else { return };
    if level.0.star_showers.is_empty() {
        return;
    }
    let delta = clock.stepper.frame().delta;
    // (position, is a car)
    let mut targets: Vec<(unity_compat::Vec2, bool)> = helicopters.iter().filter_map(|h| h.target_candidate().map(|c| (unity_compat::Vec2::new(c.position.x, c.position.y), false))).collect();
    targets.extend(cars.iter().filter_map(|c| c.target_position().map(|p| (p, true))));
    targets.extend(players.iter().filter(|(_, m)| m.avatar_active()).map(|(a, _)| {
        let p = a.position();
        (unity_compat::Vec2::new(p.x, p.y + crate::camera::CAMERA_TARGET_HEIGHT), false)
    }));
    let count = targets.len();
    if count == 0 {
        return;
    }
    let gravity = collision.rules.gravity;
    for shower in &level.0.star_showers {
        let Some(star) = library.0.get(&shower.prefab) else { continue };
        let (target, is_car) = targets[rng.0.range_int(0, count as i32) as usize];
        let Some(mut rate) = sticks_sim::scenery::star_rate(shower, target.x) else { continue };
        let mut spread = shower.random_range;
        if is_car {
            rate *= shower.vehicle_rate_multiplier;
            spread *= shower.vehicle_rate_multiplier;
        }
        rate *= count as f32;
        if rng.0.value() >= delta * rate {
            continue;
        }
        let [mut x, y] = sticks_sim::scenery::star_launch_point(shower, [target.x, target.y], gravity.y);
        x += spread * (rng.0.value() - 0.5);
        let damage = match &star.damage_scaling {
            Some(s) if session.mode == GameMode::Campaign => difficulty_modified(star.damage, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
            _ => star.damage,
        };
        let [vx, vy] = shower.launch_velocity;
        let spin = shower.launch_angular_velocity_min + (shower.launch_angular_velocity_max - shower.launch_angular_velocity_min) * rng.0.value();
        let shot = sticks_sim::weapons::Shot::launch(star, damage, unity_compat::Vec2::new(x, y), unity_compat::Vec2::new(vx, vy), 0.0, crate::enemy::ENEMY_TEAM, gravity).unrotated_with_spin(spin);
        commands.spawn((Name::new(star.name.clone()), crate::weapon::Projectile::new(shot, star.clone(), None)));
    }
}

/// Collider ids of the whales as moving obstacles (apart from the level's, hulls' and cars').
pub(crate) const WHALE_COLLIDER_BASE: u32 = 0x7E00_0000;

/// One `Whale`: its spec, its collider's convex pieces at rest (world), layer and height now.
struct WhaleState {
    spec: unity_data::scenery::WhaleSpec,
    pieces: Vec<Vec<unity_compat::Vec2>>,
    layer: u8,
    offset: f32,
}

/// The whales and the time since the level started (`OnLoadedLevelEverywhere`).
#[derive(Resource, Default)]
pub(crate) struct Whales {
    whales: Vec<WhaleState>,
    time: f32,
}

impl Whales {
    /// The whales' colliders where they are now (moving obstacles).
    pub(crate) fn obstacles(&self) -> Vec<(sticks_sim::physics::StaticCollider, Vec<Vec<unity_compat::Vec2>>)> {
        self.whales
            .iter()
            .enumerate()
            .map(|(i, w)| {
                // TODO(UNCERTAIN): the original records a whale's top as a safe respawn place like
                // any ground; once it has sunk, respawning there drops the player into the sea
                // again and again, so here it is never a respawn place.
                let collider = sticks_sim::physics::StaticCollider { id: WHALE_COLLIDER_BASE + i as u32, layer: w.layer, unsafe_ground: true, ..Default::default() };
                let lift = unity_compat::Vec2::new(0.0, w.offset);
                (collider, w.pieces.iter().map(|piece| piece.iter().map(|p| *p + lift).collect()).collect())
            })
            .collect()
    }
}

/// `Whale.Start`: the whale's polygon collider (a dynamic collider of the level) in convex pieces.
fn start_whales(mut commands: Commands, level: Res<ActiveLevel>) {
    use sticks_sim::physics::{merge_convex, triangulate};
    let whales = level
        .0
        .whales
        .iter()
        .map(|spec| {
            let mut layer = 0;
            let mut pieces = Vec::new();
            for c in level.0.colliders.iter().filter(|c| c.object == spec.object && !c.trigger && c.enabled) {
                if let unity_data::level::Shape::Polygon(path) = &c.shape {
                    layer = c.layer;
                    pieces.extend(merge_convex(triangulate(&path.iter().map(|p| unity_compat::Vec2::new(p[0], p[1])).collect::<Vec<_>>())));
                }
            }
            WhaleState { spec: spec.clone(), pieces, layer, offset: 0.0 }
        })
        .collect();
    commands.insert_resource(Whales { whales, time: 0.0 });
}

/// `Whale.Go` each frame; the sprites follow (after the scrollers' and waves' offsets).
fn step_whales(clock: Res<FrameClock>, level: Res<ActiveLevel>, mut whales: ResMut<Whales>, offsets: Option<ResMut<SpriteFollowOffsets>>) {
    let whales = &mut *whales;
    whales.time += clock.stepper.frame().delta;
    for w in &mut whales.whales {
        w.offset = sticks_sim::scenery::whale_offset(&w.spec, whales.time);
    }
    if let Some(mut offsets) = offsets {
        let base = level.0.scrollers.len() + level.0.waves.len();
        offsets.0.resize(base, [0.0, 0.0]);
        offsets.0.extend(whales.whales.iter().map(|w| [0.0, w.offset]));
    }
}

/// `Health.Awake` (with `HealthDifficultyModifier` in the campaign) and
/// `SetLayerInCampaignMode.Awake` of every prop; its exploding sprites' art.
#[allow(clippy::too_many_arguments)]
fn set_up_props(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    mut collision: ResMut<Collision>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let campaign = session.mode == GameMode::Campaign;
    let mut libraries: HashMap<String, SpriteLibrary> = HashMap::new();
    let mut props = Props::default();
    let sprites = &level.0.sprites;
    for (index, spec) in level.0.props.iter().enumerate() {
        let mut starting = spec.starting_health;
        if let (true, Some(s)) = (campaign, &spec.health_scaling) {
            starting = difficulty_modified(starting, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty);
        }
        let mut health = Health::new(starting);
        health.reset();
        if let (true, Some(layer)) = (campaign, spec.campaign_layer) {
            for &c in &spec.colliders {
                collision.world.set_layer(c as u32, layer);
            }
        }
        // `Explosion.Prepare`: the object's `SpriteShadower` sprites (those with a shadow copy).
        let casters: Vec<usize> = spec.sprites.iter().copied().filter(|&i| sprites[i].shadow_of.is_none() && sprites.iter().any(|s| s.shadow_of.is_some_and(|(c, _)| c == i))).collect();
        let collection = casters.first().map(|&i| sprites[i].collection.clone());
        let mut parts = Vec::new();
        if let Some(guid) = &collection {
            if !libraries.contains_key(guid) {
                if let Ok(lib) = SpriteLibrary::load(&assets_dir, guid) {
                    libraries.insert(guid.clone(), lib);
                }
            }
            if let Some(lib) = libraries.get(guid) {
                for &i in &casters {
                    if sprites[i].collection != *guid {
                        continue;
                    }
                    if let Some(def) = lib.collection.sprites.get(sprites[i].sprite_id) {
                        parts.push((i, def.name.clone()));
                    }
                }
                let entry = props.arts.entry(guid.clone()).or_insert_with(|| (VisualArt::sprites_only(&lib.collection, std::iter::empty()), 0));
                entry.0.add_sprites(&lib.collection, parts.iter().map(|(_, n)| n.as_str()));
                if entry.1 == 0 {
                    if let Some(path) = parts.first().and_then(|(_, n)| lib.collection.sprite(n)).and_then(|d| lib.texture_of(d)) {
                        entry.1 = textures.id(path, &mut load);
                    }
                }
            }
        }
        props.items.push(Prop { spec: index, health, phase: PropPhase::Alive, parts, collection });
    }
    // `DestroyAllForGoal.Awake`: only a one-player game counts.
    if session.player_count == 1 {
        props.goal_remaining = level.0.props.iter().filter(|p| p.destroy_all_for_goal).count();
    }
    if !props.items.is_empty() {
        info!("{} breakable props", props.items.len());
    }
    commands.insert_resource(props);
}

/// `DamageOtherOnCollision` on a prop's collider → `Health.Damage` → `ExplodeOnHealthExhausted`
/// → `Explodable.Explode` (in the physics callback: `Time.deltaTime` is the fixed step).
#[allow(clippy::too_many_arguments)]
fn hit_props(
    mut hits: MessageReader<LevelColliderHit>,
    mut props: ResMut<Props>,
    level: Res<ActiveLevel>,
    light: Res<SceneLight>,
    clock: Res<FrameClock>,
    mut rng: ResMut<DeathRng>,
    mut collision: ResMut<Collision>,
    mut hidden: Option<ResMut<HiddenSprites>>,
    mut sounds: MessageWriter<PlaySound>,
    mut memory: ResMut<crate::lobby::LobbyMemory>,
    players: Query<&crate::player::Player>,
    mut scores: Query<&mut crate::life::PlayerScore>,
) {
    let props = &mut *props;
    for hit in hits.read() {
        let Some(prop) = props.items.iter_mut().find(|p| matches!(p.phase, PropPhase::Alive) && level.0.props[p.spec].colliders.contains(&(hit.collider as usize))) else { continue };
        let before = prop.health.remaining();
        let events = prop.health.damage(hit.amount);
        // `DamageOtherOnCollision`: the shooter is credited with the health it took (props
        // carry no `ControlSlot`, so no frag), unless `DamagingDoesNotAwardScore`.
        if !level.0.props[prop.spec].no_score {
            if let Some(mut score) = hit.responsible.and_then(|r| scores.get_mut(r).ok()) {
                score.0.damage_dealt += before - prop.health.remaining();
            }
        }
        if !(&events).into_iter().any(|e| matches!(e, HealthEvent::Exhausted)) {
            continue;
        }
        let spec = &level.0.props[prop.spec];
        // `DestroyAllForGoal`'s `Exhausted` handler: the last one gives the first active player
        // "<loaded level name> all destroyed".
        if spec.destroy_all_for_goal && props.goal_remaining > 0 {
            props.goal_remaining -= 1;
            if props.goal_remaining == 0 {
                if let Some(player) = players.iter().map(|p| p.index).min() {
                    let id = sticks_sim::level_end::PlayerId::anonymous_local(player);
                    let goal = format!("{} all destroyed", level.0.name.to_lowercase());
                    info!("goal: {goal}");
                    if memory.goals.goal_done(&id, &goal, None) {
                        memory.save.goals = true;
                    }
                }
            }
        }
        let at = SimVec3::new(spec.position[0], spec.position[1], spec.position[2]);
        // `PlaySoundOnExplode`.
        if let Some(sound) = &spec.sound {
            sounds.write(PlaySound { group: sound.clone(), at: Some(at), owner: None });
        }
        // `ExplodeSpritesAndLines`: the parts fly; the object switches off in `LateUpdate`
        // (before anything is drawn), taking its colliders and sprites with it.
        prop.phase = match spec.fade_wait_time {
            Some(fade_wait) => {
                let sprites = &level.0.sprites;
                let parts = prop
                    .parts
                    .iter()
                    .map(|(i, name)| {
                        let s = &sprites[*i];
                        let plane = sprites.iter().find_map(|sh| sh.shadow_of.filter(|(c, _)| c == i).map(|(_, p)| p)).unwrap_or(0.0);
                        prop_sprite(name, &s.world, s.world_rotation, s.scale, s.color, plane, light.for_shadows)
                    })
                    .collect();
                let visual = CharacterVisual { shadows: Vec::new(), lines: Vec::new(), sprites: parts };
                PropPhase::Exploding(Explosion::go(&visual, at, at, fade_wait, &mut rng.0, clock.last_step_delta))
            }
            // No explosion object: `Explodable.Finished` at once.
            None => PropPhase::Gone,
        };
        for &c in &spec.colliders {
            collision.world.set_enabled(c as u32, false);
        }
        if let Some(hidden) = hidden.as_mut() {
            hidden.0.extend(spec.sprites.iter().copied());
        }
    }
}

/// `Explosion.Update`; `DestroyOnExplosionFinished` when it is over.
fn tick_props(mut props: ResMut<Props>, clock: Res<FrameClock>) {
    let delta = clock.stepper.frame().delta;
    for prop in &mut props.items {
        if let PropPhase::Exploding(explosion) = &mut prop.phase {
            if explosion.update(delta) {
                prop.phase = PropPhase::Gone;
            }
        }
    }
}

fn queue_props(props: Res<Props>, light: Res<SceneLight>, mut queue: ResMut<FrameQueue>) {
    for prop in &props.items {
        let PropPhase::Exploding(explosion) = &prop.phase else { continue };
        let Some((art, texture)) = prop.collection.as_ref().and_then(|c| props.arts.get(c)) else { continue };
        for geometry in explosion.renderers(art, light.for_shadows) {
            if !geometry.is_empty() {
                queue.0.push_frame(*texture, geometry);
            }
        }
    }
}
