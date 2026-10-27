//! The Rust/C# ABI (decision A13): one entry point, two function-pointer tables and
//! the structs they pass, declared once here.
//!
//! `ara_script_init(*const HostInit, *mut ScriptApi) -> i32` is the only symbol Rust
//! resolves (through hostfxr in development, as a NativeAOT export when shipping).
//! [`HostApi`] is what Rust offers C#; [`ScriptApi`] is what C# fills in. Every C#
//! entry point catches everything and returns a status. Rust never holds pointers
//! into module code: systems, behaviours and commands are ids into C# tables that
//! are swapped only at tick boundaries.
//!
//! [`layout_hash`] covers every descriptor here plus the bound engine components, so
//! stale generated C# refuses to start.

#[macro_use]
mod describe;

use core::ffi::c_void;

pub(crate) use describe::Fnv;
pub use describe::{AbiConst, AbiField, AbiFn, AbiStruct, AbiTable};

/// Bumped on any incompatible change to the semantics of the tables.
pub const ABI_VERSION: u32 = 3;

abi_structs! { PUBLIC_STRUCTS;
    /// The fixed simulation tick that every system, behaviour and command sees.
    pub struct Tick {
        /// Tick number since the session started.
        pub frame: u64,
        /// Session seed. Deterministic RNG streams derive from it; never wall-clock.
        pub seed: u64,
        /// Simulation time in seconds (`frame * dt`).
        pub elapsed: f64,
        /// Fixed step in seconds.
        pub dt: f32,
        /// Reserved; zero.
        pub reserved: u32,
    }
    /// One presented frame (variable rate), passed to `Behaviour.OnFrame`.
    pub struct Frame {
        /// Frame number since the session started.
        pub index: u64,
        /// Seconds since the previous frame (presentation only, never simulation).
        pub dt: f32,
        /// Interpolation factor between the last two ticks, in `[0, 1]`.
        pub alpha: f32,
    }
}

