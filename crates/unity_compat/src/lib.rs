//! Unity 5.1 numerical and timing behavior that is observable in Stick 'Em Up 2.
//!
//! This crate is deliberately tiny. It holds only the pieces where reproducing Unity's exact
//! arithmetic matters (`Mathf`, `SmoothDamp`, the fixed-step loop). It is *not* an engine
//! emulation layer; the game itself is built on Bevy ECS.

pub mod mathf;
pub mod quat;
pub mod time;

pub use mathf::Vec2;
pub use quat::{Quat, Vec3};
