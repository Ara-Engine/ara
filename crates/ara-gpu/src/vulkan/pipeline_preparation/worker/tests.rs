use super::*;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

struct Release(Option<Sender<()>>);
impl Release {
    fn open(&mut self) {
        if let Some(send) = self.0.take() {
            let _ = send.send(());
        }
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}

fn recipe() -> Vec<PipelinePreparation> {
    let artifacts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cooked");
    vec![PipelinePreparation {
        cooked: serde_json::from_slice(
            &std::fs::read(artifacts.join("sampled_heap.json")).unwrap(),
        )
        .unwrap(),
        artifacts,
    }]
}

#[test]
fn blocked_pipeline_compiler_does_not_wait_or_publish_and_cancellation_is_bounded()
-> anyhow::Result<()> {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/pipeline-worker");
    let mut gpu = VulkanDevice::new_raster(&[], &root)?;
    let device = gpu.context.device.clone();
    let layout = gpu.heap.layout;
    let path = root.join("test-async.bin");
    let (send, gate) = channel();
    let mut release = Release(Some(send));
    let (entered, started) = channel();
    let mut gate = Some(gate);
    gpu.pipeline_preparation = Some(Worker::start(
        move || Compiler::new(device, false, layout, path, Vec::new()),
        move || {
            assert_eq!(std::thread::current().name(), Some("ara-pipeline-compile"));
            if let Some(gate) = gate.take() {
                let _ = entered.send(());
                let _ = gate.recv();
            }
        },
    )?);
    let first = gpu.prepare_pipelines(recipe)?.expect("admitted");
    started.recv_timeout(Duration::from_secs(5))?;
    let cancelled = gpu.prepare_pipelines(recipe)?.expect("queued");
    assert!(
        gpu.prepare_pipelines(|| panic!("full queue must not resolve recipes"))?
            .is_none()
    );
    let waits = gpu.context.host_wait_stats();
    for _ in 0..16 {
        assert!(gpu.poll_prepared_pipelines(first)?.is_none());
    }
    assert_eq!(gpu.context.host_wait_stats().calls, waits.calls);
    assert_eq!(gpu.pipelines.values_mut().count(), 0);
    gpu.cancel_prepared_pipelines(cancelled)?;
    assert!(matches!(
        gpu.poll_prepared_pipelines(cancelled),
        Err(GpuError::StaleHandle)
    ));
    release.open();
    let began = Instant::now();
    let handles = loop {
        if let Some(handles) = gpu.poll_prepared_pipelines(first)? {
            break handles;
        }
        assert!(began.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(handles.len(), 1);
    assert_eq!(gpu.pipelines.values_mut().count(), 1);
    assert!(matches!(
        gpu.poll_prepared_pipelines(first),
        Err(GpuError::StaleHandle)
    ));
    gpu.retire_pipeline(handles[0])?;
    assert!(gpu.pipelines.get(handles[0]).is_err());
    let failed = gpu
        .prepare_pipelines(|| {
            let mut recipes = recipe();
            let mut invalid = recipe().pop().expect("recipe");
            invalid.cooked.binding_layout_hash = "invalid-layout".into();
            recipes.push(invalid);
            recipes
        })?
        .expect("admitted after consumption");
    loop {
        match gpu.poll_prepared_pipelines(failed) {
            Err(GpuError::Contract(_)) => break,
            Ok(None) => {
                assert!(began.elapsed() < Duration::from_secs(20));
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("partial pipeline batch must not publish: {other:?}"),
        }
    }
    assert_eq!(gpu.pipelines.values_mut().count(), 0);
    drop(gpu.pipeline_preparation.take());
    gpu.collect()?;
    gpu.assert_clean()?;
    assert!(root.join("test-async.bin").metadata()?.len() >= 32);
    Ok(())
}
