//! Flat paper-and-cardboard UI in the style of the game's Lobby (not in the original): its tk2d
//! font with the text shadows, its torn-paper button backgrounds with their shadows, its
//! cardboard and its title, drawn as unlit 2D meshes by a camera of their own.
//!
//! Used by the launcher (cheats, settings, the editors) and by the game for the network room
//! panel, which sits above the Lobby's own screens.
//!
//! * Load the art once with [`PaperArt::load`] and insert it as a resource.
//! * Spawn a [`camera`] (an overlay keeps the scene below it: `clear` false) and the pieces with
//!   a [`Spawner`].
//! * Clicks arrive as [`PaperClicked`], Enter in a text field as [`PaperSubmitted`];
//!   [`PaperInput`] tells other input handling whether the pointer or the keyboard is taken.

mod art;

pub use art::{CLEAR, HIGHLIGHTED, PAPER_SHADOW_OFFSET, PaperArt, Piece, UNAVAILABLE, WHITE, bounds, mesh, scaled, shadow_of};

use bevy::camera::ScalingMode;
use bevy::camera::visibility::RenderLayers;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::draw::Geometry;

/// How much a pointed-at button grows.
const HOVER_SCALE: f32 = 1.04;
/// Placeholder text in an empty field.
const PLACEHOLDER: [f32; 4] = [0.82, 0.82, 0.82, 0.8];

/// The kit's systems (run them before anything that reads [`PaperInput`]).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaperSet;

/// A button (or a field) was clicked.
#[derive(Message, Clone, Copy, Debug)]
pub struct PaperClicked(pub Entity);

/// Enter was pressed in a focused field.
#[derive(Message, Clone, Copy, Debug)]
pub struct PaperSubmitted(pub Entity);

/// The pointer and keyboard as the kit sees them this frame.
#[derive(Resource, Default, Debug)]
pub struct PaperInput {
    /// The cursor in the paper camera's world.
    pub cursor: Option<Vec2>,
    /// The cursor is over a visible button or panel: clicks are the kit's.
    pub pointer_captured: bool,
    /// A field has the keyboard.
    pub keyboard_captured: bool,
}

/// The camera paper pieces are drawn with.
#[derive(Component)]
pub struct PaperCamera;

/// A clickable piece.
#[derive(Component, Debug)]
pub struct PaperButton {
    pub size: Vec2,
    pub enabled: bool,
    /// Drawn highlighted while true (a selected tab, a switched-on choice).
    pub toggled: bool,
    pub hovered: bool,
}

/// A piece that takes the pointer without being a button (a panel).
#[derive(Component, Debug)]
pub struct PaperBlocker {
    pub size: Vec2,
}

/// One-line text entry; it is a [`PaperButton`] whose label shows the text.
#[derive(Component, Debug, Default)]
pub struct PaperField {
    pub text: String,
    pub placeholder: String,
    /// Shown as stars (passwords).
    pub secret: bool,
    pub max: usize,
    pub focused: bool,
}

/// Text drawn with the Lobby font and its shadow.
#[derive(Component, Debug)]
pub struct PaperLabel {
    pub text: String,
    pub color: [f32; 4],
    /// Box height the text is sized for (as a button of this height).
    pub height: f32,
    /// Longer text shrinks to this width.
    pub max_width: f32,
    /// Lines start at the label's position instead of being centred on it.
    pub left: bool,
    /// Takes its colour from the parent button (highlighted, unavailable).
    follows_button: bool,
    shown: Option<(String, [f32; 4], u32)>,
    text_mesh: Handle<Mesh>,
    shadow_mesh: Handle<Mesh>,
}

pub struct PaperPlugin;

impl Plugin for PaperPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PaperClicked>()
            .add_message::<PaperSubmitted>()
            .init_resource::<PaperInput>()
            .init_resource::<Paste>()
            .add_systems(Update, (pointer, typing, pasting, field_labels, refresh_labels).chain().in_set(PaperSet));
    }
}