abi_structs! { ABI_STRUCTS;
    /// One contiguous run of rows of one ECS table, handed to one C# system.
    pub struct SystemCall {
        /// System id (`module << 16 | index`).
        pub system: u32,
        /// Rows in this chunk.
        pub len: u32,
        /// `len` entity handles (`Entity::to_bits`).
        pub entities: *const u64,
        /// Column base pointers, in the system's declared column order.
        pub columns: *const *mut u8,
        /// Resource pointers, in the system's declared resource order.
        pub resources: *const *mut u8,
        /// The current tick.
        pub tick: *const Tick,
        /// Command buffer for structural changes, applied at the next sync point.
        pub commands: *mut c_void,
    }
    /// One chunk of entities carrying one behaviour type, for one lifecycle phase.
    pub struct BehaviourCall {
        /// Behaviour id (`module << 16 | index`).
        pub behaviour: u32,
        /// `PHASE_TICK` or `PHASE_FRAME`.
        pub phase: u32,
        /// Rows in this chunk.
        pub len: u32,
        /// Reserved; zero.
        pub reserved: u32,
        /// `len` entity handles.
        pub entities: *const u64,
        /// Base of the behaviour's data column (`len` rows).
        pub data: *mut u8,
        /// The current tick.
        pub tick: *const Tick,
        /// The current frame (`PHASE_FRAME` only; otherwise null).
        pub frame: *const Frame,
        /// Command buffer for structural changes, applied after the phase.
        pub commands: *mut c_void,
    }
    /// One gameplay command invocation.
    pub struct CommandCall {
        /// Command id (`module << 16 | index`).
        pub command: u32,
        /// Bytes of `reason` written on rejection.
        pub reason_len: u32,
        /// The params struct, in the command's params layout.
        pub params: *const u8,
        /// The current tick.
        pub tick: *const Tick,
        /// Session-wide command sequence number; seeds `ctx.Rng`.
        pub sequence: u64,
        /// The calling entity (`Entity::to_bits`), or the null entity for tools and agents.
        pub caller: u64,
        /// Command buffer for structural changes, applied when the command succeeds.
        pub commands: *mut c_void,
        /// UTF-8 reject reason (`cannot_heal`), written on rejection.
        pub reason: [u8; 64],
    }
    /// Timing and outcome of one reload, reported by C# when it is applied.
    pub struct ReloadReport {
        /// Module slot.
        pub module: u32,
        /// `RELOAD_*`.
        pub kind: u32,
        /// Why an L1 candidate took L3 (`ESC_*`), or 0.
        pub escalation: u32,
        /// ALC generation after this reload (L3 swaps so far).
        pub generation: u32,
        /// EnC deltas applied to the current generation.
        pub delta_count: u32,
        /// Members updated by an L1 delta.
        pub changed_members: u32,
        /// Compiler errors (for `RELOAD_ERROR`).
        pub error_count: u32,
        /// 1 if the module is hot (Debug IL, EnC on).
        pub hot: u32,
        /// Who triggered it: `ORIGIN_HUMAN` (file watcher, editor) or `ORIGIN_AGENT` (`request_compile`).
        pub origin: u32,
        /// Modules swapped together in this reload (a dependency cascade); 1 when alone.
        pub cascade: u32,
        /// Phase: file event to the compile starting (watcher settle, queueing), in µs.
        pub detect_us: u32,
        /// Phase: reading, parsing and classifying the changed files, in µs.
        pub parse_us: u32,
        /// Phase: the Ara source generator, in µs (0 when L1/L2 skipped it).
        pub generate_us: u32,
        /// Phase: Roslyn emit (full image or EnC delta), in µs.
        pub emit_us: u32,
        /// Phase: analyzers after the emit (the API allowlist and Ara rules), in µs.
        pub analyze_us: u32,
        /// Phase: compile server message sent to received by the host, in µs.
        pub transfer_us: u32,
        /// Phase: loading the image into a collectible ALC (includes the Windows AMSI scan), in µs.
        pub load_us: u32,
        /// Phase: pre-JIT of generated entry points, in µs.
        pub prejit_us: u32,
        /// Phase: the generated `Register` (declarations collected while staging), in µs.
        pub register_us: u32,
        /// Phase: at the tick boundary, host registration, column migration and the table swap, in µs.
        pub apply_us: u32,
        /// Compile-server time from file event to message sent, in microseconds.
        pub server_us: u64,
        /// Host clock (ns) when the message arrived.
        pub t_received: u64,
        /// Host clock when the update was staged (ALC loaded and registered, or delta ready).
        pub t_staged: u64,
        /// Host clock when it was applied at a tick boundary.
        pub t_applied: u64,
        /// Bytes received (PE + PDB for L3; metadata + IL + PDB for L1).
        pub bytes: u64,
    }
}

