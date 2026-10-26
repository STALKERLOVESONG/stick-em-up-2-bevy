//! Script GUID → class name, read from the exported `.cs.meta` files.

use std::path::Path;

use crate::assets::GuidIndex;

/// `guid → script class name`.
#[derive(Default)]
pub struct ScriptIndex {
    index: GuidIndex,
}

impl ScriptIndex {
    /// Scan a directory tree (e.g. `Assets/Scripts`) for `*.cs.meta` files.
    pub fn scan(dir: &Path) -> ScriptIndex {
        ScriptIndex { index: GuidIndex::scan(dir, "cs") }
    }

    pub fn class_name(&self, guid: &str) -> Option<&str> {
        self.index.stem(guid)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}
