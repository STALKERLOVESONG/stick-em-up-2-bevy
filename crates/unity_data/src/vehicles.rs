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
    /// File id of the `HelicopterGunnerControl` component.
    pub component: FileId,
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
                                component: c.file_id,
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
        let engine_sound = root.component(class::AUDIO_SOURCE).filter(|c| c.body.bool_of("m_Enabled").unwrap_or(true)).and_then(|c| {
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
        });
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
        });
    }
    out
}