abi_consts! { ABI_CONSTS;
    /// `HostInit::flags`: the runtime was started with EnC enabled (`DOTNET_MODIFIABLE_ASSEMBLIES=debug`).
    pub const INIT_ENC: u32 = 1;
    /// `ReloadReport::kind`: the first load of a module.
    pub const RELOAD_INITIAL: u32 = 0;
    /// `ReloadReport::kind`: an EnC body-only delta.
    pub const RELOAD_L1: u32 = 1;
    /// `ReloadReport::kind`: an additive EnC delta (new methods, fields on classes, new types, lambdas in new members).
    pub const RELOAD_L2: u32 = 2;
    /// `ReloadReport::kind`: a module swap into a new collectible ALC.
    pub const RELOAD_L3: u32 = 3;
    /// `ReloadReport::kind`: the change cannot be applied in this process (engine bindings changed, a
    /// generation leaked); the last good code keeps running until the play process restarts from a snapshot.
    pub const RELOAD_L4: u32 = 4;
    /// `ReloadReport::kind`: the edit did not compile; the last good code keeps running.
    pub const RELOAD_ERROR: u32 = 100;
    /// Escalation: none (the edit took L1).
    pub const ESC_NONE: u32 = 0;
    /// Escalation: a file was removed or renamed.
    pub const ESC_FILE_SET: u32 = 1;
    /// Escalation: a declaration changed or was removed (signatures, struct fields, attributes, virtual members).
    pub const ESC_STRUCTURAL: u32 = 2;
    /// Escalation: a changed body is an iterator or async state machine.
    pub const ESC_STATE_MACHINE: u32 = 3;
    /// Escalation: Roslyn could not emit a delta.
    pub const ESC_EMIT_FAILED: u32 = 4;
    /// Escalation: the module is cold (Release IL); its first edit makes it hot.
    pub const ESC_COLD: u32 = 5;
    /// Escalation: a debugger is attached; the runtime refuses `ApplyUpdate` then.
    pub const ESC_DEBUGGER: u32 = 6;
    /// Escalation: a changed body contains a lambda, local function or query (S2 finding 1).
    pub const ESC_CLOSURE: u32 = 7;
    /// Escalation: a constructor or initializer changed.
    pub const ESC_CONSTRUCTOR: u32 = 8;
    /// Escalation: generated code changed (attributes, partial properties, registration).
    pub const ESC_GENERATED: u32 = 9;
    /// Escalation: the host asked for a full image (load failure, heat change).
    pub const ESC_REQUESTED: u32 = 10;
    /// Escalation: the runtime rejected an EnC delta (`ApplyUpdate` threw); the edit was rebuilt as L3.
    pub const ESC_RUNTIME_REJECTED: u32 = 11;
    /// Escalation: the runtime lacks the EnC capability the additive edit needs.
    pub const ESC_CAPABILITY: u32 = 12;
    /// Escalation: a referenced module changed its public surface, so this module was rebuilt against it.
    pub const ESC_DEPENDENCY: u32 = 13;
    /// Escalation: a referenced module got a new generation; this module's image was reloaded against it unchanged.
    pub const ESC_RELINK: u32 = 14;
    /// `ReloadReport::origin`: a human edit (file watcher, editor).
    pub const ORIGIN_HUMAN: u32 = 0;
    /// `ReloadReport::origin`: an agent edit (`request_compile` with the agent origin); coalesced and run behind human work.
    pub const ORIGIN_AGENT: u32 = 1;
    /// `tick_boundary` bit: code changed.
    pub const BOUNDARY_RELOADED: u32 = 1;
    /// `tick_boundary` bit: systems, behaviours, commands or components changed; rebuild the schedule.
    pub const BOUNDARY_SCHEMA_CHANGED: u32 = 2;
    /// `BehaviourCall::phase`: the fixed tick (`OnSpawn` then `OnTick`).
    pub const PHASE_TICK: u32 = 1;
    /// `BehaviourCall::phase`: a presented frame (`OnFrame`).
    pub const PHASE_FRAME: u32 = 2;
    /// Command-buffer op: make the reserved entity real.
    pub const CMD_SPAWN: u32 = 1;
    /// Command-buffer op: despawn the entity.
    pub const CMD_DESPAWN: u32 = 2;
    /// Command-buffer op: insert (or replace) a component with the given bytes.
    pub const CMD_INSERT: u32 = 3;
    /// Command-buffer op: remove a component.
    pub const CMD_REMOVE: u32 = 4;
    /// Command-buffer op: emit a presentation cue (type index + bytes).
    pub const CMD_CUE: u32 = 5;
    /// Status: ok.
    pub const RUN_OK: i32 = 0;
    /// Status: the code threw; it is disabled until the next reload.
    pub const RUN_FAULTED: i32 = 1;
    /// Status: unknown id.
    pub const RUN_BAD_ID: i32 = 2;
    /// Status: a command rejected the call (`ctx.Reject`); nothing it wrote is kept.
    pub const RUN_REJECTED: i32 = 3;
    /// Status: the entry point is disabled (faulted earlier).
    pub const RUN_DISABLED: i32 = 4;
    /// `diag` key: distinct threads that ran C# systems.
    pub const DIAG_THREADS_SEEN: u32 = 1;
    /// `diag` key: retired ALCs still alive.
    pub const DIAG_ALCS_ALIVE: u32 = 2;
    /// `diag` key: ALCs created.
    pub const DIAG_ALCS_CREATED: u32 = 3;
    /// `diag` key: faults reported.
    pub const DIAG_FAULTS: u32 = 4;
    /// `diag` key: managed heap bytes.
    pub const DIAG_MANAGED_HEAP: u32 = 5;
    /// `diag` key: total GC pause in microseconds.
    pub const DIAG_GC_PAUSE_US: u32 = 6;
    /// `diag` key: 1 if this runtime can reload (JIT with a module loader), 0 for NativeAOT.
    pub const DIAG_CAN_RELOAD: u32 = 7;
}

