//! Input devices as the original's processors read them (`ControlTypes`, `RoleInput`,
//! `XboxishGamepad*`), independent of the engine's input API.
//!
//! * [`PadState`]: one frame of an Xbox-style gamepad. Sticks are **y-up** here (the engine's
//!   convention); the original's `XboxishGamepad` sticks are y-down (`XInputController` negates
//!   the thumbstick y), so every comparison below is the original's with the sign of y flipped.
//! * [`PadEdges`]: `XboxishGamepad.Update`'s "pressed this frame" flags (once per frame).
//! * [`PadCharacter`] (`XboxishGamepadCharacterInput`), [`PadLobby`] (`XboxishGamepadLobbyInput`)
//!   and [`pad_menu`] (`XboxishGamepadMenuInput`).
//! * [`RoleInputChoice`]: `RoleInput.Update`'s choice of the current processor among a player's
//!   processors (the first active one, else the current one, else the idle one with the highest
//!   `ProcessorPrioritizer` rank).
//!
//! * [`PadHelicopter`] (`XboxishGamepadHelicopterInput`) and [`keyboard_helicopter_status`].

use unity_compat::Vec2;

/// `XboxishGamepad.StickDeadzone`.
pub const STICK_DEADZONE: f32 = 0.2;
/// `XboxishGamepadCharacterInput.duckThreshold` (stick pushed down this far).
pub const DUCK_THRESHOLD: f32 = 0.4;
/// `XboxishGamepadCharacterInput.triggerThreshold` (1/256).
pub const TRIGGER_THRESHOLD: f32 = 0.003_906_25;
/// `XboxishGamepad.IsActive`'s trigger threshold.
const ACTIVE_TRIGGER: f32 = 0.1;

/// One frame of an Xbox-style gamepad (sticks y-up, triggers 0..1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    pub left_stick: [f32; 2],
    pub right_stick: [f32; 2],
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub start: bool,
    pub back: bool,
    pub lb: bool,
    pub rb: bool,
    pub lt: f32,
    pub rt: f32,
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub left_stick_click: bool,
    pub right_stick_click: bool,
}

impl PadState {
    /// `XboxishGamepad.IsActive`: any button, a trigger past 0.1 or a stick axis past the
    /// deadzone.
    pub fn is_active(&self) -> bool {
        let [lx, ly] = self.left_stick;
        let [rx, ry] = self.right_stick;
        self.a
            || self.b
            || self.x
            || self.y
            || self.start
            || self.back
            || self.lb
            || self.rb
            || self.lt >= ACTIVE_TRIGGER
            || self.rt >= ACTIVE_TRIGGER
            || self.dpad_up
            || self.dpad_down
            || self.dpad_left
            || self.dpad_right
            || self.left_stick_click
            || self.right_stick_click
            || lx.abs() >= STICK_DEADZONE
            || ly.abs() >= STICK_DEADZONE
            || rx.abs() >= STICK_DEADZONE
            || ry.abs() >= STICK_DEADZONE
    }
}

/// `XboxishGamepad.Update`: which buttons went down this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadEdges {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub start: bool,
    pub back: bool,
    pub lb: bool,
    pub rb: bool,
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub left_stick_click: bool,
    pub right_stick_click: bool,
}

/// The previous frame's buttons (`lastFrameA` ...), for [`PadEdges`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadHistory {
    last: PadState,
}

impl PadHistory {
    /// `Utility.ThisFrame(button, ref last)` for every button.
    pub fn update(&mut self, now: &PadState) -> PadEdges {
        let l = self.last;
        let edges = PadEdges {
            a: now.a && !l.a,
            b: now.b && !l.b,
            x: now.x && !l.x,
            y: now.y && !l.y,
            start: now.start && !l.start,
            back: now.back && !l.back,
            lb: now.lb && !l.lb,
            rb: now.rb && !l.rb,
            dpad_up: now.dpad_up && !l.dpad_up,
            dpad_down: now.dpad_down && !l.dpad_down,
            dpad_left: now.dpad_left && !l.dpad_left,
            dpad_right: now.dpad_right && !l.dpad_right,
            left_stick_click: now.left_stick_click && !l.left_stick_click,
            right_stick_click: now.right_stick_click && !l.right_stick_click,
        };
        self.last = *now;
        edges
    }
}