/// A camera for paper pieces on `layer`, showing at least `view` world units. An overlay
/// (`clear` false) draws over the cameras with a lower `order`.
pub fn camera(view: Vec2, order: isize, clear: bool, layer: RenderLayers) -> impl Bundle {
    (
        Camera2d,
        Camera { order, clear_color: if clear { ClearColorConfig::Default } else { ClearColorConfig::None }, ..default() },
        Projection::Orthographic(OrthographicProjection { scaling_mode: ScalingMode::AutoMin { min_width: view.x, min_height: view.y }, ..OrthographicProjection::default_2d() }),
        // The textures' own colours, as the Lobby's camera shows them.
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        layer,
        PaperCamera,
    )
}

/// A flat rectangle of one colour, centred on the origin.
pub fn rectangle(size: Vec2, color: [f32; 4]) -> Geometry {
    let (x, y) = (size.x * 0.5, size.y * 0.5);
    Geometry {
        positions: vec![[-x, -y, 0.0], [x, -y, 0.0], [x, y, 0.0], [-x, y, 0.0]],
        normals: Vec::new(),
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        colors: vec![color; 4],
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

/// Spawns paper pieces on one render layer.
pub struct Spawner<'a, 'w, 's> {
    pub commands: &'a mut Commands<'w, 's>,
    pub art: &'a PaperArt,
    pub meshes: &'a mut Assets<Mesh>,
    pub layer: RenderLayers,
}

impl Spawner<'_, '_, '_> {
    fn attach(&mut self, parent: Option<Entity>, child: Entity) -> Entity {
        if let Some(parent) = parent {
            self.commands.entity(parent).add_child(child);
        }
        child
    }

    /// A mesh child at `at` (z orders pieces within their parent).
    fn piece(&mut self, parent: Entity, g: &Geometry, material: Handle<ColorMaterial>, at: Vec3) -> Entity {
        let mesh = self.meshes.add(mesh(g));
        let child = self.commands.spawn((Mesh2d(mesh), MeshMaterial2d(material), Transform::from_translation(at), self.layer.clone())).id();
        self.attach(Some(parent), child)
    }

    /// An empty node to hang pieces on.
    pub fn node(&mut self, parent: Option<Entity>, at: Vec3) -> Entity {
        let e = self.commands.spawn((Transform::from_translation(at), Visibility::default())).id();
        self.attach(parent, e)
    }

    /// The Lobby's cardboard, centred on its title screen.
    pub fn background(&mut self, parent: Option<Entity>, at: Vec3) -> Entity {
        let art = self.art;
        let node = self.node(parent, at);
        if let (Some(bg), Some(material)) = (&art.background, &art.background_material) {
            let (g, material) = (bg.geometry.clone(), material.clone());
            self.piece(node, &g, material, Vec3::ZERO);
        }
        node
    }

    /// The title logo fitted into `fit`, with its shadow.
    pub fn title(&mut self, parent: Option<Entity>, at: Vec3, fit: Vec2) -> Entity {
        let art = self.art;
        let node = self.node(parent, at);
        if let (Some(title), Some(material)) = (&art.title, &art.title_material) {
            let (min, max) = bounds(&title.geometry);
            let size = (max - min).max(Vec2::splat(1e-3));
            let k = (fit.x / size.x).min(fit.y / size.y);
            let g = scaled(&title.geometry, Vec2::splat(k));
            let material = material.clone();
            if let Some((offset, color)) = art.title_shadow {
                self.piece(node, &shadow_of(&g, None, Some(color)), material.clone(), (offset * k).extend(0.0));
            }
            self.piece(node, &g, material, Vec3::new(0.0, 0.0, 0.1));
        }
        node
    }

    /// A torn paper of `size` (paper `which` of the title screen's, cycling) with its shadow.
    pub fn paper(&mut self, parent: Option<Entity>, at: Vec3, size: Vec2, which: usize) -> Entity {
        let art = self.art;
        let node = self.node(parent, at);
        if !art.papers.is_empty() {
            let i = which % art.papers.len();
            let g = scaled(&art.papers[i].geometry, size);
            let material = art.paper_materials[i].clone();
            self.piece(node, &shadow_of(&g, None, None), material.clone(), PAPER_SHADOW_OFFSET.extend(0.0));
            self.piece(node, &g, material, Vec3::new(0.0, 0.0, 0.05));
        }
        node
    }

    /// A paper that takes the pointer (clicks on it do not reach what is below).
    pub fn panel(&mut self, parent: Option<Entity>, at: Vec3, size: Vec2, which: usize) -> Entity {
        let node = self.paper(parent, at, size, which);
        self.commands.entity(node).insert(PaperBlocker { size });
        node
    }

    /// A flat coloured rectangle (icon parts); it does not take the pointer.
    pub fn rect(&mut self, parent: Option<Entity>, at: Vec3, size: Vec2, color: [f32; 4]) -> Entity {
        let node = self.node(parent, at);
        let material = self.art.white_material.clone();
        self.piece(node, &rectangle(size, color), material, Vec3::ZERO);
        node
    }

    /// A see-through rectangle that takes the pointer (behind a dialog).
    pub fn shade(&mut self, parent: Option<Entity>, at: Vec3, size: Vec2, color: [f32; 4]) -> Entity {
        let node = self.node(parent, at);
        let material = self.art.white_material.clone();
        self.piece(node, &rectangle(size, color), material, Vec3::ZERO);
        self.commands.entity(node).insert(PaperBlocker { size });
        node
    }

    fn label_with(&mut self, parent: Option<Entity>, at: Vec3, text: &str, height: f32, max_width: f32, color: [f32; 4], left: bool, follows_button: bool) -> Entity {
        let empty = Geometry::default();
        let text_mesh = self.meshes.add(mesh(&empty));
        let shadow_mesh = self.meshes.add(mesh(&empty));
        let node = self
            .commands
            .spawn((
                Transform::from_translation(at),
                Visibility::default(),
                PaperLabel { text: text.to_owned(), color, height, max_width, left, follows_button, shown: None, text_mesh: text_mesh.clone(), shadow_mesh: shadow_mesh.clone() },
            ))
            .id();
        self.attach(parent, node);
        let material = self.art.text_material.clone();
        let shadow = self.commands.spawn((Mesh2d(shadow_mesh), MeshMaterial2d(material.clone()), Transform::default(), self.layer.clone())).id();
        let front = self.commands.spawn((Mesh2d(text_mesh), MeshMaterial2d(material), Transform::from_xyz(0.0, 0.0, 0.01), self.layer.clone())).id();
        self.commands.entity(node).add_children(&[shadow, front]);
        node
    }

    /// Text sized for a box `height` high, shrunk to `max_width` when longer.
    pub fn label(&mut self, parent: Option<Entity>, at: Vec3, text: &str, height: f32, max_width: f32, color: [f32; 4], left: bool) -> Entity {
        self.label_with(parent, at, text, height, max_width, color, left, false)
    }

    /// A paper button with a label.
    pub fn button(&mut self, parent: Option<Entity>, at: Vec3, size: Vec2, which: usize, text: &str) -> Entity {
        let node = self.paper(parent, at, size, which);
        self.commands.entity(node).insert(PaperButton { size, enabled: true, toggled: false, hovered: false });
        self.label_with(Some(node), Vec3::new(0.0, 0.0, 0.1), text, size.y, size.x * 0.9, WHITE, false, true);
        node
    }

    /// A paper text field.
    pub fn field(&mut self, parent: Option<Entity>, at: Vec3, size: Vec2, which: usize, placeholder: &str, secret: bool, max: usize) -> Entity {
        let node = self.button(parent, at, size, which, "");
        self.commands.entity(node).insert(PaperField { text: String::new(), placeholder: placeholder.to_owned(), secret, max, focused: false });
        node
    }
}

