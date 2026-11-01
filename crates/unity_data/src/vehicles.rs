//! Helicopters placed in a level (`HelicopterControl` roots: a `Rigidbody2D` with a polygon
//! collider, sprites under "Visuals", rotors, guns (`HelicopterGunnerControl`), seats
//! (`VehicleOccupant`), and for enemy helicopters an "Enemy" controller child with
//! `EnemyHelicopterInput` and its AI behaviours).

use crate::level::{EnemySpawn, enemy_of};
use crate::projectile::DifficultyScaling;
use crate::math::Affine;
use crate::scene::{FileId, GameObject, Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// `Rigidbody2D` settings.
#[derive(Clone, Debug, PartialEq)]
pub struct RigidbodySpec {
    pub mass: f32,
    pub linear_drag: f32,
    pub angular_drag: f32,
    pub gravity_scale: f32,
    pub kinematic: bool,
    /// `RigidbodyConstraints2D` bits.
    pub constraints: i64,
}

/// One object of a vehicle's hierarchy (the root is node 0).
#[derive(Clone, Debug, PartialEq)]
pub struct VehicleNode {
    pub id: FileId,
    pub name: String,
    pub parent: Option<usize>,
    pub local_position: [f32; 3],
    pub local_rotation: [f32; 4],
    pub local_scale: [f32; 3],
    pub active_self: bool,
}

/// A `HelicopterGunnerControl` and the `ProjectileLauncher` under it.
#[derive(Clone, Debug, PartialEq)]
pub struct HelicopterGun {
    pub node: usize,
    /// The launcher's object ("Launch Point").
    pub launch_point: usize,
    pub firing_rate_time: f32,
    pub fully_automatic: bool,
    /// Projectile prefab GUID.
    pub projectile: String,
    /// `ProjectileLauncherFiringRateTimeDifficultyModifier` on the launcher.
    pub firing_rate_scaling: Option<DifficultyScaling>,
    /// `ProjectileLauncher.firingSound` (a MasterAudio sound group; empty for none).
    pub firing_sound: String,
    /// The gun has its own `ControlSlot` (a gunner seat; players' helicopters only).
    pub control_slot: bool,
    /// That `ControlSlot.controlRadius`.
    pub control_radius: f32,
    /// File id of the `HelicopterGunnerControl` component.
    pub component: FileId,
    /// `HelicopterGunnerControl.gunner`: the node of the `VehicleOccupant` that shows the gunner.
    pub occupant: Option<usize>,
    /// `MoveToOtherControlSlotIfAvailable` on the seat: its gunner takes the pilot's seat when it
    /// is free.
    pub moves_to_pilot: bool,
}

/// A vehicle root's enabled `AudioSource` (its engine loop).
fn looping_sound(root: &GameObject<'_>) -> Option<LoopingSound> {
    root.component(class::AUDIO_SOURCE).filter(|c| c.body.bool_of("m_Enabled").unwrap_or(true)).and_then(|c| {
        let b = &c.body;
        let clip = b.get("m_audioClip").and_then(Value::guid)?.to_owned();
        let spatial_blend = b.get("panLevelCustomCurve").and_then(|c| c.get("m_Curve")).map(Value::as_seq).and_then(|keys| keys.first()).and_then(|k| k.f32_of("value")).unwrap_or(0.0);
        Some(LoopingSound {
            clip,
            volume: b.f32_of("m_Volume").unwrap_or(1.0),
            pitch: b.f32_of("m_Pitch").unwrap_or(1.0),
            play_on_awake: b.bool_of("m_PlayOnAwake").unwrap_or(true),
            looping: b.bool_of("Loop").unwrap_or(false),
            min_distance: b.f32_of("MinDistance").unwrap_or(1.0),
            max_distance: b.f32_of("MaxDistance").unwrap_or(500.0),
            rolloff: b.i64_of("rolloffMode").unwrap_or(0),
            spatial_blend,
            mixer_group: b.get("OutputAudioMixerGroup").and_then(Value::file_id).filter(|&f| f != 0),
        })
    })
}

/// A looping `AudioSource` on a vehicle (the engine).
#[derive(Clone, Debug, PartialEq)]
pub struct LoopingSound {
    /// Clip GUID.
    pub clip: String,
    pub volume: f32,
    pub pitch: f32,
    pub play_on_awake: bool,
    pub looping: bool,
    pub min_distance: f32,
    pub max_distance: f32,
    /// `rolloffMode` (0 logarithmic, 1 linear, 2 custom).
    pub rolloff: i64,
    /// First key of `panLevelCustomCurve` (the spatial blend).
    pub spatial_blend: f32,
    /// File id of the `OutputAudioMixerGroup` in `Mixer.mixer`.
    pub mixer_group: Option<i64>,
}

/// `HelicopterTopRotor` / `HelicopterBackRotor`.
#[derive(Clone, Debug, PartialEq)]
pub struct HelicopterRotor {
    pub node: usize,
    pub top: bool,
    /// Degrees per second.
    pub angular_velocity: f32,
    pub radius: f32,
    pub line_thickness: f32,
    pub color: [u8; 4],
    pub shadow_plane_z: f32,
}

/// A `VehicleOccupant` seat.
#[derive(Clone, Debug, PartialEq)]
pub struct OccupantSeat {
    pub node: usize,
    /// GUID of the occupant prefab.
    pub prefab: String,
    pub animation_state: String,
    /// `VehicleOccupant.enabled` as serialized.
    pub enabled: bool,
}

/// The "Enemy" controller of an enemy helicopter.
#[derive(Clone, Debug, PartialEq)]
pub struct EnemyHelicopter {
    pub node: usize,
    /// The soldier-style settings the controller shares with enemy characters (targeter,
    /// `HealthDifficultyModifier`, `DeactivateOffScreen`).
    pub settings: EnemySpawn,
    pub move_to_first_target_height: bool,
    pub fire_at_first_target: bool,
    /// `HelicopterColorSetter.bodyColor` (`Color32`) and `helmetColor`.
    pub body_color: [u8; 4],
    pub helmet_color: [f32; 4],
    /// `EnemyHelicopterGunnerCount.gunnerCount` and the seats / guns it switches.
    pub gunner_count: Option<(usize, Vec<usize>, Vec<usize>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HelicopterSpawn {
    pub name: String,
    pub root: FileId,
    pub layer: u8,
    pub body: RigidbodySpec,
    /// Collider paths in the root's local space.
    pub collider: Vec<Vec<[f32; 2]>>,
    /// `Health.startingHealth`, or `SetStartingHealth` when present.
    pub starting_health: f32,
    /// `HealthDifficultyModifier` on the root.
    pub health_scaling: Option<DifficultyScaling>,
    /// `ControlSlot.controlRadius`.
    pub control_radius: f32,
    pub nodes: Vec<VehicleNode>,
    /// World transform of the root as serialized.
    pub world: Affine,
    pub guns: Vec<HelicopterGun>,
    pub rotors: Vec<HelicopterRotor>,
    pub pilot: Option<OccupantSeat>,
    pub passengers: Vec<OccupantSeat>,
    pub fuselage: Option<usize>,
    pub disembarkment_point: Option<usize>,
    pub enemy: Option<EnemyHelicopter>,
    /// The root keeps a usable `ControlSlot`: present, and not destroyed by
    /// `RemovePlayerOnlyParts` (enemy helicopters).
    pub control_slot: bool,
    /// Index into `guns` of `HelicopterControl.singlePlayerGunnerControl`.
    pub single_player_gun: Option<usize>,
    /// The `CameraPositionInfluencer` object (`FixWorldOffsetFromTransform` keeps its world
    /// offset from the root; `LinkBehaviourEnablementToControlSlotControlledness` enables it while
    /// the root slot has a controller).
    pub camera_influencer: Option<usize>,
    /// The `Targetable` object ("Target"), unless `RemovePlayerOnlyParts` destroys it.
    pub target: Option<usize>,
    /// `RespawnerInstantiator` (kept unless `RemovePlayerOnlyParts` removes it).
    pub respawns: bool,
    /// The root's `AudioSource` (the engine loop).
    pub engine_sound: Option<LoopingSound>,
    /// `ExplodeSpritesAndLines.fadeWaitTime` on the root (none: `Explodable.Finished` at once).
    pub explosion_fade_wait: Option<f32>,
    /// `PlaySoundOnExplode.sound`.
    pub explode_sound: Option<String>,
}

impl HelicopterSpawn {
    /// The node holding object `id`.
    pub fn node_of(&self, id: FileId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    /// World transform of node `i` for a root transform `root` (local transforms below).
    pub fn world_of(&self, i: usize, root: &Affine, overrides: &dyn Fn(usize) -> Option<[f32; 4]>) -> Affine {
        let n = &self.nodes[i];
        match n.parent {
            None => *root,
            Some(p) => {
                let parent = self.world_of(p, root, overrides);
                let r = overrides(i).unwrap_or(n.local_rotation);
                parent.mul(&Affine::from_trs(n.local_position, (r[0], r[1], r[2], r[3]), n.local_scale))
            }
        }
    }
}

fn class_of<'s>(doc: &crate::yaml::Document, scripts: &'s ScriptIndex) -> Option<&'s str> {
    doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g))
}

