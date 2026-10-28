//! Fail closed when a project-marker generator cannot be verified statically.
use std::collections::BTreeMap;

use proc_macro2::{Delimiter, TokenTree};
use syn::{Expr, Lit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Current,
    Legacy,
    Unknown,
}

pub fn writer(name: &str) -> bool {
    matches!(name, "write" | "write_json" | "write_file" | "put" | "json")
}

pub fn marker(expr: &Expr, locals: &BTreeMap<String, Expr>, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match expr {
        Expr::Lit(lit) => {
            matches!(&lit.lit, Lit::Str(text) if text.value().replace('\\', "/").rsplit('/').next() == Some("ara.project.json"))
        }
        Expr::Path(path) => {
            let name = path.path.segments.last().unwrap().ident.to_string();
            name == "MARKER"
                || locals
                    .get(&name)
                    .is_some_and(|expr| marker(expr, locals, depth + 1))
        }
        Expr::Reference(value) => marker(&value.expr, locals, depth + 1),
        Expr::Paren(value) => marker(&value.expr, locals, depth + 1),
        Expr::MethodCall(call)
            if matches!(call.method.to_string().as_str(), "join" | "with_file_name") =>
        {
            call.args.iter().any(|arg| marker(arg, locals, depth + 1))
        }
        _ => false,
    }
}

