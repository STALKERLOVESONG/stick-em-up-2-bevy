//! Player options kept next to the save files: the settings (`Settings.txt`, changed in the
//! Lobby's settings page or the launcher) and the options a launcher starts the game with
//! (cheats, a player name). None of this is in the original, which had no settings on desktop.
//!
//! Both are plain `key = value` text so a person can read and edit them; unknown keys and broken
//! lines keep their defaults.

use std::path::PathBuf;

/// The settings file in [`save_dir`].
pub const SETTINGS_FILENAME: &str = "Settings.txt";
/// The environment variable a launcher passes [`LaunchOptions`] in (their [`LaunchOptions::to_text`]).
pub const LAUNCH_OPTIONS_VARIABLE: &str = "STICK_LAUNCH_OPTIONS";
/// The game speeds the speed cheat cycles through.
pub const GAME_SPEEDS: [f32; 4] = [1.0, 0.25, 0.5, 2.0];

/// Where the save files live: `STICK_SAVE_DIR` when set, else `%APPDATA%\StickEmUp2Reconstruction`
/// on Windows and `$XDG_DATA_HOME` (or `~/.local/share`)`/StickEmUp2Reconstruction` elsewhere.
/// `None` when no folder can be named.
pub fn save_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("STICK_SAVE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.map(|b| b.join("StickEmUp2Reconstruction"))
}

/// The exported Unity project (`ExportedProject`, with `Assets/` and `ProjectSettings/`): from
/// `STICK_EXPORT_ROOT`, else `first_argument`, else an `assets_exported/ExportedProject`
/// directory found by walking up from the working directory and from the executable.
pub fn find_export_root(first_argument: Option<&str>) -> Option<PathBuf> {
    let explicit = std::env::var_os("STICK_EXPORT_ROOT").map(PathBuf::from).or_else(|| first_argument.map(PathBuf::from));
    explicit.or_else(|| {
        let starts = [std::env::current_dir().ok(), std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf))];
        starts.into_iter().flatten().find_map(|start| start.ancestors().map(|dir| dir.join("assets_exported/ExportedProject")).find(|candidate| candidate.join("Assets").is_dir()))
    })
}

/// `key = value` lines.
fn entries(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|line| line.split_once('=')).map(|(k, v)| (k.trim(), v.trim()))
}

fn flag(value: &str) -> Option<bool> {
    match value {
        "true" | "on" | "1" => Some(true),
        "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn fraction(value: &str) -> Option<f32> {
    value.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// 0..=1, multiplies the music.
    pub music_volume: f32,
    /// 0..=1, multiplies every other sound.
    pub sound_volume: f32,
    pub fullscreen: bool,
    pub vsync: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { music_volume: 1.0, sound_volume: 1.0, fullscreen: false, vsync: true }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for (key, value) in entries(text) {
            match key {
                "music_volume" => s.music_volume = fraction(value).unwrap_or(s.music_volume),
                "sound_volume" => s.sound_volume = fraction(value).unwrap_or(s.sound_volume),
                "fullscreen" => s.fullscreen = flag(value).unwrap_or(s.fullscreen),
                "vsync" => s.vsync = flag(value).unwrap_or(s.vsync),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!("music_volume = {}\nsound_volume = {}\nfullscreen = {}\nvsync = {}\n", self.music_volume, self.sound_volume, self.fullscreen, self.vsync)
    }

    /// The saved settings, or the defaults.
    pub fn load() -> Settings {
        save_dir().and_then(|dir| std::fs::read_to_string(dir.join(SETTINGS_FILENAME)).ok()).map_or_else(Settings::default, |t| Settings::parse(&t))
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = save_dir().ok_or_else(|| std::io::Error::other("no folder for the settings"))?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(SETTINGS_FILENAME), self.to_text())
    }

    /// The next volume a click picks: 100 %, 75 %, 50 %, 25 %, off, then 100 % again.
    pub fn next_volume(volume: f32) -> f32 {
        let step = (volume * 4.0).round() - 1.0;
        if step < 0.0 { 1.0 } else { step / 4.0 }
    }
}

/// Cheats a launcher can switch on before the game starts. The same cheats have hotkeys in a
/// level when the game is built with its local debug hacks (`sticks_game/src/hacks.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cheats {
    /// No damage (the abyss included).
    pub god: bool,
    /// A lost life is put back.
    pub infinite_lives: bool,
    /// Every weapon can be fired as fast as the game draws.
    pub rapid_fire: bool,
    /// Every weapon, whatever the level gives.
    pub all_weapons: bool,
    /// The players' helicopters take no damage.
    pub helicopter_god: bool,
    /// Index into [`GAME_SPEEDS`].
    pub speed: usize,
    /// The hotkey list is shown in levels.
    pub show_hotkeys: bool,
}

impl Cheats {
    pub fn any(&self) -> bool {
        self.god || self.infinite_lives || self.rapid_fire || self.all_weapons || self.helicopter_god || self.speed != 0
    }

    pub fn speed_factor(&self) -> f32 {
        GAME_SPEEDS.get(self.speed).copied().unwrap_or(1.0)
    }
}

