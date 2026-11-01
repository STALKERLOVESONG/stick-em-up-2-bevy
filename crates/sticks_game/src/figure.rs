//! What the `Man` looks like on screen.
//!
//! The original draws no skinned mesh (the prefab's `SkinnedMeshRenderer` has no materials). The
//! humanoid animation poses the rig, `LinesToChildren` turns bone pairs into round-capped lines
//! (plus black shadow copies) in one `LinesRenderer` mesh, and `FlatSprite`s (helmet, weapons)
//! follow their bones as tk2d sprites, each with a `SpriteShadower` copy. `sticks_sim::visual`
//! reproduces that geometry from the real avatar, rig and clips; this module pushes those
//! renderers into the frame's transparent queue (`unity_render`), where they are lit and sorted
//! together with the level, as in the original.
//!
//! A character that explodes (`ExplodeSpritesAndLines`) leaves an `Explosion` behind: the lines
//! and sprites it drew last frame fly apart and fade (`sticks_sim::explosion`).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use sticks_sim::explosion::Explosion;
use sticks_sim::life::{LifeEvent, MAN_EXPLOSION_FADE_WAIT, Stage};
use sticks_sim::pose::CharacterRigModel;
use sticks_sim::visual::{CharacterPalette, CharacterVisual, VisualArt, projectile_visual};
use unity_data::humanoid::parse_avatar;
use unity_data::projectile::try_load_projectile;
use unity_data::rig::load_man_rig;
use unity_data::sprites::load_collection_with_sprites;

use crate::config::GameConfig;
use crate::level::{ActiveLevel, LevelSource};
use crate::life::{DeathRng, LifeNotice, Mortal};
use crate::player::{Avatar, FrameClock};
use crate::state::AppState;
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, TextureLoad, UnityLitMaterial, UnityTextures};
use crate::weapon::{Armament, Posed, Projectile, ProjectileLibrary};

/// `FlashCharacterOnDamage.flashTime`.
const FLASH_SECONDS: f32 = 0.1;

/// Everything shared by all characters drawn with the `Man` rig.
#[derive(Resource)]
pub struct ManArt {
    model: Arc<CharacterRigModel>,
    pub(crate) art: VisualArt,
    palette: CharacterPalette,
    /// Queue texture of the atlas the `Line` sprite lives in.
    pub(crate) texture: u32,
    /// Each rig weapon's firing animation (`firingAnimator`), when it has one.
    actions: Vec<Option<Arc<unity_data::anim::TransformClip>>>,
}

impl ManArt {
    /// The player's default body colour (`CharacterColor.bodyColor`), as 8-bit sRGB.
    pub fn body_color(&self) -> [u8; 4] {
        self.palette.body
    }

    /// The `Man` prefab's own colours.
    pub fn prefab_palette(&self) -> CharacterPalette {
        self.palette
    }

    /// The rig, avatar and animation-to-bone mapping every `Man` is posed from.
    pub fn model(&self) -> &CharacterRigModel {
        &self.model
    }

    /// The firing animation of rig weapon `weapon` (the Sword's swing, the throws).
    pub fn action(&self, weapon: usize) -> Option<&unity_data::anim::TransformClip> {
        self.actions.get(weapon).and_then(|a| a.as_deref())
    }
}

impl ManArt {
    /// A `VehicleOccupant` posed in its animation state at the seat (`rotation` is the seat's
    /// world rotation), optionally holding `weapon` aimed (`CarGunnerControl`'s
    /// `SetUpNewWeapon` + `CorrectAndSetAim` + `VehicleOccupantIK`).
    pub(crate) fn occupant_posed(
        &self,
        animation: &Arc<sticks_sim::animator::AnimatorModel>,
        state: &str,
        position: unity_compat::Vec3,
        rotation: unity_compat::Quat,
        weapon: Option<&sticks_sim::aim::AimedWeapon>,
    ) -> Option<sticks_sim::aim::PosedCharacter> {
        let state = animation.state_index(state)?;
        let mut animator = sticks_sim::animator::Animator::new(animation.clone());
        animator.play(state);
        let pose = animator.pose();
        let solved = match weapon {
            Some(aimed) => self.model.solve_aimed_acting(&pose, rotation, aimed, None),
            None => self.model.solve(&pose),
        };
        // Posed at the usual depth; the renderers are moved to the seat with their shadow planes.
        let placement = sticks_sim::visual::BodyPlacement { position: unity_compat::Vec3::new(position.x, position.y, sticks_sim::character::DEFAULT_Z), rotation };
        Some(sticks_sim::aim::PosedCharacter { solved, placement })
    }

