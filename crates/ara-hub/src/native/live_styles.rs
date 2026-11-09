//! Debug builds watch `hub.css`, the shared Select CSS and both theme files in the
//! checkout this Hub was built from, and apply each validated save to the running
//! window. An invalid save keeps the last good styles and prints why. Release builds
//! compile this out and use the embedded resources.
use std::{collections::BTreeMap, path::Path, path::PathBuf, sync::mpsc::Receiver};

const HUB_CSS: &str = "crates/ara-hub/src/view/hub.css";
const SELECT_CSS: &str = "crates/ara-ui/src/select/select.css";
const THEMES: [&str; 2] = [
    "assets/themes/ara-dark.json",
    "assets/themes/ara-light.json",
];

/// A validated composed stylesheet and both palettes, indexed by `light`.
pub(super) struct Styles {
    pub css: String,
    pub tokens: [BTreeMap<String, String>; 2],
}

/// The resource worker; dropping it stops the thread.
pub(super) struct LiveStyles {
    updates: Receiver<Result<Styles, String>>,
    _stop: ara_ui::live::Stop,
}

impl LiveStyles {
    pub(super) fn start() -> std::io::Result<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let files = root.clone();
        let (updates, _stop) = ara_ui::live::watch(
            "hub-styles",
            move || {
                [HUB_CSS, SELECT_CSS]
                    .into_iter()
                    .chain(THEMES)
                    .map(|path| files.join(path))
                    .collect()
            },
            move || read(&root),
            super::wake::wake,
        )?;
        Ok(Self { updates, _stop })
    }

    /// The newest snapshot published since the last call, without filesystem work.
    pub(super) fn poll(&self) -> Option<Result<Styles, String>> {
        self.updates.try_iter().last()
    }
}

/// Validate each CSS source on its own (so errors name the file), then the composed
/// sheet against both palettes, before anything reaches the UI.
fn read(root: &Path) -> Result<Styles, String> {
    let source =
        |path: &str| std::fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"));
    let (hub, select) = (source(HUB_CSS)?, source(SELECT_CSS)?);
    for (path, text) in [(HUB_CSS, &hub), (SELECT_CSS, &select)] {
        ara_ui::theme::check_editor("<ui id='style-validation' />", text)
            .map_err(|e| format!("{path}: {e}"))?;
    }
    let css = crate::view::compose(&hub, &select);
    let mut tokens = [BTreeMap::new(), BTreeMap::new()];
    for (path, palette) in THEMES.into_iter().zip(&mut tokens) {
        *palette =
            ara_ui::theme::tokens_from_json(&source(path)?).map_err(|e| format!("{path}: {e}"))?;
        ara_ui::style::Sheet::with_tokens(&css, palette.clone())
            .map_err(|e| format!("{HUB_CSS} with {path}: {e}"))?;
    }
    Ok(Styles { css, tokens })
}

#[cfg(test)]
mod tests {
    use ara_ui::theme::EditorTheme;

    #[test]
    fn checkout_sources_compose_to_the_embedded_styles() -> Result<(), String> {
        let styles = super::read(&super::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))?;
        assert_eq!(styles.css, crate::view::stylesheet());
        for (palette, theme) in styles
            .tokens
            .iter()
            .zip([EditorTheme::Dark, EditorTheme::Light])
        {
            assert_eq!(palette, &theme.tokens().map_err(|e| e.to_string())?);
        }
        Ok(())
    }
}
