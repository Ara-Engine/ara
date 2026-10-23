//! Starting the C# runtime and resolving `ara_script_init`.
//!
//! - **JIT (dev):** hostfxr (`netcorehost`) loads .NET 10 with `Ara.Host`'s runtime
//!   config, `Ara.Host.dll` goes into the default ALC (so module ALCs share
//!   `Ara.Runtime`'s types), and `Ara.Hosting.DevExports.Init` is the entry point.
//! - **NativeAOT (shipping):** the module image is a native library exporting
//!   `ara_script_init`.
//!
//! A process hosts one .NET runtime. The JIT runtime is started once and reused by
//! later hosts in the same process; each host calls `ara_script_init` again, which
//! resets the C# runtime state.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use ara_diag::Diagnostic;
use netcorehost::hostfxr::{Hostfxr, HostfxrContext, InitializedForRuntimeConfig};
use netcorehost::pdcstring::PdCString;

use crate::abi::{HostInit, ScriptApi};
use crate::codes;

/// `ara_script_init`.
pub(crate) type InitFn = unsafe extern "C" fn(*const HostInit, *mut ScriptApi) -> i32;

/// Which C# runtime to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    /// CoreCLR JIT through hostfxr: `dir` holds the built `Ara.Host.dll`, its
    /// `runtimeconfig.json` and `Ara.Runtime.dll`. `enc` enables EnC (L1 reload) by
    /// setting `DOTNET_MODIFIABLE_ASSEMBLIES=debug` before the runtime starts.
    Jit {
        /// The `Ara.Host` build output directory.
        dir: PathBuf,
        /// Enable Edit-and-Continue deltas (L1).
        enc: bool,
    },
    /// A NativeAOT shared library exporting `ara_script_init` (modules linked in).
    NativeAot {
        /// Path to the library (`Outpost.Sample.dll`).
        library: PathBuf,
    },
}

struct Jit {
    dir: PathBuf,
    enc: bool,
    init: InitFn,
    _context: HostfxrContext<InitializedForRuntimeConfig>,
}

// SAFETY: the hostfxr context is only kept alive (never used again after start), and
// `init` is a plain function pointer into the runtime, callable from any thread.
unsafe impl Send for Jit {}

static JIT: OnceLock<Mutex<Option<Jit>>> = OnceLock::new();

/// A started runtime: the entry point and whatever keeps it loaded.
pub(crate) enum Loaded {
    Jit,
    Aot {
        /// Keeps the image mapped for as long as the host uses it.
        _library: libloading::Library,
    },
}

fn start_error(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_START, msg)
}

fn pd(p: &Path) -> Result<PdCString, Diagnostic> {
    PdCString::from_os_str(p.as_os_str()).map_err(|e| start_error(format!("{}: {e}", p.display())))
}

/// Starts (or reuses) the runtime and returns `ara_script_init`.
pub(crate) fn start(runtime: &Runtime) -> Result<(InitFn, Loaded), Diagnostic> {
    match runtime {
        Runtime::Jit { dir, enc } => {
            let slot = JIT.get_or_init(|| Mutex::new(None));
            let mut jit = slot
                .lock()
                .map_err(|_| start_error("runtime lock poisoned"))?;
            if let Some(j) = jit.as_ref() {
                if j.dir != *dir || j.enc != *enc {
                    return Err(start_error(format!(
                        "this process already runs .NET from `{}` (enc: {}); one runtime per process",
                        j.dir.display(),
                        j.enc
                    )));
                }
                return Ok((j.init, Loaded::Jit));
            }
            let started = start_jit(dir, *enc)?;
            let init = started.init;
            *jit = Some(started);
            Ok((init, Loaded::Jit))
        }
        Runtime::NativeAot { library } => {
            // SAFETY: loading the library runs its initializers; NativeAOT images built
            // from Ara.Runtime have no unsound initializers.
            let lib = unsafe { libloading::Library::new(library) }
                .map_err(|e| start_error(format!("load `{}`: {e}", library.display())))?;
            // SAFETY: `ara_script_init` has the `InitFn` signature (generated `[UnmanagedCallersOnly]`).
            let init: InitFn =
                *unsafe { lib.get::<InitFn>(b"ara_script_init\0") }.map_err(|e| {
                    start_error(format!(
                        "`{}` has no `ara_script_init`: {e}",
                        library.display()
                    ))
                })?;
            Ok((init, Loaded::Aot { _library: lib }))
        }
    }
}

