//! The Lobby's posed `Man` models: the four figures on the game summary screen and on each
//! colour customisation screen.
//!
//! In the original these are full enemy-style `Man` instances (`CharacterControl2D`, physics
//! body, `EnemyCharacterInput`) standing on the screen's ground: they drop from where the scene
//! places them, stand idle holding the Lobby's only allowed weapon (`AllowedWeapons`: the
//! Shuriken) and aim along `ColorModels.aim`. `RemovePlayerOnlyParts` strips their camera
//! target; their targeter finds nothing to shoot at. `ColorModels` shows one per active player,
//! fades their colours towards the player's (`Color.Lerp` at 15 × Δt per frame) and, on the
//! summary screen, closes the gaps between them and groups them by team around a "vs" label.

use std::sync::Arc;

use crate::aim::{AimedWeapon, PosedCharacter};
use crate::animator::AnimatorModel;
use crate::character::{Character, CharacterInput, StandingColliders};
use crate::lobby::{LobbyTeams, MAX_PLAYERS, color32_to_color, lerp_color};
use crate::physics::{PhysicsRules, World};
use crate::pose::CharacterRigModel;
use crate::visual::{CharacterPalette, CharacterVisual, to_color32};
use crate::weapons::Loadout;
use crate::weapons::corrected_aim;
use unity_compat::Vec2;

/// `ColorModels.aim` (normalised).
pub fn model_aim() -> Vec2 {
    let (x, y) = (1.91f32, 0.51f32);
    let length = (x * x + y * y).sqrt();
    Vec2::new(x / length, y / length)
}

/// `ColorModels.Update`'s colour fade rate (`t = 15 × Time.deltaTime`).
pub const COLOR_FADE_RATE: f32 = 15.0;

/// One posed model.
#[derive(Clone, Debug)]
pub struct LobbyModel {
    pub character: Character,
    pub input: CharacterInput,
    /// `CharacterControl2D`'s corrected aim (kept between frames).
    pub corrected_aim: Vec2,
    /// `CharacterColor` (body and face are `Color32`, the helmet a `Color`).
    pub palette: CharacterPalette,
    /// `GameObject.activeSelf` of the model's root.
    pub active: bool,
}

impl LobbyModel {
    /// A model whose root stands at `position`; `palette` is what `CharacterColorSetter.Awake`
    /// gave it (grey body and helmet, a random face).
    pub fn new(animation: Arc<AnimatorModel>, rules: &PhysicsRules, position: Vec2, palette: CharacterPalette) -> LobbyModel {
        let character = Character::new(animation, rules, StandingColliders::MAN, position);
        let aim = model_aim();
        LobbyModel { character, input: CharacterInput { aim, ..CharacterInput::default() }, corrected_aim: aim, palette, active: true }
    }

    /// `ColorModels.SetLocalX`: a move of less than 0.1 is skipped; the rigid body goes with
    /// its transform.
    pub fn set_x(&mut self, x: f32) {
        if (self.character.body.position.x - x).abs() >= 0.1 {
            self.character.body.position.x = x;
        }
    }

    /// `EnemyCharacterInput.RelativeAim`.
    pub fn set_aim(&mut self, aim: Vec2) {
        self.input.aim = aim;
    }

    /// One physics step of an active model.
    pub fn fixed_step(&mut self, world: &World, rules: &PhysicsRules, delta: f32, frame_count: u64) {
        if self.active {
            self.character.fixed_update(&self.input, world, rules, delta, frame_count);
        }
    }

    /// `CharacterControl2D.Update` and the animator; then the aim the weapon is posed with.
    pub fn frame_update(&mut self, delta: f32) -> Vec2 {
        if self.active && delta > 0.0 {
            self.character.frame_update(&self.input, delta);
        }
        self.corrected_aim = corrected_aim(self.input.aim, self.corrected_aim, self.character.facing_right());
        self.corrected_aim
    }

    /// `ColorModels.Update`'s fade towards the player's colours. Each frame goes through
    /// `Color` and back to `Color32` for the body and face (truncating), as the original
    /// assigns into `Color32` fields.
    pub fn fade_colors(&mut self, body: [f32; 4], helmet: [f32; 4], face: Option<[f32; 4]>, delta: f32) {
        let t = COLOR_FADE_RATE * delta;
        let body_now = color32_to_color(self.palette.body);
        self.palette.body = to_color32(lerp_color(body_now, body, t));
        self.palette.helmet = lerp_color(self.palette.helmet, helmet, t);
        let face_now = color32_to_color(self.palette.face);
        self.palette.face = to_color32(lerp_color(face_now, face.unwrap_or(face_now), t));
    }
}

