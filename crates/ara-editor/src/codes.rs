//! Editor diagnostic codes (`ARA4500`–`ARA4599`, owner `ara-editor`). Each has an
//! entry in `crates/ara-diag/codes/editor.md`. Editor failures go to the project's
//! diagnostics bus (Console, Problems, `ara logs`), never only to stderr.
use ara_diag::Code;

/// A menu, panel or shortcut action failed.
pub const ACTION: Code = Code(4500);
/// A thumbnail or preview could not be produced.
pub const THUMBNAIL: Code = Code(4501);
/// Preferences could not be read or written.
pub const SETTINGS: Code = Code(4502);
/// The Engine Protocol transport stopped accepting connections.
pub const TRANSPORT: Code = Code(4503);
/// An editor window failed to render a frame (Scene view, previews, floating windows).
pub const RENDER: Code = Code(4504);
/// A build (Build Current Profile) or its launch failed, or reports progress.
pub const BUILD: Code = Code(4505);
/// A scene references an asset file that is missing; it draws a placeholder.
pub const MISSING_SOURCE: Code = Code(4506);
