//! The cheats only work in a game built with its local debug hacks (`sticks_game/src/hacks.rs`,
//! git-ignored; see the game's `build.rs`). The launcher offers them when that file is here too.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(game_hacks)");
    println!("cargo::rerun-if-changed=../sticks_game/src/hacks.rs");
    if std::path::Path::new("../sticks_game/src/hacks.rs").exists() {
        println!("cargo::rustc-cfg=game_hacks");
    }
}
