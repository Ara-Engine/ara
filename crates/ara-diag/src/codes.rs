//! The catalog of kernel diagnostic codes.
//!
//! Ranges: `1000` JSON and files, `1100` schema, `1200` documents, `1300`
//! references, `1400` prefabs, `1500` transactions, `1600` commands, `1700` merge,
//! `1800` Engine Protocol and sessions, `1900` projects, the daemon and persistence.
//! `ARA0001`–`ARA0999` are reserved for the C# analyzers. Add new codes at the end
//! of their range; never renumber or reuse one.

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