/// What a launcher starts the game with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchOptions {
    /// Only what the original game has: no cheats, no internet play, no rooms, no working Send
    /// Link, LAN games as the original announces them.
    pub original_only: bool,
    pub cheats: Cheats,
    /// The original's `AllLevelUnlockCheat`: every level can be picked.
    pub unlock_all_levels: bool,
    /// The name other machines see for this one in network games (the machine name otherwise).
    pub player_name: Option<String>,
}

impl LaunchOptions {
    pub fn parse(text: &str) -> LaunchOptions {
        let mut o = LaunchOptions::default();
        let c = &mut o.cheats;
        for (key, value) in entries(text) {
            let on = flag(value);
            match key {
                "god" => c.god = on.unwrap_or(c.god),
                "infinite_lives" => c.infinite_lives = on.unwrap_or(c.infinite_lives),
                "rapid_fire" => c.rapid_fire = on.unwrap_or(c.rapid_fire),
                "all_weapons" => c.all_weapons = on.unwrap_or(c.all_weapons),
                "helicopter_god" => c.helicopter_god = on.unwrap_or(c.helicopter_god),
                "show_hotkeys" => c.show_hotkeys = on.unwrap_or(c.show_hotkeys),
                "speed" => c.speed = value.parse::<usize>().ok().filter(|&s| s < GAME_SPEEDS.len()).unwrap_or(c.speed),
                "unlock_all_levels" => o.unlock_all_levels = on.unwrap_or(o.unlock_all_levels),
                "original_only" => o.original_only = on.unwrap_or(o.original_only),
                "player_name" => o.player_name = clean_name(value),
                _ => {}
            }
        }
        o
    }

    pub fn to_text(&self) -> String {
        let c = &self.cheats;
        let mut text = format!(
            "original_only = {}\ngod = {}\ninfinite_lives = {}\nrapid_fire = {}\nall_weapons = {}\nhelicopter_god = {}\nspeed = {}\nshow_hotkeys = {}\nunlock_all_levels = {}\n",
            self.original_only, c.god, c.infinite_lives, c.rapid_fire, c.all_weapons, c.helicopter_god, c.speed, c.show_hotkeys, self.unlock_all_levels
        );
        if let Some(name) = &self.player_name {
            text.push_str(&format!("player_name = {name}\n"));
        }
        text
    }

    /// The options this process was started with ([`LAUNCH_OPTIONS_VARIABLE`]); the defaults
    /// (nothing switched on) when the game was not started by a launcher.
    pub fn from_environment() -> LaunchOptions {
        std::env::var(LAUNCH_OPTIONS_VARIABLE).map_or_else(|_| LaunchOptions::default(), |t| LaunchOptions::parse(&t.replace(';', "\n")))
    }

    /// What the game is started with: in original-only mode nothing of the additions travels.
    pub fn effective(&self) -> LaunchOptions {
        if self.original_only {
            LaunchOptions { original_only: true, ..LaunchOptions::default() }
        } else {
            self.clone()
        }
    }

    /// The value for [`LAUNCH_OPTIONS_VARIABLE`] (one line: `;` separates the entries).
    pub fn to_environment(&self) -> String {
        self.to_text().trim_end().replace('\n', ";")
    }
}

/// A player name as it can travel: one line, no separators, at most 25 characters (what the
/// original's "Join Game: …" label holds), `None` when nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    let cleaned: String = name.chars().filter(|c| !c.is_control() && *c != ';' && *c != '/' && *c != '\0').take(25).collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_volumes_cycle() {
        let s = Settings { music_volume: 0.25, sound_volume: 0.5, fullscreen: true, vsync: false };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse("music_volume = 7\nnonsense\nvsync = maybe"), Settings { music_volume: 1.0, ..Settings::default() });
        let mut v = 1.0;
        let mut seen = Vec::new();
        for _ in 0..6 {
            v = Settings::next_volume(v);
            seen.push(v);
        }
        assert_eq!(seen, vec![0.75, 0.5, 0.25, 0.0, 1.0, 0.75]);
    }

    #[test]
    fn launch_options_round_trip_through_the_environment_form() {
        let o = LaunchOptions {
            original_only: false,
            cheats: Cheats { god: true, rapid_fire: true, speed: 3, show_hotkeys: true, ..Cheats::default() },
            unlock_all_levels: true,
            player_name: Some("Big Dan".to_owned()),
        };
        assert_eq!(LaunchOptions::parse(&o.to_text()), o);
        assert_eq!(LaunchOptions::parse(&o.to_environment().replace(';', "\n")), o);
        let original = LaunchOptions { original_only: true, ..o.clone() };
        assert_eq!(LaunchOptions::parse(&original.to_text()), original);
        assert_eq!(original.effective(), LaunchOptions { original_only: true, ..LaunchOptions::default() });
        assert_eq!(o.effective(), o);
        assert!(!o.to_environment().contains('\n'));
        assert_eq!(LaunchOptions::parse("speed = 9\ngod = maybe"), LaunchOptions::default());
        assert_eq!(clean_name("  a;b/c\n "), Some("abc".to_owned()));
        assert_eq!(clean_name(" \t"), None);
        assert_eq!(clean_name(&"x".repeat(40)).unwrap().len(), 25);
    }
}
