//! Hosting .NET 10 through hostfxr (`netcorehost`), and the host side of the ABI.

use crate::schema::{self, HostApi, HostInit, ReloadReport, ScriptApi};
use core::ffi::c_void;
use netcorehost::{
    hostfxr::HostfxrContext, hostfxr::InitializedForRuntimeConfig, nethost, pdcstring::PdCString,
};
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Probe values are taken modulo this.
pub const STAMP_SLOTS: usize = 1 << 16;

/// State the C# side reaches through `HostInit::ctx`. Diagnostics only; no gameplay state.
pub struct HostCtx {
    pub start: Instant,
    /// `seen[v]` is set once code carrying probe value `v` has run (values are unique per edit).
    pub seen: Box<[AtomicBool]>,
    pub reports: Mutex<Vec<ReloadReport>>,
    pub faults: Mutex<Vec<(u32, String)>>,
    /// Print info-level C# logs.
    pub verbose: AtomicBool,
}

impl HostCtx {
    pub fn new(verbose: bool) -> Arc<Self> {
        Arc::new(HostCtx {
            start: Instant::now(),
            seen: (0..STAMP_SLOTS).map(|_| AtomicBool::new(false)).collect(),
            reports: Mutex::new(Vec::new()),
            faults: Mutex::new(Vec::new()),
            verbose: AtomicBool::new(verbose),
        })
    }

    /// True once code stamping `value` has run.
    pub fn seen(&self, value: u32) -> bool {
        self.seen[value as usize % STAMP_SLOTS].load(Ordering::Acquire)
    }

    /// Nanoseconds since host start; the clock C# timestamps use.
    pub fn now_ns(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_nanos()).expect("clock fits u64")
    }
}

/// A started runtime with the C# script API bound.
pub struct Clr {
    pub api: ScriptApi,
    /// hostfxr load + runtime init + first function pointer.
    pub runtime_init_ms: f64,
    /// `ara_script_init` (Ara.Runtime static init, layout check).
    pub script_init_ms: f64,
    /// Keeps the context C# points at alive for as long as the runtime is bound.
    _ctx: Arc<HostCtx>,
    _context: HostfxrContext<InitializedForRuntimeConfig>,
}

/// Directory of the built `Ara.Runtime` (see RESULT.md for the build command).
pub fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("dotnet/Ara.Runtime/bin/Release/net10.0")
}

/// Loads the runtime and calls `ara_script_init`. `enc` sets `DOTNET_MODIFIABLE_ASSEMBLIES=debug`,
/// which must happen before the runtime starts. Call this before any other thread exists. `ctx` must
/// outlive the runtime: C# holds a raw pointer to it.
pub fn start(enc: bool, ctx: Arc<HostCtx>) -> Clr {
    if enc {
        // SAFETY: called at the top of main before the task pool or any other thread is started,
        // so nothing else can be reading the environment concurrently.
        unsafe { std::env::set_var("DOTNET_MODIFIABLE_ASSEMBLIES", "debug") };
    }
    let dir = runtime_dir();
    let t0 = Instant::now();
    let hostfxr =
        nethost::load_hostfxr().expect("load hostfxr (is the .NET 10 runtime installed?)");
    let config = pd(&dir.join("Ara.Runtime.runtimeconfig.json"));
    let context = hostfxr
        .initialize_for_runtime_config(&config)
        .expect("initialize runtime (did you build dotnet/Ara.Runtime?)");
    // Default ALC (not an isolated component context), so module ALCs share Ara.Runtime's types.
    context
        .load_assembly_from_path(pd(&dir.join("Ara.Runtime.dll")))
        .expect("load Ara.Runtime.dll into the default ALC");
    let loader = context.get_delegate_loader().expect("delegate loader");
    let init = loader
        .get_function_with_unmanaged_callers_only::<fn(*const HostInit, *mut ScriptApi) -> i32>(
            &PdCString::from_str("Ara.Hosting.Exports, Ara.Runtime").expect("type name"),
            &PdCString::from_str("Init").expect("method name"),
        )
        .expect("resolve Ara.Hosting.Exports.Init");
    let runtime_init_ms = ms(t0);

    let host = HostInit {
        abi_version: schema::ABI_VERSION,
        reserved: 0,
        layout_hash: schema::layout_hash(),
        ctx: Arc::as_ptr(&ctx).cast_mut().cast::<c_void>(),
        api: HostApi {
            log: host_log,
            now_ns: host_now_ns,
            stamp: host_stamp,
            report_reload,
            report_fault,
        },
    };
    let mut api = MaybeUninit::<ScriptApi>::uninit();
    let t1 = Instant::now();
    let rc = init(&host, api.as_mut_ptr());
    let script_init_ms = ms(t1);
    assert_eq!(rc, 0, "ara_script_init failed (see log above)");
    // SAFETY: Init returned 0, which means it wrote every field of the table.
    let api = unsafe { api.assume_init() };
    Clr {
        api,
        runtime_init_ms,
        script_init_ms,
        _ctx: ctx,
        _context: context,
    }
}

