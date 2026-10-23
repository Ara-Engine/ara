# ara-l10n

Canonical localization tables, compiled ICU messages and human/agent workflows.
No provider SDK, credentials, filesystem watcher, or mutable gameplay state here.

## APIs

- `register_schema` installs the l10n document kind (`lc_` asset IDs), the single
  `l_config` locale record, and stable `m_` message records. Reflected names use L10n.
- `TableCommand::edits` produces standard `Edit`s: submit through `Store::transact`.
  `Table::read/write` use the canonical document reader/writer. Table is read-only.
- `Message::parse/format`: nested plural/select/selectordinal, exact branches,
  integer offsets, apostrophe quoting, exact decimals, ICU4X number/date/currency.
  Parameters are explicit; messages must use exactly their declared signature.
  Values use sanitized FSI/PDI isolation. No concatenated English fragments.
- `Catalog` compiles loaded tables, follows structural/configured/source fallback,
  checks signatures, returns parity diagnostics with runnable fixes and font assets.
- `generate_keys`: Rust `Key<Params>` constants + C# `L10nKey<Params>` records.
  Generated C# carries values only; the host supplies the localization service/ABI.
- `LiveCatalog::stage/apply_boundary`: compile on a worker, publish at a host-approved
  frame boundary. Bindings invalidate text caches when `generation` changes. Use
  ara-asset FileWatcher/committed op events to schedule Table/Catalog recompilation.
- `Catalog::story_locale` feeds ara-ui-stories/ara-ui-test directly, reusing their
  pseudo-localization and RTL mirror pass. Provide fixtures for parameterized keys.
  Resolve `fonts()` assets on a worker and pass bytes/families to TextSystem::new.
- XLIFF 2.0 and PO carry whole ICU messages, context, signatures and provenance.
  `import_commands` rejects stale source and returns typed transaction mutations.
  Inline XLIFF codes/native gettext plural arrays require whole-pattern conversion.
- `tools::register` installs l10n.find_missing, l10n.extract, l10n.translate_job,
  l10n.put and l10n.remove. Scope is edit:l10n for mutations. Generation providers
  run off-thread with `prepare_translation/run_provider`; commit the response via
  TranslateJob for atomic source/target freshness checks, provenance and undo.
- Extraction scans markup text/visible attributes and C# plain/verbatim literals.
  It returns byte spans for reviewed code replacements; only table ops are applied.
  Dynamic/interpolated C# requires an explicit key/signature instead of flattening.

## Checks

Set CARGO_BUILD_JOBS=2. Run cargo check/test/clippy -p ara-l10n (clippy --all-targets
-- -D warnings) and cargo fmt -p ara-l10n -- --check. Tests exercise the native
Harness as well as Arabic CLDR, isolation, fallback, exchange, registry ops and jobs.
Set ARA_L10N_CAPTURE_DIR to a scratch directory to capture the two native UI tests.

## Gotchas

ICU4X uses compiled Rust data, including currency patterns/fractions. No OS locale,
ICU C library or build-time generator is needed. ICU experimental currency APIs are
isolated in format.rs. Keep canonical authoring files separate from XLIFF/PO export.
Missing/extra translations warn; invalid syntax/signatures block publication.
The host registers tools and schedules workers; this crate does not edit ara-host.
