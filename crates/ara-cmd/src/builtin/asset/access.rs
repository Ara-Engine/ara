//! Checked file-state view for a single prepared asset command. Importer BLAKE3
//! identities and shared SHA-256 file-state tokens are intentionally distinct.
use super::{
    editable, inside, invalid,
    metadata::{AssetConsumer, AssetMetadata, AssetOperationMetadata},
};
use ara_diag::Diagnostic;
use ara_doc::file_state::{FileKind, FileState};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Inventory {
    pub token: String,
    pub authored_prefix: String,
    pub case_insensitive: bool,
    pub files: BTreeMap<String, FileState>,
    pub source_paths: BTreeSet<String>,
    pub consumers: BTreeMap<String, AssetConsumer>,
    pub complete: bool,
}
impl Inventory {
    pub fn checked(
        ctx: &crate::EditCtx<'_, '_>,
        metadata: &AssetMetadata,
        operation: &AssetOperationMetadata,
    ) -> Result<Self, Diagnostic> {
        let authored_prefix = ctx.read().workspace().reference_root().to_owned();
        let inventory = Self {
            token: metadata.token.clone(),
            authored_prefix: authored_prefix.clone(),
            case_insensitive: operation.inventory.case_insensitive,
            files: ctx
                .file_entries()
                .filter(|(path, _)| {
                    authored_prefix.is_empty()
                        || *path == authored_prefix
                        || inside(path, &authored_prefix)
                })
                .map(|(path, state)| (path.to_owned(), state.clone()))
                .collect(),
            source_paths: operation.inventory.source_paths.clone(),
            consumers: operation.inventory.consumers.clone(),
            complete: operation.inventory.complete,
        };
        inventory.validate()?;
        Ok(inventory)
    }
    pub fn reference<'a>(&self, storage: &'a str) -> Result<&'a str, Diagnostic> {
        if self.authored_prefix.is_empty() {
            return Ok(storage);
        }
        storage
            .strip_prefix(&self.authored_prefix)
            .and_then(|tail| tail.strip_prefix('/'))
            .filter(|tail| !tail.is_empty())
            .ok_or_else(|| invalid(format!("`{storage}` is outside the authored root")))
    }
    pub fn storage(&self, reference: &str) -> String {
        if self.authored_prefix.is_empty() {
            reference.to_owned()
        } else {
            format!("{}/{reference}", self.authored_prefix)
        }
    }
    pub fn editable(&self, storage: &str) -> Result<(), Diagnostic> {
        editable(storage)?;
        let reference = self.reference(storage)?;
        if self.authored_prefix.is_empty()
            && [
                "settings",
                "packages",
                "builds",
                "target",
                "node_modules",
                "spikes",
                "evidence",
                "tests",
            ]
            .iter()
            .any(|root| inside(&self.path_key(reference), &self.path_key(root)))
        {
            return Err(invalid("policy, packages and derived roots stay in place"));
        }
        Ok(())
    }
    pub fn path_key(&self, path: &str) -> String {
        ara_doc::file_state::path_key_with_case(path, self.case_insensitive)
    }
    pub fn same_path(&self, a: &str, b: &str) -> bool {
        self.path_key(a) == self.path_key(b)
    }
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if self.token.is_empty() || !self.complete {
            return Err(invalid(
                "asset dependency inventory is incomplete; finish background discovery and retry",
            ));
        }
        if self.source_paths != self.consumers.keys().cloned().collect() {
            return Err(invalid(
                "asset dependency inventory does not cover every recognized source",
            ));
        }
        let mut spellings = BTreeSet::new();
        for (path, state) in &self.files {
            ara_doc::file_state::check_path(path)?;
            state.validate()?;
            if !spellings.insert(self.path_key(path)) {
                return Err(invalid(format!(
                    "ambiguous filesystem spelling in asset inventory: `{path}`"
                )));
            }
        }
        for (source, consumer) in &self.consumers {
            editable(source)?;
            self.require_state(source, &consumer.state)?;
            proof(&consumer.blake3)?;
            let parent = source.rsplit_once('/').map_or("", |(parent, _)| parent);
            for input in &consumer.inputs {
                editable(&input.path)?;
                self.require_state(&input.path, &input.state)?;
                proof(&input.blake3)?;
                if !self.same_path(
                    &super::dependencies::resolve(parent, &input.uri)?,
                    &input.path,
                ) {
                    return Err(invalid(format!(
                        "local URI `{}` for `{source}` no longer resolves to `{}`; refresh dependency discovery",
                        input.uri, input.path
                    )));
                }
            }
        }
        Ok(())
    }
    fn require_state(&self, path: &str, state: &FileState) -> Result<(), Diagnostic> {
        if state.kind != FileKind::File || self.files.get(&self.storage(path)) != Some(state) {
            return Err(invalid(format!(
                "source/input SHA snapshot for `{path}` does not match the checked file inventory; refresh and retry"
            )));
        }
        Ok(())
    }
}
fn proof(hash: &str) -> Result<(), Diagnostic> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "invalid importer BLAKE3 proof in prepared metadata",
        ));
    }
    Ok(())
}
