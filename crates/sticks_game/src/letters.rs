//! The level's texts, and the shootable letters of its `ExplodableText`s (the Skyline tutorial
//! texts): see `sticks_sim::letters`. Each letter is a solid layer-0 collider with a tiny
//! `Health`; a projectile that hits it (`DamageOtherOnCollision`) exhausts it, it explodes
//! (`Explosion` with one flying text part and its shadow) and is destroyed when that finishes.
//! Texts that are not explodable are drawn as they are.

use std::collections::HashMap;

use bevy::prelude::*;
use sticks_sim::draw::{Geometry, shadow_offset_and_scale};
use sticks_sim::letters::{LETTER_HEALTH, Letter, LetterExplosion, split};
use sticks_sim::life::{Health, HealthEvent};
use sticks_sim::physics::{StaticCollider, StaticShape};
use sticks_sim::text::{TextLayout, format_text, place, text_local_geometry};
use unity_compat::{Quat as SimQuat, Vec2 as SimVec2, Vec3 as SimVec3};
use unity_data::fonts::FontLibrary;
use unity_data::level::LevelText;
use unity_data::math::Affine;

use crate::config::GameConfig;
use crate::level::{ActiveLevel, Collision};
use crate::life::DeathRng;
use crate::player::FrameClock;
use crate::state::AppState;
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, TextureLoad, UnityLitMaterial, UnityTextures};

/// Collider ids of letters start here (level colliders are numbered from 0).
pub(crate) const LETTER_COLLIDER_BASE: u32 = 1 << 24;
/// `TextShadowPositioner.realShadowOffset`.
const TEXT_SHADOW_DEPTH: f32 = 0.025;

/// A projectile hit a level collider (`DamageOtherOnCollision` on whatever it touched).
#[derive(Message, Debug, Clone, Copy)]
pub struct LevelColliderHit {
    pub collider: u32,
    pub amount: f32,
}

enum LetterState {
    Alive { health: Health },
    /// Exploded this frame; the object deactivates in `LateUpdate`.
    Exploding { explosion: LetterExplosion, deactivate_pending: bool },
    Gone,
}

struct LevelLetter {
    letter: Letter,
    font: String,
    shadow: (Option<String>, f32),
    collider: u32,
    state: LetterState,
}

/// A text drawn as it is.
struct StaticText {
    text: LevelText,
}

#[derive(Resource)]
struct LevelTexts {
    fonts: FontLibrary,
    textures: HashMap<String, u32>,
    letters: Vec<LevelLetter>,
    texts: Vec<StaticText>,
}

pub struct LettersPlugin;

impl Plugin for LettersPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LevelColliderHit>()
            .add_systems(OnEnter(AppState::InGame), spawn_level_texts)
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| commands.remove_resource::<LevelTexts>())
            .add_systems(
                Update,
                (
                    hit_letters.in_set(crate::FrameSet::FixedSteps).after(crate::weapon::step_projectiles),
                    tick_letters.in_set(crate::FrameSet::Characters).after(crate::life::tick_lives),
                    late_update_letters.in_set(crate::FrameSet::Present),
                )
                    .run_if(resource_exists::<LevelTexts>),
            )
            .add_systems(Update, queue_level_texts.in_set(QueueSet::Collect).run_if(resource_exists::<LevelTexts>));
    }
}

/// `ExplodableText.OnLoadedLevelEverywhere` → `Go`: replace every explodable text with letters
/// (solid colliders on layer 0).
#[allow(clippy::too_many_arguments)]
fn spawn_level_texts(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    mut collision: ResMut<Collision>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut fonts = FontLibrary::new(&assets_dir);
    let mut font_textures = HashMap::new();
    let mut letters = Vec::new();
    let mut texts = Vec::new();
    for text in &level.0.texts {
        for guid in std::iter::once(&text.text.font).chain(text.shadower.as_ref().and_then(|s| s.0.as_ref())) {
            if !font_textures.contains_key(guid) {
                if let Some(path) = fonts.get(guid).and_then(|f| f.texture.clone()) {
                    font_textures.insert(guid.clone(), textures.id(&path, &mut load));
                }
            }
        }
        if !text.explodable {
            texts.push(StaticText { text: text.clone() });
            continue;
        }
        let Some(font) = fonts.get(&text.text.font) else {
            warn!("explodable text {:?}: font {} not found", text.text.text, text.text.font);
            continue;
        };
        for letter in split(font, &text.text) {
            let collider = LETTER_COLLIDER_BASE + letters.len() as u32;
            let c = SimVec2::new(letter.position.x + letter.collider_offset[0], letter.position.y + letter.collider_offset[1]);
            let (hx, hy) = (letter.collider_size[0] * 0.5, letter.collider_size[1] * 0.5);
            let corners = vec![SimVec2::new(c.x - hx, c.y - hy), SimVec2::new(c.x + hx, c.y - hy), SimVec2::new(c.x + hx, c.y + hy), SimVec2::new(c.x - hx, c.y + hy)];
            collision.world.add(StaticCollider { id: collider, layer: 0, ..default() }, StaticShape::Polygon(corners));
            let mut health = Health::new(LETTER_HEALTH);
            health.reset();
            letters.push(LevelLetter {
                letter,
                font: text.text.font.clone(),
                shadow: text.shadower.clone().unwrap_or((None, 0.0)),
                collider,
                state: LetterState::Alive { health },
            });
        }
    }
    info!("level texts: {} letters, {} plain texts", letters.len(), texts.len());
    commands.insert_resource(LevelTexts { fonts, textures: font_textures, letters, texts });
}

