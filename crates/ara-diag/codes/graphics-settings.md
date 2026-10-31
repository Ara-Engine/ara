# User graphics settings

## ARA3500 GRAPHICS_INVALID: Invalid or unavailable graphics preference
owner: ara-graphics-settings

A numeric graphics preference was clamped, or Apply requested a capability that
the current device or display cannot provide. Rejected applications retain the
last accepted configuration and leave the candidate available for correction.

Causes:
- A resolution, scale, frame cap or texture budget lies outside its supported range.
- HDR, hardware ray tracing or a monitor is unavailable on this player surface.

Fixes:
- Query play.graphics capabilities and choose a supported value, then Apply again.
- ResetToDefaults stages the project's defaults; Apply accepts them explicitly.

## ARA3501 GRAPHICS_STORAGE: Graphics settings could not be loaded or saved
owner: ara-graphics-settings

The per-user graphics settings file is unreadable, malformed, too large, uses an
unsupported version, or could not be atomically replaced. Startup retains project
defaults; Save reports the error without replacing the last good file.

Causes:
- The settings file is invalid strict JSON or uses a future schema version.
- The engine user config directory is unavailable or not writable.

Fixes:
- Set ARA_CONFIG_DIR to a writable user configuration directory.
- Use the engine version that wrote a future file; this engine will not overwrite it.
