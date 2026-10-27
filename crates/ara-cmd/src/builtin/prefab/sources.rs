//! Explicit source choices retain nested and variant boundaries instead of flattening them.
use ara_diag::{Diagnostic, codes};
use ara_doc::{AssetId, Workspace, prefab_of};
use serde::Serialize;
use std::collections::BTreeSet;

/// An authoring boundary to which an instance's selected overrides can be applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefabSource {
    /// Prefab asset identity.
    pub doc: AssetId,
    /// Canonical project-relative source path.
    pub path: String,
    /// Entity address in that prefab; it may itself be a nested instance.
    pub entity: String,
}

/// Sources from the nearest containing prefab to its nested or variant base sources.
/// The first choice preserves the surrounding prefab or variant's inheritance.
pub fn prefab_sources(
    workspace: &Workspace,
    doc: &str,
    entity: &str,
) -> Result<Vec<PrefabSource>, Diagnostic> {
    trace(workspace, doc, entity, None)
}

pub(super) fn address_in_source(
    workspace: &Workspace,
    doc: &str,
    entity: &str,
    source: &AssetId,
) -> Result<String, Diagnostic> {
    trace(workspace, doc, entity, Some(source))?
        .into_iter()
        .find(|candidate| &candidate.doc == source)
        .map(|candidate| candidate.entity)
        .ok_or_else(|| {
            Diagnostic::error(
                codes::INVALID_PARAMS,
                "The chosen prefab is not a source of this entity",
            )
        })
}

fn trace(
    workspace: &Workspace,
    doc: &str,
    entity: &str,
    stop: Option<&AssetId>,
) -> Result<Vec<PrefabSource>, Diagnostic> {
    let mut doc = workspace.find(doc).cloned().ok_or_else(|| {
        Diagnostic::error(
            codes::DOC_NOT_FOUND,
            format!("Document `{doc}` is not loaded"),
        )
    })?;
    let mut entity = entity.to_owned();
    let mut sources = Vec::new();
    let mut seen = BTreeSet::from([doc.clone()]);
    loop {
        let document = workspace.doc(doc.as_str()).ok_or_else(|| {
            Diagnostic::error(codes::PREFAB_NOT_FOUND, "A source prefab is not loaded")
        })?;
        let (head, inner) = entity
            .split_once('/')
            .map_or((entity.as_str(), None), |(head, inner)| (head, Some(inner)));
        let record = document.record(head).ok_or_else(|| {
            Diagnostic::error(
                codes::RECORD_NOT_FOUND,
                format!("No entity `{entity}` in `{doc}`"),
            )
        })?;
        let Some(reference) = prefab_of(record) else {
            if inner.is_some() {
                return Err(Diagnostic::error(
                    codes::NOT_AN_INSTANCE,
                    format!("`{head}` is not a prefab instance"),
                ));
            }
            return Ok(sources);
        };
        let (reference, _) = ara_doc::refs::resolve(reference, Some("prefab"), workspace.index())?;
        let id = ara_schema::grammar::parse_ref(&reference)
            .map_err(|error| Diagnostic::error(codes::PREFAB_NOT_FOUND, error))?
            .id
            .ok_or_else(|| {
                Diagnostic::error(
                    codes::PREFAB_NOT_FOUND,
                    "The prefab reference has no asset identity",
                )
            })?;
        let source = workspace.doc(id).ok_or_else(|| {
            Diagnostic::error(
                codes::PREFAB_NOT_FOUND,
                format!("Prefab `{reference}` is not loaded"),
            )
        })?;
        if !seen.insert(source.id.clone()) {
            return Err(Diagnostic::error(
                codes::PREFAB_CYCLE,
                "A prefab source chain contains a cycle",
            ));
        }
        let address = match inner {
            Some(inner) => inner.to_owned(),
            None => source
                .children(None)
                .first()
                .map(ToString::to_string)
                .ok_or_else(|| {
                    Diagnostic::error(codes::ROOT_COUNT, "A prefab source has no root")
                })?,
        };
        sources.push(PrefabSource {
            doc: source.id.clone(),
            path: workspace.path_of(id).unwrap_or_default().into(),
            entity: address.clone(),
        });
        if stop == Some(&source.id) {
            return Ok(sources);
        }
        doc = source.id.clone();
        entity = address;
    }
}