/// What a character processor hands `CharacterInput` this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterControls {
    pub movement: f32,
    pub jump: bool,
    pub duck: bool,
    pub fire: bool,
    /// `RelativeAim` (the gamepad has no absolute aim).
    pub aim: Vec2,
    pub next_weapon: bool,
    pub enter_vehicle: bool,
}

/// `XboxishGamepadCharacterInput`: its `FireTrigger` and the aim it keeps between frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadCharacter {
    old_fire_trigger: f32,
    fire: bool,
    /// `RelativeAim` (`Vector2.right` from `Awake`; only a stick past the deadzone changes it).
    aim: Vec2,
}

impl Default for PadCharacter {
    fn default() -> Self {
        Self { old_fire_trigger: 0.0, fire: false, aim: Vec2::new(1.0, 0.0) }
    }
}

impl PadCharacter {
    /// `ProcessGamepadInput`.
    pub fn process(&mut self, pad: &PadState, edges: &PadEdges) -> CharacterControls {
        let [lx, ly] = pad.left_stick;
        let movement = if lx.abs() < STICK_DEADZONE { 0.0 } else { lx };
        // `leftStick.y >= duckThreshold` with the original's y pointing down.
        let duck = -ly >= DUCK_THRESHOLD;
        let jump = pad.lt >= TRIGGER_THRESHOLD;
        fire_trigger(&mut self.old_fire_trigger, &mut self.fire, pad.rt);
        let [rx, ry] = pad.right_stick;
        if rx.abs() > STICK_DEADZONE || ry.abs() > STICK_DEADZONE {
            // `new Vector2(rightStick.x, -rightStick.y)`: up-positive, as here.
            self.aim = Vec2::new(rx, ry);
        }
        CharacterControls { movement, jump, duck, fire: self.fire, aim: self.aim, next_weapon: edges.y, enter_vehicle: edges.a }
    }
}

/// `XboxishGamepadCharacterInput.FireTrigger.Set`: the trigger fires on a rise past 1/256 and
/// stops on a fall below 0.2.
fn fire_trigger(old_trigger: &mut f32, fire: &mut bool, rt: f32) {
    let old = *old_trigger;
    let rising = rt - old >= 0.01 || (old == 0.0 && rt > 0.0);
    let falling = old - rt >= 0.01 || (rt == 0.0 && old > 0.0);
    if rising && rt >= TRIGGER_THRESHOLD {
        *fire = true;
    }
    if falling && rt <= 0.2 {
        *fire = false;
    }
    *old_trigger = rt;
}

/// What a helicopter processor hands `HelicopterInput` this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HelicopterControls {
    pub movement: Vec2,
    pub fire: bool,
    /// `RelativeAim` (`None` until the right stick first leaves its deadzone).
    pub aim: Option<Vec2>,
    pub exit: bool,
}

/// `XboxishGamepadHelicopterInput`: left stick flies (past the deadzone on either axis), the
/// right trigger fires (`FireTrigger`), the right stick aims, A leaves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadHelicopter {
    old_fire_trigger: f32,
    fire: bool,
    aim: Option<Vec2>,
}

impl PadHelicopter {
    pub fn process(&mut self, pad: &PadState, edges: &PadEdges) -> HelicopterControls {
        let [lx, ly] = pad.left_stick;
        // `new Vector2(leftStick.x, -leftStick.y)`: up-positive, as here.
        let movement = if lx.abs() > STICK_DEADZONE || ly.abs() > STICK_DEADZONE { Vec2::new(lx, ly) } else { Vec2::new(0.0, 0.0) };
        fire_trigger(&mut self.old_fire_trigger, &mut self.fire, pad.rt);
        let [rx, ry] = pad.right_stick;
        if rx.abs() > STICK_DEADZONE || ry.abs() > STICK_DEADZONE {
            self.aim = Some(Vec2::new(rx, ry));
        }
        HelicopterControls { movement, fire: self.fire, aim: self.aim, exit: edges.a }
    }
}