/// Hover, clicks and focus.
#[allow(clippy::type_complexity)]
fn pointer(
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<PaperCamera>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut input: ResMut<PaperInput>,
    mut buttons: Query<(Entity, &mut PaperButton, &GlobalTransform, &mut Transform, &InheritedVisibility, Option<&mut PaperField>)>,
    blockers: Query<(&PaperBlocker, &GlobalTransform, &InheritedVisibility)>,
    mut clicked: MessageWriter<PaperClicked>,
) {
    input.cursor = windows.single().ok().and_then(|w| w.cursor_position()).and_then(|c| {
        let (camera, transform) = cameras.single().ok()?;
        camera.viewport_to_world_2d(transform, c).ok()
    });
    let inside = |cursor: Vec2, at: &GlobalTransform, size: Vec2, own_scale: f32| {
        let scale = at.scale().truncate() / own_scale.max(1e-6);
        ((cursor - at.translation().truncate()).abs() * 2.0).cmple(size * scale.abs()).all()
    };
    // The topmost thing under the cursor.
    let mut top: Option<(f32, Option<Entity>)> = None;
    if let Some(cursor) = input.cursor {
        for (entity, b, at, transform, visible, _) in &buttons {
            if visible.get() && inside(cursor, at, b.size, transform.scale.x) && top.is_none_or(|(z, _)| at.translation().z >= z) {
                top = Some((at.translation().z, Some(entity)));
            }
        }
        for (b, at, visible) in &blockers {
            if visible.get() && inside(cursor, at, b.size, 1.0) && top.is_none_or(|(z, _)| at.translation().z > z) {
                top = Some((at.translation().z, None));
            }
        }
    }
    input.pointer_captured = top.is_some();
    let over = top.and_then(|(_, e)| e);
    let click = mouse.just_pressed(MouseButton::Left);
    let mut focus: Option<Option<Entity>> = click.then_some(None);
    for (entity, mut b, _, mut transform, _, field) in &mut buttons {
        let hovered = over == Some(entity) && b.enabled;
        if b.hovered != hovered {
            b.hovered = hovered;
            transform.scale = Vec3::splat(if hovered { HOVER_SCALE } else { 1.0 });
        }
        if hovered && click {
            clicked.write(PaperClicked(entity));
            if field.is_some() {
                focus = Some(Some(entity));
            }
        }
    }
    if let Some(focus) = focus {
        for (entity, _, _, _, _, field) in &mut buttons {
            if let Some(mut field) = field {
                let focused = focus == Some(entity);
                if field.focused != focused {
                    field.focused = focused;
                }
            }
        }
    }
}