impl Clr {
    /// Compiles and loads the gameplay module. Panics with the C# diagnostics on failure.
    pub fn load_module(&self, dir: &Path, flags: u32) -> f64 {
        let s = dir.to_string_lossy();
        let t = Instant::now();
        // SAFETY: the pointer/length pair describes a live UTF-8 string for the call.
        let rc = unsafe { (self.api.load_module)(s.as_ptr(), s.len() as u32, flags) };
        assert_eq!(rc, 0, "load_module failed");
        ms(t)
    }

    /// Calls the C# shutdown (stops the watcher, unloads, GCs) and returns the leaked-ALC count.
    pub fn shutdown(&self) -> i32 {
        // SAFETY: plain call into the C# runtime.
        unsafe { (self.api.shutdown)() }
    }

    pub fn diag(&self, key: u32) -> u64 {
        // SAFETY: plain call into the C# runtime.
        unsafe { (self.api.diag)(key) }
    }
}

fn pd(p: &Path) -> PdCString {
    PdCString::from_os_str(p.as_os_str()).expect("path without interior NUL")
}

pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// # Safety
/// `ctx` must be the `HostCtx` pointer handed out in `HostInit`, which outlives the runtime.
unsafe fn ctx<'a>(ctx: *mut c_void) -> &'a HostCtx {
    // SAFETY: guaranteed by the caller (see above).
    unsafe { &*ctx.cast::<HostCtx>() }
}

/// # Safety
/// `ptr`/`len` must describe readable bytes.
unsafe fn text(ptr: *const u8, len: u32) -> String {
    // SAFETY: guaranteed by the caller.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr, len as usize) }).into_owned()
}

unsafe extern "C" fn host_log(c: *mut c_void, level: u32, msg: *const u8, len: u32) {
    // SAFETY: C# passes back the ctx it was given and a valid UTF-8 span.
    let (c, msg) = unsafe { (ctx(c), text(msg, len)) };
    if level > 0 || c.verbose.load(Ordering::Relaxed) {
        let tag = ["info", "warn", "error"]
            .get(level as usize)
            .copied()
            .unwrap_or("?");
        eprintln!("[cs {tag}] {msg}");
    }
}

unsafe extern "C" fn host_now_ns(c: *mut c_void) -> u64 {
    // SAFETY: C# passes back the ctx it was given.
    unsafe { ctx(c) }.now_ns()
}

unsafe extern "C" fn host_stamp(c: *mut c_void, value: u32) {
    // SAFETY: C# passes back the ctx it was given.
    unsafe { ctx(c) }.seen[value as usize % STAMP_SLOTS].store(true, Ordering::Release);
}

unsafe extern "C" fn report_reload(c: *mut c_void, report: *const ReloadReport) {
    // SAFETY: C# passes back the ctx it was given and a pointer to a live report.
    let (c, r) = unsafe { (ctx(c), *report) };
    c.reports.lock().expect("reports lock").push(r);
}

unsafe extern "C" fn report_fault(c: *mut c_void, system: u32, msg: *const u8, len: u32) {
    // SAFETY: C# passes back the ctx it was given and a valid UTF-8 span.
    let (c, msg) = unsafe { (ctx(c), text(msg, len)) };
    eprintln!("[fault] system {system}: {msg}");
    c.faults.lock().expect("faults lock").push((system, msg));
}
