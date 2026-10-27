//! Bin-only tip works around dx's lib+bin replay gap (#5778).
//! Normal path dependencies also keep the editor visible to dx's metadata watcher.
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../ara-editor/src/main.rs"
));
