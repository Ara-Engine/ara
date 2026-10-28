use super::{AssetMetadata, access::Inventory, invalid, operation_key};
use crate::{Command, EditCtx};
use ara_diag::Diagnostic;
use ara_doc::file_state::{FileKind, FileState};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Parameters of `folder.create`.
#[derive(Clone, Debug, Schema, Serialize, Deserialize)]
#[ara(params, module = "core")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderCreateParams {
    /// New folder path relative to the authored root.
    pub path: String,
    /// Also create missing intermediate empty directories; false requires an existing parent.
    #[serde(default)]
    #[ara(default = false)]
    pub parents: bool,
}
/// Create authored folders through the shared file-state transaction and undo history.
pub struct FolderCreate;
impl Command for FolderCreate {
    type Params = FolderCreateParams;
    type Output = Value;
    const NAME: &'static str = "folder.create";
    const SCOPE: &'static str = "edit:assets";
    const SUMMARY: &'static str = "Create an authored folder, optionally adding missing parents, in one undoable transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"path":"props/warehouse","parents":true})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        super::editable(&params.path).map_err(|diagnostic| diagnostic.path("path"))?;
        let metadata = AssetMetadata::read(ctx.prepared_metadata())?;
        let key = metadata.slot(ctx, &operation_key(Self::NAME, &params)?);
        let operation = metadata.operation(&key)?;
        // Creating directories needs complete file facts, not importer discovery.
        let prefix = ctx.read().workspace().reference_root().to_owned();
        let inventory = Inventory {
            token: metadata.token,
            authored_prefix: prefix.clone(),
            case_insensitive: operation.inventory.case_insensitive,
            files: ctx
                .file_entries()
                .map(|(path, state)| (path.to_owned(), state.clone()))
                .collect(),
            source_paths: BTreeSet::new(),
            consumers: BTreeMap::new(),
            complete: true,
        };
        let leaf = inventory.storage(&params.path);
        inventory
            .editable(&leaf)
            .map_err(|diagnostic| diagnostic.path("path"))?;
        if inventory
            .files
            .keys()
            .any(|path| inventory.same_path(path, &leaf))
        {
            return Err(invalid(format!("folder `{}` already exists", params.path)).path("path"));
        }
        let components: Vec<_> = params.path.split('/').collect();
        let mut create = Vec::new();
        for length in 1..=components.len() {
            let logical = components[..length].join("/");
            let path = inventory.storage(&logical);
            if let Some((_, state)) = inventory
                .files
                .iter()
                .find(|(existing, _)| inventory.same_path(existing, &path))
            {
                if state.kind != FileKind::Directory {
                    return Err(
                        invalid(format!("folder ancestor `{logical}` is a file")).path("path")
                    );
                }
            } else {
                if length < components.len() && !params.parents {
                    return Err(invalid(format!(
                        "parent folder `{logical}` does not exist; set parents:true to create it"
                    ))
                    .path("parents"));
                }
                create.push((logical, path));
            }
        }
        for (_, path) in &create {
            ctx.set_file(path, Some(FileState::directory()))?;
        }
        Ok(
            json!({"path":params.path,"created":create.into_iter().map(|(logical,_)|logical).collect::<Vec<_>>()}),
        )
    }
}
