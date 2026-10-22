//! Rust-side source of truth for everything that crosses into C#.
//!
//! Each macro below declares real `#[repr(C)]` Rust types *and* a descriptor of their layout
//! (names, field types, `offset_of!` offsets, docs). `gen.rs` turns the descriptors into C#, so the
//! two sides cannot drift: a layout change changes [`layout_hash`], and the C# runtime refuses to
//! start against stale bindings.

use bevy_ecs::component::Component;
use core::ffi::c_void;

/// One field of a described struct.
pub struct FieldDesc {
    pub name: &'static str,
    /// The Rust type as written (`stringify!`); `gen.rs` maps it to C#.
    pub ty: &'static str,
    pub offset: usize,
    pub doc: &'static str,
}

/// A `#[repr(C)]` struct shared with C#.
pub struct StructDesc {
    pub name: &'static str,
    pub doc: &'static str,
    pub size: usize,
    pub align: usize,
    pub fields: &'static [FieldDesc],
}

/// One function pointer in an ABI table.
pub struct FnDesc {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [(&'static str, &'static str)],
    /// Return type as written, or `""` for none.
    pub ret: &'static str,
}

/// A `#[repr(C)]` table of `extern "C"` function pointers.
pub struct TableDesc {
    pub name: &'static str,
    pub doc: &'static str,
    pub fns: &'static [FnDesc],
}

/// One column of a query shape.
pub struct ColumnDesc {
    /// Field name on the generated C# chunk view.
    pub name: &'static str,
    pub component: &'static str,
    pub write: bool,
}

/// A query shape: the fixed column set a C# system iterates per chunk.
pub struct ShapeDesc {
    pub name: &'static str,
    pub doc: &'static str,
    pub columns: &'static [ColumnDesc],
}

