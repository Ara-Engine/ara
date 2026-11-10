//! The editor's shared font set, used by the studio, the shell and the loading window.
use ara_ui_text::TextSystem;

/// The editor's fonts: Inter (UI), JetBrains Mono (code, timings) and Noto Naskh
/// Arabic (right-to-left text).
pub(crate) fn text_system() -> ara_ui::Result<TextSystem> {
    TextSystem::new(
        [
            include_bytes!("../assets/fonts/InterVariable.ttf").to_vec(),
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").to_vec(),
            include_bytes!("../../ara-ui-text/fonts/NotoNaskhArabic.ttf").to_vec(),
        ],
        "'Inter Variable', 'Noto Naskh Arabic'",
    )
}
