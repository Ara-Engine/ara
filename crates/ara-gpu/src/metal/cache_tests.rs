//! Pipeline archives across single-device processes, as the player and editor use
//! them: the parent test re-runs this test binary on one fresh cache directory, cold,
//! warm and warm again, then once under shader validation. Each child creates one
//! device, checks what the archive already holds, compiles and runs the test
//! pipelines, and flushes at drop. The third run proves a warm run's flush keeps
//! binaries it only looked up; the last proves validated runs leave the archive alone.
use super::tests::{ABI, SHADERS};
use super::*;
use crate::commands::Command;
use crate::cooked::{RenderState, Stage};
use objc2_foundation::NSString;

const CHILD: &str = "metal::cache_tests::single_device_process";

#[test]
fn archives_persist_across_single_device_processes() {
    let dir = std::env::temp_dir().join(format!("ara-metal-archive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let archive = || {
        let entries = std::fs::read_dir(&dir).expect("cache directory");
        let files = entries
            .map(|entry| entry.expect("entry").path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1, "one archive and no temporaries: {files:?}");
        std::fs::read(&files[0]).expect("archive bytes")
    };
    let mut saved = Vec::new();
    for run in ["cold", "warm", "rewarm", "validated"] {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .args([
                CHILD,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("ARA_METAL_ARCHIVE_DIR", &dir)
            .env("ARA_METAL_ARCHIVE_RUN", run)
            .env_remove("MTL_SHADER_VALIDATION");
        if run == "validated" {
            child.env("MTL_SHADER_VALIDATION", "1");
        }
        let output = child.output().expect("spawn child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(&format!("archive run {run}: ok")),
            "{run}: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if run == "validated" {
            assert!(
                archive() == saved,
                "a validated run left the archive untouched"
            );
        }
        saved = archive();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "child process of archives_persist_across_single_device_processes"]
fn single_device_process() {
    let (Some(dir), Ok(run)) = (
        std::env::var_os("ARA_METAL_ARCHIVE_DIR"),
        std::env::var("ARA_METAL_ARCHIVE_RUN"),
    ) else {
        return;
    };
    let mut gpu = MetalDevice::new_raster(&[], dir).expect("Metal 4 device");
    let cached = native::pool(|| {
        gpu.compiler
            .archive()
            .map(|archive| archived(&gpu.device, archive))
    });
    match run.as_str() {
        "cold" => assert!(cached.is_none(), "a fresh directory has no archive"),
        "validated" => assert!(cached.is_none(), "validated runs load no archive"),
        _ => assert_eq!(cached, Some(true), "{run}: the archive holds `fill`"),
    }
    let fill = gpu.msl_pipeline(
        &[(Stage::Compute, "fill")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let state = RenderState {
        color_formats: vec![vk::Format::R8G8B8A8_UNORM.as_raw()],
        vertex_stride: 48,
        ..RenderState::default()
    };
    gpu.msl_pipeline(
        &[(Stage::Vertex, "mesh"), (Stage::Fragment, "shade")],
        &state,
        None,
    );
    let buffer = gpu
        .create_buffer(
            64 * 4,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )
        .expect("storage buffer");
    let slot = gpu.storage_buffer(buffer).expect("slot");
    let mut rec = gpu.begin(Queue::Compute);
    rec.push(Command::Pipeline(fill));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, 7, 64, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    gpu.resolve(point).expect("resolve");
    let data: Vec<u32> = bytemuck::cast_slice(&gpu.read(buffer).expect("read")).to_vec();
    assert_eq!(data, (7..71).collect::<Vec<_>>());
    drop(gpu);
    match cache::path() {
        Some(path) => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            assert!(
                size > 0,
                "{run}: the last device out saved {}",
                path.display()
            );
        }
        None => assert_eq!(run, "validated", "only validated runs skip capture"),
    }
    println!("archive run {run}: ok");
}

/// Whether `archive` alone, without compiling, yields the `fill` compute pipeline.
fn archived(device: &ProtocolObject<dyn MTLDevice>, archive: &cache::Archive) -> bool {
    let source = NSString::from_str(&format!("{ABI}{SHADERS}"));
    let library = device
        .newLibraryWithSource_options_error(&source, None)
        .expect("test MSL compiles");
    let function = MTL4LibraryFunctionDescriptor::new();
    function.setName(Some(&NSString::from_str("fill")));
    function.setLibrary(Some(&library));
    let descriptor = MTL4ComputePipelineDescriptor::new();
    descriptor.setComputeFunctionDescriptor(Some(&function));
    archive
        .newComputePipelineStateWithDescriptor_error(&descriptor)
        .is_ok()
}