/// `KeyboardAndMouseHelicopterInput.ProcessInput`'s status: active on movement (squared length
/// past 0.0025), the exit key, the fire button or a mouse move last frame.
pub fn keyboard_helicopter_status(movement: Vec2, fire: bool, exit: bool, mouse_moved_last_frame: bool) -> ProcessorStatus {
    let active = movement.x * movement.x + movement.y * movement.y > 0.002_500_000_2 || exit || fire || mouse_moved_last_frame;
    if active { ProcessorStatus::Active } else { ProcessorStatus::Idle }
}

/// What a lobby processor reports this frame (`LobbyInput.Processor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LobbyMoves {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub select: bool,
    pub back: bool,
    pub anything_pressed: bool,
}

/// `XboxishGamepadLobbyInput`: moves fire once when the stick (its dominant axis, past the
/// deadzone) or the d-pad goes in a direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadLobby {
    last_up: bool,
    last_down: bool,
    last_left: bool,
    last_right: bool,
}

impl PadLobby {
    pub fn process(&mut self, pad: &PadState, edges: &PadEdges) -> LobbyMoves {
        let [mut x, y_up] = pad.left_stick;
        // The original's y points down.
        let mut y = -y_up;
        if x.abs() < STICK_DEADZONE {
            x = 0.0;
        }
        if y.abs() < STICK_DEADZONE {
            y = 0.0;
        }
        if x.abs() > y.abs() {
            y = 0.0;
        } else {
            x = 0.0;
        }
        let up = y < 0.0 || pad.dpad_up;
        let down = y > 0.0 || pad.dpad_down;
        let left = x < 0.0 || pad.dpad_left;
        let right = x > 0.0 || pad.dpad_right;
        let moves = LobbyMoves {
            up: up && !self.last_up,
            down: down && !self.last_down,
            left: left && !self.last_left,
            right: right && !self.last_right,
            select: edges.a,
            back: edges.b || edges.back,
            anything_pressed: pad.a || pad.b || pad.x || pad.y || pad.lb || pad.rb || pad.start || pad.back || pad.left_stick_click || pad.right_stick_click,
        };
        self.last_up = up;
        self.last_down = down;
        self.last_left = left;
        self.last_right = right;
        moves
    }
}

/// `XboxishGamepadMenuInput`: (`Activate`, `Dismiss`).
pub fn pad_menu(edges: &PadEdges) -> (bool, bool) {
    (edges.start, edges.b || edges.back || edges.start)
}

/// `RoleInputProcessor.ProcessInput`'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessorStatus {
    Active,
    Idle,
    Unavailable,
}

/// `ProcessorPrioritizer`'s order (lower first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessorKind {
    SteamController = 0,
    XboxishGamepad = 1,
    KeyboardAndMouse = 2,
    Touch = 3,
}

impl ProcessorKind {
    /// The `ControlTypes` interface name (`ControlAssignment.ControlTypesForPlayer`,
    /// `LevelTips`, `ControlTypeDependentActivator`).
    pub fn control_type(self) -> &'static str {
        match self {
            ProcessorKind::SteamController => "SteamControllerProcessor",
            ProcessorKind::XboxishGamepad => "XboxishGamepadProcessor",
            ProcessorKind::KeyboardAndMouse => "KeyboardAndMouseProcessor",
            ProcessorKind::Touch => "TouchProcessor",
        }
    }
}

/// `RoleInput.currentProcessor` and its per-frame choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoleInputChoice {
    pub current: Option<usize>,
}

