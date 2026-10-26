//! Shared helpers for the integration tests that need the exported game data.

use std::path::PathBuf;

/// The `ExportedProject` directory: `STICK_EXPORT_ROOT`, else the repository's own
/// `assets_exported/ExportedProject`. `None` (the test skips) when neither holds the data.
pub fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}
