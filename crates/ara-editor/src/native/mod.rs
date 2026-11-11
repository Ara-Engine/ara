//! Editor-only SDL clipboard and monitor-scale adapters.
mod coordinates;
pub(crate) mod frame;
use ara_ui_text::Clipboard;

pub(crate) struct OsClipboard;
impl Clipboard for OsClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        ara_platform::text::read_clipboard().map_err(|error| error.to_string().into())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        let result: ara_ui_text::Result<()> =
            ara_platform::text::write_clipboard(text).map_err(|error| error.to_string().into());
        // Command-driven copies may intentionally ignore the return value.
        if let Err(error) = &result {
            eprintln!(
                "{}: Editor clipboard event failed: {error}",
                ara_diag::codes::IO
            );
            ara_diag::Diagnostic::warning(
                ara_diag::codes::IO,
                format!("Editor clipboard event failed: {error}"),
            )
            .emit();
        }
        result
    }
}
