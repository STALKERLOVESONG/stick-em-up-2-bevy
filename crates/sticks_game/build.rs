//! Local debug hacks: `src/hacks.rs` is git-ignored. When the file exists on this machine the
//! game is built with `cfg(debug_hacks)` and `lib.rs` includes it; a fresh clone builds without.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(debug_hacks)");
    println!("cargo::rerun-if-changed=src");
    if std::path::Path::new("src/hacks.rs").exists() {
        println!("cargo::rustc-cfg=debug_hacks");
    }
}