impl RoleInputChoice {
    /// `RoleInput.Update` over `count` processors (in the order they were added): `process(i)`
    /// runs processor `i`'s `ProcessInput` (processors after the first active one are not run
    /// this frame); `kind(i)` ranks idle ones. Returns the processor whose values the role reads
    /// (`None`: every value is cleared).
    pub fn update(&mut self, count: usize, mut process: impl FnMut(usize) -> ProcessorStatus, kind: impl Fn(usize) -> ProcessorKind) -> Option<usize> {
        if count == 0 {
            self.current = None;
            return None;
        }
        let mut idle = Vec::new();
        for i in 0..count {
            match process(i) {
                ProcessorStatus::Active => {
                    self.current = Some(i);
                    break;
                }
                ProcessorStatus::Unavailable if self.current == Some(i) => self.current = None,
                ProcessorStatus::Idle => idle.push(i),
                ProcessorStatus::Unavailable => {}
            }
        }
        if self.current.is_none() && !idle.is_empty() {
            // `List.Sort(ProcessorPrioritizer.Compare)`.
            idle.sort_by_key(|&i| kind(i));
            self.current = Some(idle[0]);
        }
        self.current
    }
}

/// `KeyboardAndMouseCharacterInput.ProcessInput`'s status: active on any movement past 0.05,
/// a held button, a weapon change, a vehicle press or a mouse move last frame.
pub fn keyboard_character_status(c: &CharacterControls, previous_weapon: bool, mouse_moved_last_frame: bool) -> ProcessorStatus {
    let active = c.movement.abs() > 0.05 || c.jump || c.duck || c.fire || c.next_weapon || previous_weapon || c.enter_vehicle || mouse_moved_last_frame;
    if active { ProcessorStatus::Active } else { ProcessorStatus::Idle }
}