/// `DamageOtherOnCollision` on a letter → `Health.Damage` → `ExplodeOnHealthExhausted` →
/// `Explosion.Go` (inside the physics callback: `Time.deltaTime` is the fixed step).
fn hit_letters(mut hits: MessageReader<LevelColliderHit>, mut texts: ResMut<LevelTexts>, mut rng: ResMut<DeathRng>, clock: Res<FrameClock>) {
    for hit in hits.read() {
        let Some(item) = texts.letters.iter_mut().find(|l| l.collider == hit.collider) else { continue };
        let LetterState::Alive { health } = &mut item.state else { continue };
        let events = health.damage(hit.amount);
        if (&events).into_iter().any(|e| matches!(e, HealthEvent::Exhausted)) {
            let explosion = LetterExplosion::go(&item.letter, &mut rng.0, clock.last_step_delta);
            item.state = LetterState::Exploding { explosion, deactivate_pending: true };
        }
    }
}

/// `Explosion.Update`; `DestroyOnExplosionFinished`.
fn tick_letters(mut texts: ResMut<LevelTexts>, clock: Res<FrameClock>) {
    let delta = clock.stepper.frame().delta;
    for item in &mut texts.letters {
        if let LetterState::Exploding { explosion, .. } = &mut item.state {
            if explosion.update(delta) {
                item.state = LetterState::Gone;
            }
        }
    }
}

/// `ExplodeSpritesAndLines.LateUpdate`: the exploded letter object (and its collider) goes.
fn late_update_letters(mut texts: ResMut<LevelTexts>, mut collision: ResMut<Collision>) {
    for item in &mut texts.letters {
        if let LetterState::Exploding { deactivate_pending, .. } = &mut item.state {
            if *deactivate_pending {
                *deactivate_pending = false;
                collision.world.set_enabled(item.collider, false);
            }
        }
    }
}

/// A text mesh and its `TextShadower` copy (`TextShadowPositioner.LateUpdate`: offset by the
/// light's shadow delta, 0.025 deeper, scaled, black with the text's alpha, drawn with the
/// shadow font).
#[allow(clippy::too_many_arguments)]
fn text_with_shadow(
    texts: &mut LevelTexts,
    font: &str,
    shadow: &(Option<String>, f32),
    layout: &TextLayout,
    s: &str,
    colors: ([f32; 4], [f32; 4], bool),
    position: SimVec3,
    rotation: SimQuat,
    light: Option<SimVec3>,
) -> Vec<(u32, Geometry)> {
    let mut out = Vec::new();
    let r = (rotation.x, rotation.y, rotation.z, rotation.w);
    let shadow_font = shadow.0.clone().unwrap_or_else(|| font.to_owned());
    let (Some(&texture), Some(&shadow_texture)) = (texts.textures.get(font), texts.textures.get(&shadow_font)) else { return out };
    let Some(f) = texts.fonts.get(font) else { return out };
    let formatted = format_text(f, layout, s);
    let local = text_local_geometry(f, layout, &formatted, colors.0, colors.1, colors.2);
    let world = Affine::from_trs([position.x, position.y, position.z], r, [1.0; 3]);
    if let Some(light) = light {
        let (delta, k) = shadow_offset_and_scale(light, position, shadow.1);
        let mut shadow_layout = layout.clone();
        shadow_layout.formatting = false;
        shadow_layout.scale = [layout.scale[0] * k, layout.scale[1] * k, 1.0];
        let alpha = colors.0[3];
        if let Some(sf) = texts.fonts.get(&shadow_font) {
            let local_shadow = text_local_geometry(sf, &shadow_layout, &formatted, [0.0, 0.0, 0.0, alpha], [0.0, 0.0, 0.0, alpha], false);
            let at = [position.x + delta[0], position.y + delta[1], position.z + TEXT_SHADOW_DEPTH];
            out.push((shadow_texture, place(&local_shadow, &Affine::from_trs(at, r, [1.0; 3]))));
        }
    }
    out.push((texture, place(&local, &world)));
    out
}

fn queue_level_texts(mut texts: ResMut<LevelTexts>, light: Res<SceneLight>, mut queue: ResMut<FrameQueue>) {
    let texts = &mut *texts;
    let light = light.for_shadows;
    let mut renderers = Vec::new();
    for i in 0..texts.texts.len() {
        let t = texts.texts[i].text.clone();
        let p = t.text.world.t;
        let q = t.text.world_rotation;
        renderers.extend(text_with_shadow(
            texts,
            &t.text.font,
            &t.shadower.clone().unwrap_or((None, 0.0)),
            &TextLayout::of(&t.text),
            &t.text.text,
            (t.text.color, t.text.color2, t.text.use_gradient),
            SimVec3::new(p[0], p[1], p[2]),
            SimQuat::from_array(q),
            if t.shadower.is_some() { light } else { None },
        ));
    }
    for i in 0..texts.letters.len() {
        let (letter, font, shadow) = {
            let item = &texts.letters[i];
            (item.letter.clone(), item.font.clone(), item.shadow.clone())
        };
        let (position, rotation, color) = match &texts.letters[i].state {
            LetterState::Alive { .. } => (letter.position, SimQuat::IDENTITY, letter.color),
            // The object deactivates in this frame's `LateUpdate`, before anything is drawn.
            LetterState::Exploding { explosion, .. } => (explosion.position, explosion.rotation, explosion.color),
            LetterState::Gone => continue,
        };
        renderers.extend(text_with_shadow(texts, &font, &shadow, &letter.layout, &letter.ch.to_string(), (color, color, false), position, rotation, light));
    }
    for (texture, geometry) in renderers {
        if !geometry.is_empty() {
            queue.0.push_frame(texture, geometry);
        }
    }
}
