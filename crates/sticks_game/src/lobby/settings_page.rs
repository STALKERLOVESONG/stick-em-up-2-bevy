//! The settings page (not in the original): a "Settings" button at the top left of the title
//! screen (a copy of the Exit button at the top right, widened for its word) opens a page of its
//! own on the paper world, built from the Lobby's own pieces — a copy of the server list screen
//! (its Back button and wide buttons, one per setting) with a heading copied from the
//! achievements screen. Each click on a setting moves it on (volumes
//! 100 % → 75 % → 50 % → 25 % → off; switches on/off). See `crate::settings`.

use super::{Art, Lobby, LobbyMemory};
use crate::settings::Settings;

/// An empty spot of the paper world, above the server list.
const PAGE_POSITION: [f32; 3] = [56.4, 91.6, 0.0];
/// How much wider than the Exit button the Settings button is (the word is about twice as long).
const OPEN_BUTTON_STRETCH: f32 = 1.9;
/// The rows' heights on the page.
const ROW_Y: [f32; 5] = [6.6, 2.2, -2.2, -6.6, -11.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Music,
    Sound,
    Fullscreen,
    VSync,
    ShowFps,
}

const ROWS: [Row; 5] = [Row::Music, Row::Sound, Row::Fullscreen, Row::VSync, Row::ShowFps];

pub(super) struct SettingsPage {
    /// The page's camera target (`ActivateExclusively`).
    screen: usize,
    back: usize,
    open: usize,
    rows: Vec<(Row, usize)>,
}

fn named_root(lobby: &Lobby, name: &str) -> Option<usize> {
    lobby.scene.objects.iter().position(|o| o.name == name && o.parent.is_none())
}

/// Build the page and the title screen's button. `None` when the scene lacks the pieces.
pub(super) fn build(lobby: &mut Lobby) -> Option<SettingsPage> {
    let list = named_root(lobby, "Server List")?;
    let title = named_root(lobby, "Title Screen")?;
    let page = lobby.instantiate(list, None, PAGE_POSITION);
    let child = |lobby: &Lobby, name: &str| lobby.scene.child(page, name);
    let screen = child(lobby, "Server List")?;
    // The copy is a plain screen: no server list behind it.
    lobby.scene.scripts.retain(|s| !(s.object == screen && s.class == "ServerList"));
    let back = child(lobby, "Back")?;
    let game = child(lobby, "Game 0")?;
    let mut buttons: Vec<usize> = ["Game 0", "Game 1", "Game 2"].iter().filter_map(|n| child(lobby, n)).collect();
    while buttons.len() < ROWS.len() {
        let copy = lobby.instantiate(game, Some(page), lobby.local[game]);
        buttons.push(copy);
    }
    for (i, &b) in buttons.iter().enumerate() {
        lobby.local[b] = [0.0, ROW_Y[i], lobby.local[b][2]];
        if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == b) {
            m.target = lobby.local[b];
        }
    }
    // The page-turning buttons become the title screen's "Settings" button; the page has none.
    for name in ["Previous Page", "Next Page"] {
        if let Some(o) = child(lobby, name) {
            lobby.active[o] = false;
        }
    }
    let open = open_button(lobby, title)?;
    // A heading like the other pages'.
    if let Some(heading) = named_root(lobby, "Achievements Screen 1").and_then(|a| lobby.scene.child(a, "Heading")) {
        let copy = lobby.instantiate(heading, Some(page), [0.0, 10.75, lobby.local[heading][2]]);
        lobby.active[copy] = true;
        lobby.set_text(copy, "Settings");
    }
    // Gamepad navigation: Back above the first row, the rows in a column.
    let rows: Vec<(Row, usize)> = ROWS.iter().copied().zip(buttons.iter().copied()).collect();
    let mut order = vec![back];
    order.extend(buttons.iter().copied());
    for (k, &o) in order.iter().enumerate() {
        if let Some(i) = lobby.button_index(o) {
            let b = &mut lobby.buttons[i];
            b.neighbours = [k.checked_sub(1).map(|p| order[p]), order.get(k + 1).copied(), None, None];
            b.is_default = k == 1;
            b.return_to_last_selected = false;
        }
    }
    if let Some(i) = lobby.button_index(open) {
        let b = &mut lobby.buttons[i];
        b.neighbours = [None; 4];
        b.is_default = false;
    }
    Some(SettingsPage { screen, back, open, rows })
}