/// `AllClientControlTypes` offline: the control types in use, sorted and comma-joined (the
/// name of the child a `ControlTypeDependentActivator` switches on).
pub fn control_types_name(kinds: impl IntoIterator<Item = ProcessorKind>) -> String {
    let mut names: Vec<&str> = kinds.into_iter().map(ProcessorKind::control_type).collect();
    names.sort_unstable();
    names.dedup();
    names.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_trigger_rises_and_falls_with_hysteresis() {
        let mut p = PadCharacter::default();
        let e = PadEdges::default();
        let mut pad = PadState::default();
        assert!(!p.process(&pad, &e).fire);
        pad.rt = 0.5;
        assert!(p.process(&pad, &e).fire);
        // Falling, but still above 0.2: keeps firing.
        pad.rt = 0.3;
        assert!(p.process(&pad, &e).fire);
        pad.rt = 0.15;
        assert!(!p.process(&pad, &e).fire);
        // A slow creep up of less than 0.01 a frame does not fire again.
        pad.rt = 0.155;
        assert!(!p.process(&pad, &e).fire);
    }

    #[test]
    fn character_axes_follow_the_original_thresholds() {
        let mut p = PadCharacter::default();
        let e = PadEdges { y: true, a: true, ..PadEdges::default() };
        let pad = PadState { left_stick: [0.1, -0.5], right_stick: [0.0, 0.1], lt: 0.004, ..PadState::default() };
        let c = p.process(&pad, &e);
        assert_eq!(c.movement, 0.0);
        assert!(c.duck);
        assert!(c.jump);
        assert!(c.next_weapon && c.enter_vehicle);
        // The aim stays at its default inside the deadzone, then follows the stick (y up).
        assert_eq!(c.aim, Vec2::new(1.0, 0.0));
        let pad = PadState { left_stick: [-0.7, 0.3], right_stick: [-0.1, 0.9], ..PadState::default() };
        let c = p.process(&pad, &PadEdges::default());
        assert_eq!(c.movement, -0.7);
        assert!(!c.duck && !c.jump);
        assert_eq!(c.aim, Vec2::new(-0.1, 0.9));
        let c = p.process(&PadState::default(), &PadEdges::default());
        assert_eq!(c.aim, Vec2::new(-0.1, 0.9));
    }

    #[test]
    fn lobby_moves_fire_once_on_the_dominant_axis() {
        let mut l = PadLobby::default();
        let e = PadEdges::default();
        let right_up = PadState { left_stick: [0.8, 0.5], ..PadState::default() };
        let m = l.process(&right_up, &e);
        assert!(m.right && !m.up && !m.left && !m.down);
        assert!(!l.process(&right_up, &e).right);
        let up = PadState { left_stick: [0.1, 0.9], ..PadState::default() };
        let m = l.process(&up, &e);
        assert!(m.up && !m.right);
        let dpad = PadState { dpad_left: true, ..PadState::default() };
        assert!(l.process(&dpad, &e).left);
        let m = l.process(&PadState::default(), &PadEdges { a: true, back: true, ..PadEdges::default() });
        assert!(m.select && m.back);
    }

    #[test]
    fn edges_and_activity() {
        let mut h = PadHistory::default();
        let pad = PadState { a: true, ..PadState::default() };
        assert!(h.update(&pad).a);
        assert!(!h.update(&pad).a);
        assert!(pad.is_active());
        assert!(!PadState { left_stick: [0.19, -0.19], lt: 0.09, ..PadState::default() }.is_active());
        assert_eq!(pad_menu(&PadEdges { start: true, ..PadEdges::default() }), (true, true));
    }

    #[test]
    fn role_input_prefers_active_then_keeps_then_ranks_idle() {
        let kinds = [ProcessorKind::KeyboardAndMouse, ProcessorKind::XboxishGamepad];
        let mut c = RoleInputChoice::default();
        // Both idle: the gamepad outranks the keyboard.
        assert_eq!(c.update(2, |_| ProcessorStatus::Idle, |i| kinds[i]), Some(1));
        // The keyboard becomes active.
        assert_eq!(c.update(2, |i| if i == 0 { ProcessorStatus::Active } else { ProcessorStatus::Idle }, |i| kinds[i]), Some(0));
        // Both idle again: the current one stays.
        assert_eq!(c.update(2, |_| ProcessorStatus::Idle, |i| kinds[i]), Some(0));
        // The current one goes away: the idle gamepad takes over.
        assert_eq!(c.update(2, |i| if i == 0 { ProcessorStatus::Unavailable } else { ProcessorStatus::Idle }, |i| kinds[i]), Some(1));
        // The first active processor stops the scan.
        let mut ran = Vec::new();
        c.update(2, |i| {
            ran.push(i);
            ProcessorStatus::Active
        }, |i| kinds[i]);
        assert_eq!(ran, vec![0]);
        assert_eq!(c.update(0, |_| ProcessorStatus::Active, |i| kinds[i]), None);
    }

    #[test]
    fn helicopter_pad_flies_fires_and_keeps_its_aim() {
        let mut h = PadHelicopter::default();
        let c = h.process(&PadState { left_stick: [0.1, 0.5], rt: 0.6, ..PadState::default() }, &PadEdges { a: true, ..PadEdges::default() });
        assert_eq!(c.movement, Vec2::new(0.1, 0.5));
        assert!(c.fire && c.exit);
        assert_eq!(c.aim, None);
        h.process(&PadState { right_stick: [0.0, -0.9], ..PadState::default() }, &PadEdges::default());
        let c = h.process(&PadState::default(), &PadEdges::default());
        assert_eq!(c.movement, Vec2::new(0.0, 0.0));
        assert_eq!(c.aim, Some(Vec2::new(0.0, -0.9)));
        assert!(!c.fire);
        assert_eq!(keyboard_helicopter_status(Vec2::new(0.0, 0.04), false, false, false), ProcessorStatus::Idle);
        assert_eq!(keyboard_helicopter_status(Vec2::new(0.0, 1.0), false, false, false), ProcessorStatus::Active);
    }

    #[test]
    fn control_type_names_are_sorted_and_unique() {
        let k = [ProcessorKind::XboxishGamepad, ProcessorKind::KeyboardAndMouse, ProcessorKind::XboxishGamepad];
        assert_eq!(control_types_name(k), "KeyboardAndMouseProcessor,XboxishGamepadProcessor");
        assert_eq!(control_types_name([ProcessorKind::KeyboardAndMouse]), "KeyboardAndMouseProcessor");
    }
}