    /// What a `VehicleOccupant` draws: its `Man` (the "Vehicle Occupant" prefab, the same rig with
    /// no weapon sprite showing) played into the animator state `state` ("Driving", "Sitting"),
    /// at the seat's world position and rotation, in the controlling player's colours.
    ///
    /// TODO(UNCERTAIN): the occupant prefab keeps all three weapon pivots active with their
    /// sprites off; it is drawn here with nothing equipped, so the arms follow the clip alone.
    /// A gunner's occupant shows only the weapon `CarGunnerControl` set up.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn occupant_renderers(
        &self,
        animation: &Arc<sticks_sim::animator::AnimatorModel>,
        state: &str,
        position: unity_compat::Vec3,
        rotation: unity_compat::Quat,
        palette: CharacterPalette,
        light: Option<unity_compat::Vec3>,
        weapon: Option<&sticks_sim::aim::AimedWeapon>,
    ) -> Vec<(u32, sticks_sim::draw::Geometry)> {
        let Some(posed) = self.occupant_posed(animation, state, position, rotation, weapon) else { return Vec::new() };
        let default_z = sticks_sim::character::DEFAULT_Z;
        let equipped = weapon.map(|w| w.weapon);
        let visual = self.model.visual(&posed.solved, &posed.placement, &palette, light, &|bone| self.model.active_with_weapon(bone, equipped)).shifted_z(position.z - default_z);
        visual.renderers_textured(&self.art).into_iter().filter(|(_, g)| !g.is_empty()).map(|(t, g)| (t.unwrap_or(self.texture), g)).collect()
    }
}

/// `CharacterColor` of an avatar whose colours differ from the prefab's (`CharacterColorSetter`).
#[derive(Component, Clone, Copy, Deref)]
pub struct Palette(pub CharacterPalette);

/// `ColorCycler` (Corruption 2's "Big Bad"): its `CharacterColor` runs round the hue circle every
/// `period` seconds, at half brightness for `FlashCharacterOnDamage.flashTime` after each hit.
#[derive(Component)]
pub struct ColorCycler {
    period: f32,
    /// One pending end per `Damaged` coroutine; the first to end clears `hit`.
    remaining: Vec<f32>,
    hit: bool,
}

impl ColorCycler {
    pub fn new(period: f32) -> ColorCycler {
        ColorCycler { period: period.max(1e-3), remaining: Vec::new(), hit: false }
    }
}

/// `ColorCycler.Damaged` coroutines and `Update`, on `Time.timeSinceLevelLoad`.
fn cycle_colors(clock: Res<FrameClock>, mut notices: MessageReader<LifeNotice>, mut cyclers: Query<(&mut ColorCycler, &mut Palette)>) {
    let delta = clock.stepper.frame().delta;
    for (mut cycler, _) in &mut cyclers {
        if cycler.remaining.is_empty() {
            continue;
        }
        for t in &mut cycler.remaining {
            *t -= delta;
        }
        let before = cycler.remaining.len();
        cycler.remaining.retain(|t| *t > 0.0);
        if cycler.remaining.len() != before {
            cycler.hit = false;
        }
    }
    for notice in notices.read() {
        if let (LifeEvent::Damaged(_), Ok((mut cycler, _))) = (&notice.event, cyclers.get_mut(notice.avatar)) {
            cycler.hit = true;
            cycler.remaining.push(FLASH_SECONDS);
        }
    }
    for (cycler, mut palette) in &mut cyclers {
        let (body, helmet) = sticks_sim::visual::color_cycler_colors(clock.time, cycler.period, cycler.hit);
        palette.0.body = body.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        palette.0.helmet = helmet;
    }
}

