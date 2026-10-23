use ara_asset::ImportOutput;
#[cfg(windows)]
pub fn peak_memory_bytes() -> Result<usize, std::io::Error> {
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    counters.cb = size;
    // SAFETY: The pseudo handle names this process, and counters is writable for
    // exactly cb bytes. The API takes no ownership and retains no pointers.
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(counters.PeakWorkingSetSize)
}
#[cfg(not(windows))]
pub fn peak_memory_bytes() -> Result<usize, std::io::Error> {
    Ok(0)
}
// Independent BC7 decoder; RGB source-code values, excluding alpha. Kept outside
// timed import and memory measurement so validation work cannot distort either.
pub fn quality(
    source: &[u8],
    output: &ImportOutput,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let gltf = gltf::Gltf::from_slice(source)?;
    let mut results = serde_json::Map::new();
    for image in gltf.images() {
        let gltf::image::Source::View { view, .. } = image.source() else {
            continue;
        };
        let blob = gltf.blob.as_ref().ok_or("expected GLB binary chunk")?;
        let reference =
            image::load_from_memory(&blob[view.offset()..view.offset() + view.length()])?
                .into_rgba8();
        for usage in ["Color", "Linear"] {
            let name = format!("textures/{}-{usage}.ktx2", image.index());
            let Some(artifact) = output.artifacts.iter().find(|a| a.name == name) else {
                continue;
            };
            let texture = ara_asset_format::Texture::read(&artifact.bytes)?;
            let [w, h] = texture.dimensions();
            let mut decoded = vec![0; w as usize * h as usize];
            texture2ddecoder::decode_bc7(
                texture.levels().next().ok_or("missing mip")?,
                w as usize,
                h as usize,
                &mut decoded,
            )
            .map_err(std::io::Error::other)?;
            let mut square = 0.0;
            for (p, d) in reference.pixels().zip(decoded.iter()) {
                let c = d.to_le_bytes();
                for (a, b) in p.0[..3].iter().zip([c[2], c[1], c[0]]) {
                    square += (*a as f64 - b as f64).powi(2);
                }
            }
            let psnr = 10.0 * (255.0f64.powi(2) / (square / (decoded.len() * 3) as f64)).log10();
            results.insert(name, serde_json::json!(psnr));
        }
    }
    Ok(serde_json::Value::Object(results))
}

#[cfg(windows)]
pub fn process_cpu_ms() -> Result<f64, std::io::Error> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: All four pointers reference writable FILETIMEs, and the pseudo
    // handle names this process. No ownership is transferred or pointer retained.
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Ok((ticks(kernel) + ticks(user)) as f64 / 10000.0)
}
#[cfg(not(windows))]
pub fn process_cpu_ms() -> Result<f64, std::io::Error> {
    Ok(0.0)
}
