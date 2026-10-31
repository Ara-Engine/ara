# ara-graphics-settings

Renderer-neutral per-user options. `Settings` is a complete candidate, `Service`
owns pending/applied/previous/default values and a versioned per-game store.
This is presentation state: never reflect it, replicate it, hash it or restore it
from a gameplay snapshot. The player installs its transient state provider.

`Request` is shared by `play.graphics` and the generated C# host callback.
`Apply` accepts a validated configuration; the presenter adopts it at its next
frame boundary. Ray tracing changes require a device reopen/player restart.
Save is explicit and saves applied values, including the next-start RT preference.
Revert applies the previous configuration, reset only stages project defaults.

Persistence uses canonical JSON, a bounded strict reader and atomic replacement.
Future versions are retained and cannot be overwritten by this engine version.
Tests use fresh `target/` directories and explicit paths, never the OS profile.

Check: `cargo test -p ara-graphics-settings`, `cargo clippy -p
ara-graphics-settings --all-targets -- -D warnings`. Mapping belongs to
ara-runtime-render; display application belongs to ara-player/ara-platform.