/// `FlashCharacterOnDamage` state of an avatar: one pending end time per `Damaged` coroutine.
/// Each finishing coroutine restores the normal colours, even if a later hit started another.
#[derive(Component, Default)]
pub struct HitFlash {
    remaining: Vec<f32>,
    flashing: bool,
}

pub struct FigurePlugin;

impl Plugin for FigurePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), load_man_art)
            .add_systems(Update, (attach_hit_flash, track_hits, cycle_colors).chain().in_set(crate::FrameSet::Present).before(QueueSet::Collect))
            // `Explosion.Go` happens in the damage callback; its `Update` runs after it in the
            // same frame, next to the life cycle's own explosion clock.
            .add_systems(
                Update,
                (start_explosions.before(crate::life::tick_lives), tick_explosions.after(crate::life::tick_lives), constant_explosions.after(crate::life::tick_lives)).in_set(crate::FrameSet::Characters),
            )
            .add_systems(Update, (queue_figures, queue_projectiles, queue_explosions).in_set(QueueSet::Collect).run_if(in_state(AppState::InGame)));
    }
}

/// Load the rig, avatar and sprite atlas the `Man` is drawn from.
#[allow(clippy::too_many_arguments)]
pub(crate) fn load_man_art(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    source: Res<LevelSource>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let assets = config.assets_dir();
    let scene = config.export_root.join(&source.0);
    let (rig, avatar, library, texture) = load_man_parts(&assets, &scene, &mut textures, &mut TextureLoad::new(&assets, &asset_server, &mut images, &mut materials));
    let palette = CharacterPalette::of_rig(&rig);
    let mut art = VisualArt::new(&library.collection, &rig).expect("the character's line and helmet sprites");

    // Every weapon's projectile prefab, the replacements the level's enemies launch instead
    // (`ProjectileLauncherModifier.projectilePrefab`), and the sprites those are drawn with (the
    // same atlas).
    let mut projectiles = HashMap::new();
    let mut melee = HashMap::new();
    let rig_prefabs = rig.weapons.iter().filter(|w| !w.projectile.is_empty()).map(|w| (w.name.as_str(), w.projectile.as_str()));
    let enemy_prefabs = level.0.enemies.iter().flat_map(|e| &e.launcher_modifiers).filter_map(|m| Some((m.weapon.as_str(), m.projectile.as_deref()?)));
    // Helicopter guns' launchers too.
    let helicopter_prefabs = level.0.helicopters.iter().flat_map(|h| &h.guns).map(|g| ("Helicopter Gun", g.projectile.as_str()));
    // And the cannons' (Pirates).
    let cannon_prefabs = level.0.cannons.iter().flat_map(|c| &c.launchers).map(|l| ("Cannon", l.projectile.as_str()));
    for (weapon, guid) in rig_prefabs.chain(enemy_prefabs).chain(helicopter_prefabs).chain(cannon_prefabs) {
        if projectiles.contains_key(guid) {
            continue;
        }
        match try_load_projectile(&assets, guid) {
            Ok(Some(spec)) => {
                projectiles.insert(guid.to_owned(), Arc::new(spec));
            }
            // Melee weapons launch an effect prefab (the Sword's "Sword Swish") that is not a
            // flying `Projectile` (`sticks_sim::melee`).
            Ok(None) => match unity_data::projectile::try_load_melee(&assets, guid) {
                Ok(Some(spec)) => {
                    melee.insert(guid.to_owned(), Arc::new(spec));
                }
                Ok(None) => warn!("{weapon} launches {guid}, which is neither a projectile nor a melee effect"),
                Err(e) => warn!("melee prefab of {weapon}: {e}"),
            },
            Err(e) => warn!("launcher prefab of {weapon}: {e}"),
        }
    }
    // Moon's falling stars (`StarShower.prefab`): bodies without a `Projectile` script, launched
    // at the length of `launchVelocity` (`crate::scenery`).
    for shower in &level.0.star_showers {
        if projectiles.contains_key(&shower.prefab) {
            continue;
        }
        match unity_data::projectile::try_load_projectile_with(&assets, &shower.prefab, false) {
            Ok(Some(mut star)) => {
                let [x, y] = shower.launch_velocity;
                star.speed = (x * x + y * y).sqrt();
                projectiles.insert(shower.prefab.clone(), Arc::new(star));
            }
            Ok(None) => {}
            Err(e) => warn!("star shower prefab: {e}"),
        }
    }
    art.add_sprites(&library.collection, projectiles.values().flat_map(|spec| spec.sprites.iter().map(|s| s.sprite_name.as_str())));

    // Weapons (and their projectiles) whose sprites this level's collection lacks are not drawn
    // in the original (`SpriteFinder` only searches the level). A weapon can only be held here
    // through the All Weapons cheat, so outside "Original Game Only" those sprites are borrowed
    // from another level's collection, with that collection's atlas.
    if !crate::settings::original_only() {
        let rig_names = rig.bones.iter().filter_map(|b| b.flat_sprite.as_ref()).map(|f| f.sprite_name.as_str());
        let projectile_names = projectiles.values().flat_map(|spec| spec.sprites.iter().map(|s| s.sprite_name.as_str()));
        let missing = art.missing(rig_names.chain(projectile_names).filter(|n| !n.is_empty()));
        if !missing.is_empty() {
            let mut load = TextureLoad::new(&assets, &asset_server, &mut images, &mut materials);
            for (other, names) in unity_data::sprites::collections_supplying(&assets, &missing, &[]) {
                for name in &names {
                    let Some(atlas) = other.collection.sprite(name).and_then(|def| other.texture_of(def)) else { continue };
                    let texture = textures.id(atlas, &mut load);
                    art.borrow_sprites(&other.collection, [name.as_str()], texture);
                }
            }
            let still = art.missing(missing.iter().copied());
            if !still.is_empty() {
                info!("sprites no collection has: {still:?}");
            }
        }
    }
    commands.insert_resource(ProjectileLibrary(projectiles));
    commands.insert_resource(crate::weapon::MeleeLibrary(melee));
    let actions = rig.weapons.iter().map(|w| unity_data::anim::load_weapon_action(&assets, &w.name).map(Arc::new)).collect();
    let model = CharacterRigModel::new(avatar, rig).expect("the humanoid avatar does not fit the Man rig");
    commands.insert_resource(ManArt { model: Arc::new(model), art, palette, texture, actions });
}