/// Typing into the focused field.
/// A clipboard read for Ctrl+V (reading takes a moment, so it runs on a helper thread) and the
/// field it goes into.
#[derive(Resource, Default)]
struct Paste(Option<(Entity, std::sync::Mutex<std::sync::mpsc::Receiver<Option<String>>>)>);

/// The clipboard's text, read with the platform's own tool.
pub fn read_clipboard() -> std::sync::mpsc::Receiver<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let tools: &[(&str, &[&str])] = if cfg!(windows) {
            &[("powershell", &["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard"])]
        } else if cfg!(target_os = "macos") {
            &[("pbpaste", &[])]
        } else {
            &[("wl-paste", &["--no-newline"]), ("xclip", &["-selection", "clipboard", "-o"]), ("xsel", &["--clipboard", "--output"])]
        };
        let mut out = None;
        for (tool, args) in tools {
            let mut command = std::process::Command::new(tool);
            command.args(*args);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            if let Ok(o) = command.output() {
                if o.status.success() {
                    out = Some(String::from_utf8_lossy(&o.stdout).trim().to_owned());
                    break;
                }
            }
        }
        let _ = tx.send(out);
    });
    rx
}

/// Ctrl+V's text lands in the field once the clipboard has been read.
fn pasting(mut paste: ResMut<Paste>, mut fields: Query<&mut PaperField>) {
    let Some((entity, rx)) = &paste.0 else { return };
    let Ok(result) = rx.lock().map(|rx| rx.try_recv()) else { return };
    let text = match result {
        Ok(text) => text,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
    };
    if let (Some(text), Ok(mut field)) = (text, fields.get_mut(*entity)) {
        for c in text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control()) {
            if field.text.chars().count() < field.max {
                field.text.push(c);
            }
        }
    }
    paste.0 = None;
}

