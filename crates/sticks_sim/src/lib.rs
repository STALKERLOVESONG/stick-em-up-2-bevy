//! Engine-independent rules of Stick 'Em Up 2.
//!
//! The Bevy game crate owns entities, scheduling, input and rendering; this crate owns the
//! *numbers*: collision geometry, the character controller's per-step logic, the animator-driven
//! ground locomotion, and the camera rig. Keeping it free of engine types lets every rule be
//! unit-tested against the recovered constants and serialized data.

pub mod ai;
pub mod aim;
pub mod animator;
pub mod audio;
pub mod camera;
pub mod character;
pub mod draw;
pub mod helicopter;
pub mod explosion;
pub mod letters;
pub mod life;
pub mod lobby;
pub mod particles;
pub mod physics;
pub mod pose;
pub mod rigid_body;
pub mod rng;
pub mod setup;
pub mod surface;
pub mod text;
pub mod visual;
pub mod vehicles;
pub mod weapons;

pub use unity_compat::Vec2;
