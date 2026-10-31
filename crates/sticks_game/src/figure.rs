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
}

/// `CharacterColor` of an avatar whose colours differ from the prefab's (`CharacterColorSetter`).
#[derive(Component, Clone, Copy, Deref)]
pub struct Palette(pub CharacterPalette);

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
            .add_systems(Update, (attach_hit_flash, track_hits).chain().in_set(crate::FrameSet::Present).before(QueueSet::Collect))
            // `Explosion.Go` happens in the damage callback; its `Update` runs after it in the
            // same frame, next to the life cycle's own explosion clock.
            .add_systems(
                Update,
                (start_explosions.before(crate::life::tick_lives), tick_explosions.after(crate::life::tick_lives)).in_set(crate::FrameSet::Characters),
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
    for (weapon, guid) in rig_prefabs.chain(enemy_prefabs).chain(helicopter_prefabs) {
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
    art.add_sprites(&library.collection, projectiles.values().flat_map(|spec| spec.sprites.iter().map(|s| s.sprite_name.as_str())));
    commands.insert_resource(ProjectileLibrary(projectiles));
    commands.insert_resource(crate::weapon::MeleeLibrary(melee));
    let model = CharacterRigModel::new(avatar, rig).expect("the humanoid avatar does not fit the Man rig");
    commands.insert_resource(ManArt { model: Arc::new(model), art, palette, texture });
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
        Some(ManArt { model: Arc::new(model), art, palette, texture })
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
        for geometry in visual.renderers(&art.art) {
            queue.0.push_frame(art.texture, geometry);
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

fn queue_explosions(art: Option<Res<ManArt>>, light: Res<SceneLight>, explosions: Query<&Exploding>, mut queue: ResMut<FrameQueue>) {
    let Some(art) = art else { return };
    for exploding in &explosions {
        for geometry in exploding.0.renderers(&art.art, light.for_shadows) {
            queue.0.push_frame(art.texture, geometry);
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
        for geometry in visual.renderers(&art.art).into_iter().skip(1) {
            queue.0.push_frame(art.texture, geometry);
        }
    }
}