impl LobbyModel {
    /// The rig as it stands this frame, holding `weapon` aimed along the corrected aim.
    pub fn posed(&self, rig: &CharacterRigModel, weapon: Option<usize>) -> PosedCharacter {
        let face_right = self.character.facing_right();
        let aimed = weapon.map(|weapon| AimedWeapon { weapon, aim: self.corrected_aim, face_right });
        rig.pose_character(&self.character.animator.pose(), self.character.position(), face_right, aimed)
    }

    /// What the model draws (`LinesRenderer` lines and flat sprites, with shadows).
    pub fn visual(&self, rig: &CharacterRigModel, weapon: Option<usize>, light: Option<unity_compat::Vec3>) -> CharacterVisual {
        let posed = self.posed(rig, weapon);
        rig.visual(&posed.solved, &posed.placement, &self.palette, light, &|bone| rig.active_with_weapon(bone, weapon))
    }
}

/// `CharacterControl2D.Start` for an enemy-style model: the first of the rig's weapons the
/// scene's `AllowedWeapons` allows (every weapon when there is none).
pub fn equipped_weapon(rig: &unity_data::rig::Rig, allowed: Option<&unity_data::level::AllowedWeapons>) -> Option<usize> {
    let names: Vec<&str> = rig.weapons.iter().map(|w| w.name.as_str()).collect();
    Loadout::new(&names, |name| allowed.is_none_or(|a| a.allows(name, true))).current()
}

/// Where `ColorModels.Update` puts the summary screen's models (`removeGapsAndGroupByTeams`).
#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    /// New x of each model (`None`: left where it is).
    pub x: [Option<f32>; MAX_PLAYERS],
    /// Models of the second team aim to the left.
    pub mirrored: [bool; MAX_PLAYERS],
    /// The "vs" label's world x when it is shown (Team Versus only).
    pub versus_label_x: Option<f32>,
}

/// `ColorModels.Update` with `removeGapsAndGroupByTeams`: shown models side by side from
/// `start_x`, `offset_x` apart; in Team Versus team A, a gap of 1.1 offsets with the label in
/// its middle, then team B (mirrored).
pub fn arrange_models(shown: [bool; MAX_PLAYERS], start_x: f32, offset_x: f32, team_versus: Option<&LobbyTeams>) -> Arrangement {
    let mut out = Arrangement { x: [None; MAX_PLAYERS], mirrored: [false; MAX_PLAYERS], versus_label_x: None };
    let mut x = start_x;
    match team_versus {
        None => {
            for (i, &on) in shown.iter().enumerate() {
                if on {
                    out.x[i] = Some(x);
                    x += offset_x;
                }
            }
        }
        Some(teams) => {
            for team in 0..2 {
                for &player in teams.team(team) {
                    if player < MAX_PLAYERS && shown[player] {
                        out.x[player] = Some(x);
                        out.mirrored[player] = team == 1;
                        x += offset_x;
                    }
                }
                if team == 0 {
                    let gap = offset_x * 1.1;
                    out.versus_label_x = Some((x - offset_x + x + gap) / 2.0);
                    x += gap;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_models_close_gaps_and_group_by_team() {
        let a = arrange_models([false, true, false, true], 10.0, 2.0, None);
        assert_eq!(a.x, [None, Some(10.0), None, Some(12.0)]);
        assert_eq!(a.versus_label_x, None);
        let teams = LobbyTeams::default();
        let t = arrange_models([true, true, true, false], 0.0, 2.0, Some(&teams));
        // Team A: 0, 1 at 0 and 2; gap 2.2; team B: 2 at 6.2.
        assert_eq!(t.x[0], Some(0.0));
        assert_eq!(t.x[1], Some(2.0));
        assert!((t.x[2].unwrap() - 6.2).abs() < 1e-5);
        assert_eq!(t.mirrored, [false, false, true, false]);
        assert!((t.versus_label_x.unwrap() - (2.0 + 6.2) / 2.0).abs() < 1e-5);
    }

    #[test]
    fn colours_fade_through_color32() {
        let rig_palette = CharacterPalette { body: [120, 120, 120, 255], face: [200, 150, 120, 255], helmet: [0.35, 0.35, 0.35, 1.0] };
        let mut palette = rig_palette;
        let target = [239.0 / 255.0, 66.0 / 255.0, 66.0 / 255.0, 1.0];
        for _ in 0..200 {
            let t = COLOR_FADE_RATE / 60.0;
            let body_now = color32_to_color(palette.body);
            palette.body = to_color32(lerp_color(body_now, target, t));
        }
        // Truncation stalls a rising channel once a step adds less than one unit: 239 × 0.25
        // steps stop at 236 (3 × 0.25 < 1).
        assert_eq!(palette.body[0], 236);
        assert_eq!(palette.body[1], 66);
    }
}
