//! Readers for the exported Unity 5.1 project (`assets_exported/ExportedProject`).
//!
//! These read *serialized data* (scenes, prefabs, animation clips, controllers, settings). They
//! do not model Unity's object system; the game consumes the typed results.

pub mod anim;
pub mod assets;
pub mod audio;
pub mod fonts;
pub mod humanoid;
pub mod interactive;
pub mod level;
pub mod material;
pub mod particles;
pub mod math;
pub mod projectile;
pub mod rig;
pub mod scene;
pub mod scripts;
pub mod settings;
pub mod sprites;
pub mod vehicles;
pub mod yaml;
