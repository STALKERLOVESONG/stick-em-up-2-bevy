//! Local debug hacks: `src/hacks.rs` is git-ignored. When the file exists on this machine the
//! game is built with `cfg(debug_hacks)` and `lib.rs` includes it; a fresh clone builds without.

fn main() {
    // The original game's icon (from the player's SEU2.exe), else the remake's drawn one.
    exe_icon::link_icon(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/icons/game.res")));
    println!("cargo::rustc-check-cfg=cfg(debug_hacks)");
    println!("cargo::rerun-if-changed=src");
    if std::path::Path::new("src/hacks.rs").exists() {
        println!("cargo::rustc-cfg=debug_hacks");
    }
}

