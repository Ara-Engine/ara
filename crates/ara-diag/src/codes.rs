//! Diagnostic code constants for the kernel and for events the bus carries.
//!
//! Kernel ranges: `1000` JSON and files, `1100` schema, `1200` documents, `1300`
//! references, `1400` prefabs, `1500` transactions, `1600` commands, `1700` merge,
//! `1800` Engine Protocol and sessions, `1900` projects, the daemon and persistence.
//! Bus ranges: `2000` the bus, `ara why` and runtime events (reloads, frames),
//! `2100` assets, `2200` world streaming, `2400` networking, `3300` render
//! visibility, `6000` GPU. Other crates own their ranges (see the crate's
//! `AGENTS.md`); every code has a doc entry in [`crate::catalog`]. Add new codes at
//! the end of their range; never renumber or reuse one.

use crate::Code;

macro_rules! codes {
    ($($(#[doc = $doc:literal])+ $name:ident = $num:literal,)*) => {
        $($(#[doc = $doc])+ pub const $name: Code = Code($num);)*

        /// Every code with its constant name and description, for docs and `ara explain`.
        pub const ALL: &[(Code, &str, &str)] = &[$(($name, stringify!($name), concat!($($doc),+))),*];
    };
}

codes! {
    /// The file is not valid JSON.
    JSON_SYNTAX = 1001,
    /// An object has the same key twice.
    DUPLICATE_KEY = 1002,
    /// The file could not be read or written.
    IO = 1003,

    /// A key is not a field of its type.
    UNKNOWN_FIELD = 1101,
    /// A value has the wrong JSON type for its field.
    TYPE_MISMATCH = 1102,
    /// A number is outside the field's range or its integer type.
    OUT_OF_RANGE = 1103,
    /// A string is not one of the enum's variants.
    UNKNOWN_VARIANT = 1104,
    /// A required field is missing.
    MISSING_REQUIRED = 1105,
    /// A type name is not registered in the Schema DB.
    UNKNOWN_TYPE = 1106,
    /// A deprecated field or type is used.
    DEPRECATED = 1107,
    /// An old name (alias) was used and has been renamed.
    RENAMED = 1108,
    /// A type definition is invalid (missing doc, bad name, bad default, ...).
    INVALID_TYPE_DEF = 1109,
    /// Two different types were registered under one name.
    DUPLICATE_TYPE = 1110,
    /// A field path is malformed or does not exist in the schema.
    INVALID_PATH = 1111,
    /// A type upgrade hook failed.
    UPGRADE_FAILED = 1112,
    /// The data was written by a newer version of a type or document kind.
    NEWER_VERSION = 1113,
    /// A keyed collection item has a missing or duplicate `id`.
    INVALID_KEYED_ITEM = 1114,
    /// A type was upgraded from an older version.
    UPGRADED = 1115,

    /// `$schema` is missing or names an unknown document kind.
    UNKNOWN_SCHEMA = 1201,
    /// A record or asset ID is malformed or has the wrong prefix.
    INVALID_ID = 1202,
    /// A record with this ID already exists.
    RECORD_EXISTS = 1203,
    /// No record with this ID exists.
    RECORD_NOT_FOUND = 1204,
    /// A record's `parent` does not exist.
    MISSING_PARENT = 1205,
    /// Following `parent` links loops back to the record.
    PARENT_CYCLE = 1206,
    /// An `order` key is not a valid fractional index.
    INVALID_ORDER = 1207,
    /// A prefab document must have exactly one root record.
    ROOT_COUNT = 1208,
    /// The document still contains unresolved merge conflicts.
    UNRESOLVED_CONFLICT = 1209,
    /// A record key names a type that is not a component.
    NOT_A_COMPONENT = 1210,
    /// A prefab instance record carries inline components; use overrides.
    INSTANCE_COMPONENTS = 1211,
    /// An entity reference points at a record that does not exist.
    DANGLING_ENTITY_REF = 1212,
    /// The document has no asset `id`.
    MISSING_DOC_ID = 1213,

    /// A reference is not of the form `@path#id`.
    INVALID_REF = 1301,
    /// Neither the path nor the ID of a reference resolves.
    UNRESOLVED_REF = 1302,
    /// A reference's path was stale and has been rewritten from its ID.
    REF_PATH_REPAIRED = 1303,
    /// A reference was missing its ID or path, which has been filled in.
    REF_FILLED = 1304,
    /// A reference points at the wrong kind of asset.
    REF_KIND_MISMATCH = 1305,

    /// The prefab of an instance cannot be found.
    PREFAB_NOT_FOUND = 1401,
    /// Prefabs instantiate each other in a loop.
    PREFAB_CYCLE = 1402,
    /// An override targets an entity or component that does not exist.
    OVERRIDE_TARGET_MISSING = 1403,
    /// An override key is malformed.
    INVALID_OVERRIDE_KEY = 1404,
    /// A `removed` entry targets something that does not exist.
    REMOVED_TARGET_MISSING = 1405,
    /// The entity is not a prefab instance (or inside one).
    NOT_AN_INSTANCE = 1406,

    /// The transaction's base revision is stale and another edit touched the same path.
    STALE_BASE = 1501,
    /// Undo would overwrite a later edit to the same path.
    UNDO_CONFLICT = 1502,
    /// The transaction was already reverted.
    ALREADY_REVERTED = 1503,
    /// No transaction with this ID exists.
    TXN_NOT_FOUND = 1504,
    /// There is nothing to undo or redo.
    NOTHING_TO_UNDO = 1505,
    /// No document with this ID or path is loaded.
    DOC_NOT_FOUND = 1506,
    /// An author string is not `human` or `agent:<name>`.
    INVALID_AUTHOR = 1507,

    /// No command with this name is registered.
    UNKNOWN_COMMAND = 1601,
    /// Command parameters do not match the command's params type.
    INVALID_PARAMS = 1602,
    /// The session lacks the capability scope the command requires.
    MISSING_SCOPE = 1603,
    /// A symbolic ID (`$c1`) is used before a command defines it.
    UNRESOLVED_SYMBOL = 1604,
    /// A symbolic ID is defined twice in one batch.
    SYMBOL_REDEFINED = 1605,
    /// The entity already has this component.
    COMPONENT_EXISTS = 1606,
    /// The entity does not have this component.
    COMPONENT_MISSING = 1607,
    /// The registry entry has a different kind (command, query or job) than requested.
    WRONG_KIND = 1608,
    /// An entity selector (`name:Crate`) matches more than one entity; use an ID or a
    /// longer name path (`name:Floor/Crate`).
    AMBIGUOUS_ENTITY = 1609,
    /// An agent's change has no `why`; agent transactions say why they were made, for
    /// the human who reviews them.
    MISSING_WHY = 1610,

    /// Both sides of a merge changed the same field differently.
    MERGE_CONFLICT = 1701,
    /// The three merge inputs are not versions of the same document.
    MERGE_ID_MISMATCH = 1702,

    /// A message is not valid JSON-RPC 2.0, or its params are not an object.
    INVALID_REQUEST = 1801,
    /// No Engine Protocol method with this name is registered.
    UNKNOWN_METHOD = 1802,
    /// The session handle is unknown or the session has ended; begin a new one.
    SESSION_NOT_FOUND = 1803,
    /// A scope name is not part of the scope vocabulary.
    UNKNOWN_SCOPE = 1804,
    /// The connection-approval policy refused some requested scopes.
    SCOPE_DENIED = 1805,
    /// No service providing this method is registered yet.
    SERVICE_UNAVAILABLE = 1806,
    /// The connection did not present the project token.
    UNAUTHORIZED = 1807,
    /// A query string or query object is malformed.
    INVALID_QUERY = 1808,
    /// A pagination cursor is malformed or from another query.
    INVALID_CURSOR = 1809,
    /// No checkpoint with this ID exists in the session.
    CHECKPOINT_NOT_FOUND = 1810,
    /// No job with this ID exists.
    JOB_NOT_FOUND = 1811,
    /// A session was approved; records the granted scopes and who decided.
    SESSION_APPROVED = 1812,
    /// The transaction or entity has nothing left to review.
    NOT_REVIEWABLE = 1813,
    /// The call names no document and none can be inferred from its entities or the
    /// project's main scene; pass `doc`.
    DOC_REQUIRED = 1814,
    /// A CLI preparation job stopped reporting progress within its configured budget.
    JOB_STALLED = 1815,

    /// No `ara.project.json` was found; the command needs a project.
    NO_PROJECT = 1901,
    /// `ara init` found an existing project.
    PROJECT_EXISTS = 1902,
    /// Another daemon already serves this project.
    DAEMON_RUNNING = 1903,
    /// The daemon in `.ara/daemon.json` did not answer.
    DAEMON_UNREACHABLE = 1904,
    /// A journal entry could not be read and was skipped.
    JOURNAL_CORRUPT = 1905,
    /// Transactions from the journal were recovered after an unclean shutdown.
    JOURNAL_REPLAYED = 1906,
    /// A document changed on disk and the change was merged in.
    EXTERNAL_EDIT = 1907,
    /// A document changed on disk in a way that conflicts with unsaved changes.
    EXTERNAL_CONFLICT = 1908,
    /// A document changed on disk but no longer reads cleanly; it was not loaded.
    EXTERNAL_INVALID = 1909,
    /// A loaded document was deleted on disk.
    DOC_DELETED = 1910,
    /// A document could not be written to disk.
    FLUSH_FAILED = 1911,
    /// Project types (`*.types.json`) changed on disk; restart the daemon to load them.
    TYPES_CHANGED = 1912,
    /// A Hub project template is missing pinned files (sample art not fetched yet).
    TEMPLATE_INCOMPLETE = 1913,
    /// An old project must be explicitly migrated before any authoring/runtime open.
    PROJECT_MIGRATION_REQUIRED = 1914,

    /// Diagnostics were dropped because an emitting thread's bus ring was full.
    DIAG_DROPPED = 2001,
    /// A diagnostic reported by another process (`diag.report`) could not be read.
    DIAG_INVALID_REPORT = 2002,
    /// No explainer understands an `ara why` question.
    WHY_UNANSWERED = 2010,
    /// No diagnostic code has this name.
    UNKNOWN_CODE = 2011,
    /// A C# module was reloaded (L1, L2 or L3), with its timing.
    RELOAD_APPLIED = 2020,
    /// A C# edit that could have been an in-place reload (L1/L2) took a full module
    /// swap (L3); the reason is in the message.
    RELOAD_ESCALATED = 2021,
    /// A frame took longer than its budget.
    FRAME_OVER_BUDGET = 2030,

    /// An asset import failed; the last good version stays loaded.
    ASSET_IMPORT_FAILED = 2101,
    /// An asset's source file is missing: its `.import.json` sidecar exists, the
    /// source media does not.
    ASSET_SOURCE_MISSING = 2102,
    /// An import worker process failed or broke the worker protocol.
    ASSET_WORKER_FAILED = 2103,
    /// An import was cancelled or superseded before it was published.
    ASSET_IMPORT_CANCELLED = 2104,

    /// World data, a cooked cell or the streaming configuration is invalid.
    WORLD_INVALID = 2201,
    /// A world cell failed to load; it stays unloaded until it is retried.
    WORLD_CELL_FAILED = 2202,
    /// An entity's world cell is not resident (streamed out), so it is not simulated
    /// or drawn.
    WORLD_NOT_RESIDENT = 2203,

    /// A network configuration, packet or session operation was rejected.
    NET_REJECTED = 2401,

    /// The entity has nothing to draw: no renderer component.
    RENDER_NO_RENDERER = 3301,
    /// The entity, an ancestor or its renderer is disabled or hidden.
    RENDER_DISABLED = 3302,
    /// The entity's world scale is zero on some axis (its own or an ancestor's), so
    /// its mesh has no area.
    RENDER_DEGENERATE_TRANSFORM = 3303,
    /// A transform value of the entity or an ancestor is not a finite number.
    RENDER_NON_FINITE_TRANSFORM = 3304,
    /// The renderer names no mesh, or its mesh asset does not resolve or has no
    /// source.
    RENDER_NO_MESH = 3305,
    /// The renderer's material does not resolve or does not compile.
    RENDER_MATERIAL = 3306,
    /// The material's opacity or the renderer's tint alpha is zero.
    RENDER_TRANSPARENT = 3307,
    /// No camera's culling mask includes the entity's render layer.
    RENDER_LAYER_CULLED = 3308,
    /// The entity is beyond the camera's far plane or its own draw distance.
    RENDER_DISTANCE_CULLED = 3309,
    /// The scene has no enabled camera.
    RENDER_NO_CAMERA = 3310,
    /// The renderer left the entity out of every pass (reported by the renderer).
    RENDER_PASS_EXCLUDED = 3311,
    /// The entity is behind every camera.
    RENDER_OUT_OF_VIEW = 3312,

    /// A GPU handle was used after its resource was destroyed (`ARA_GPU_STALE_HANDLE`).
    GPU_STALE_HANDLE = 6001,
    /// The graphics backend (Vulkan or Metal) reported an error (`ARA_GPU_BACKEND`).
    GPU_BACKEND = 6002,
    /// A GPU API contract was broken: a bad descriptor, barrier or size
    /// (`ARA_GPU_CONTRACT`).
    GPU_CONTRACT = 6003,
    /// The Vulkan validation layers reported a problem (`ARA_GPU_VALIDATION`).
    GPU_VALIDATION = 6004,
    /// A message from the GPU driver or loader environment (`ARA_GPU_ENVIRONMENT`).
    GPU_ENVIRONMENT = 6005,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::ALL;

    #[test]
    fn codes_are_unique_and_outside_the_analyzer_range() {
        let mut seen = BTreeSet::new();
        for (code, name, _) in ALL {
            assert!(code.0 >= 1000, "{name} is in the C# analyzer range");
            assert!(seen.insert(code.0), "{name} reuses {code}");
        }
    }
}
