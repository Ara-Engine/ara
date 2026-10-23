//! S7 conformance ABI loader. Actual engine templates use ara-host's generated ABI.
use crate::abi::{
    ABI_VERSION, CoreclrCreateDelegate, CoreclrInitialize, HostApi, ScriptApi, ScriptInit,
};
use crate::log;
use crate::runtime::ScriptLane;
use libloading::Library;
use std::ffi::{CString, c_void};
use std::path::Path;
use std::time::{Duration, Instant};

extern "C" fn host_log(level: u32, msg: *const u8, len: usize) {
    // SAFETY: C# passes a UTF-8 buffer of `len` bytes that lives for the call.
    let bytes = unsafe { std::slice::from_raw_parts(msg, len) };
    log::write(level, "cs", &String::from_utf8_lossy(bytes));
}

extern "C" fn host_nop(x: u32) -> u32 {
    x.wrapping_add(1)
}

static HOST: HostApi = HostApi {
    abi_version: ABI_VERSION,
    struct_size: size_of::<HostApi>() as u32,
    log: host_log,
    nop: host_nop,
};

/// A bound C# script. The images stay loaded for the process lifetime: neither a
/// NativeAOT image nor CoreCLR supports unloading.
pub struct Script {
    pub(crate) api: ScriptApi,
}

impl Script {
    /// Loads the script image for `lane` and calls `ara_script_init`.
    /// Returns the script plus the image-load time (excluding the init call).
    pub fn load(lane: &ScriptLane) -> Result<(Self, Duration), String> {
        let started = Instant::now();
        let init = match lane {
            ScriptLane::Aot { lib } => load_aot(lib)?,
            ScriptLane::Clr { dir, assembly } => load_clr(dir, assembly)?,
        };
        let load = started.elapsed();
        let mut api = ScriptApi::default();
        // SAFETY: `init` is `ara_script_init` with the ABI in `abi.rs`; `HOST` is 'static.
        let status = unsafe { init(&HOST, &mut api) };
        if status != 0 {
            return Err(format!("ara_script_init failed with status {status}"));
        }
        if api.abi_version != ABI_VERSION || api.struct_size as usize != size_of::<ScriptApi>() {
            return Err(format!(
                "ABI mismatch: C# v{} size {}, host v{ABI_VERSION} size {}",
                api.abi_version,
                api.struct_size,
                size_of::<ScriptApi>()
            ));
        }
        Ok((Self { api }, load))
    }
}

fn load_aot(lib: &Path) -> Result<ScriptInit, String> {
    // SAFETY: loading our own NativeAOT image; its initializers are the .NET runtime's.
    let library =
        unsafe { Library::new(lib) }.map_err(|e| format!("loading {}: {e}", lib.display()))?;
    // SAFETY: the symbol has the `ScriptInit` signature (see game/Exports.cs).
    let init =
        *unsafe { library.get::<ScriptInit>(b"ara_script_init\0") }.map_err(|e| e.to_string())?;
    std::mem::forget(library);
    Ok(init)
}

fn load_clr(dir: &Path, assembly: &str) -> Result<ScriptInit, String> {
    let name = if cfg!(windows) {
        "coreclr.dll"
    } else if cfg!(target_os = "macos") {
        "libcoreclr.dylib"
    } else {
        "libcoreclr.so"
    };
    let dir = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    // SAFETY: loading the bundled CoreCLR from the export folder.
    let library =
        unsafe { Library::new(&path) }.map_err(|e| format!("loading {}: {e}", path.display()))?;
    // SAFETY: signatures from coreclrhost.h.
    let initialize = *unsafe { library.get::<CoreclrInitialize>(b"coreclr_initialize\0") }
        .map_err(|e| e.to_string())?;
    // SAFETY: as above.
    let create = *unsafe { library.get::<CoreclrCreateDelegate>(b"coreclr_create_delegate\0") }
        .map_err(|e| e.to_string())?;
    std::mem::forget(library);

    let mut tpa: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "dll"))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    tpa.sort();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let dir_s = dir.to_string_lossy().into_owned();
    // hostfxr normally supplies RUNTIME_IDENTIFIER; direct coreclr hosting must pass it itself.
    let os = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "osx",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let props = [
        ("RUNTIME_IDENTIFIER", format!("{os}-{arch}")),
        ("TRUSTED_PLATFORM_ASSEMBLIES", tpa.join(sep)),
        ("APP_PATHS", dir_s.clone()),
        ("NATIVE_DLL_SEARCH_DIRECTORIES", dir_s),
        ("System.Globalization.Invariant", "true".into()),
    ];
    let cstr = |s: &str| CString::new(s).map_err(|e| e.to_string());
    let keys = props
        .iter()
        .map(|(k, _)| cstr(k))
        .collect::<Result<Vec<_>, _>>()?;
    let values = props
        .iter()
        .map(|(_, v)| cstr(v))
        .collect::<Result<Vec<_>, _>>()?;
    let key_ptrs: Vec<_> = keys.iter().map(|k| k.as_ptr()).collect();
    let value_ptrs: Vec<_> = values.iter().map(|v| v.as_ptr()).collect();
    let exe = cstr(
        &std::env::current_exe()
            .map_err(|e| e.to_string())?
            .to_string_lossy(),
    )?;
    let mut handle: *mut c_void = std::ptr::null_mut();
    let mut domain = 0u32;
    // SAFETY: all pointers are valid NUL-terminated strings that outlive the call.
    let hr = unsafe {
        initialize(
            exe.as_ptr(),
            c"ara".as_ptr(),
            key_ptrs.len() as i32,
            key_ptrs.as_ptr(),
            value_ptrs.as_ptr(),
            &mut handle,
            &mut domain,
        )
    };
    if hr < 0 {
        return Err(format!("coreclr_initialize failed: 0x{:08x}", hr as u32));
    }
    let mut fp: *mut c_void = std::ptr::null_mut();
    let asm = cstr(assembly)?;
    // SAFETY: valid handle/domain from coreclr_initialize; NUL-terminated names.
    let hr = unsafe {
        create(
            handle,
            domain,
            asm.as_ptr(),
            c"Ara.Game.Exports".as_ptr(),
            c"Init".as_ptr(),
            &mut fp,
        )
    };
    if hr < 0 || fp.is_null() {
        return Err(format!(
            "coreclr_create_delegate(Exports.Init) failed: 0x{:08x}",
            hr as u32
        ));
    }
    // SAFETY: `Exports.Init` is [UnmanagedCallersOnly] with the `ScriptInit` signature.
    Ok(unsafe { std::mem::transmute::<*mut c_void, ScriptInit>(fp) })
}
