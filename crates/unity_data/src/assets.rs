//! GUID → file lookups built from the `.meta` files that sit next to every exported asset.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `guid → asset path`, for assets with a given extension (e.g. `anim`, `cs`).
#[derive(Default)]
pub struct GuidIndex {
    by_guid: HashMap<String, PathBuf>,
}

impl GuidIndex {
    /// Recursively scan `dir` for `*.<extension>.meta` files.
    pub fn scan(dir: &Path, extension: &str) -> GuidIndex {
        let mut index = GuidIndex::default();
        let suffix = format!(".{extension}.meta");
        index.scan_dir(dir, &suffix);
        index
    }

    fn scan_dir(&mut self, dir: &Path, suffix: &str) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.scan_dir(&path, suffix);
            } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(suffix)) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Some(guid) = text.lines().find_map(|l| l.strip_prefix("guid: ")) {
                        // Strip the trailing ".meta" to get the asset path.
                        let asset = path.with_extension("");
                        self.by_guid.insert(guid.trim().to_owned(), asset);
                    }
                }
            }
        }
    }

    pub fn path(&self, guid: &str) -> Option<&Path> {
        self.by_guid.get(guid).map(PathBuf::as_path)
    }

    /// File stem of the asset (for scripts this is the class name).
    pub fn stem(&self, guid: &str) -> Option<&str> {
        self.path(guid)?.file_stem()?.to_str()
    }

    /// Every GUID in the index.
    pub fn guids(&self) -> Vec<String> {
        self.by_guid.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.by_guid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_guid.is_empty()
    }
}
