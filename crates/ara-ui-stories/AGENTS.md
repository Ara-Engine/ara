# ara-ui-stories

Purpose: schema-registered, canonical component stories and reproducible native UI fixtures.
No renderer, protocol, OS input or window belongs here.

Public API:
- `register` installs the `stories` document kind and reflected story types.
- `StoryBook::{read, new, edit, stories, write}` uses ara-doc validation, typed edits and ops.
- `Story::{validate, mount}` resolves fixture models, localized source and theme tokens.
- `StoryView` fixes logical size, DPI, locale, input mode, theme and animation time.
- `localize` expands/accentuates pseudo-locales; `Locale` supplies explicit Arabic translations.
- `catalogue` provides every runtime primitive plus editor panels and game screens.

Tests: `CARGO_BUILD_JOBS=2 cargo test -p ara-ui-stories`.
Run targeted Clippy with `--all-targets -- -D warnings` and package rustfmt check.
The complete native render/semantic/golden suite lives in ara-ui-test.

Rules:
- Stories are ordinary Ara documents. Edits return ara-doc ops; never expose mutable records.
- Fixture models must be registered types. Bindings initialize typed native control values.
- Localization changes only a temporary source document through source commands/ops.
- `loc:key` strings are resolved before component expansion, including template defaults.
- Pseudo-localization never changes action names, binding keys, IDs or model values.
- The caller supplies fonts and dispatches emitted gameplay actions at its simulation boundary.