fn script<'a>(go: &GameObject<'a>, scripts: &ScriptIndex, name: &str) -> Option<&'a crate::yaml::Document> {
    go.components().into_iter().find(|c| c.class_id == class::MONO_BEHAVIOUR && class_of(c, scripts) == Some(name))
}

fn color32(v: Option<&Value>) -> Option<[u8; 4]> {
    v.and_then(|c| c.i64_of("rgba")).map(|rgba| (rgba as u32).to_le_bytes())
}

/// Every active `HelicopterControl` root of the scene.
pub fn helicopters(scene: &Scene, scripts: &ScriptIndex) -> Vec<HelicopterSpawn> {
    let mut out = Vec::new();
    for root in scene.game_objects() {
        if root.parent().is_some() || !root.active_in_hierarchy() || script(&root, scripts, "HelicopterControl").is_none() {
            continue;
        }
        // The subtree, parents first.
        let mut nodes: Vec<VehicleNode> = Vec::new();
        let mut stack: Vec<(GameObject<'_>, Option<usize>)> = vec![(root, None)];
        let mut objects: Vec<GameObject<'_>> = Vec::new();
        while let Some((go, parent)) = stack.pop() {
            let t = go.transform();
            let v3 = |k: &str, d: [f32; 3]| t.and_then(|t| t.body.get(k)).and_then(Value::as_vec3).map_or(d, |(x, y, z)| [x, y, z]);
            let index = nodes.len();
            nodes.push(VehicleNode {
                id: go.id(),
                name: go.name().to_owned(),
                parent,
                local_position: v3("m_LocalPosition", [0.0; 3]),
                local_rotation: t.and_then(|t| t.body.get("m_LocalRotation")).and_then(Value::as_quat).map_or([0.0, 0.0, 0.0, 1.0], |(x, y, z, w)| [x, y, z, w]),
                local_scale: v3("m_LocalScale", [1.0; 3]),
                active_self: go.active_self(),
            });
            objects.push(go);
            for child in go.children().into_iter().rev() {
                stack.push((child, Some(index)));
            }
        }
        let world = root.world_affine();
        let inverse = world.inverse();
        let node_of = |id: FileId| nodes.iter().position(|n| n.id == id);
        let rb = root.component(class::RIGIDBODY_2D).map(|c| &c.body);
        let f = |k: &str, d: f32| rb.and_then(|b| b.f32_of(k)).unwrap_or(d);
        let body = RigidbodySpec {
            mass: f("m_Mass", 1.0),
            linear_drag: f("m_LinearDrag", 0.0),
            angular_drag: f("m_AngularDrag", 0.05),
            gravity_scale: f("m_GravityScale", 1.0),
            kinematic: rb.and_then(|b| b.bool_of("m_IsKinematic")).unwrap_or(false),
            constraints: rb.and_then(|b| b.i64_of("m_Constraints")).unwrap_or(0),
        };
        let mut collider = Vec::new();
        let mut layer = root.layer();
        let mut guns = Vec::new();
        let mut rotors = Vec::new();
        let mut pilot = None;
        let mut passengers = Vec::new();
        let mut enemy = None;
        let control = script(&root, scripts, "HelicopterControl").map(|c| &c.body);
        let pilot_id = control.and_then(|c| c.get("pilot")).and_then(Value::file_id);
        for (i, go) in objects.iter().enumerate() {
            for c in go.components() {
                if c.class_id == class::POLYGON_COLLIDER_2D && c.body.bool_of("m_Enabled").unwrap_or(true) && !c.body.bool_of("m_IsTrigger").unwrap_or(false) {
                    layer = go.layer();
                    let to_root = inverse.mul(&go.world_affine());
                    let offset = c.body.get("m_Offset").and_then(Value::as_vec2).unwrap_or((0.0, 0.0));
                    if let Some(paths) = c.body.get("m_Points").and_then(|p| p.get("m_Paths")) {
                        for path in paths.as_seq() {
                            let pts = path
                                .as_seq()
                                .iter()
                                .filter_map(Value::as_vec2)
                                .map(|(x, y)| {
                                    let p = to_root.transform_point([x + offset.0, y + offset.1, 0.0]);
                                    [p[0], p[1]]
                                })
                                .collect::<Vec<_>>();
                            if pts.len() >= 3 {
                                collider.push(pts);
                            }
                        }
                    }
                }
                if c.class_id != class::MONO_BEHAVIOUR {
                    continue;
                }
                let b = &c.body;
                match class_of(c, scripts) {
                    Some("HelicopterGunnerControl") => {
                        // The launcher under the gun.
                        let launcher = go.children().into_iter().find_map(|child| script(&child, scripts, "ProjectileLauncher").map(|l| (child, l)));
                        if let Some((child, l)) = launcher {
                            let lb = &l.body;
                            let scaling = script(&child, scripts, "ProjectileLauncherFiringRateTimeDifficultyModifier").map(|m| crate::level::scaling_of(&m.body));
                            guns.push(HelicopterGun {
                                node: i,
                                launch_point: node_of(child.id()).unwrap_or(i),
                                firing_rate_time: lb.f32_of("firingRateTime").unwrap_or(0.0),
                                fully_automatic: lb.bool_of("fullyAutomatic").unwrap_or(false),
                                projectile: lb.get("prefab").and_then(Value::guid).unwrap_or("").to_owned(),
                                firing_rate_scaling: scaling,
                                firing_sound: lb.str_of("firingSound").unwrap_or("").to_owned(),
                                control_slot: script(go, scripts, "ControlSlot").is_some(),
                                control_radius: script(go, scripts, "ControlSlot").and_then(|s| s.body.f32_of("controlRadius")).unwrap_or(10.0),
                                component: c.file_id,
                                occupant: b
                                    .get("gunner")
                                    .and_then(Value::file_id)
                                    .and_then(|id| scene.get(id))
                                    .and_then(|d| d.body.get("m_GameObject"))
                                    .and_then(Value::file_id)
                                    .and_then(node_of),
                                moves_to_pilot: script(go, scripts, "MoveToOtherControlSlotIfAvailable").is_some(),
                            });
                        }
                    }
                    Some(kind @ ("HelicopterTopRotor" | "HelicopterBackRotor")) => rotors.push(HelicopterRotor {
                        node: i,
                        top: kind == "HelicopterTopRotor",
                        angular_velocity: b.f32_of("angularVelocity").unwrap_or(720.0),
                        radius: b.f32_of("radius").unwrap_or(1.0),
                        line_thickness: b.f32_of("lineThickness").unwrap_or(0.25),
                        color: color32(b.get("color")).unwrap_or([128, 128, 128, 255]),
                        shadow_plane_z: b.f32_of("shadowPlaneZ").unwrap_or(0.0),
                    }),
                    Some("VehicleOccupant") => {
                        let seat = OccupantSeat {
                            node: i,
                            prefab: b.get("prefab").and_then(Value::guid).unwrap_or("").to_owned(),
                            animation_state: b.str_of("animationState").unwrap_or("").to_owned(),
                            enabled: b.bool_of("m_Enabled").unwrap_or(true),
                        };
                        // `HelicopterControl.pilot` references the component.
                        if pilot_id == Some(c.file_id) {
                            pilot = Some(seat);
                        } else {
                            passengers.push(seat);
                        }
                    }
                    Some("EnemyHelicopterInput") => {
                        let setter = script(go, scripts, "HelicopterColorSetter").map(|s| &s.body);
                        let gunner_count = script(go, scripts, "EnemyHelicopterGunnerCount").or_else(|| script(&root, scripts, "EnemyHelicopterGunnerCount")).map(|g| {
                            let refs = |k: &str| -> Vec<usize> {
                                g.body.get(k).map(Value::as_seq).unwrap_or(&[]).iter().filter_map(|r| r.file_id()).filter_map(|id| scene.get(id)).filter_map(|d| d.body.get("m_GameObject").and_then(Value::file_id)).filter_map(node_of).collect()
                            };
                            (g.body.i64_of("gunnerCount").unwrap_or(3).max(0) as usize, refs("gunners"), refs("rocketLaunchers"))
                        });
                        enemy = Some(EnemyHelicopter {
                            node: i,
                            settings: enemy_of(scene, go, scripts),
                            move_to_first_target_height: script(go, scripts, "HelicopterMoveToFirstTargetHeight").is_some(),
                            fire_at_first_target: script(go, scripts, "HelicopterFireAtFirstTarget").is_some(),
                            body_color: color32(setter.and_then(|s| s.get("bodyColor"))).unwrap_or([255; 4]),
                            helmet_color: setter.and_then(|s| s.get("helmetColor")).map_or([1.0; 4], |c| ["r", "g", "b", "a"].map(|k| c.f32_of(k).unwrap_or(1.0))),
                            gunner_count,
                        });
                    }
                    _ => {}
                }
            }
        }
        let health = script(&root, scripts, "SetStartingHealth")
            .and_then(|s| s.body.f32_of("startingHealth"))
            .or_else(|| script(&root, scripts, "Health").and_then(|s| s.body.f32_of("startingHealth")))
            .unwrap_or(100.0);
        // `RemovePlayerOnlyParts.Awake` (on an enemy's controller child) destroys the root's
        // "Target" child, its `RespawnerInstantiator` and its `ControlSlot`.
        // TODO(UNCERTAIN): the script also sets `enabled` on the instantiator it just destroyed;
        // the helicopters that carry it have one, so it runs to the end.
        let removes_player_parts = objects.iter().any(|go| script(go, scripts, "RemovePlayerOnlyParts").is_some());
        let single_player_gun = control.and_then(|c| c.get("singlePlayerGunnerControl")).and_then(Value::file_id).and_then(|id| guns.iter().position(|g| g.component == id));
        let camera_influencer = objects.iter().position(|go| script(go, scripts, "CameraPositionInfluencer").is_some());
        let target = objects.iter().position(|go| script(go, scripts, "Targetable").is_some()).filter(|&i| !(removes_player_parts && nodes[i].name == "Target" && nodes[i].parent == Some(0)));
        let engine_sound = looping_sound(&root);
        let fuselage = script(&root, scripts, "HelicopterColor").and_then(|s| s.body.get("fuselage")).and_then(Value::file_id).and_then(|id| scene.get(id)).and_then(|d| d.body.get("m_GameObject").and_then(Value::file_id)).and_then(node_of);
        out.push(HelicopterSpawn {
            name: root.name().to_owned(),
            root: root.id(),
            layer,
            body,
            collider,
            starting_health: health,
            health_scaling: script(&root, scripts, "HealthDifficultyModifier").map(|m| crate::level::scaling_of(&m.body)),
            control_radius: script(&root, scripts, "ControlSlot").and_then(|s| s.body.f32_of("controlRadius")).unwrap_or(10.0),
            disembarkment_point: nodes.iter().position(|n| n.name == "Disembarkment Point" && n.parent == Some(0)),
            nodes,
            world,
            guns,
            rotors,
            pilot,
            passengers,
            fuselage,
            enemy,
            control_slot: script(&root, scripts, "ControlSlot").is_some() && !removes_player_parts,
            single_player_gun,
            camera_influencer,
            target,
            respawns: script(&root, scripts, "RespawnerInstantiator").is_some() && !removes_player_parts,
            engine_sound,
            explosion_fade_wait: script(&root, scripts, "ExplodeSpritesAndLines").map(|d| d.body.f32_of("fadeWaitTime").unwrap_or(2.0)),
            explode_sound: script(&root, scripts, "PlaySoundOnExplode").and_then(|d| d.body.str_of("sound")).filter(|s| !s.is_empty()).map(str::to_owned),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Cars (`CarControl`: Moon, Moon Race, Corruption 2)
// ---------------------------------------------------------------------------------------------

/// A `WheelJoint2D` on a car root with the wheel body it connects.
#[derive(Clone, Debug, PartialEq)]
pub struct CarWheel {
    /// The wheel object (its own `Rigidbody2D` and `CircleCollider2D`; a child of the car).
    pub node: usize,
    pub body: RigidbodySpec,
    pub radius: f32,
    /// `CircleCollider2D.m_Offset` in the wheel's space.
    pub offset: [f32; 2],
    /// `PhysicsMaterial2D` friction of the wheel's collider (GUID; `None`: the default 0.4).
    pub material: Option<String>,
    /// `m_Anchor` (car space) and `m_ConnectedAnchor` (wheel space).
    pub anchor: [f32; 2],
    pub connected_anchor: [f32; 2],
    /// `m_Suspension`: `m_Frequency`, `m_DampingRatio`, `m_Angle` (degrees).
    pub frequency: f32,
    pub damping_ratio: f32,
    pub angle: f32,
}

/// `CarControl`'s tuning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarTuning {
    pub max_speed: f32,
    pub max_acceleration: f32,
    pub max_deceleration: f32,
    pub max_angular_acceleration: f32,
    pub acceleration_body_torque_factor: f32,
}

/// `RocketControl`'s tuning (Moon's lunar module): thrust along the body's up from `engine`, and
/// an angular velocity steered towards `-clockwise × max_angular_velocity`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RocketTuning {
    /// The `engine` transform (a node index), where the thrust is applied.
    pub engine: Option<usize>,
    pub max_acceleration: f32,
    /// Degrees per second.
    pub max_angular_velocity: f32,
    /// Degrees per second².
    pub max_angular_acceleration: f32,
    pub engine_particle_rate: f32,
}

/// A `CarGunnerControl` seat: its `VehicleOccupant` holds `weaponName` (set up on the occupant's
/// weapon pivots) and aims it with the gunner's input.
#[derive(Clone, Debug, PartialEq)]
pub struct CarGunner {
    pub seat: OccupantSeat,
    pub weapon: String,
    /// The seat's own `ControlSlot.controlRadius`.
    pub control_radius: f32,
}

/// A car placed in a level (also the lunar module: a `RocketControl` body without wheels).
#[derive(Clone, Debug, PartialEq)]
pub struct CarSpawn {
    pub name: String,
    pub root: FileId,
    pub layer: u8,
    pub body: RigidbodySpec,
    /// The body's polygon collider paths in root space.
    pub collider: Vec<Vec<[f32; 2]>>,
    pub nodes: Vec<VehicleNode>,
    pub world: Affine,
    pub wheels: Vec<CarWheel>,
    pub tuning: CarTuning,
    pub driver: Option<OccupantSeat>,
    /// `CarGunnerControl` seats (a `ControlSlot` each, taken once the driver's is).
    pub gunners: Vec<CarGunner>,
    pub control_radius: f32,
    pub disembarkment_point: Option<usize>,
    pub camera_influencer: Option<usize>,
    pub target: Option<usize>,
    pub starting_health: f32,
    pub explosion_fade_wait: Option<f32>,
    pub explode_sound: Option<String>,
    pub engine_sound: Option<LoopingSound>,
    /// `RocketControl` instead of `CarControl`.
    pub rocket: Option<RocketTuning>,
    /// Nodes with a `PassengerControl` seat (its own `ControlSlot`, taken once the pilot's is).
    pub passengers: Vec<usize>,
    /// `RocketControl.engineParticles`: the first `ParticleSystem` under the engine (the lunar
    /// module's "Emissions") and its node.
    pub exhaust: Option<(usize, crate::particles::ParticleEffectSpec)>,
    /// `GroundRaycastCameraInfluencer` on the root (the lunar module).
    pub ground_raycast_camera: bool,
    /// `TellOwnerToExplodeOnHealthExhausted` on the root: an exhausted vehicle explodes (the
    /// lunar module has `Health` but nothing that acts on it).
    pub explodes: bool,
}

impl CarSpawn {
    /// World transform of node `i` with the root at `root`.
    pub fn world_of_node(&self, i: usize, root: &Affine) -> Affine {
        let n = &self.nodes[i];
        match n.parent {
            None => *root,
            Some(p) => {
                let parent = self.world_of_node(p, root);
                let r = n.local_rotation;
                parent.mul(&Affine::from_trs(n.local_position, (r[0], r[1], r[2], r[3]), n.local_scale))
            }
        }
    }
}

fn v2(v: Option<&Value>) -> [f32; 2] {
    v.and_then(Value::as_vec2).map_or([0.0, 0.0], |(x, y)| [x, y])
}

fn rigidbody_of(go: &GameObject<'_>) -> RigidbodySpec {
    let rb = go.component(class::RIGIDBODY_2D).map(|c| &c.body);
    let f = |k: &str, d: f32| rb.and_then(|b| b.f32_of(k)).unwrap_or(d);
    RigidbodySpec {
        mass: f("m_Mass", 1.0),
        linear_drag: f("m_LinearDrag", 0.0),
        angular_drag: f("m_AngularDrag", 0.05),
        gravity_scale: f("m_GravityScale", 1.0),
        kinematic: rb.and_then(|b| b.bool_of("m_IsKinematic")).unwrap_or(false),
        constraints: rb.and_then(|b| b.i64_of("m_Constraints")).unwrap_or(0),
    }
}

/// Every active `CarControl` root of the scene.
pub fn cars(scene: &Scene, scripts: &ScriptIndex) -> Vec<CarSpawn> {
    let mut out = Vec::new();
    for root in scene.game_objects() {
        if !root.active_in_hierarchy() {
            continue;
        }
        let rocket_control = script(&root, scripts, "RocketControl");
        let Some(control) = script(&root, scripts, "CarControl").or(rocket_control) else { continue };
        let mut nodes: Vec<VehicleNode> = Vec::new();
        let mut objects: Vec<GameObject<'_>> = Vec::new();
        let mut stack: Vec<(GameObject<'_>, Option<usize>)> = vec![(root, None)];
        while let Some((go, parent)) = stack.pop() {
            let t = go.transform();
            let v3 = |k: &str, d: [f32; 3]| t.and_then(|t| t.body.get(k)).and_then(Value::as_vec3).map_or(d, |(x, y, z)| [x, y, z]);
            let index = nodes.len();
            nodes.push(VehicleNode {
                id: go.id(),
                name: go.name().to_owned(),
                parent,
                local_position: v3("m_LocalPosition", [0.0; 3]),
                local_rotation: t.and_then(|t| t.body.get("m_LocalRotation")).and_then(Value::as_quat).map_or([0.0, 0.0, 0.0, 1.0], |(x, y, z, w)| [x, y, z, w]),
                local_scale: v3("m_LocalScale", [1.0; 3]),
                active_self: go.active_self(),
            });
            objects.push(go);
            for child in go.children().into_iter().rev() {
                stack.push((child, Some(index)));
            }
        }
        let node_of = |id: FileId| nodes.iter().position(|n| n.id == id);
        let world = root.world_affine();
        let mut collider = Vec::new();
        for c in root.components() {
            if c.class_id != class::POLYGON_COLLIDER_2D || !c.body.bool_of("m_Enabled").unwrap_or(true) || c.body.bool_of("m_IsTrigger").unwrap_or(false) {
                continue;
            }
            let offset = v2(c.body.get("m_Offset"));
            for path in c.body.get("m_Points").and_then(|p| p.get("m_Paths")).map(Value::as_seq).unwrap_or(&[]) {
                let pts: Vec<[f32; 2]> = path.as_seq().iter().filter_map(Value::as_vec2).map(|(x, y)| [x + offset[0], y + offset[1]]).collect();
                if pts.len() >= 3 {
                    collider.push(pts);
                }
            }
        }
        // `WheelJoint2D` (class 235) components of the root and the bodies they connect.
        let mut wheels = Vec::new();
        for c in root.components() {
            if c.class_id != 235 || !c.body.bool_of("m_Enabled").unwrap_or(true) {
                continue;
            }
            let b = &c.body;
            let Some(wheel_go) = b.get("m_ConnectedRigidBody").and_then(Value::file_id).and_then(|id| scene.get(id)).and_then(|d| d.body.get("m_GameObject").and_then(Value::file_id)).and_then(|id| scene.game_object(id)) else { continue };
            let Some(node) = node_of(wheel_go.id()) else { continue };
            let circle = wheel_go.component(class::CIRCLE_COLLIDER_2D).map(|c| &c.body);
            let suspension = b.get("m_Suspension");
            wheels.push(CarWheel {
                node,
                body: rigidbody_of(&wheel_go),
                radius: circle.and_then(|c| c.f32_of("m_Radius")).unwrap_or(0.5),
                offset: v2(circle.and_then(|c| c.get("m_Offset"))),
                material: circle.and_then(|c| c.get("m_Material")).and_then(Value::guid).map(str::to_owned),
                anchor: v2(b.get("m_Anchor")),
                connected_anchor: v2(b.get("m_ConnectedAnchor")),
                frequency: suspension.and_then(|s| s.f32_of("m_Frequency")).unwrap_or(2.0),
                damping_ratio: suspension.and_then(|s| s.f32_of("m_DampingRatio")).unwrap_or(0.7),
                angle: suspension.and_then(|s| s.f32_of("m_Angle")).unwrap_or(90.0),
            });
        }
        let cb = &control.body;
        let driver_id = cb.get("driver").and_then(Value::file_id);
        let mut driver = None;
        let mut gunners = Vec::new();
        for (i, go) in objects.iter().enumerate() {
            for c in go.components() {
                if c.class_id != class::MONO_BEHAVIOUR {
                    continue;
                }
                match class_of(c, scripts) {
                    Some("VehicleOccupant") if driver_id == Some(c.file_id) => {
                        driver = Some(OccupantSeat {
                            node: i,
                            prefab: c.body.get("prefab").and_then(Value::guid).unwrap_or("").to_owned(),
                            animation_state: c.body.str_of("animationState").unwrap_or("").to_owned(),
                            enabled: c.body.bool_of("m_Enabled").unwrap_or(true),
                        });
                    }
                    Some("CarGunnerControl") => {
                        // `[RequireComponent(typeof(VehicleOccupant))]`: the occupant is on the same object.
                        let occupant = go.components().into_iter().find(|o| o.class_id == class::MONO_BEHAVIOUR && class_of(o, scripts) == Some("VehicleOccupant"));
                        let slot = go.components().into_iter().find(|o| o.class_id == class::MONO_BEHAVIOUR && class_of(o, scripts) == Some("ControlSlot"));
                        gunners.push(CarGunner {
                            seat: OccupantSeat {
                                node: i,
                                prefab: occupant.and_then(|o| o.body.get("prefab").and_then(Value::guid)).unwrap_or("").to_owned(),
                                animation_state: occupant.and_then(|o| o.body.str_of("animationState")).unwrap_or("").to_owned(),
                                enabled: occupant.and_then(|o| o.body.bool_of("m_Enabled")).unwrap_or(false),
                            },
                            weapon: c.body.str_of("weaponName").unwrap_or("").to_owned(),
                            control_radius: slot.and_then(|s| s.body.f32_of("controlRadius")).unwrap_or(10.0),
                        });
                    }
                    _ => {}
                }
            }
        }
        let health = script(&root, scripts, "SetStartingHealth")
            .and_then(|s| s.body.f32_of("startingHealth"))
            .or_else(|| script(&root, scripts, "Health").and_then(|s| s.body.f32_of("startingHealth")))
            .unwrap_or(100.0);
        // `RocketControl.Awake`: `engine.GetComponentInChildren<ParticleSystem>()`.
        let engine_node = rocket_control.and_then(|r| r.body.get("engine").and_then(Value::file_id).and_then(|t| scene.get(t)).and_then(|t| t.body.get("m_GameObject")).and_then(Value::file_id).and_then(node_of));
        let under = |mut i: usize, ancestor: usize| loop {
            if i == ancestor {
                return true;
            }
            match nodes[i].parent {
                Some(p) => i = p,
                None => return false,
            }
        };
        let exhaust = engine_node.and_then(|e| objects.iter().enumerate().filter(|(i, _)| under(*i, e)).find_map(|(i, go)| crate::particles::particle_effect_of(go, scripts).map(|spec| (i, spec))));
        out.push(CarSpawn {
            name: root.name().to_owned(),
            root: root.id(),
            layer: root.layer(),
            body: rigidbody_of(&root),
            collider,
            world,
            wheels,
            tuning: CarTuning {
                max_speed: cb.f32_of("maxSpeed").unwrap_or(18.0),
                max_acceleration: cb.f32_of("maxAcceleration").unwrap_or(18.0),
                max_deceleration: cb.f32_of("maxDeceleration").unwrap_or(18.0),
                max_angular_acceleration: cb.f32_of("maxAngularAcceleration").unwrap_or(270.0),
                acceleration_body_torque_factor: cb.f32_of("accelerationBodyTorqueFactor").unwrap_or(0.25),
            },
            driver,
            gunners,
            control_radius: script(&root, scripts, "ControlSlot").and_then(|s| s.body.f32_of("controlRadius")).unwrap_or(10.0),
            disembarkment_point: nodes.iter().position(|n| n.name == "Disembarkment Point" && n.parent == Some(0)),
            camera_influencer: objects.iter().position(|go| script(go, scripts, "CameraPositionInfluencer").is_some()),
            target: objects.iter().position(|go| script(go, scripts, "Targetable").is_some()),
            starting_health: health,
            explosion_fade_wait: script(&root, scripts, "ExplodeSpritesAndLines").map(|d| d.body.f32_of("fadeWaitTime").unwrap_or(2.0)),
            explode_sound: script(&root, scripts, "PlaySoundOnExplode").and_then(|d| d.body.str_of("sound")).filter(|s| !s.is_empty()).map(str::to_owned),
            engine_sound: looping_sound(&root),
            rocket: rocket_control.map(|r| RocketTuning {
                engine: engine_node,
                max_acceleration: r.body.f32_of("maxAcceleration").unwrap_or(20.0),
                max_angular_velocity: r.body.f32_of("maxAngularVelocity").unwrap_or(60.0),
                max_angular_acceleration: r.body.f32_of("maxAngularAcceleration").unwrap_or(135.0),
                engine_particle_rate: r.body.f32_of("engineParticleRate").unwrap_or(500.0),
            }),
            passengers: objects.iter().enumerate().filter(|(_, go)| script(go, scripts, "PassengerControl").is_some()).map(|(i, _)| i).collect(),
            explodes: script(&root, scripts, "TellOwnerToExplodeOnHealthExhausted").is_some(),
            exhaust,
            ground_raycast_camera: script(&root, scripts, "GroundRaycastCameraInfluencer").is_some(),
            nodes,
        });
    }
    out
}

/// A gun emplacement a player operates without leaving their character (`CannonControl` on a
/// non-exclusive `ControlSlot` with `IdleControllingCharacter`: Pirates' deck cannon). Its
/// "Pivot" turns to the aim (`HelicopterGunnerInput.RelativeAim`), the launchers under it fire.
#[derive(Clone, Debug, PartialEq)]
pub struct CannonSpawn {
    pub name: String,
    pub object: FileId,
    /// World position of the control slot (the cannon object).
    pub position: [f32; 3],
    pub control_radius: f32,
    /// The "Pivot": its world transform as saved and its world rotation about z (degrees).
    pub pivot: Affine,
    pub pivot_angle: f32,
    /// Every object under the pivot and its transform relative to the pivot (the sprites that
    /// turn with it, the launch points).
    pub parts: Vec<(FileId, Affine)>,
    /// The "Barrel" (`CannonColor` tints it with the operator's body colour).
    pub barrel: Option<FileId>,
    pub launchers: Vec<CannonLauncher>,
}

/// A `ProjectileLauncher` under a cannon's pivot.
#[derive(Clone, Debug, PartialEq)]
pub struct CannonLauncher {
    /// The launcher's object (its position relative to the pivot is in [`CannonSpawn::parts`]).
    pub object: FileId,
    pub projectile: String,
    pub firing_rate_time: f32,
    pub fully_automatic: bool,
    pub firing_rate_scaling: Option<DifficultyScaling>,
    pub firing_sound: String,
}

/// Every active `CannonControl`.
pub fn cannons(scene: &Scene, scripts: &ScriptIndex) -> Vec<CannonSpawn> {
    let mut out = Vec::new();
    for root in scene.game_objects() {
        if !root.active_in_hierarchy() || script(&root, scripts, "CannonControl").is_none() {
            continue;
        }
        let Some(pivot) = root.children().into_iter().find(|c| c.name() == "Pivot") else { continue };
        let pivot_world = pivot.world_affine();
        let to_pivot = pivot_world.inverse();
        let mut parts = Vec::new();
        let mut launchers = Vec::new();
        let mut barrel = None;
        let mut stack = pivot.children();
        while let Some(go) = stack.pop() {
            parts.push((go.id(), to_pivot.mul(&go.world_affine())));
            if go.name() == "Barrel" {
                barrel = Some(go.id());
            }
            if go.active_in_hierarchy() {
                if let Some(l) = script(&go, scripts, "ProjectileLauncher").filter(|l| l.body.bool_of("m_Enabled").unwrap_or(true)) {
                    let b = &l.body;
                    launchers.push(CannonLauncher {
                        object: go.id(),
                        projectile: b.get("prefab").and_then(Value::guid).unwrap_or("").to_owned(),
                        firing_rate_time: b.f32_of("firingRateTime").unwrap_or(0.1),
                        fully_automatic: b.bool_of("fullyAutomatic").unwrap_or(false),
                        firing_rate_scaling: script(&go, scripts, "ProjectileLauncherFiringRateTimeDifficultyModifier").map(|m| crate::level::scaling_of(&m.body)),
                        firing_sound: b.str_of("firingSound").unwrap_or("").to_owned(),
                    });
                }
            }
            stack.extend(go.children());
        }
        out.push(CannonSpawn {
            name: root.name().to_owned(),
            object: root.id(),
            position: root.world_affine().t,
            control_radius: script(&root, scripts, "ControlSlot").and_then(|s| s.body.f32_of("controlRadius")).unwrap_or(10.0),
            pivot_angle: pivot_world.m[1][0].atan2(pivot_world.m[0][0]).to_degrees(),
            pivot: pivot_world,
            parts,
            barrel,
            launchers,
        });
    }
    out
}
