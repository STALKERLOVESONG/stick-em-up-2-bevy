//! Projectile prefabs (`Staple.prefab`, `Rocket.prefab`, ...): the physical and damage data a
//! `ProjectileLauncher` spawns, and what happens when one hits something or runs out of time.

use std::path::Path;

use crate::assets::GuidIndex;
use crate::scene::{GameObject, Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// Where a projectile sprite's image comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpriteSource {
    /// `CurrentCollectionSprite.spriteName`: looked up by name in the level's collection
    /// (`SpriteFinder`), so the same prefab follows each level's art.
    Named,
    /// A `tk2dSprite` authored in the prefab: a fixed sprite of a fixed collection (`collection`
    /// GUID, `_spriteId`). `ProjectileSprite::sprite_name` holds its name when the collection
    /// could be read, and is empty otherwise.
    Fixed { collection: String, sprite_id: usize },
}

/// A sprite drawn for a projectile, relative to the projectile.
///
/// Both kinds end up as a `tk2dSprite` with a `SpriteShadower` (`CurrentCollectionSprite.Start`
/// adds them at run time; the `tk2dSprite` prefabs carry them already), which is what
/// `Explosion.Prepare` collects as the projectile's explosion parts.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileSprite {
    pub sprite_name: String,
    pub source: SpriteSource,
    pub scale: [f32; 3],
    pub color: [f32; 4],
    /// `shadowPlaneZ`: the plane its shadow is cast onto.
    pub shadow_plane_z: f32,
    /// Position and rotation `(x, y, z, w)` relative to the projectile's root.
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

/// `DifficultyModifier` of a projectile property (`amount` is the authored string, e.g. `50%`).
#[derive(Clone, Debug, PartialEq)]
pub struct DifficultyScaling {
    pub amount: String,
    pub affected_by_player_count: bool,
}

/// `ExplodeSpritesAndLines` on a projectile: its sprites become an `Explosion` when it explodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExplosionSpec {
    /// `fadeWaitTime` (seconds before the parts start fading).
    pub fade_wait_time: f32,
    /// `sticks`: parts of a collision explosion stay pinned to what was hit.
    pub sticks: bool,
}

/// Which collision callback turns a contact into `Explodable.Explode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactExplosion {
    /// Neither script: contacts only deal damage.
    Never,
    /// `TellOwnerToExplodeOnCollision`: the first contact (that is not with a
    /// `DontExplodeOnCollisionWithMe` collider) explodes it inside `OnCollisionEnter2D`.
    FirstContact,
    /// `TellOwnerToExplodeOnCollisionUnlessOtherIsDestroyed`: the contact is remembered and the
    /// explosion happens in `LateUpdate`, unless what was hit has been destroyed/exploded by then.
    UnlessOtherIsDestroyed,
}

/// `SetProjectileSpeedByCharge`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedByCharge {
    pub maximum_charge_time: f32,
    pub minimum_charge_time: f32,
}

