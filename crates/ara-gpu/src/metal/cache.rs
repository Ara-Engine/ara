//! Persistent pipeline binaries: the Metal counterpart of the persisted
//! `VkPipelineCache`.
//!
//! On macOS 27 pipeline capture is process state: a data-set serializer flush cleared
//! what other live devices had captured, and compilers created or destroyed around
//! other compiles corrupted the heap. So one compiler with one serializer serves the
//! whole process and is never destroyed. The first device's cache directory picks the
//! archive, named after the GPU and OS build so updates never reuse foreign binaries;
//! every device compiles with it as a lookup archive. Captured binaries are flushed
//! once per process, when its live device count first drops to zero: single-device
//! processes (player, editor) save at shutdown, when no compile can be in flight.
//! Saves are atomic (a per-process temporary file, then rename); a missing or
//! unreadable archive is a cold cache, never an error.
//!
//! Shader validation (`MTL_SHADER_VALIDATION`) instruments every pipeline, and the
//! driver rejects archives captured that way on reload ("deployment target ... not
//! compatible with current OS"). Validated processes therefore neither load nor save
//! archives: loading would bypass instrumentation, saving would replace a good cache.
use super::native::{ns_error, pool};
use crate::Result;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray, NSError, NSProcessInfo, NSString, NSURL};
use objc2_metal::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A loaded pipeline archive.
pub(super) type Archive = Retained<ProtocolObject<dyn MTL4Archive>>;

/// The process compiler and lookup archive; clones share both (preparation workers).
#[derive(Clone)]
pub(super) struct Compiler {
    compiler: Retained<ProtocolObject<dyn MTL4Compiler>>,
    archive: Option<Archive>,
}

impl Compiler {
    /// Compile (or look up) a render pipeline; the serializer captures its binaries.
    pub fn render(
        &self,
        descriptor: &MTL4RenderPipelineDescriptor,
    ) -> std::result::Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, Retained<NSError>>
    {
        self.compiler
            .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(
                descriptor,
                Some(&self.options()),
            )
    }
    /// Compile (or look up) a compute pipeline; the serializer captures its binaries.
    pub fn compute(
        &self,
        descriptor: &MTL4ComputePipelineDescriptor,
    ) -> std::result::Result<Retained<ProtocolObject<dyn MTLComputePipelineState>>, Retained<NSError>>
    {
        self.compiler
            .newComputePipelineStateWithDescriptor_compilerTaskOptions_error(
                descriptor,
                Some(&self.options()),
            )
    }
    /// The archive loaded at process start; None on a cold cache.
    #[cfg(test)]
    pub fn archive(&self) -> Option<&Archive> {
        self.archive.as_ref()
    }
    fn options(&self) -> Retained<MTL4CompilerTaskOptions> {
        let options = MTL4CompilerTaskOptions::new();
        if let Some(archive) = &self.archive {
            options.setLookupArchives(Some(&NSArray::from_retained_slice(std::slice::from_ref(
                archive,
            ))));
        }
        options
    }
}

/// A live device's share of the process cache; dropping the last one flushes once.
/// Devices drop it after joining their workers, so no compile is in flight.
pub(super) struct Lease(());

impl Drop for Lease {
    fn drop(&mut self) {
        let mut process = lock();
        let Some(process) = process.as_mut() else {
            return;
        };
        process.live -= 1;
        if process.live == 0 && !process.flushed {
            process.flushed = true;
            if let Some(capture) = &process.capture {
                // Like the Vulkan cache save on drop: a failed save only loses warm starts.
                let _ = pool(|| capture.flush());
            }
        }
    }
}

struct Process {
    compiler: Compiler,
    /// None under shader validation.
    capture: Option<Capture>,
    live: usize,
    flushed: bool,
}

/// The serializer recording this process's binaries and the archive it saves to.
struct Capture {
    serializer: Retained<ProtocolObject<dyn MTL4PipelineDataSetSerializer>>,
    path: PathBuf,
}

impl Process {
    fn new(device: &ProtocolObject<dyn MTLDevice>, dir: &Path) -> Result<Self> {
        let descriptor = MTL4CompilerDescriptor::new();
        let validated = std::env::var_os("MTL_SHADER_VALIDATION").is_some_and(|v| v != "0");
        let capture = (!validated).then(|| {
            let options = MTL4PipelineDataSetSerializerDescriptor::new();
            options.setConfiguration(MTL4PipelineDataSetSerializerConfiguration::CaptureBinaries);
            let serializer = device.newPipelineDataSetSerializerWithDescriptor(&options);
            descriptor.setPipelineDataSetSerializer(Some(&serializer));
            Capture {
                serializer,
                path: archive_path(device, dir),
            }
        });
        let compiler = device
            .newCompilerWithDescriptor_error(&descriptor)
            .map_err(|e| ns_error("MTL4 compiler", &e))?;
        let archive = capture
            .as_ref()
            .filter(|capture| capture.path.is_file())
            .and_then(|capture| device.newArchiveWithURL_error(&url(&capture.path)).ok());
        Ok(Self {
            compiler: Compiler { compiler, archive },
            capture,
            live: 0,
            flushed: false,
        })
    }
}

/// The archive file for this GPU and OS build in `dir`.
fn archive_path(device: &ProtocolObject<dyn MTLDevice>, dir: &Path) -> PathBuf {
    let key = format!(
        "ara-metal-archive-1|{}|{}",
        device.name(),
        NSProcessInfo::processInfo().operatingSystemVersionString()
    );
    let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
    dir.join(format!("metal-pipelines-{}.mtl4archive", &digest[..16]))
}

impl Capture {
    fn flush(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
        }
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        if let Err(error) = self
            .serializer
            .serializeAsArchiveAndFlushToURL_error(&url(&temporary))
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(ns_error("pipeline archive", &error));
        }
        let renamed = std::fs::rename(&temporary, &self.path);
        if renamed.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        renamed.map_err(anyhow::Error::from)?;
        Ok(())
    }
}

struct Global(Mutex<Option<Process>>);
// SAFETY: the compiler and archive are thread-safe Metal objects; the serializer is
// only created and flushed while holding the mutex, and the driver uses it internally
// from compiler calls, which Metal allows on any thread.
unsafe impl Sync for Global {}

static PROCESS: Global = Global(Mutex::new(None));

fn lock() -> MutexGuard<'static, Option<Process>> {
    PROCESS.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Join the process cache, creating its compiler and loading its archive from `dir`
/// on first use. Keep the lease until the device's compiles have finished.
pub(super) fn acquire(
    device: &ProtocolObject<dyn MTLDevice>,
    dir: &Path,
) -> Result<(Compiler, Lease)> {
    let mut process = lock();
    let process = match &mut *process {
        Some(process) => process,
        empty => empty.insert(Process::new(device, dir)?),
    };
    process.live += 1;
    Ok((process.compiler.clone(), Lease(())))
}

/// The process archive's path, once a device has been created; None when the process
/// does not capture (shader validation).
#[cfg(test)]
pub(super) fn path() -> Option<PathBuf> {
    lock()
        .as_ref()
        .and_then(|process| Some(process.capture.as_ref()?.path.clone()))
}

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}