fn typing(
    mut keys: MessageReader<KeyboardInput>,
    held: Res<ButtonInput<KeyCode>>,
    mut fields: Query<(Entity, &mut PaperField)>,
    mut submitted: MessageWriter<PaperSubmitted>,
    mut input: ResMut<PaperInput>,
    mut paste: ResMut<Paste>,
) {
    let ctrl = held.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    for key in keys.read() {
        if key.state != ButtonState::Pressed {
            continue;
        }
        let Some((entity, mut field)) = fields.iter_mut().find(|(_, f)| f.focused) else { continue };
        if ctrl {
            if key.key_code == KeyCode::KeyV && paste.0.is_none() {
                paste.0 = Some((entity, std::sync::Mutex::new(read_clipboard())));
            }
            continue;
        }
        match &key.logical_key {
            Key::Character(s) => {
                for c in s.chars().filter(|c| !c.is_control()) {
                    if field.text.chars().count() < field.max {
                        field.text.push(c);
                    }
                }
            }
            Key::Space => {
                if field.text.chars().count() < field.max {
                    field.text.push(' ');
                }
            }
            Key::Backspace => {
                field.text.pop();
            }
            Key::Enter => {
                submitted.write(PaperSubmitted(entity));
            }
            Key::Escape => field.focused = false,
            _ => {}
        }
    }
    input.keyboard_captured = fields.iter().any(|(_, f)| f.focused);
}

/// A field's label shows its text (stars for a secret), its placeholder, and a cursor.
fn field_labels(time: Res<Time>, fields: Query<(&PaperField, &Children)>, mut labels: Query<&mut PaperLabel>) {
    let blink = (time.elapsed_secs() * 2.0) as i64 % 2 == 0;
    for (field, children) in &fields {
        let shown = if field.secret { "*".repeat(field.text.chars().count()) } else { field.text.clone() };
        let (text, placeholder) = if shown.is_empty() && !field.focused { (field.placeholder.clone(), true) } else { (format!("{shown}{}", if field.focused && blink { "_" } else { " " }), false) };
        for child in children.to_vec() {
            if let Ok(mut label) = labels.get_mut(child) {
                if label.text != text {
                    label.text = text.clone();
                }
                let color = if placeholder { PLACEHOLDER } else { WHITE };
                if label.color != color {
                    label.color = color;
                }
            }
        }
    }
}

/// Labels follow their text, colour and button.
fn refresh_labels(art: Option<Res<PaperArt>>, mut labels: Query<(&mut PaperLabel, Option<&ChildOf>)>, buttons: Query<(&PaperButton, Option<&PaperField>)>, mut meshes: ResMut<Assets<Mesh>>) {
    let Some(art) = art else { return };
    for (mut label, parent) in &mut labels {
        let button = parent.filter(|_| label.follows_button).and_then(|p| buttons.get(p.parent()).ok());
        let color = match button {
            Some((b, _)) if !b.enabled => UNAVAILABLE,
            Some((b, field)) if (b.hovered || b.toggled || field.is_some_and(|f| f.focused)) && label.color == WHITE => HIGHLIGHTED,
            _ => label.color,
        };
        let scale = art.fit_scale(&label.text, label.max_width, label.height);
        let wanted = (label.text.clone(), color, scale.to_bits());
        if label.shown.as_ref() == Some(&wanted) {
            continue;
        }
        let g = art.aligned_text(&label.text, scale, color, label.left);
        let shadow_offset = art.text_shadow.0 * (scale / art.text.scale[1].abs().max(1e-3));
        let shadow = art::shadow_of(&g, None, Some(art.text_shadow.1));
        let mut shadow = shadow;
        for p in &mut shadow.positions {
            p[0] += shadow_offset.x;
            p[1] += shadow_offset.y;
        }
        let _ = meshes.insert(label.text_mesh.id(), mesh(&g));
        let _ = meshes.insert(label.shadow_mesh.id(), mesh(&shadow));
        label.shown = Some(wanted);
    }
}
