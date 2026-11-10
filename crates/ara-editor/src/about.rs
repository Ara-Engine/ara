//! What the About and System Info windows say: the editor's version and build, the
//! platform, and the renderer's device (identity, features, limits), read from its
//! `DeviceInfo` report.
use serde_json::Value;

/// The renderer's pretty-printed `DeviceInfo` JSON, once a device exists.
pub(crate) fn parse_device(device: &str) -> Option<Value> {
    serde_json::from_str::<Value>(device)
        .ok()
        .filter(Value::is_object)
}

/// "Vulkan 1.4.351"; Metal devices already name their API ("Metal 4 (…)").
fn graphics_api(info: &Value) -> String {
    let api = info["api_version"].as_str().unwrap_or("");
    if api.starts_with("Metal") {
        api.to_owned()
    } else {
        format!("Vulkan {api}")
    }
}

/// "11.7 GB", "4 GB" or "512 MB".
fn memory(mib: f64) -> String {
    let gb = mib / 1024.;
    if gb < 1. {
        format!("{mib:.0} MB")
    } else if (gb - gb.round()).abs() < 0.05 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

/// "65,536".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The About card's detail rows; Copy puts the same rows on the clipboard.
pub(crate) fn about_details(device: &str) -> Vec<(&'static str, String)> {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "ARM64",
        other => other,
    };
    let mut rows = vec![
        ("Version", env!("CARGO_PKG_VERSION").to_owned()),
        (
            "Build",
            if cfg!(debug_assertions) {
                "Development (debug)"
            } else {
                "Release"
            }
            .to_owned(),
        ),
        ("Platform", format!("{os} {arch}")),
    ];
    match parse_device(device) {
        Some(info) => {
            rows.push(("GPU", info["name"].as_str().unwrap_or("Unknown").to_owned()));
            rows.push(("Graphics API", graphics_api(&info)));
        }
        None => rows.push(("GPU", "Not started yet".into())),
    }
    rows
}

/// One System info row: label, value, and whether it reads as available.
pub(crate) type Detail = (&'static str, String, bool);

/// What people compare between machines: identity, features and limits. Queue
/// families, cache UUIDs and the rest stay in the copied full report.
pub(crate) fn system_sections(info: &Value) -> [(&'static str, &'static str, Vec<Detail>); 3] {
    let number = |key: &str| info[key].as_u64().unwrap_or(0);
    let on = |key: &str| info[key].as_bool().unwrap_or(false);
    let vendor_id = number("vendor_id");
    let mut device = vec![(
        "GPU",
        info["name"].as_str().unwrap_or("Unknown").to_owned(),
        true,
    )];
    let vendor = match vendor_id {
        0 => None,
        0x10DE => Some("NVIDIA".to_owned()),
        0x1002 => Some("AMD".to_owned()),
        0x8086 => Some("Intel".to_owned()),
        0x106B => Some("Apple".to_owned()),
        0x13B5 => Some("Arm".to_owned()),
        0x5143 => Some("Qualcomm".to_owned()),
        id => Some(format!("0x{id:04X}")),
    };
    if let Some(vendor) = vendor {
        device.push(("Vendor", vendor, true));
    }
    let kind = if on("discrete") {
        "Discrete"
    } else if on("unified_device_local") {
        "Integrated, unified memory"
    } else {
        "Integrated"
    };
    device.push(("Type", kind.into(), true));
    device.push((
        "Video memory",
        memory(number("device_local_mib") as f64),
        true,
    ));
    device.push(("Graphics API", graphics_api(info), true));
    // Vendors pack driver versions differently; NVIDIA reads 617.14, not 617.3.0.
    let driver = number("driver_version") as u32;
    if driver != 0 {
        let version = match vendor_id {
            0x10DE => format!("{}.{:02}", driver >> 22, (driver >> 14) & 0xff),
            0x8086 if cfg!(windows) => format!("{}.{}", driver >> 14, driver & 0x3fff),
            _ => format!(
                "{}.{}.{}",
                driver >> 22,
                (driver >> 12) & 0x3ff,
                driver & 0xfff
            ),
        };
        device.push(("Driver", version, true));
    }
    let features = [
        ("Ray tracing", "ray_tracing"),
        ("Mesh shaders", "mesh_shading"),
        ("Wave operations", "wave_ops"),
        ("64-bit atomics", "buffer_int64_atomics"),
        ("BC textures", "texture_compression_bc"),
        ("HDR output", "hdr_metadata"),
    ]
    .into_iter()
    .map(|(label, key)| {
        let supported = on(key);
        let value = if supported {
            "Supported"
        } else {
            "Not available"
        };
        (label, value.to_owned(), supported)
    })
    .collect();
    let limits = vec![
        (
            "Max texture size",
            format!("{} px", grouped(number("max_image_dimension"))),
            true,
        ),
        (
            "Texture slots",
            grouped(number("sampled_image_capacity")),
            true,
        ),
        (
            "Buffer slots",
            grouped(number("storage_buffer_capacity")),
            true,
        ),
        (
            "Largest buffer",
            memory(number("max_storage_buffer_range") as f64 / 1048576.),
            true,
        ),
    ];
    [
        ("system-device", "Device", device),
        ("system-features", "Features", features),
        ("system-limits", "Limits", limits),
    ]
}