/// The title screen's "Settings" button: the Exit button mirrored to the top left, its paper
/// stretched sideways (the text keeps its size).
fn open_button(lobby: &mut Lobby, title: usize) -> Option<usize> {
    let exit = lobby.scene.child(title, "-1 Exit")?;
    let exit_local = lobby.local[exit];
    let in_button = |o: usize| o == exit || lobby.scene.objects[o].parent == Some(exit);
    let half_width = lobby.scene.boxes.iter().find(|b| in_button(b.object)).map_or(2.0, |b| b.size[0].abs() * 0.5);
    let open = lobby.instantiate(exit, Some(title), exit_local);
    lobby.scene.scripts.retain(|s| !(s.object == open && s.class == "ExitButton"));
    lobby.scene.objects[open].name = "-1 Settings".to_owned();
    lobby.active[open] = true;
    lobby.set_button_text(open, "Settings");
    stretch_x(lobby, open, OPEN_BUTTON_STRETCH);
    // The outer edge where the Exit button's is, on the other side.
    let x = -(exit_local[0] + half_width) + half_width * OPEN_BUTTON_STRETCH;
    lobby.local[open] = [x, exit_local[1], exit_local[2]];
    if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == open) {
        m.target = lobby.local[open];
    }
    Some(open)
}

/// Widen the paper (surfaces and their shadows) and the click box of the copied button `root`
/// by `k` about its centre. Surface geometry is kept in scene world space, where the copy still
/// is until it is drawn.
fn stretch_x(lobby: &mut Lobby, root: usize, k: f32) {
    let mut subtree = vec![root];
    let mut i = 0;
    while i < subtree.len() {
        subtree.extend(lobby.scene.objects[subtree[i]].children.iter().copied());
        i += 1;
    }
    let cx = lobby.scene.objects[root].world.t[0];
    for r in &mut lobby.art {
        if !subtree.contains(&r.object) {
            continue;
        }
        if let Art::Surface { surface, geometry, .. } = &mut r.art {
            for p in &mut geometry.positions {
                p[0] = cx + (p[0] - cx) * k;
            }
            surface.axis_x = [surface.axis_x[0] * k, surface.axis_x[1] * k];
        }
    }
    for b in &mut lobby.scene.boxes {
        if subtree.contains(&b.object) {
            b.size[0] *= k;
            b.center[0] *= k;
        }
    }
}

fn percent(v: f32) -> String {
    if v <= 0.0 { "Off".to_owned() } else { format!("{}%", (v * 100.0).round() as i32) }
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

/// The rows show the current values.
pub(super) fn update(lobby: &mut Lobby, memory: &LobbyMemory) {
    let Some(page) = lobby.settings_page.take() else { return };
    let s = memory.settings;
    for &(row, button) in &page.rows {
        let text = match row {
            Row::Music => format!("Music Volume: {}", percent(s.music_volume)),
            Row::Sound => format!("Sound Volume: {}", percent(s.sound_volume)),
            Row::Fullscreen => format!("Fullscreen: {}", on_off(s.fullscreen)),
            Row::VSync => format!("Vertical Sync: {}", on_off(s.vsync)),
            Row::ShowFps => format!("Show FPS: {}", on_off(memory.fps_display)),
        };
        lobby.set_button_text(button, &text);
    }
    lobby.settings_page = Some(page);
}

/// Clicks on the page and on the title screen's button. Returns whether `object` was one.
pub(super) fn press(lobby: &mut Lobby, memory: &mut LobbyMemory, object: usize) -> bool {
    let Some(page) = lobby.settings_page.take() else { return false };
    let mut handled = true;
    if object == page.open {
        lobby.activate(page.screen);
    } else if object == page.back {
        // Back to the title screen (`NetworkSetup`'s screen).
        if let Some(s) = lobby.named_script("NetworkSetup").map(|s| s.object) {
            lobby.activate(s);
        }
    } else if let Some(&(row, _)) = page.rows.iter().find(|(_, b)| *b == object) {
        let s = &mut memory.settings;
        match row {
            Row::Music => s.music_volume = Settings::next_volume(s.music_volume),
            Row::Sound => s.sound_volume = Settings::next_volume(s.sound_volume),
            Row::Fullscreen => s.fullscreen = !s.fullscreen,
            Row::VSync => s.vsync = !s.vsync,
            Row::ShowFps => memory.fps_display = !memory.fps_display,
        }
    } else {
        handled = false;
    }
    lobby.settings_page = Some(page);
    handled
}
