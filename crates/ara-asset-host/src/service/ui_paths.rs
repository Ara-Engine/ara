//! Resolve transactional UI policy through the same authored reference contract as Play.
use super::{bindings::Bindings, inventory::one};
use ara_diag::{Diagnostic, codes};

pub(super) fn configured(
    bindings: &Bindings,
    field: &str,
    fallback: &str,
) -> Result<String, Vec<Diagnostic>> {
    let policy = ara_host::project_layout::policy(&bindings.workspace, "ui");
    let Some(value) = policy
        .and_then(|policy| policy.get(field))
        .filter(|value| !value.is_null())
    else {
        return Ok(fallback.to_owned());
    };
    let value = value
        .as_str()
        .ok_or_else(|| invalid(&format!("ui.{field}"), "UI resource paths must be strings"))?;
    resolve(bindings, &format!("ui.{field}"), value)
}

pub(super) fn resolve(
    bindings: &Bindings,
    field: &str,
    value: &str,
) -> Result<String, Vec<Diagnostic>> {
    let index = bindings.workspace.index();
    let path = if value.starts_with('@') {
        let parts = ara_schema::grammar::parse_ref(value).map_err(|error| invalid(field, error))?;
        parts
            .id
            .and_then(|id| index.path_of(id))
            .or(parts.path)
            .map(str::to_owned)
            .ok_or_else(|| unresolved(field, value))?
    } else if let Some(path) = index.path_of(value) {
        path.to_owned()
    } else if ara_schema::grammar::split_id(value).is_some() {
        return Err(unresolved(field, value));
    } else {
        value.to_owned()
    };
    ara_doc::file_state::check_path(&path).map_err(|_| {
        invalid(
            field,
            format!("Path `{value}` must stay inside the authored root"),
        )
    })?;
    Ok(path)
}

fn invalid(field: &str, message: impl Into<String>) -> Vec<Diagnostic> {
    one(Diagnostic::error(codes::INVALID_REF, message)
        .file(ara_host::project_layout::PREFERENCES)
        .path(field))
}

fn unresolved(field: &str, value: &str) -> Vec<Diagnostic> {
    one(Diagnostic::error(codes::UNRESOLVED_REF,format!("{field} reference `{value}` has no live authored path; set {field} to an authored file path or a valid @path#id reference")).file(ara_host::project_layout::PREFERENCES).path(field))
}
