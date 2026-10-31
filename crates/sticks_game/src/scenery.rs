//! Level scenery that moves or breaks (`unity_data::scenery`, `sticks_sim::scenery`):
//!
//! * `HorizontalScroller2D` bodies (Japan's clouds) drift with the fixed steps and wrap; their
//!   sprites follow through [`SpriteFollowOffsets`].
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
            .add_systems(Update, step_scrollers.in_set(crate::FrameSet::FixedSteps).after(crate::player::run_fixed_steps))
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
) {
    let props = &mut *props;
    for hit in hits.read() {
        let Some(prop) = props.items.iter_mut().find(|p| matches!(p.phase, PropPhase::Alive) && level.0.props[p.spec].colliders.contains(&(hit.collider as usize))) else { continue };
        let events = prop.health.damage(hit.amount);
        if !(&events).into_iter().any(|e| matches!(e, HealthEvent::Exhausted)) {
            continue;
        }
        let spec = &level.0.props[prop.spec];
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