/// Declares `#[repr(C)]` structs plus a `StructDesc` list named `$list`.
macro_rules! described_structs {
    ($list:ident $derives:tt;
     $( $(#[doc = $doc:literal])* pub struct $name:ident { $( $(#[doc = $fdoc:literal])* pub $field:ident : $fty:ty ),* $(,)? } )*) => {
        $(
            $(#[doc = $doc])*
            #[repr(C)]
            #[derive $derives]
            pub struct $name { $( $(#[doc = $fdoc])* pub $field: $fty ),* }
        )*
        pub const $list: &[StructDesc] = &[ $( StructDesc {
            name: stringify!($name),
            doc: concat!($($doc, "\n"),*),
            size: size_of::<$name>(),
            align: align_of::<$name>(),
            fields: &[ $( FieldDesc {
                name: stringify!($field),
                ty: stringify!($fty),
                offset: core::mem::offset_of!($name, $field),
                doc: concat!($($fdoc, "\n"),*),
            } ),* ],
        } ),* ];
    };
}

/// Declares a `#[repr(C)]` function-pointer table plus its `TableDesc` named `$desc`.
macro_rules! fn_table {
    ($(#[doc = $doc:literal])* pub struct $name:ident as $desc:ident {
        $( $(#[doc = $fdoc:literal])* fn $f:ident ( $( $p:ident : $pty:ty ),* ) $(-> $ret:ty)? ; )*
    }) => {
        $(#[doc = $doc])*
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct $name {
            $( $(#[doc = $fdoc])* pub $f: unsafe extern "C" fn($($pty),*) $(-> $ret)?, )*
        }
        pub const $desc: TableDesc = TableDesc {
            name: stringify!($name),
            doc: concat!($($doc, "\n"),*),
            fns: &[ $( FnDesc {
                name: stringify!($f),
                doc: concat!($($fdoc, "\n"),*),
                params: &[ $( (stringify!($p), stringify!($pty)) ),* ],
                ret: { #[allow(unused_variables)] let r = ""; $( let r = stringify!($ret); )? r },
            } ),* ],
        };
    };
}

// ---------------------------------------------------------------------------------------------
// Math value types (plain data; map to System.Numerics on the C# side).

described_structs! { VALUE_TYPES (Clone, Copy, Debug, Default, PartialEq);
    /// A 3D vector. Maps to `System.Numerics.Vector3`.
    pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }
    /// A rotation quaternion (x, y, z, w). Maps to `System.Numerics.Quaternion`.
    pub struct Quat { pub x: f32, pub y: f32, pub z: f32, pub w: f32 }
}

// ---------------------------------------------------------------------------------------------
// ECS components (stored in bevy_ecs table columns, viewed by C# through spans).

described_structs! { COMPONENTS (Component, Clone, Copy, Debug, Default, PartialEq);
    /// World-space placement of an entity.
    pub struct Transform {
        /// Position in metres.
        pub position: Vec3,
        /// Orientation as a unit quaternion.
        pub rotation: Quat,
        /// Uniform scale factor.
        pub scale: f32,
    }
    /// Linear and angular velocity.
    pub struct Velocity {
        /// Metres per second.
        pub linear: Vec3,
        /// Radians per second around each axis.
        pub angular: Vec3,
    }
    /// Hit points with passive regeneration.
    pub struct Health {
        /// Current hit points.
        pub value: f32,
        /// Maximum hit points.
        pub max: f32,
        /// Hit points regained per second.
        pub regen: f32,
    }
}

// ---------------------------------------------------------------------------------------------
// ABI structs passed across the boundary.

/// Maximum number of component columns in one chunk.
pub const MAX_COLUMNS: usize = 4;

described_structs! { ABI_STRUCTS (Clone, Copy, Debug);
    /// One archetype chunk: a contiguous run of rows in one bevy_ecs table.
    pub struct Chunk {
        /// Number of rows (entities) in this chunk.
        pub len: u32,
        /// Number of valid column pointers.
        pub column_count: u32,
        /// `len` entity handles (`Entity::to_bits`).
        pub entities: *const u64,
        /// Column 0 base pointer (shape order).
        pub col0: *mut u8,
        /// Column 1 base pointer.
        pub col1: *mut u8,
        /// Column 2 base pointer.
        pub col2: *mut u8,
        /// Column 3 base pointer.
        pub col3: *mut u8,
    }
    /// The fixed simulation tick passed to every system.
    pub struct Tick {
        /// Tick number since start.
        pub frame: u64,
        /// Fixed step in seconds.
        pub dt: f32,
        /// Simulation time in seconds (`frame * dt`), never wall-clock.
        pub elapsed: f32,
    }
    /// Timing and outcome of one reload, reported by C# when it is applied at a tick boundary.
    pub struct ReloadReport {
        /// One of the `RELOAD_*` constants.
        pub kind: u32,
        /// Why an L1 candidate was escalated to L3 (`ESC_*`), or 0.
        pub escalation: u32,
        /// ALC generation after this reload (number of L3 swaps so far).
        pub generation: u32,
        /// EnC deltas applied to the current ALC.
        pub delta_count: u32,
        /// Members updated by an L1 delta.
        pub changed_members: u32,
        /// Compiler errors (for `RELOAD_ERROR`).
        pub error_count: u32,
        /// Host clock (ns) when the file watcher saw the change.
        pub t_event: u64,
        /// Host clock when the changed sources were read and parsed.
        pub t_parsed: u64,
        /// Host clock when the edit was classified.
        pub t_classified: u64,
        /// Host clock when emit (delta or full assembly) finished.
        pub t_emitted: u64,
        /// Host clock when the update was staged (ALC loaded and bound, or delta ready).
        pub t_staged: u64,
        /// Host clock when it was applied at a tick boundary.
        pub t_applied: u64,
        /// Bytes emitted (PE+PDB for L3, metadata+IL+PDB deltas for L1).
        pub emit_bytes: u64,
    }
    /// Describes one registered C# system.
    pub struct SystemInfo {
        /// Index into `SHAPES`.
        pub shape: u32,
        /// Length of the UTF-8 name written to the caller's buffer.
        pub name_len: u32,
    }
}

/// Reload kinds in `ReloadReport::kind`.
pub const RELOAD_L1: u32 = 1;
pub const RELOAD_L3: u32 = 3;
pub const RELOAD_ERROR: u32 = 100;

/// `tick_boundary` result bits.
pub const BOUNDARY_RELOADED: u32 = 1;
pub const BOUNDARY_SYSTEMS_CHANGED: u32 = 2;

/// `load_module` flags.
pub const LOAD_DEBUG_IL: u32 = 1;
pub const LOAD_WATCH: u32 = 2;

/// `diag` keys.
pub const DIAG_THREADS_SEEN: u32 = 1;
pub const DIAG_ALCS_ALIVE: u32 = 2;
pub const DIAG_ALCS_CREATED: u32 = 3;
pub const DIAG_FAULTS: u32 = 4;
pub const DIAG_MANAGED_HEAP: u32 = 5;
pub const DIAG_WORKING_SET: u32 = 6;
pub const DIAG_GC_PAUSE_US: u32 = 7;
pub const DIAG_GC_GEN0: u32 = 8;
pub const DIAG_GC_GEN1: u32 = 9;
pub const DIAG_GC_GEN2: u32 = 10;

fn_table! {
    /// Functions the Rust host provides to C#. Every function takes the host's opaque `ctx`.
    pub struct HostApi as HOST_API {
        /// Logs a UTF-8 message. Level: 0 info, 1 warn, 2 error.
        fn log(ctx: *mut c_void, level: u32, msg: *const u8, len: u32);
        /// Monotonic host clock in nanoseconds since host start (diagnostics only; never simulation).
        fn now_ns(ctx: *mut c_void) -> u64;
        /// Records that code carrying probe `value` ran (the host keeps a seen-set).
        fn stamp(ctx: *mut c_void, value: u32);
        /// Called once per reload, on the tick thread, when it is applied.
        fn report_reload(ctx: *mut c_void, report: *const ReloadReport);
        /// Called when a system throws; the system is disabled until the next reload.
        fn report_fault(ctx: *mut c_void, system: u32, msg: *const u8, len: u32);
    }
}

fn_table! {
    /// Functions the C# runtime fills in for the host. Exceptions never cross these boundaries.
    pub struct ScriptApi as SCRIPT_API {
        /// Compiles the module in `dir` with in-process Roslyn and loads it into a collectible ALC.
        fn load_module(dir: *const u8, dir_len: u32, flags: u32) -> i32;
        /// Warms the L1 and L3 compile paths without applying anything. Returns 0 on success.
        fn prewarm() -> i32;
        /// Non-zero if a staged reload will be applied by the next `tick_boundary`.
        fn has_pending() -> u32;
        /// Applies a staged reload (L1 delta or L3 swap). Returns `BOUNDARY_*` bits.
        fn tick_boundary(tick: *const Tick) -> u32;
        /// Number of registered systems.
        fn system_count() -> u32;
        /// Fills `out` and writes the system name (UTF-8) into `name_buf`. Returns 0 if `index` is valid.
        fn system_info(index: u32, out: *mut SystemInfo, name_buf: *mut u8, name_cap: u32) -> i32;
        /// Runs system `id` over one chunk. Returns 0 ok, 1 faulted or disabled, 2 bad id.
        fn run_system(id: u32, chunk: *const Chunk, tick: *const Tick) -> i32;
        /// Diagnostics counters (`DIAG_*`).
        fn diag(key: u32) -> u64;
        /// Stops the watcher and compile thread, unloads the module, and reports leaked ALCs.
        fn shutdown() -> i32;
    }
}

described_structs! { INIT_STRUCTS (Clone, Copy);
    /// Passed to `ara_script_init`: ABI identity, the host context and the host function table.
    pub struct HostInit {
        /// ABI version; C# refuses to start on mismatch.
        pub abi_version: u32,
        /// Reserved; keeps `layout_hash` 8-aligned explicitly.
        pub reserved: u32,
        /// Hash of every described layout; C# refuses to start against stale generated code.
        pub layout_hash: u64,
        /// Opaque host context passed back to every `HostApi` function.
        pub ctx: *mut c_void,
        /// Host function table.
        pub api: HostApi,
    }
}

/// Bumped on any incompatible change to the tables' semantics.
pub const ABI_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Query shapes. Each shape is a fixed column set; Rust builds the bevy query, C# gets a typed view.

/// Declares query shapes: the `Shape` enum, `SHAPES` descriptors and `add_shape_system`, which adds a
/// Bevy system for shape `$name` that hands each contiguous chunk to C#.
macro_rules! shapes {
    ($( $(#[doc = $doc:literal])* $name:ident { $( $acc:ident $var:ident : $comp:ident ),* } )*) => {
        /// Query shape ids (index into [`SHAPES`]).
        #[repr(u32)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Shape { $( $name ),* }

        pub const SHAPES: &[ShapeDesc] = &[ $( ShapeDesc {
            name: stringify!($name),
            doc: concat!($($doc, "\n"),*),
            columns: &[ $( ColumnDesc { name: stringify!($var), component: stringify!($comp), write: shape_is_write!($acc) } ),* ],
        } ),* ];

        /// Adds a Bevy system running C# system `sys` (of shape `shape`) over `contiguous_par_iter_mut`
        /// chunks, so chunks run on the compute task pool. It runs in `CsSet(sys.id)`, after every set
        /// in `after`. Returns false for an unknown shape.
        pub fn add_shape_system(
            schedule: &mut bevy_ecs::schedule::Schedule,
            shape: u32,
            sys: crate::sched::CsSystem,
            after: &[crate::sched::CsSet],
        ) -> bool {
            use bevy_ecs::prelude::*;
            $(
                if shape == Shape::$name as u32 {
                    let id = sys.id;
                    let system = move |mut q: Query<(Entity, $( shape_query_ty!($acc $comp) ),*)>,
                                       tick: Res<crate::sched::SimTick>| {
                        let tick = tick.0;
                        let iter = q.contiguous_par_iter_mut().expect("shape queries only use dense components");
                        iter.for_each(|(entities, $($var),*)| {
                            let mut chunk = crate::sched::chunk_for(entities);
                            $( crate::sched::push_column(&mut chunk, shape_column_ptr!($acc $var)); )*
                            sys.run(&chunk, &tick);
                        });
                    };
                    let mut config = system.in_set(crate::sched::CsSet(id));
                    for set in after {
                        config = config.after(*set);
                    }
                    schedule.add_systems(config);
                    return true;
                }
            )*
            false
        }
    };
}

macro_rules! shape_is_write {
    (w) => {
        true
    };
    (r) => {
        false
    };
}

macro_rules! shape_query_ty {
    (w $c:ident) => { &mut $c };
    (r $c:ident) => { &$c };
}

/// Column base pointer for one contiguous item; writes mark the rows changed (bevy change ticks).
macro_rules! shape_column_ptr {
    (w $v:ident) => {
        bevy_ecs::change_detection::ContiguousMut::split($v).0.as_mut_ptr().cast::<u8>()
    };
    (r $v:ident) => {
        $v.as_ptr().cast_mut().cast::<u8>()
    };
}

shapes! {
    /// Writes transforms from velocities.
    Move { w transform: Transform, r velocity: Velocity }
    /// Writes velocities, reading positions (forces, contacts).
    Accel { w velocity: Velocity, r transform: Transform }
    /// Writes velocities only.
    Damp { w velocity: Velocity }
    /// Writes transforms only.
    Place { w transform: Transform }
    /// Writes health only.
    Vitals { w health: Health }
}

/// Hash of every described layout and table signature. Shared with C# through the generator.
pub fn layout_hash() -> u64 {
    // FNV-1a over a canonical text rendering; stable across runs and platforms.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |s: &str| {
        for b in s.bytes().chain([0xffu8]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for s in VALUE_TYPES.iter().chain(COMPONENTS).chain(ABI_STRUCTS).chain(INIT_STRUCTS) {
        eat(s.name);
        eat(&s.size.to_string());
        for f in s.fields {
            eat(f.name);
            eat(f.ty);
            eat(&f.offset.to_string());
        }
    }
    for t in [&HOST_API, &SCRIPT_API] {
        eat(t.name);
        for f in t.fns {
            eat(f.name);
            for (p, ty) in f.params {
                eat(p);
                eat(ty);
            }
            eat(f.ret);
        }
    }
    for s in SHAPES {
        eat(s.name);
        for c in s.columns {
            eat(c.name);
            eat(c.component);
            eat(if c.write { "w" } else { "r" });
        }
    }
    h
}
