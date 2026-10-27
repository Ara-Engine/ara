//! D19 shared surfaces across two processes on macOS, as embedded Play uses them: this
//! binary re-spawns itself as the producer, which publishes a cleared frame and prints
//! its `SurfaceDesc` and `Frame`; the owner imports the slots with
//! `SharedSurface::duplicate_from`, checks the pixels and tells the producer to exit.
//! Custom harness: the child must run the producer path, not a test runner.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use std::io::{BufRead, BufReader, Write};
    if std::env::var_os("ARA_SHARED_PRODUCER").is_some() {
        return producer();
    }
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .env("ARA_SHARED_PRODUCER", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn producer");
    let mut lines = BufReader::new(child.stdout.take().expect("producer stdout")).lines();
    let mut next = || lines.next().expect("producer line").expect("utf-8 line");
    let desc: ara_gpu::shared::SurfaceDesc = serde_json::from_str(&next()).expect("desc");
    let frame: ara_gpu::shared::Frame = serde_json::from_str(&next()).expect("frame");
    assert_eq!(
        desc.process,
        child.id(),
        "slots belong to the producer process"
    );
    let shared = ara_gpu::shared::SharedSurface::duplicate_from(&child, &desc).expect("lookup");
    let gpu =
        ara_gpu::metal::MetalDevice::new_raster(&[], std::env::temp_dir()).expect("owner device");
    // SAFETY: the device outlives the import, this thread owns its queue, and the
    // producer published `frame` before printing it.
    let mut imported = unsafe { ara_gpu::metal::external::ImportedSurface::create(&gpu, &shared) }
        .expect("import");
    // SAFETY: as above.
    unsafe { imported.exchange(Some(frame.slot), None) }.expect("acquire");
    // SAFETY: as above.
    let pixels = unsafe { imported.read_pixels(&gpu, frame.slot) }.expect("read");
    let words: &[u32] = bytemuck::cast_slice(&pixels);
    assert!(words.iter().all(|&p| p == 0xff00_ff00), "{:08x}", words[0]);
    // SAFETY: the readback completed.
    unsafe { imported.exchange(None, Some(frame.slot)) }.expect("release");
    let mut stdin = child.stdin.take().expect("producer stdin");
    writeln!(stdin, "done").expect("signal producer");
    assert!(child.wait().expect("producer exit").success());
    println!(
        "metal_shared_process: imported a {}x{} frame from process {}",
        desc.width, desc.height, desc.process
    );
}

#[cfg(target_os = "macos")]
fn producer() {
    use ara_gpu::commands::{Attachment, Command};
    use ara_gpu::{Queue, vk};
    let mut gpu = ara_gpu::metal::MetalDevice::new_raster(&[], std::env::temp_dir())
        .expect("producer device");
    let size = [24, 12];
    // SAFETY: the device outlives the ring; this thread owns its queue.
    let mut ring =
        unsafe { ara_gpu::metal::external::ExportRing::create(&gpu, size) }.expect("ring");
    let source = gpu
        .create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("source");
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: source,
            clear: Some([0., 1., 0., 1.]),
        }],
        depth: None,
        size,
    });
    rec.push(Command::EndRendering);
    gpu.submit(rec.finish(), &[]).expect("clear");
    // SAFETY: `source` was cleared and submitted above on this device.
    let frame = unsafe { ring.publish_image(&gpu, source, size, 1) }
        .expect("publish")
        .expect("free slot");
    println!(
        "{}",
        serde_json::to_string(&ring.desc()).expect("desc json")
    );
    println!("{}", serde_json::to_string(&frame).expect("frame json"));
    // Keep the surfaces alive until the owner has imported and read them.
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).expect("owner signal");
    drop(ring);
}