abi_table! {
    /// Functions the Rust host offers C#. Each takes the opaque `ctx` from `HostInit`.
    pub struct HostApi as HOST_API {
        /// Logs a UTF-8 message. Level: 0 info, 1 warning, 2 error.
        fn log(ctx: *mut c_void, level: u32, msg: *const u8, len: u32);
        /// Monotonic host clock in nanoseconds (diagnostics only; never simulation).
        fn now_ns(ctx: *mut c_void) -> u64;
        /// Reports a structured diagnostic (UTF-8 JSON in the `ara-diag` shape).
        fn diagnostic(ctx: *mut c_void, json: *const u8, len: u32);
        /// Reports an applied reload (called on the tick thread).
        fn report_reload(ctx: *mut c_void, report: *const ReloadReport);
        /// Registers module descriptors (a UTF-8 JSON array, in dependency order) all or nothing,
        /// and writes the response JSON to `out`. Returns the bytes written, or `-needed` when `cap`
        /// is too small. Only valid during `load_modules` and `tick_boundary`.
        fn register_modules(ctx: *mut c_void, json: *const u8, len: u32, out: *mut u8, cap: u32) -> i32;
        /// Pointer to `entity`'s component (script index `component`), or null. Valid in
        /// exclusive phases (behaviours, commands) until the phase ends. `write` journals it.
        fn component_ptr(ctx: *mut c_void, entity: u64, component: u32, write: u32) -> *mut u8;
        /// Pointer to a resource (script index), or null. Same validity as `component_ptr`.
        fn resource_ptr(ctx: *mut c_void, resource: u32, write: u32) -> *mut u8;
        /// 1 if `entity` exists. Same validity as `component_ptr`.
        fn entity_alive(ctx: *mut c_void, entity: u64) -> u32;
        /// Reserves an entity handle for command buffer `buf` (any thread); `CMD_SPAWN` makes it real at the sync point.
        fn reserve_entity(ctx: *mut c_void, buf: *mut c_void) -> u64;
        /// Appends one structural op to a command buffer.
        fn cmd_push(ctx: *mut c_void, buf: *mut c_void, op: u32, type_index: u32, entity: u64, data: *const u8, len: u32);
        /// The compile server started (1) or finished (0) a compile; the host raises tick-thread priority meanwhile.
        fn compile_state(ctx: *mut c_void, active: u32);
        /// Copies entity handles carrying a component in ascending handle order. Returns
        /// the required count; writes nothing when cap is too small. Exclusive phases only.
        fn query_entities(ctx: *mut c_void, component: u32, out: *mut u64, cap: u32) -> u32;
        /// Reads a fixed-tick action axis (button = axis 0). Missing actions are zero.
        /// Exclusive phases only; the embedding runtime publishes the tick's input snapshot.
        fn input_axis(ctx: *mut c_void, player: u64, name: *const u8, len: u32, axis: u32) -> f32;
        /// Authored input owner for an entity, or zero. Exclusive gameplay phases only.
        fn input_player(ctx: *mut c_void, entity: u64) -> u64;
        /// Queues a navmesh destination after the exclusive gameplay phase; enabled=0 stops.
        fn navigate(ctx: *mut c_void, entity: u64, x: f32, y: f32, z: f32, enabled: u32);
        /// Suppresses locomotion during this fixed tick (downed actors or completed runs).
        fn character_stop(ctx: *mut c_void, entity: u64);
        /// Queues a named boolean animator parameter after exclusive gameplay callbacks.
        fn animation_bool(ctx: *mut c_void, entity: u64, name: *const u8, len: u32, value: u32);
    }
}

