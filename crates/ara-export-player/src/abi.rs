//! The R10 hosting ABI, trimmed to what S7 needs. Mirrored by hand in `game/Abi.cs`.
//!
//! Rust owns all component memory. C# receives one call per batch of chunks and
//! reaches back into Rust only through [`HostApi`]. The same `ara_script_init`
//! entry point is used by both lanes: exported by name from a NativeAOT image,
//! or fetched with `coreclr_create_delegate` from an R2R assembly.

use std::ffi::{c_char, c_void};

/// Bumped on any layout change; both sides refuse to bind on mismatch.
pub const ABI_VERSION: u32 = 1;

/// A position plus yaw: the column the C# `Movement` system writes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    /// World-space X in metres.
    pub x: f32,
    /// World-space Y (up) in metres.
    pub y: f32,
    /// World-space Z in metres.
    pub z: f32,
    /// Heading in radians.
    pub yaw: f32,
}

/// Linear velocity plus yaw rate: read and written by `Movement`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Velocity {
    /// Metres per second along X.
    pub x: f32,
    /// Metres per second along Y.
    pub y: f32,
    /// Metres per second along Z.
    pub z: f32,
    /// Radians per second.
    pub yaw_rate: f32,
}

/// One archetype chunk: `len` rows of two columns, in system shape order.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    /// Rows in this chunk.
    pub len: u32,
    /// Valid column pointers (always 2 here).
    pub column_count: u32,
    /// `Transform` column.
    pub col0: *mut u8,
    /// `Velocity` column.
    pub col1: *mut u8,
}

/// The fixed simulation tick passed to every system. Never wall-clock.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Tick {
    /// Tick number since start.
    pub frame: u64,
    /// Fixed step in seconds.
    pub dt: f32,
    /// `frame * dt` in seconds.
    pub elapsed: f32,
}

/// Rust → C#: the host table C# keeps after init.
#[repr(C)]
pub struct HostApi {
    /// Must equal [`ABI_VERSION`].
    pub abi_version: u32,
    /// `size_of::<HostApi>()`, a cheap layout check.
    pub struct_size: u32,
    /// UTF-8 log line from C#; level 0 debug, 1 info, 2 error.
    pub log: extern "C" fn(level: u32, msg: *const u8, len: usize),
    /// Empty callback used to measure C# → Rust call cost.
    pub nop: extern "C" fn(x: u32) -> u32,
}

/// C# → Rust: filled by `ara_script_init`.
#[repr(C)]
#[derive(Default)]
pub struct ScriptApi {
    /// C#'s ABI version.
    pub abi_version: u32,
    /// C#'s `sizeof(ScriptApi)`.
    pub struct_size: u32,
    /// Runs system `id` over `n` chunks. 0 = ok, 1 = faulted (already logged), 2 = unknown id.
    pub run_system: Option<
        unsafe extern "C" fn(id: u32, chunks: *const Chunk, n: u32, tick: *const Tick) -> i32,
    >,
    /// FNV-1a over the `Transform` column; C# also logs it through `HostApi::log`.
    pub checksum: Option<unsafe extern "C" fn(chunks: *const Chunk, n: u32) -> u64>,
    /// Empty export used to measure Rust → C# call cost.
    pub nop: Option<unsafe extern "C" fn(x: u32) -> u32>,
    /// Calls `HostApi::nop` `n` times from C#; `fast != 0` uses `SuppressGCTransition`.
    pub call_host: Option<unsafe extern "C" fn(n: u64, fast: u32) -> u64>,
    /// Loads an IL mod assembly and runs it. Fails (status 2, logged) under NativeAOT.
    pub load_mod: Option<unsafe extern "C" fn(path: *const u8, len: usize) -> i32>,
}

/// `ara_script_init(host, out) -> status`; 0 means bound.
pub type ScriptInit = unsafe extern "C" fn(host: *const HostApi, out: *mut ScriptApi) -> i32;

/// `coreclr_initialize` from `coreclrhost.h` (UTF-8 strings on every OS).
pub type CoreclrInitialize = unsafe extern "C" fn(
    exe_path: *const c_char,
    app_domain_friendly_name: *const c_char,
    property_count: i32,
    property_keys: *const *const c_char,
    property_values: *const *const c_char,
    host_handle: *mut *mut c_void,
    domain_id: *mut u32,
) -> i32;

/// `coreclr_create_delegate` from `coreclrhost.h`.
pub type CoreclrCreateDelegate = unsafe extern "C" fn(
    host_handle: *mut c_void,
    domain_id: u32,
    assembly_name: *const c_char,
    type_name: *const c_char,
    method_name: *const c_char,
    delegate: *mut *mut c_void,
) -> i32;