/// Finds `hostfxr` the way `nethost` does: `DOTNET_ROOT`, the `dotnet` on `PATH`, then the
/// platform's default install location; the highest installed version wins. Doing it
/// here avoids linking nethost's static library (and downloading it at build time).
fn find_hostfxr() -> Result<PathBuf, Diagnostic> {
    let lib = if cfg!(windows) {
        "hostfxr.dll"
    } else if cfg!(target_os = "macos") {
        "libhostfxr.dylib"
    } else {
        "libhostfxr.so"
    };
    let exe = if cfg!(windows) {
        "dotnet.exe"
    } else {
        "dotnet"
    };
    let mut roots: Vec<PathBuf> = std::env::var_os("DOTNET_ROOT")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        roots.extend(
            std::env::split_paths(&path)
                .map(|d| d.join(exe))
                .filter(|p| p.is_file())
                .filter_map(|p| {
                    std::fs::canonicalize(p)
                        .ok()?
                        .parent()
                        .map(Path::to_path_buf)
                }),
        );
    }
    if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles")
            .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
        roots.push(pf.join("dotnet"));
    } else {
        roots.extend(
            [
                "/usr/local/share/dotnet",
                "/usr/share/dotnet",
                "/usr/lib/dotnet",
                "/opt/homebrew/share/dotnet",
            ]
            .map(PathBuf::from),
        );
    }
    let version = |p: &Path| -> Vec<u64> {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|n| n.parse().unwrap_or(0))
            .collect()
    };
    roots
        .iter()
        .filter_map(|root| {
            let mut versions: Vec<PathBuf> = std::fs::read_dir(root.join("host/fxr"))
                .ok()?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|d| d.join(lib).is_file())
                .collect();
            versions.sort_by_key(|d| version(d));
            versions.pop().map(|d| d.join(lib))
        })
        .next()
        .ok_or_else(|| {
            start_error("no .NET runtime found (set DOTNET_ROOT or install the .NET 10 runtime)")
        })
}

fn start_jit(dir: &Path, enc: bool) -> Result<Jit, Diagnostic> {
    if enc {
        // SAFETY: the runtime reads this variable once, at startup, which is the next
        // call. On Windows the environment block is synchronized by the OS. On other
        // platforms the caller must start the host before other threads read the
        // environment (`ScriptHost::start` documents this).
        unsafe { std::env::set_var("DOTNET_MODIFIABLE_ASSEMBLIES", "debug") };
    }
    let fxr = find_hostfxr()?;
    let hostfxr = Hostfxr::load_from_path(&fxr)
        .map_err(|e| start_error(format!("load `{}`: {e}", fxr.display())))?;
    let config = dir.join("Ara.Host.runtimeconfig.json");
    let context = hostfxr
        .initialize_for_runtime_config(pd(&config)?)
        .map_err(|e| {
            start_error(format!(
                "initialize .NET from `{}` (build dotnet/ first): {e}",
                config.display()
            ))
        })?;
    // The default ALC, not an isolated component context: module ALCs must share Ara.Runtime's types.
    context
        .load_assembly_from_path(pd(&dir.join("Ara.Host.dll"))?)
        .map_err(|e| start_error(format!("load Ara.Host.dll: {e}")))?;
    let loader = context
        .get_delegate_loader()
        .map_err(|e| start_error(format!("delegate loader: {e}")))?;
    let type_name = PdCString::from_str("Ara.Hosting.DevExports, Ara.Host")
        .map_err(|e| start_error(e.to_string()))?;
    let method = PdCString::from_str("Init").map_err(|e| start_error(e.to_string()))?;
    let f = loader
        .get_function_with_unmanaged_callers_only::<fn(*const HostInit, *mut ScriptApi) -> i32>(
            &type_name, &method,
        )
        .map_err(|e| start_error(format!("resolve Ara.Hosting.DevExports.Init: {e}")))?;
    let raw: extern "system" fn(*const HostInit, *mut ScriptApi) -> i32 = *f;
    // SAFETY: the managed method is `[UnmanagedCallersOnly]` with exactly this signature in
    // the platform default convention; "system" and "C" are the same ABI on every 64-bit
    // target Ara supports.
    let init = unsafe {
        std::mem::transmute::<extern "system" fn(*const HostInit, *mut ScriptApi) -> i32, InitFn>(
            raw,
        )
    };
    Ok(Jit {
        dir: dir.to_path_buf(),
        enc,
        init,
        _context: context,
    })
}