/// The `Man` rig, its humanoid avatar, and the sprite collection its lines and helmet are drawn
/// from: whichever collection the scene's own sprites use that holds the "Line" and "Helmet"
/// sprites (as `SpriteFinder` resolves them), with the queue texture of its atlas.
pub(crate) fn load_man_parts(
    assets: &std::path::Path,
    scene: &std::path::Path,
    textures: &mut UnityTextures,
    load: &mut TextureLoad<'_, '_>,
) -> (unity_data::rig::Rig, unity_data::humanoid::HumanAvatar, unity_data::sprites::SpriteLibrary, u32) {
    let rig = load_man_rig(assets).unwrap_or_else(|e| panic!("Man.prefab: {e}"));
    let avatar_text = std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap_or_else(|e| panic!("ManAvatar.asset: {e}"));
    let avatar = parse_avatar(&avatar_text).expect("ManAvatar.asset: not a humanoid avatar");
    let library = load_collection_with_sprites(assets, scene, &["Line", "Helmet"]).unwrap_or_else(|e| panic!("sprite collection: {e}"));
    let line = library.collection.sprite("Line").expect("Line sprite");
    let atlas = library.texture_of(line).expect("the Line sprite's atlas");
    let texture = textures.id(atlas, load);
    (rig, avatar, library, texture)
}

impl ManArt {
    /// The `Man` art for a scene other than a level (the Lobby's posed models); `None` when a
    /// part is missing.
    pub(crate) fn for_scene(assets: &std::path::Path, scene: &std::path::Path, textures: &mut UnityTextures, load: &mut TextureLoad<'_, '_>) -> Option<ManArt> {
        let rig = load_man_rig(assets).ok()?;
        let avatar = parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).ok()?)?;
        let library = load_collection_with_sprites(assets, scene, &["Line", "Helmet"]).ok()?;
        let atlas = library.texture_of(library.collection.sprite("Line")?)?;
        let texture = textures.id(atlas, load);
        let palette = CharacterPalette::of_rig(&rig);
        let art = VisualArt::new(&library.collection, &rig)?;
        let model = CharacterRigModel::new(avatar, rig)?;
        Some(ManArt { model: Arc::new(model), art, palette, texture, actions: Vec::new() })
    }
}