pub fn layout(expr: &Expr, locals: &BTreeMap<String, Expr>, depth: usize) -> Layout {
    if depth > 12 {
        return Layout::Unknown;
    }
    match expr {
        Expr::Lit(value) => {
            let text = match &value.lit {
                Lit::Str(text) => Some(text.value()),
                Lit::ByteStr(text) => String::from_utf8(text.value()).ok(),
                _ => None,
            };
            text.as_deref().map_or(Layout::Unknown, json_layout)
        }
        Expr::Macro(value)
            if value.mac.path.is_ident("json")
                || value
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|part| part.ident == "json") =>
        {
            // Macro expressions may contain dynamic names/IDs. Only the top-level
            // layoutVersion value is relevant, and it must be explicit.
            let Some(TokenTree::Group(object)) = value.mac.tokens.clone().into_iter().next() else {
                return Layout::Unknown;
            };
            if object.delimiter() != Delimiter::Brace {
                return Layout::Unknown;
            }
            let mut fields = object.stream().into_iter().peekable();
            let mut version = None;
            while let Some(key) = fields.next() {
                if fields.next().is_none_or(|colon| colon.to_string() != ":") {
                    return Layout::Unknown;
                }
                let mut field = Vec::new();
                while fields.peek().is_some_and(|token| token.to_string() != ",") {
                    field.push(fields.next().unwrap());
                }
                fields.next();
                if key.to_string() == "\"layoutVersion\"" {
                    version = Some(
                        field
                            .into_iter()
                            .map(|token| token.to_string())
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
            let Some(value) = version else {
                return Layout::Legacy;
            };
            if value.ends_with("PROJECT_LAYOUT_VERSION")
                || value.ends_with("project_layout : : VERSION")
            {
                Layout::Current
            } else {
                match value.parse::<u64>() {
                    Ok(version) if version >= ara_doc::PROJECT_LAYOUT_VERSION => Layout::Current,
                    Ok(_) => Layout::Legacy,
                    Err(_) => Layout::Unknown,
                }
            }
        }
        Expr::Reference(value) => layout(&value.expr, locals, depth + 1),
        Expr::Paren(value) => layout(&value.expr, locals, depth + 1),
        Expr::Path(path) => locals
            .get(&path.path.segments.last().unwrap().ident.to_string())
            .map_or(Layout::Unknown, |expr| layout(expr, locals, depth + 1)),
        Expr::Call(call) => call
            .args
            .first()
            .map_or(Layout::Unknown, |expr| layout(expr, locals, depth + 1)),
        Expr::MethodCall(call) => layout(&call.receiver, locals, depth + 1),
        _ => Layout::Unknown,
    }
}

fn json_layout(text: &str) -> Layout {
    match ara_json::parse(text) {
        Ok(value) if ara_doc::detect_project_layout(&value).is_current() => Layout::Current,
        _ => Layout::Legacy,
    }
}

/// These are old-format *inputs*, not projects regular consumers should open.
/// Keep exemptions function-scoped so a new accidental fixture in the same file fails.
pub fn allowed(file: &str, function: &str, layout: Layout) -> bool {
    if layout == Layout::Current {
        return true;
    }
    // Mutators of an existing marker retain its version. An explicit old literal
    // still fails here; only data-dependent read/modify/write sites are allowed.
    if layout == Layout::Unknown
        && matches!(
            (file, function),
            ("crates/ara-dev/src/d26/measure.rs", "Context < '_ >::pack")
                | ("crates/ara-dev/src/lab/scenes.rs", "freeze")
                | ("crates/ara-hub-service/src/registry.rs", "Registry::add")
                | ("crates/ara-hub-service/src/service.rs", "Hub::call_for")
                | (
                    "crates/ara-hub-service/tests/migration_consent.rs",
                    "content_project_refuses_undeclared_engine_before_launch_and_typed_choice_refreshes_registry"
                )
                | ("crates/ara-samples/src/editor.rs", "run")
        )
    {
        return true;
    }
    matches!(
        (file, function),
        // Open-time rejection, old policy parsing, and explicit migration inputs.
        (
            "crates/ara-asset/src/source.rs",
            "tests::source_open_rejects_old_projects_and_accepts_explicit_authored_roots"
        ) | (
            "crates/ara-host/tests/authored_index.rs",
            "legacy_projects_are_rejected_before_indexing_or_creating_state"
        ) | (
            "crates/ara-host/src/project_policy.rs",
            "tests::disk_policy_diagnoses_malformed_and_nonobject_documents"
        ) | ("crates/ara-host/tests/migration_v4.rs", "preview3")
            | (
                "crates/ara-host/tests/migration_v4.rs",
                "prior_attributed_history_is_retained_and_requires_undoing_migration_first"
            )
            | (
                "crates/ara-host/src/project_layout/history/tests.rs",
                "file_snapshots_migrate_read_only_then_support_undo_with_valid_references_and_ids"
            )
            | (
                "crates/ara-host/src/project_layout/history/tests.rs",
                "undo_restores_deleted_gltf_companions_under_the_same_reference_root"
            )
            | (
                "crates/ara-host/src/project_layout/history/tests.rs",
                "invalid_missing_and_corrupt_snapshots_fail_planning_without_writes_or_moves"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "migrated_project_keeps_ids_references_and_round_trips"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "collisions_and_live_authorities_leave_authored_files_intact"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "text_markup_references_move_without_rewriting_existing_journal_lines"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "legacy_build_profiles_keep_every_authored_field_and_pin"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "overlapping_draft_roots_merge_and_modules_and_local_uris_migrate"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "migration_keeps_freely_named_nested_derived_folders"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "migration_pins_legacy_ui_identity_before_storage_path_changes"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "outpost_native_ui_preserves_its_pristine_legacy_identity"
            )
            | (
                "crates/ara-host/tests/project_layout.rs",
                "migration_repairs_known_paths_and_references_without_renaming_labels"
            )
            | (
                "crates/ara-hub-service/tests/migration_consent.rs",
                "Fixture::new"
            )
            | (
                "crates/ara-hub-service/tests/process_exit.rs",
                "undeclared_preview_three_cli_launches_and_proxies_its_old_project"
            )
            | (
                "crates/ara-play-host/tests/game_ui.rs",
                "legacy_resource_contract_survives_explicit_migration_before_read"
            )
            | (
                "crates/ara-play-host/tests/ui_paths.rs",
                "legacy_ui_sources_require_migration_without_policy_or_scaffold_creation"
            )
            | (
                "crates/ara-play-host/tests/ui_paths.rs",
                "migration_materializes_movable_legacy_ui_paths"
            )
            | (
                "crates/ara/tests/asset_ops.rs",
                "legacy_project_asset_journey_requires_explicit_migration"
            )
            | (
                "crates/ara/tests/project_migrate.rs",
                "dry_run_migrate_check_undo_and_blocked_reopen_cross_process"
            )
    )
}

pub fn csharp(file: &str, text: &str, count: &mut usize, errors: &mut Vec<String>) {
    for call in text.split("File.WriteAllText(").skip(1) {
        let call = call.split(';').next().unwrap();
        if !call.contains("ara.project.json") {
            continue;
        }
        *count += 1;
        let literals = csharp_strings(call);
        if !literals
            .iter()
            .any(|text| json_layout(text) == Layout::Current)
        {
            errors.push(format!(
                "{file}: File.WriteAllText has no current-layout marker: {call}"
            ));
        }
    }
    // Named test helpers such as Write("ara.project.json", "...").
    for line in text.lines().filter(|line| {
        line.trim_start().starts_with("Write(\"") && line.contains("ara.project.json")
    }) {
        *count += 1;
        if !csharp_strings(line)
            .iter()
            .any(|text| json_layout(text) == Layout::Current)
        {
            errors.push(format!("{file}: helper writes a legacy marker: {line}"));
        }
    }
}

fn csharp_strings(text: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut start = None;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if let Some(begin) = start {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                if let Ok(value) = serde_json::from_str::<String>(&text[begin..=index]) {
                    strings.push(value);
                }
                start = None;
            }
        } else if ch == '"' {
            start = Some(index);
        }
    }
    strings
}
