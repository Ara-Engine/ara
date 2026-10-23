//! Host-dispatched typed runtime tools. These operate on save slots/simulation,
//! never authoring documents; authorization is trusted host state, not request data.
use crate::{
    LoadedSlot, Result, SaveGame, SaveSchema, SlotInfo, SlotListing, SlotMetadata, SlotStore,
    StateHash, codes, diff, error,
};
use ara_schema::{Schema, SchemaDb};
use serde::{Deserialize, Serialize};

/// Parameters for `save.write`; capture at a tick boundary, write the owned save off-thread.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(name = "SaveWriteParams", params, module = "save")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteParams {
    /// Cloud-visible slot metadata; the host supplies timestamp and playtime.
    pub metadata: SlotMetadata,
    /// Expected prior whole-file hash, lowercase 64-digit hex, for conflict detection.
    pub expected_hash: Option<String>,
}
/// Parameters for `save.load` and `save.restore_backup`.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(name = "SaveLoadParams", params, module = "save")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoadParams {
    /// Portable save slot identifier.
    pub slot: String,
}
/// Parameters for `save.list`.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(name = "SaveListParams", params, module = "save")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListParams {}
/// Parameters for `save.diff` after both saves have migrated to the current schema.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(name = "SaveDiffParams", params, module = "save")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffParams {
    /// Prior slot.
    pub before: String,
    /// New slot.
    pub after: String,
}

/// Thumbnail capture seam. A host prepares the referenced asset off the simulation thread.
pub trait ThumbnailHook {
    /// Produces a standard asset reference or None when thumbnails are disabled.
    fn thumbnail(&mut self, slot: &str) -> Result<Option<ara_schema::AssetRef>>;
}

/// Tool description for host Engine Protocol/MCP registration.
#[derive(Debug, Serialize)]
pub struct ToolInfo {
    /// Stable agent tool name.
    pub name: &'static str,
    /// Reflected parameter type.
    pub params: &'static str,
    /// Required host capability; runtime writes are separate from authoring edits.
    pub scope: &'static str,
}
/// Registers parameter schemas and returns runtime tool descriptors for the host.
pub fn register(db: &mut SchemaDb) -> Vec<ToolInfo> {
    crate::register_schema(db);
    db.register::<WriteParams>()
        .register::<LoadParams>()
        .register::<ListParams>()
        .register::<DiffParams>();
    vec![
        ToolInfo {
            name: "save.write",
            params: "SaveWriteParams",
            scope: "runtime:save",
        },
        ToolInfo {
            name: "save.load",
            params: "SaveLoadParams",
            scope: "runtime:save",
        },
        ToolInfo {
            name: "save.list",
            params: "SaveListParams",
            scope: "read",
        },
        ToolInfo {
            name: "save.diff",
            params: "SaveDiffParams",
            scope: "read",
        },
        ToolInfo {
            name: "save.restore_backup",
            params: "SaveLoadParams",
            scope: "runtime:save",
        },
    ]
}

/// Prepared write: capture and thumbnail work are separate from disk publication.
pub struct PreparedWrite {
    /// Owned validated capture; send to a background writer without borrowing the world.
    pub game: SaveGame,
    /// Expected prior cloud/local generation.
    pub expected: Option<StateHash>,
}
impl PreparedWrite {
    /// Attaches an optional thumbnail on a capture worker before disk publication.
    pub fn capture_thumbnail(&mut self, hook: &mut impl ThumbnailHook) -> Result<()> {
        self.game.metadata.thumbnail = hook.thumbnail(&self.game.metadata.slot)?;
        Ok(())
    }
}
/// Validates authority and captures selected ECS state at a tick boundary.
pub fn prepare_write(
    store: &SlotStore,
    schema: &SaveSchema,
    session: &ara_sim::Session,
    params: WriteParams,
) -> Result<PreparedWrite> {
    store.authority().require_write()?;
    let expected = params
        .expected_hash
        .as_deref()
        .map(parse_hash)
        .transpose()?;
    Ok(PreparedWrite {
        game: SaveGame::capture(schema, session, params.metadata)?,
        expected,
    })
}
/// Runs the IO half of `save.write` on a worker.
pub fn write(store: &SlotStore, schema: &SaveSchema, prepared: PreparedWrite) -> Result<SlotInfo> {
    store.write(&prepared.game, schema, prepared.expected)
}
/// Runs the IO/migration half of `save.load` on a worker without mutating a world.
pub fn load(store: &SlotStore, schema: &SaveSchema, params: LoadParams) -> Result<LoadedSlot> {
    store.authority().require_write()?;
    store.load(&params.slot, schema)
}
/// Applies a prepared load only at a host-approved tick boundary. On false, returns
/// ARA7503 without touching the world; the host retains/requeues its prepared load.
pub fn apply_load(
    store: &SlotStore,
    schema: &SaveSchema,
    session: &mut ara_sim::Session,
    loaded: &LoadedSlot,
    allowed: bool,
) -> Result<StateHash> {
    store.authority().require_write()?;
    if !allowed {
        return Err(error(
            codes::AUTHORITY,
            "host apply policy deferred save load",
        ));
    }
    schema.restore(session, &loaded.save.game.state)?;
    let actual = schema
        .capture(session)?
        .state_hash()
        .map_err(|e| error(codes::SCHEMA, e.to_string()))?;
    if actual != loaded.save.game.state_hash {
        return Err(error(
            codes::SCHEMA,
            "restored persistent state hash differs from the save",
        ));
    }
    Ok(actual)
}
/// Runs the read-only `save.list` tool.
pub fn list(store: &SlotStore, schema: &SaveSchema, _: ListParams) -> Result<SlotListing> {
    store.list(schema)
}
/// Runs `save.diff`, retaining backup/migration diagnostics alongside field changes.
pub fn diff_slots(
    store: &SlotStore,
    schema: &SaveSchema,
    params: DiffParams,
) -> Result<DiffOutput> {
    let before = store.load(&params.before, schema)?;
    let after = store.load(&params.after, schema)?;
    let fields = diff(&before.save.game, &after.save.game)?;
    let diagnostics = before
        .diagnostics
        .into_iter()
        .chain(before.save.diagnostics)
        .chain(after.diagnostics)
        .chain(after.save.diagnostics)
        .collect();
    Ok(DiffOutput {
        fields,
        diagnostics,
    })
}
/// Structured result of `save.diff`.
#[derive(Debug, Serialize)]
pub struct DiffOutput {
    /// Deterministically ordered changes in persistent state fields.
    pub fields: Vec<crate::FieldDiff>,
    /// Corruption recovery or schema migration information from either input.
    pub diagnostics: Vec<ara_diag::Diagnostic>,
}
fn parse_hash(text: &str) -> Result<StateHash> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error(
            codes::IO,
            "expected hash must have 64 hexadecimal digits",
        ));
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|e| error(codes::IO, e.to_string()))?;
    }
    Ok(StateHash(bytes))
}