fn attach_hit_flash(mut commands: Commands, new_avatars: Query<Entity, (Added<Posed>, Without<HitFlash>)>) {
    for entity in &new_avatars {
        commands.entity(entity).insert(HitFlash::default());
    }
}

/// `FlashCharacterOnDamage`: a non-lethal hit turns the character lighter for 0.1 s.
fn track_hits(clock: Res<FrameClock>, mut notices: MessageReader<LifeNotice>, mut avatars: Query<(&Mortal, &mut HitFlash)>) {
    let delta = clock.stepper.frame().delta;
    for (_, mut flash) in &mut avatars {
        if flash.remaining.is_empty() {
            continue;
        }
        for t in &mut flash.remaining {
            *t -= delta;
        }
        let before = flash.remaining.len();
        flash.remaining.retain(|t| *t > 0.0);
        if flash.remaining.len() != before {
            flash.flashing = false;
        }
    }
    for notice in notices.read() {
        let Ok((mortal, mut flash)) = avatars.get_mut(notice.avatar) else { continue };
        match notice.event {
            // The coroutine checks `IsExhausted` when it starts, after `Health` clamped.
            LifeEvent::Damaged(_) if !mortal.health.is_exhausted() => {
                flash.flashing = true;
                flash.remaining.push(FLASH_SECONDS);
            }
            // `OnEnable` restores the standard colours.
            LifeEvent::Respawned { .. } => {
                flash.flashing = false;
                flash.remaining.clear();
            }
            _ => {}
        }
    }
}

/// What a character drew in its last `LateUpdate` (`LinesRenderer.GetLastLines`, the flat
/// sprites' transforms) and where its root was then.
#[derive(Component)]
pub struct LastVisual {
    visual: CharacterVisual,
    root: unity_compat::Vec3,
}

/// A character's running `Explosion`.
#[derive(Component)]
pub struct Exploding(Explosion);

/// Every active avatar's renderers this frame. A deactivated avatar (dead, or an enemy asleep
/// off screen: a `Disabled` entity) draws nothing; a dead one's explosion replaces it.
fn queue_figures(
    mut commands: Commands,
    art: Option<Res<ManArt>>,
    light: Res<SceneLight>,
    mut avatars: Query<(Entity, &Posed, &Armament, &Mortal, Option<&HitFlash>, Option<&Palette>, Option<&mut LastVisual>)>,
    mut queue: ResMut<FrameQueue>,
) {
    let Some(art) = art else { return };
    let model = &art.model;
    for (entity, posed, armament, mortal, flash, palette, last) in &mut avatars {
        if !mortal.avatar_active() {
            continue;
        }
        let palette = palette.map_or(art.palette, |p| p.0);
        let palette = if flash.is_some_and(|f| f.flashing) { palette.hit_version() } else { palette };
        let equipped = armament.equipped();
        let visual = model.visual(&posed.solved, &posed.placement, &palette, light.for_shadows, &|bone| model.active_with_weapon(bone, equipped));
        for (texture, geometry) in visual.renderers_textured(&art.art) {
            queue.0.push_frame(texture.unwrap_or(art.texture), geometry);
        }
        let root = posed.placement.position;
        match last {
            Some(mut last) => {
                last.visual = visual;
                last.root = root;
            }
            None => {
                commands.entity(entity).insert(LastVisual { visual, root });
            }
        }
    }
}

