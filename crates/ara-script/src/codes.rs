//! Diagnostic codes of the C# host: `ARA0900`–`ARA0999`.
//!
//! `ara-diag` reserves `ARA0001`–`ARA0999` for C#: the Roslyn analyzers own
//! `0001`–`0899` (declared in `Ara.Analyzers`), the host owns this range. The list is
//! generated into C# (`AraCodes`) so the runtime and compile server report the same
//! codes. Add new codes at the end of their group; never renumber or reuse one.

use ara_diag::Code;

macro_rules! codes {
    ($($(#[doc = $doc:literal])+ $name:ident = $num:literal,)*) => {
        $($(#[doc = $doc])+ pub const $name: Code = Code($num);)*

        /// Every host code with its constant name and description.
        pub const ALL: &[(Code, &str, &str)] = &[$(($name, stringify!($name), concat!($($doc),+))),*];
    };
}

codes! {
    /// A C# compile error (the Roslyn `CS` id is in the message); the last good code keeps running.
    COMPILE_ERROR = 900,
    /// A C# compile warning.
    COMPILE_WARNING = 901,
    /// A module is past its soft size cap (5k lines); split it to keep L3 reloads fast.
    MODULE_LARGE = 902,
    /// A module is past 8k lines; L3 reloads will miss their latency target.
    MODULE_TOO_LARGE = 903,
    /// The compile server failed or exited; reloads stop until it is restarted.
    COMPILE_SERVER = 904,
    /// The runtime rejected an EnC delta (L1/L2); the edit is rebuilt as a full module (L3).
    DELTA_REJECTED = 905,
    /// A scoped C# compilation attempt started, bound its producer, or completed; application is reported separately.
    COMPILE_RESULT = 906,
    /// A C# system threw; it is disabled until the next reload.
    SYSTEM_FAULT = 910,
    /// A behaviour callback threw; the behaviour type is disabled until the next reload.
    BEHAVIOUR_FAULT = 911,
    /// A command threw; the call is rolled back and the command is disabled until the next reload.
    COMMAND_FAULT = 912,
    /// A module failed to load or register; the last good code keeps running.
    LOAD_FAILED = 920,
    /// The generated C# bindings are stale (ABI version or layout hash differ).
    STALE_BINDINGS = 921,
    /// The .NET runtime or the script runtime could not be started.
    RUNTIME_START = 922,
    /// Modules reference each other in a cycle; none of the modules in it load.
    MODULE_CYCLE = 923,
    /// A module references a module that is not loaded (`references` in `ara.module.toml`).
    UNKNOWN_MODULE = 924,
    /// A module could not be rebuilt against a changed dependency; every module in the
    /// cascade keeps its last good version.
    DEPENDENCY_FAILED = 925,
    /// The change cannot be applied in this process (L4): restart the play process from a snapshot.
    RESTART_REQUIRED = 926,
    /// A retired module generation is still alive after unloading (something pins its ALC).
    GENERATION_LEAKED = 927,
    /// A component type is not blittable, so C# cannot view it in place.
    NOT_BLITTABLE = 930,
    /// A C# struct's layout differs from the Schema DB's canonical layout.
    LAYOUT_MISMATCH = 931,
    /// A system, behaviour or command names a component the host does not know.
    UNKNOWN_COMPONENT = 932,
    /// A module's descriptor is invalid (schema problems, duplicates, bad access sets).
    BAD_DESCRIPTOR = 933,
    /// A component in a C# query uses sparse-set storage; C# systems need table storage.
    SPARSE_COMPONENT = 934,
    /// No command with this name is registered by a C# module.
    UNKNOWN_COMMAND = 950,
    /// Command params do not match the command's params type.
    INVALID_PARAMS = 951,
    /// A command rejected the call (`ctx.Reject`); nothing it wrote is kept.
    COMMAND_REJECTED = 952,
    /// The undo record no longer applies (an entity it touched is gone).
    UNDO_FAILED = 953,
    /// An `ara_run` script exceeded its wall-clock or memory limit and was killed.
    RUN_LIMIT = 960,
    /// An `ara_run` script uses an API outside the agent whitelist.
    RUN_ACCESS = 961,
    /// An `ara_run` script failed to compile or threw.
    RUN_FAILED = 962,
    /// The `ara_run` worker process failed or broke the protocol.
    RUN_WORKER = 963,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::ALL;

    #[test]
    fn codes_are_unique_and_in_the_host_range() {
        let mut seen = BTreeSet::new();
        for (code, name, _) in ALL {
            assert!((900..1000).contains(&code.0), "{name} is outside 0900-0999");
            assert!(seen.insert(code.0), "{name} reuses {code}");
        }
    }
}
