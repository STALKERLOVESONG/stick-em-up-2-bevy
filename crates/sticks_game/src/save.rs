//! `DataManager`: the four save files (`Goals.dat`, `Scores.dat`, `Colors.dat`, `Tips.dat`).
//!
//! The original keeps them in Steam Remote Storage, loads them all once at start-up and writes a
//! file in `LateUpdate` whenever something asked for it (`SaveGoals()` and friends). The port
//! keeps the same files, in the same byte format (`sticks_sim::save_data`), in a local folder:
//! `STICK_SAVE_DIR` when set, else `%APPDATA%\StickEmUp2Reconstruction` on Windows and
//! `$XDG_DATA_HOME` (or `~/.local/share`)`/StickEmUp2Reconstruction` elsewhere. Files copied
//! from the original's Steam cloud folder load as they are.

use std::path::PathBuf;

use bevy::prelude::*;
use sticks_sim::save_data;

use crate::lobby::LobbyMemory;

const GOALS_FILENAME: &str = "Goals.dat";
const SCORES_FILENAME: &str = "Scores.dat";
const COLORS_FILENAME: &str = "Colors.dat";
const TIPS_FILENAME: &str = "Tips.dat";

/// `DataManager.needToSave*`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SaveFlags {
    pub goals: bool,
    pub scores: bool,
    pub colors: bool,
    pub tips: bool,
}

/// Where the save files live (see the module docs); `None` when no folder can be named.
pub fn save_dir() -> Option<PathBuf> {
    sticks_sim::options::save_dir()
}

/// `DataManager.LoadAll`: every file that exists is read into `memory`.
pub fn load_all(memory: &mut LobbyMemory) {
    let Some(dir) = save_dir() else { return };
    let read = |name: &str| std::fs::read(dir.join(name)).ok();
    if let Some(data) = read(GOALS_FILENAME) {
        if save_data::read_goals(&data, &mut memory.goals).is_none() {
            warn!("{GOALS_FILENAME}: unreadable, ignored");
        }
    }
    if let Some(data) = read(SCORES_FILENAME) {
        if save_data::read_scores(&data, &mut memory.high_scores).is_none() {
            warn!("{SCORES_FILENAME}: unreadable, ignored");
        }
    }
    if let Some(data) = read(COLORS_FILENAME) {
        if save_data::read_colors(&data, &mut memory.colors).is_none() {
            warn!("{COLORS_FILENAME}: unreadable, ignored");
        }
    }
    if let Some(data) = read(TIPS_FILENAME) {
        if save_data::read_tips(&data, &mut memory.tips_seen).is_none() {
            warn!("{TIPS_FILENAME}: unreadable, ignored");
        }
    }
    info!("save data folder: {}", dir.display());
}

/// `DataManager.LateUpdate`: write the files something asked to save.
fn save_requested(mut memory: ResMut<LobbyMemory>) {
    let flags = memory.save;
    if !(flags.goals || flags.scores || flags.colors || flags.tips) {
        return;
    }
    memory.save = SaveFlags::default();
    let Some(dir) = save_dir() else { return };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("cannot create the save folder {}: {e}", dir.display());
        return;
    }
    let mut files: Vec<(&str, Vec<u8>)> = Vec::new();
    if flags.goals {
        files.push((GOALS_FILENAME, save_data::write_goals(&memory.goals)));
    }
    if flags.scores {
        files.push((SCORES_FILENAME, save_data::write_scores(&memory.high_scores)));
    }
    if flags.colors {
        files.push((COLORS_FILENAME, save_data::write_colors(&memory.colors)));
    }
    if flags.tips {
        files.push((TIPS_FILENAME, save_data::write_tips(&memory.tips_seen)));
    }
    for (name, bytes) in files {
        if let Err(e) = std::fs::write(dir.join(name), bytes) {
            warn!("cannot save {name}: {e}");
        }
    }
}

pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Last, save_requested);
    }
}