abi_table! {
    /// Functions the C# runtime fills in for the host. Exceptions never cross them.
    pub struct ScriptApi as SCRIPT_API {
        /// Loads modules described by UTF-8 JSON (dev: starts the compile server; AOT: the linked modules).
        fn load_modules(json: *const u8, len: u32) -> i32;
        /// Applies staged reloads at a tick boundary. Returns `BOUNDARY_*` bits.
        fn tick_boundary(tick: *const Tick) -> u32;
        /// Non-zero if a staged reload is waiting for the next boundary.
        fn has_pending() -> u32;
        /// Runs one system over one chunk. Returns `RUN_*`.
        fn run_system(call: *const SystemCall) -> i32;
        /// Runs one behaviour type over one chunk for one phase. Returns `RUN_*`.
        fn run_behaviours(call: *const BehaviourCall) -> i32;
        /// Runs one command. Returns `RUN_*`.
        fn run_command(call: *mut CommandCall) -> i32;
        /// Recompiles a module hot (1: Debug IL, EnC) or cold (0: optimized).
        fn set_heat(module: u32, hot: u32) -> i32;
        /// Asks for a compile of changed files (UTF-8 JSON `{"module", "paths", "origin"}`). Agent edits
        /// are coalesced and run behind human ones. Never blocks: the result arrives as a staged reload.
        fn request_compile(json: *const u8, len: u32) -> i32;
        /// Diagnostics counters (`DIAG_*`).
        fn diag(key: u32) -> u64;
        /// Stops the compile client, unloads every module and returns the leaked-ALC count.
        fn shutdown() -> i32;
    }
}

abi_structs! { INIT_STRUCTS;
    /// Passed to `ara_script_init`: ABI identity, the host context and the host table.
    pub struct HostInit {
        /// `ABI_VERSION`; C# refuses to start on mismatch.
        pub abi_version: u32,
        /// `INIT_*` bits.
        pub flags: u32,
        /// [`layout_hash`] of the host; C# refuses to start against stale generated code.
        pub layout_hash: u64,
        /// Opaque host context passed back to every `HostApi` function.
        pub ctx: *mut c_void,
        /// The host function table.
        pub api: HostApi,
    }
}

/// Every ABI struct, in generation order.
pub fn structs() -> impl Iterator<Item = &'static AbiStruct> {
    PUBLIC_STRUCTS.iter().chain(ABI_STRUCTS).chain(INIT_STRUCTS)
}

/// Hash of every ABI descriptor plus the engine component layouts (name, size, field
/// names, types and offsets, in binding order). Generated C# embeds the same value.
pub fn layout_hash(engine: &[ara_schema::StructLayout]) -> u64 {
    let mut h = Fnv::new();
    h.eat(&ABI_VERSION.to_string());
    for s in structs() {
        h.eat(s.name);
        h.eat(&s.size.to_string());
        h.eat(&s.align.to_string());
        for f in s.fields {
            h.eat(f.name);
            h.eat(f.ty);
            h.eat(&f.offset.to_string());
        }
    }
    for t in [&HOST_API, &SCRIPT_API] {
        h.eat(t.name);
        for f in t.fns {
            h.eat(f.name);
            for (p, ty) in f.params {
                h.eat(p);
                h.eat(ty);
            }
            h.eat(f.ret);
        }
    }
    for c in ABI_CONSTS {
        h.eat(c.name);
        h.eat(&c.value.to_string());
    }
    for c in engine {
        h.eat(&c.name);
        h.eat(&c.size.to_string());
        for f in &c.fields {
            h.eat(&f.name);
            h.eat(&f.ty.to_string());
            h.eat(&f.offset.to_string());
        }
    }
    h.finish()
}
