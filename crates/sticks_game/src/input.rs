//! Input devices: the keyboard and mouse and the gamepads, as the original's lobby and role
//! processors see them (`sticks_sim::devices` holds the processors' rules).
//!
//! * Every connected Bevy gamepad takes the lowest free index (0..11,
//!   `XboxishGamepadProcessor.IndexCount`), like an XInput user index; a gamepad that comes back
//!   after a disconnect usually gets its old index, and so its old place in control assignment.
//! * [`Gamepads`] holds each index's state for this frame ([`PadState`] with Bevy's y-up sticks,
//!   `XboxishGamepad.Update`'s pressed-this-frame flags) and the per-index processor state
//!   (`XboxishGamepadCharacterInput`, `XboxishGamepadLobbyInput`).
//! * [`MouseActivity`]: `MouseCursorManager.MouseMovedLastFrame` for the keyboard processor's
//!   status.
//!
//! Mapping (Xbox layout): A = South, B = East, X = West, Y = North, LB/RB = Left/RightTrigger,
//! LT/RT = Left/RightTrigger2 (analog), Back = Select, Start = Start, stick clicks =
//! Left/RightThumb.
//!
//! TODO(UNCERTAIN): the original lists an XInput and a "preconfigured" processor per index
//! ("Xbox Controller N" / "Controller N"); every gamepad is taken as an XInput one here.

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use sticks_sim::devices::{PadCharacter, PadEdges, PadHelicopter, PadHistory, PadLobby, PadState, ProcessorKind};

/// `XboxishGamepadProcessor.IndexCount`.
pub const MAX_GAMEPADS: usize = 11;

/// One lobby input / role processor family: the keyboard and mouse, or a gamepad index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InputDevice {
    KeyboardMouse,
    Gamepad(u8),
}

impl InputDevice {
    pub fn kind(self) -> ProcessorKind {
        match self {
            InputDevice::KeyboardMouse => ProcessorKind::KeyboardAndMouse,
            InputDevice::Gamepad(_) => ProcessorKind::XboxishGamepad,
        }
    }

    /// `LobbyInput.Label` (`KeyboardAndMouseLobbyInput.Label`, `XInputController.Name`).
    pub fn label(self) -> String {
        match self {
            InputDevice::KeyboardMouse => "Keyboard and Mouse".to_owned(),
            InputDevice::Gamepad(i) => format!("Xbox Controller {}", i + 1),
        }
    }
}

/// One gamepad index this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct PadFrame {
    pub state: PadState,
    pub edges: PadEdges,
    history: PadHistory,
    /// `XboxishGamepadCharacterInput`, `XboxishGamepadHelicopterInput` and
    /// `XboxishGamepadLobbyInput` of this index.
    pub character: PadCharacter,
    pub helicopter: PadHelicopter,
    pub lobby: PadLobby,
}

#[derive(Resource, Default)]
pub struct Gamepads {
    entities: [Option<Entity>; MAX_GAMEPADS],
    pub frames: [PadFrame; MAX_GAMEPADS],
}

impl Gamepads {
    /// `IsAvailable`: the keyboard and mouse always are (`Keyboard.AreKeyboardAndMouseAvailable`
    /// on desktop).
    pub fn available(&self, device: InputDevice) -> bool {
        match device {
            InputDevice::KeyboardMouse => true,
            InputDevice::Gamepad(i) => self.entities.get(i as usize).is_some_and(Option::is_some),
        }
    }

    /// `LobbyInput.All` that are available: the keyboard and mouse, then the gamepads by index.
    /// TODO(UNCERTAIN): the order of the Permanent System's processor components (the keyboard
    /// is taken to come first).
    pub fn devices(&self) -> Vec<InputDevice> {
        let mut out = vec![InputDevice::KeyboardMouse];
        out.extend((0..MAX_GAMEPADS).filter(|&i| self.entities[i].is_some()).map(|i| InputDevice::Gamepad(i as u8)));
        out
    }

    pub fn frame(&self, index: u8) -> Option<&PadFrame> {
        self.frames.get(index as usize).filter(|_| self.available(InputDevice::Gamepad(index)))
    }

    pub fn frame_mut(&mut self, index: u8) -> Option<&mut PadFrame> {
        if !self.available(InputDevice::Gamepad(index)) {
            return None;
        }
        self.frames.get_mut(index as usize)
    }
}

/// `MouseCursorManager.MouseMovedLastFrame`.
#[derive(Resource, Default)]
pub struct MouseActivity {
    moved_this_frame: bool,
    pub moved_last_frame: bool,
}

/// Device state is read before anything else of the frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceSet;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Gamepads>()
            .init_resource::<MouseActivity>()
            .configure_sets(Update, DeviceSet.before(crate::FrameSet::Input))
            .add_systems(Update, (update_gamepads, update_mouse_activity).in_set(DeviceSet));
    }
}

fn pad_state(g: &Gamepad) -> PadState {
    let analog = |b: GamepadButton| g.get(b).unwrap_or(if g.pressed(b) { 1.0 } else { 0.0 });
    let l = g.left_stick();
    let r = g.right_stick();
    PadState {
        left_stick: [l.x, l.y],
        right_stick: [r.x, r.y],
        a: g.pressed(GamepadButton::South),
        b: g.pressed(GamepadButton::East),
        x: g.pressed(GamepadButton::West),
        y: g.pressed(GamepadButton::North),
        start: g.pressed(GamepadButton::Start),
        back: g.pressed(GamepadButton::Select),
        lb: g.pressed(GamepadButton::LeftTrigger),
        rb: g.pressed(GamepadButton::RightTrigger),
        lt: analog(GamepadButton::LeftTrigger2),
        rt: analog(GamepadButton::RightTrigger2),
        dpad_up: g.pressed(GamepadButton::DPadUp),
        dpad_down: g.pressed(GamepadButton::DPadDown),
        dpad_left: g.pressed(GamepadButton::DPadLeft),
        dpad_right: g.pressed(GamepadButton::DPadRight),
        left_stick_click: g.pressed(GamepadButton::LeftThumb),
        right_stick_click: g.pressed(GamepadButton::RightThumb),
    }
}

/// Indices for new gamepads, freed ones for gone gamepads; this frame's states and edges.
fn update_gamepads(mut pads: ResMut<Gamepads>, gamepads: Query<(Entity, &Gamepad)>) {
    let pads = &mut *pads;
    for slot in &mut pads.entities {
        if slot.is_some_and(|e| gamepads.get(e).is_err()) {
            info!("gamepad disconnected");
            *slot = None;
        }
    }
    for (entity, _) in &gamepads {
        if pads.entities.contains(&Some(entity)) {
            continue;
        }
        if let Some(i) = pads.entities.iter().position(Option::is_none) {
            info!("gamepad connected as Xbox Controller {}", i + 1);
            pads.entities[i] = Some(entity);
            // A new device starts from released buttons and fresh processors.
            pads.frames[i] = PadFrame::default();
        }
    }
    for i in 0..MAX_GAMEPADS {
        let Some(gamepad) = pads.entities[i].and_then(|e| gamepads.get(e).ok()).map(|(_, g)| g) else { continue };
        let state = pad_state(gamepad);
        let frame = &mut pads.frames[i];
        frame.edges = frame.history.update(&state);
        frame.state = state;
    }
}

fn update_mouse_activity(motion: Res<AccumulatedMouseMotion>, mut activity: ResMut<MouseActivity>) {
    activity.moved_last_frame = activity.moved_this_frame;
    activity.moved_this_frame = motion.delta != Vec2::ZERO;
}
