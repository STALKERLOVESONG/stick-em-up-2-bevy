//! The Lobby's posed `Man` models (`sticks_sim::lobby_models`): stepped on the Lobby's own
//! static colliders, faded and arranged by `ColorModels`, and drawn with the same lines and flat
//! sprites as the characters in a level.

use std::path::Path;

use sticks_sim::draw::Geometry;
use sticks_sim::lobby_models::{LobbyModel, equipped_weapon};
use sticks_sim::physics::{PhysicsRules, World};
use sticks_sim::rng::GameRng;
use sticks_sim::visual::CharacterPalette;
use unity_compat::Vec2;
use unity_compat::time::FixedStepper;
use unity_data::interactive::InteractiveScene;

use crate::figure::ManArt;
use crate::unity_render::{TextureLoad, UnityTextures};

pub(super) struct Figures {
    pub(super) art: ManArt,
    rules: PhysicsRules,
    world: World,
    stepper: FixedStepper,
    /// The weapon every model holds (`AllowedWeapons` of the Lobby: the Shuriken).
    weapon: Option<usize>,
    /// Each model's root object and its simulation.
    pub(super) models: Vec<(usize, LobbyModel)>,
    /// `UnityEngine.Random` for the models' faces.
    pub(super) rng: GameRng,
}

impl Figures {
    /// Every model a `ColorModels` lists, as `CharacterColorSetter.Awake` coloured it (its
    /// "Enemy" child's body and helmet, a random face).
    pub(super) fn load(scene: &InteractiveScene, export_root: &Path, scene_path: &str, textures: &mut UnityTextures, load: &mut TextureLoad<'_, '_>) -> Option<Figures> {
        let assets = export_root.join("Assets");
        let settings_text = std::fs::read_to_string(export_root.join("ProjectSettings/Physics2DSettings.asset")).ok()?;
        let settings = unity_data::settings::parse_physics2d(&settings_text)?;
        let rules = sticks_sim::setup::physics_rules(&settings);
        let animation = sticks_sim::animator::AnimatorModel::new(unity_data::anim::load_character_animation(&assets).ok()?);
        let art = ManArt::for_scene(&assets, &export_root.join(scene_path), textures, load)?;
        let weapon = equipped_weapon(art.model().rig(), scene.level.allowed_weapons.as_ref());
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.subsec_nanos());
        let mut rng = GameRng::from_seed(seed);
        let prefab = art.prefab_palette();
        let mut models = Vec::new();
        for s in scene.scripts_named("ColorModels") {
            for root in scene.field_objects(s, "models").into_iter().flatten() {
                if models.iter().any(|(r, _)| *r == root) {
                    continue;
                }
                let setter = scene.child(root, "Enemy").and_then(|e| scene.script(e, "CharacterColorSetter"));
                let body = setter.and_then(|s| s.fields.get("bodyColor")).and_then(|c| c.i64_of("rgba")).map_or(prefab.body, |rgba| (rgba as u32).to_le_bytes());
                let helmet = setter.and_then(|s| s.fields.get("helmetColor")).map_or(prefab.helmet, |c| ["r", "g", "b", "a"].map(|k| c.f32_of(k).unwrap_or(1.0)));
                let face = sticks_sim::visual::random_face_color(&mut rng);
                let p = scene.objects[root].world.t;
                let model = LobbyModel::new(animation.clone(), &rules, Vec2::new(p[0], p[1]), CharacterPalette { body, face, helmet });
                models.push((root, model));
            }
        }
        let world = sticks_sim::setup::build_world(&scene.level);
        Some(Figures { art, rules, world, stepper: FixedStepper::new(), weapon, models, rng })
    }

    pub(super) fn model_mut(&mut self, root: usize) -> Option<&mut LobbyModel> {
        self.models.iter_mut().find(|(r, _)| *r == root).map(|(_, m)| m)
    }

    /// The frame's fixed steps for the models whose root is active (`active`).
    pub(super) fn fixed_steps(&mut self, real_delta: f32, active: impl Fn(usize) -> bool) -> f32 {
        let frame = self.stepper.begin_frame(real_delta);
        for (root, model) in &mut self.models {
            model.active = active(*root);
        }
        while let Some(delta) = self.stepper.next_step() {
            for (_, model) in &mut self.models {
                model.fixed_step(&self.world, &self.rules, delta, frame.frame_count);
            }
            self.stepper.synchronize_fixed_delta();
        }
        frame.delta
    }

    /// `CharacterControl2D.Update` and the animator of every active model; the roots' local
    /// positions follow their bodies (`Rigidbody2D` moves the transform).
    pub(super) fn frame_update(&mut self, delta: f32, local: &mut [[f32; 3]]) {
        for (root, model) in &mut self.models {
            model.frame_update(delta);
            let p = model.character.position();
            local[*root][0] = p.x;
            local[*root][1] = p.y;
        }
    }

    /// The renderers of the models whose root is shown.
    pub(super) fn renderers(&self, shown: impl Fn(usize) -> bool, light: Option<unity_compat::Vec3>) -> Vec<Geometry> {
        let mut out = Vec::new();
        for (root, model) in &self.models {
            if shown(*root) {
                out.extend(model.visual(self.art.model(), self.weapon, light).renderers(&self.art.art));
            }
        }
        out
    }
}