/// `Explodable.Explode` → `ExplodeSpritesAndLines` → `Explosion.Go` for characters whose
/// explosion object exists (the life cycle went on `Exploding`).
fn start_explosions(
    mut commands: Commands,
    clock: Res<FrameClock>,
    mut rng: ResMut<DeathRng>,
    mut notices: MessageReader<LifeNotice>,
    avatars: Query<(&Mortal, &Avatar, &LastVisual)>,
) {
    for notice in notices.read() {
        if notice.event != LifeEvent::Exploded {
            continue;
        }
        let Ok((mortal, avatar, last)) = avatars.get(notice.avatar) else { continue };
        if mortal.stage() != Stage::Exploding {
            continue;
        }
        let p = avatar.position();
        let root = unity_compat::Vec3::new(p.x, p.y, last.root.z);
        // Inside the physics callback `Time.deltaTime` is the fixed step.
        // TODO(UNCERTAIN): damage dealt outside a physics callback would use the frame delta.
        let explosion = Explosion::go(&last.visual, last.root, root, MAN_EXPLOSION_FADE_WAIT, &mut rng.0, clock.last_step_delta);
        commands.entity(notice.avatar).insert(Exploding(explosion));
    }
}

/// `Explosion.Update`; a finished explosion turns itself off.
fn tick_explosions(mut commands: Commands, clock: Res<FrameClock>, mut explosions: Query<(Entity, &mut Exploding)>) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut exploding) in &mut explosions {
        if exploding.0.update(delta) {
            commands.entity(entity).remove::<Exploding>();
        }
    }
}

fn queue_explosions(art: Option<Res<ManArt>>, light: Res<SceneLight>, explosions: Query<&Exploding>, constant: Query<&ConstantExplosion>, mut queue: ResMut<FrameQueue>) {
    let Some(art) = art else { return };
    let running = explosions.iter().map(|e| &e.0).chain(constant.iter().flat_map(|c| c.running.iter()));
    for explosion in running {
        for (texture, geometry) in explosion.renderers_textured(&art.art, light.for_shadows) {
            queue.0.push_frame(texture.unwrap_or(art.texture), geometry);
        }
    }
}

/// `ConstantExplosion` (Corruption 2's "Big Bad"): every `timeBetweenExplosions` the character's
/// lines and sprites as they are fly apart once more (`Explosion.Go` of a pooled explosion) while
/// the character itself stays; a pool of `ceil(3 s / interval)` explosions, each back in the pool
/// when finished, caps how many run at once.
#[derive(Component)]
pub struct ConstantExplosion {
    interval: f32,
    until_next: f32,
    capacity: usize,
    running: Vec<Explosion>,
}

impl ConstantExplosion {
    pub fn new(interval: f32) -> ConstantExplosion {
        let interval = interval.max(1e-3);
        ConstantExplosion { interval, until_next: interval, capacity: (3.0 / interval).ceil() as usize, running: Vec::new() }
    }
}

fn constant_explosions(clock: Res<FrameClock>, mut rng: ResMut<DeathRng>, mut characters: Query<(&Mortal, Option<&LastVisual>, &mut ConstantExplosion)>) {
    let delta = clock.stepper.frame().delta;
    for (mortal, last, mut constant) in &mut characters {
        let constant = &mut *constant;
        constant.running.retain_mut(|e| !e.update(delta));
        // `OnEnable` restarts the timer: a deactivated character (dead, asleep) does not run it.
        if !mortal.avatar_active() {
            constant.until_next = constant.interval;
            continue;
        }
        let Some(last) = last else { continue };
        constant.until_next -= delta;
        if constant.until_next <= 0.0 && constant.running.len() < constant.capacity {
            constant.until_next += constant.interval;
            constant.running.push(Explosion::go(&last.visual, last.root, last.root, MAN_EXPLOSION_FADE_WAIT, &mut rng.0, delta));
        }
    }
}

/// Every projectile's `CurrentCollectionSprite`s and their shadows.
fn queue_projectiles(art: Option<Res<ManArt>>, light: Res<SceneLight>, projectiles: Query<&Projectile>, mut queue: ResMut<FrameQueue>) {
    let Some(art) = art else { return };
    for projectile in &projectiles {
        if !projectile.visible() {
            continue;
        }
        let shot = &projectile.shot;
        let visual: CharacterVisual = projectile_visual(&projectile.spec.sprites, shot.position, projectile.spec.spawn_z, shot.rotation_degrees, light.for_shadows);
        // A projectile has no lines: skip the empty `LinesRenderer` entry.
        for (texture, geometry) in visual.renderers_textured(&art.art).into_iter().skip(1) {
            queue.0.push_frame(texture.unwrap_or(art.texture), geometry);
        }
    }
}
