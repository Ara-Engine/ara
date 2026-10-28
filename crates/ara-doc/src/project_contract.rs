//! Shared on-disk authoring convention. Project hosts own path validation and I/O.

/// The single authored-content root. Asset references are relative to this directory.
pub const AUTHORING_ROOT: &str = "content";

/// The first layout with the final authored root and project/player boundary.
pub const PROJECT_LAYOUT_VERSION: u64 = 4;

/// An on-disk layout detected before opening a project or preparing migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectLayout {
    /// Preview-4 or newer; all consumers use the shared authored root.
    Current,
    /// Preview-3's provisional, isolated authored root.
    Preview3,
    /// Earlier projects kept authored folders beside the project marker.
    Root,
}

impl ProjectLayout {
    /// Storage prefix used only by the open-time check and migration planner.
    pub const fn authoring_root(self) -> &'static str {
        match self {
            Self::Current => AUTHORING_ROOT,
            Self::Preview3 => "game",
            Self::Root => "",
        }
    }

    /// Whether regular project consumers may open this layout.
    pub const fn is_current(self) -> bool {
        matches!(self, Self::Current)
    }
}

/// Detects a marker's layout without probing folders or modifying the project.
/// The explicit marker wins over coincidental folders named `content` or `game`.
pub fn detect_project_layout(marker: &serde_json::Value) -> ProjectLayout {
    match marker.get("layoutVersion").and_then(serde_json::Value::as_u64) {
        Some(version) if version >= PROJECT_LAYOUT_VERSION => ProjectLayout::Current,
        Some(3) => ProjectLayout::Preview3,
        _ => ProjectLayout::Root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_marker_selects_the_layout_and_missing_version_is_legacy() {
        for marker in [json!({}), json!({"layoutVersion":1}), json!({"layoutVersion":2})] {
            assert_eq!(detect_project_layout(&marker), ProjectLayout::Root);
        }
        assert_eq!(detect_project_layout(&json!({"layoutVersion":3})).authoring_root(), "game");
        for version in [4,5] {
            let layout = detect_project_layout(&json!({"layoutVersion":version}));
            assert!(layout.is_current());
            assert_eq!(layout.authoring_root(), AUTHORING_ROOT);
        }
        assert_eq!(detect_project_layout(&json!({"main":"content/level.scene"})), ProjectLayout::Root);
    }
}