impl SpeedByCharge {
    /// `GetSpeedMultiplierForChargeTime`: `min(1, t (1 - minimum) / maximum + minimum)`.
    pub fn multiplier(&self, charge_time: f32) -> f32 {
        (charge_time * (1.0 - self.minimum_charge_time) / self.maximum_charge_time + self.minimum_charge_time).min(1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileSpec {
    pub name: String,
    /// GUID of the prefab (the pool key).
    pub guid: String,
    /// Layer the prefab was authored on; launching sets the team's projectile layer instead.
    pub layer: u8,
    /// `prefab.transform.localPosition.z`: the depth projectiles are spawned at.
    pub spawn_z: f32,
    pub radius: f32,
    pub collider_offset: [f32; 2],
    pub mass: f32,
    pub gravity_scale: f32,
    pub linear_drag: f32,
    pub angular_drag: f32,
    /// `Rigidbody2D.collisionDetectionMode`: 1 is continuous.
    pub collision_detection: i64,
    pub damage: f32,
    pub damage_scaling: Option<DifficultyScaling>,
    pub speed: f32,
    /// Propelled projectiles ignore gravity and hold their launch velocity every fixed step.
    pub propelled: bool,
    /// Unstoppable projectiles use the "unstoppable" layers (they pass what others stop on).
    pub unstoppable: bool,
    /// `Projectile.launchAngularVelocity` (degrees per second, `Rigidbody2D.angularVelocity`).
    pub launch_angular_velocity: f32,
    /// `RotateToFaceVelocity`: every fixed step the body turns to its velocity.
    pub rotates_to_face_velocity: bool,
    /// `SpawnExpiration.time`.
    pub lifetime: Option<f32>,
    /// `DespawnOnSpawnExpiration`: running out of time removes it silently.
    pub despawns_on_expiration: bool,
    /// `ExplodeOnSpawnExpiration`: running out of time explodes it (fireworks).
    pub explodes_on_expiration: bool,
    /// `TellOwnerToExplodeOnCollision`: the first contact ends the projectile. Kept for callers
    /// that only need to know whether a contact stops it; [`ProjectileSpec::contact`] says how.
    pub explodes_on_collision: bool,
    pub contact: ContactExplosion,
    /// `ExplodeSpritesAndLines`.
    pub explosion: Option<ExplosionSpec>,
    /// `DespawnOnExplode`: back to the pool the frame after it explodes.
    pub despawns_on_explode: bool,
    /// `DespawnOnExplosionFinished`: back to the pool when its `Explosion` has faded.
    pub despawns_on_explosion_finished: bool,
    /// `Chargeable`: the launcher charges while the trigger is held and fires on release.
    pub chargeable: bool,
    pub speed_by_charge: Option<SpeedByCharge>,
    /// `Seeker.maximumAngularAcceleration` (degrees per second).
    pub seeker: Option<f32>,
    /// `SpawnLocallyOnExplode.prefab`.
    pub spawns_on_explode: Option<String>,
    /// `PlaySoundOnExplode.sound` (MasterAudio group).
    pub explosion_sound: Option<String>,
    /// `LifetimeSound.sound`: plays (following the projectile) while it is active.
    pub lifetime_sound: Option<String>,
    /// `LifetimeParticleSystem.prefab` and the position of its object relative to the root.
    pub lifetime_particles: Option<(String, [f32; 3])>,
    pub sprites: Vec<ProjectileSprite>,
}

fn relative_to_root(go: GameObject<'_>, root: GameObject<'_>) -> ([f32; 3], [f32; 4]) {
    // Compose local transforms from the root down (all authored scales are 1).
    let mut chain = Vec::new();
    let mut current = Some(go);
    while let Some(g) = current {
        if g.id() == root.id() {
            break;
        }
        chain.push(g);
        current = g.parent();
    }
    let mut position = unity_compat::Vec3::ZERO;
    let mut rotation = unity_compat::Quat::IDENTITY;
    for g in chain.iter().rev() {
        let Some(t) = g.transform() else { continue };
        let p = t.body.get("m_LocalPosition").and_then(Value::as_vec3).unwrap_or((0.0, 0.0, 0.0));
        let q = t.body.get("m_LocalRotation").and_then(Value::as_quat).unwrap_or((0.0, 0.0, 0.0, 1.0));
        position = position + rotation * unity_compat::Vec3::new(p.0, p.1, p.2);
        rotation = rotation * unity_compat::Quat::from_array([q.0, q.1, q.2, q.3]);
    }
    ([position.x, position.y, position.z], [rotation.x, rotation.y, rotation.z, rotation.w])
}

fn color_of(v: Option<&Value>) -> [f32; 4] {
    ["r", "g", "b", "a"].map(|k| v.and_then(|c| c.f32_of(k)).unwrap_or(1.0))
}

/// Parse a projectile prefab. Fixed `tk2dSprite`s keep an empty `sprite_name` (see
/// [`try_load_projectile`], which resolves them).
pub fn parse_projectile(scene: &Scene, scripts: &ScriptIndex) -> Option<ProjectileSpec> {
    let objects = scene.game_objects();
    let root = *objects.iter().find(|g| g.parent().is_none())?;
    let mut spec = ProjectileSpec {
        name: root.name().to_owned(),
        guid: String::new(),
        layer: root.layer(),
        spawn_z: root
            .transform()
            .and_then(|t| t.body.get("m_LocalPosition"))
            .and_then(Value::as_vec3)
            .map_or(0.0, |p| p.2),
        radius: 0.0,
        collider_offset: [0.0; 2],
        mass: 1.0,
        gravity_scale: 1.0,
        linear_drag: 0.0,
        angular_drag: 0.05,
        collision_detection: 0,
        damage: 0.0,
        damage_scaling: None,
        speed: 0.0,
        propelled: true,
        unstoppable: false,
        launch_angular_velocity: 0.0,
        rotates_to_face_velocity: false,
        lifetime: None,
        despawns_on_expiration: false,
        explodes_on_expiration: false,
        explodes_on_collision: false,
        contact: ContactExplosion::Never,
        explosion: None,
        despawns_on_explode: false,
        despawns_on_explosion_finished: false,
        chargeable: false,
        speed_by_charge: None,
        seeker: None,
        spawns_on_explode: None,
        explosion_sound: None,
        lifetime_sound: None,
        lifetime_particles: None,
        sprites: Vec::new(),
    };
    let mut have_projectile = false;
    for component in root.components() {
        match component.class_id {
            class::RIGIDBODY_2D => {
                let b = &component.body;
                spec.mass = b.f32_of("m_Mass").unwrap_or(1.0);
                spec.gravity_scale = b.f32_of("m_GravityScale").unwrap_or(1.0);
                spec.linear_drag = b.f32_of("m_LinearDrag").unwrap_or(0.0);
                spec.angular_drag = b.f32_of("m_AngularDrag").unwrap_or(0.05);
                spec.collision_detection = b.i64_of("m_CollisionDetection").unwrap_or(0);
            }
            class::CIRCLE_COLLIDER_2D => {
                spec.radius = component.body.f32_of("m_Radius").unwrap_or(0.0);
                spec.collider_offset = component.body.get("m_Offset").and_then(Value::as_vec2).map_or([0.0; 2], |(x, y)| [x, y]);
            }
            class::MONO_BEHAVIOUR => {
                let Some(guid) = component.body.get("m_Script").and_then(Value::guid) else { continue };
                let b = &component.body;
                match scripts.class_name(guid) {
                    Some("DamageOtherOnCollision") => spec.damage = b.f32_of("damage").unwrap_or(10.0),
                    Some("DamageOtherOnCollisionDifficultyModifier") => {
                        spec.damage_scaling = Some(DifficultyScaling {
                            amount: b.str_of("amount").unwrap_or("").to_owned(),
                            affected_by_player_count: b.bool_of("affectedByPlayerCount").unwrap_or(true),
                        });
                    }
                    Some("Projectile") => {
                        have_projectile = true;
                        spec.speed = b.f32_of("speed").unwrap_or(0.0);
                        spec.propelled = b.bool_of("propelled").unwrap_or(true);
                        spec.unstoppable = b.bool_of("unstoppable").unwrap_or(false);
                        spec.launch_angular_velocity = b.f32_of("launchAngularVelocity").unwrap_or(0.0);
                    }
                    Some("SpawnExpiration") => spec.lifetime = Some(b.f32_of("time").unwrap_or(8.0)),
                    Some("DespawnOnSpawnExpiration") => spec.despawns_on_expiration = true,
                    Some("ExplodeOnSpawnExpiration") => spec.explodes_on_expiration = true,
                    Some("TellOwnerToExplodeOnCollision") => {
                        spec.explodes_on_collision = true;
                        spec.contact = ContactExplosion::FirstContact;
                    }
                    Some("TellOwnerToExplodeOnCollisionUnlessOtherIsDestroyed") => {
                        spec.explodes_on_collision = true;
                        spec.contact = ContactExplosion::UnlessOtherIsDestroyed;
                    }
                    Some("ExplodeSpritesAndLines") => {
                        spec.explosion = Some(ExplosionSpec {
                            fade_wait_time: b.f32_of("fadeWaitTime").unwrap_or(2.0),
                            sticks: b.bool_of("sticks").unwrap_or(false),
                        });
                    }
                    Some("DespawnOnExplode") => spec.despawns_on_explode = true,
                    Some("DespawnOnExplosionFinished") => spec.despawns_on_explosion_finished = true,
                    Some("RotateToFaceVelocity") => spec.rotates_to_face_velocity = true,
                    Some("Chargeable") => spec.chargeable = true,
                    // `SetProjectileSpeedByCharge` derives from `Chargeable`.
                    Some("SetProjectileSpeedByCharge") => {
                        spec.chargeable = true;
                        spec.speed_by_charge = Some(SpeedByCharge {
                            maximum_charge_time: b.f32_of("maximumChargeTime").unwrap_or(1.6),
                            minimum_charge_time: b.f32_of("minimumChargeTime").unwrap_or(0.25),
                        });
                    }
                    Some("Seeker") => spec.seeker = Some(b.f32_of("maximumAngularAcceleration").unwrap_or(57.3)),
                    Some("SpawnLocallyOnExplode") => spec.spawns_on_explode = b.get("prefab").and_then(Value::guid).map(str::to_owned),
                    Some("PlaySoundOnExplode") => spec.explosion_sound = b.str_of("sound").map(str::to_owned),
                    Some("LifetimeSound") => spec.lifetime_sound = b.str_of("sound").map(str::to_owned),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if !have_projectile {
        return None;
    }
    for go in &objects {
        // Sprites under a "Sprite Shadow" object are the shadow copies `SpriteShadower` drives.
        if go.name() == "Sprite Shadow" {
            continue;
        }
        let mut sprite: Option<ProjectileSprite> = None;
        let mut shadow_plane: Option<f32> = None;
        for component in go.components() {
            if component.class_id != class::MONO_BEHAVIOUR {
                continue;
            }
            let Some(guid) = component.body.get("m_Script").and_then(Value::guid) else { continue };
            let b = &component.body;
            match scripts.class_name(guid) {
                Some("CurrentCollectionSprite") => {
                    let (position, rotation) = relative_to_root(*go, root);
                    sprite = Some(ProjectileSprite {
                        sprite_name: b.str_of("spriteName").unwrap_or("").to_owned(),
                        source: SpriteSource::Named,
                        scale: b.get("scale").and_then(Value::as_vec3).map_or([1.0; 3], |(x, y, z)| [x, y, z]),
                        color: color_of(b.get("color")),
                        shadow_plane_z: b.f32_of("shadowPlaneZ").unwrap_or(0.0),
                        position,
                        rotation,
                    });
                }
                Some("tk2dSprite") => {
                    let collection = b.get("collection").and_then(Value::guid).unwrap_or("").to_owned();
                    let sprite_id = b.i64_of("_spriteId").unwrap_or(0).max(0) as usize;
                    let (position, rotation) = relative_to_root(*go, root);
                    sprite = Some(ProjectileSprite {
                        sprite_name: String::new(),
                        source: SpriteSource::Fixed { collection, sprite_id },
                        scale: b.get("_scale").and_then(Value::as_vec3).map_or([1.0; 3], |(x, y, z)| [x, y, z]),
                        color: color_of(b.get("_color")),
                        shadow_plane_z: 0.0,
                        position,
                        rotation,
                    });
                }
                Some("SpriteShadower") => shadow_plane = Some(b.f32_of("shadowPlaneZ").unwrap_or(0.0)),
                Some("LifetimeParticleSystem") => {
                    if let Some(prefab) = b.get("prefab").and_then(Value::guid) {
                        let (position, _) = relative_to_root(*go, root);
                        spec.lifetime_particles = Some((prefab.to_owned(), position));
                    }
                }
                _ => {}
            }
        }
        if let Some(mut sprite) = sprite {
            // A prefab `tk2dSprite` is only an explosion part (and has a shadow) when a
            // `SpriteShadower` sits next to it; `CurrentCollectionSprite` always adds one.
            if let Some(plane) = shadow_plane {
                sprite.shadow_plane_z = plane;
            } else if matches!(sprite.source, SpriteSource::Fixed { .. }) {
                continue;
            }
            spec.sprites.push(sprite);
        }
    }
    Some(spec)
}

/// Find a launcher's prefab by GUID under `Assets/GameObject` and parse it. `Ok(None)` means the
/// prefab exists but is not a `Projectile` (the Sword's "Sword Swish" is a melee effect, not a
/// flying object). Fixed `tk2dSprite`s get their sprite names from their collections when those
/// can be found.
pub fn try_load_projectile(assets_dir: &Path, guid: &str) -> std::io::Result<Option<ProjectileSpec>> {
    let prefabs = GuidIndex::scan(&assets_dir.join("GameObject"), "prefab");
    let path = prefabs.path(guid).ok_or_else(|| std::io::Error::other(format!("no prefab with guid {guid}")))?;
    let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
    let scene = Scene::load(path)?;
    let Some(mut spec) = parse_projectile(&scene, &scripts) else { return Ok(None) };
    spec.guid = guid.to_owned();
    let mut libraries: Vec<(String, Option<crate::sprites::SpriteLibrary>)> = Vec::new();
    for sprite in &mut spec.sprites {
        let SpriteSource::Fixed { collection, sprite_id } = &sprite.source else { continue };
        if !libraries.iter().any(|(g, _)| g == collection) {
            libraries.push((collection.clone(), crate::sprites::SpriteLibrary::load(assets_dir, collection).ok()));
        }
        let library = libraries.iter().find(|(g, _)| g == collection).and_then(|(_, l)| l.as_ref());
        if let Some(def) = library.and_then(|l| l.collection.sprites.get(*sprite_id)) {
            sprite.sprite_name = def.name.clone();
        }
    }
    Ok(Some(spec))
}

/// [`try_load_projectile`] for callers that need a real projectile.
pub fn load_projectile(assets_dir: &Path, guid: &str) -> std::io::Result<ProjectileSpec> {
    try_load_projectile(assets_dir, guid)?.ok_or_else(|| std::io::Error::other(format!("prefab {guid}: not a projectile")))
}

/// One collider path of a melee effect, in the effect root's local space.
#[derive(Clone, Debug, PartialEq)]
pub struct MeleePiece {
    pub path: Vec<[f32; 2]>,
    /// `DontExplodeOnCollisionWithMe` on its object: projectiles bounce off it instead of
    /// exploding (the Sword's "Deflector").
    pub deflector: bool,
    /// The collider's `PhysicsMaterial2D` bounciness (0 without a material).
    pub bounciness: f32,
}

/// A melee effect a `ProjectileLauncher` spawns (the Sword's "Sword Swish"): a kinematic body
/// that `FollowProjectileLauncher` keeps at the launch point, damaging what it touches
/// (`DamageOtherOnCollision`) until `SpawnExpiration` despawns it.
#[derive(Clone, Debug, PartialEq)]
pub struct MeleeSpec {
    pub name: String,
    pub guid: String,
    /// `DamageOtherOnCollision.damage`.
    pub damage: f32,
    /// `DamageOtherOnCollisionDifficultyModifier` (none on the Sword Swish).
    pub damage_scaling: Option<DifficultyScaling>,
    /// `SpawnExpiration.time`.
    pub lifetime: f32,
    /// `TakeResponsibilityForCollidingDamagers`: what bounces off it becomes the swinger's.
    pub takes_responsibility: bool,
    pub pieces: Vec<MeleePiece>,
}

/// `PhysicsMaterial2D.bounciness` of the material with this GUID.
fn bounciness_of(assets_dir: &Path, guid: &str) -> Option<f32> {
    let materials = GuidIndex::scan(&assets_dir.join("PhysicsMaterial2D"), "physicsMaterial2D");
    let text = std::fs::read_to_string(materials.path(guid)?).ok()?;
    text.lines().find_map(|l| l.trim().strip_prefix("bounciness:").and_then(|v| v.trim().parse().ok()))
}

/// Parse a melee effect prefab (`None` when it is not one).
pub fn parse_melee(scene: &Scene, scripts: &ScriptIndex, assets_dir: &Path) -> Option<MeleeSpec> {
    let class_of = |doc: &crate::yaml::Document| doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g));
    let objects = scene.game_objects();
    let root = objects.iter().find(|go| go.parent().is_none())?;
    let field = |class: &str| root.components().into_iter().find(|c| class_of(c) == Some(class)).map(|c| &c.body);
    field("FollowProjectileLauncher")?;
    let damage = field("DamageOtherOnCollision")?.f32_of("damage").unwrap_or(10.0);
    let mut spec = MeleeSpec {
        name: root.name().to_owned(),
        guid: String::new(),
        damage,
        damage_scaling: field("DamageOtherOnCollisionDifficultyModifier").map(crate::level::scaling_of),
        lifetime: field("SpawnExpiration").and_then(|b| b.f32_of("time")).unwrap_or(8.0),
        takes_responsibility: field("TakeResponsibilityForCollidingDamagers").is_some(),
        pieces: Vec::new(),
    };
    for go in &objects {
        let Some(polygon) = go.component(class::POLYGON_COLLIDER_2D) else { continue };
        if !polygon.body.bool_of("m_Enabled").unwrap_or(true) {
            continue;
        }
        let (position, rotation) = relative_to_root(*go, *root);
        let q = unity_compat::Quat::from_array(rotation);
        let offset = polygon.body.get("m_Offset").and_then(Value::as_vec2).unwrap_or((0.0, 0.0));
        let deflector = go.components().iter().any(|c| class_of(c) == Some("DontExplodeOnCollisionWithMe"));
        let bounciness = polygon.body.get("m_Material").and_then(Value::guid).and_then(|g| bounciness_of(assets_dir, g)).unwrap_or(0.0);
        for path in polygon.body.get("m_Points").and_then(|p| p.get("m_Paths")).map(Value::as_seq).unwrap_or(&[]) {
            let points = path
                .as_seq()
                .iter()
                .filter_map(Value::as_vec2)
                .map(|(x, y)| {
                    let p = q * unity_compat::Vec3::new(x + offset.0, y + offset.1, 0.0);
                    [p.x + position[0], p.y + position[1]]
                })
                .collect();
            spec.pieces.push(MeleePiece { path: points, deflector, bounciness });
        }
    }
    Some(spec)
}

/// Find a launcher's prefab by GUID and parse it as a melee effect.
pub fn try_load_melee(assets_dir: &Path, guid: &str) -> std::io::Result<Option<MeleeSpec>> {
    let prefabs = GuidIndex::scan(&assets_dir.join("GameObject"), "prefab");
    let path = prefabs.path(guid).ok_or_else(|| std::io::Error::other(format!("no prefab with guid {guid}")))?;
    let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
    let scene = Scene::load(path)?;
    Ok(parse_melee(&scene, &scripts, assets_dir).map(|mut spec| {
        spec.guid = guid.to_owned();
        spec
    }))
}
