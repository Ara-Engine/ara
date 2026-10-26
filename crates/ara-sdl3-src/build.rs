//! Compile the committed SDL subset without a build system or SDK downloads.
use std::{env, error::Error, fs, path::Path, time::Instant};

const SDL: &str = "vendor/SDL3-3.4.18";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=SDL3_LIB_DIR");
    if let Some(dir) = env::var_os("SDL3_LIB_DIR") {
        println!(
            "cargo:rustc-link-search=native={}",
            Path::new(&dir).display()
        );
        println!("cargo:rustc-link-lib=dylib=SDL3");
        return Ok(());
    }
    let os = env::var("CARGO_CFG_TARGET_OS")?;
    let dirs: &[&str] = match os.as_str() {
        "windows" => &[
            "core/windows", "main/windows", "io/windows", "misc/windows",
            "video/windows", "thread/windows", "sensor/windows", "power/windows",
            "locale/windows", "filesystem/windows", "time/windows", "timer/windows",
            "loadso/windows", "joystick/windows", "haptic/windows", "dialog/windows",
            "audio/wasapi", "process/windows",
        ],
        "linux" => &[
            "core/linux", "core/unix", "misc/unix", "video/x11", "video/wayland",
            "thread/pthread", "sensor/dummy", "power/linux", "locale/unix",
            "filesystem/unix", "filesystem/posix", "time/unix", "timer/unix",
            "loadso/dlopen", "joystick/linux", "haptic/linux", "dialog/unix",
            "process/posix", "audio/alsa", "audio/pulseaudio", "audio/pipewire",
        ],
        "android" => &[
            "core/android", "misc/android", "video/android", "thread/pthread",
            "sensor/android", "power/android", "locale/android", "filesystem/android",
            "filesystem/posix", "time/unix", "timer/unix", "loadso/dlopen",
            "joystick/android", "haptic/android", "dialog/android", "process/dummy",
            "audio/aaudio", "audio/openslES",
        ],
        "macos" => &[
            "core/unix", "misc/macos", "video/cocoa", "thread/pthread", "sensor/dummy",
            "power/macos", "locale/macos", "filesystem/cocoa", "filesystem/posix",
            "time/unix", "timer/unix", "loadso/dlopen", "joystick/darwin",
            "joystick/apple", "haptic/darwin", "dialog/cocoa", "process/posix",
            "audio/coreaudio",
        ],
        "ios" => return Err("TODO(D15): add iOS SDL cc sources/config (M4 export); SDL3_LIB_DIR can select a prebuilt SDL meanwhile".into()),
        _ => return Err(format!("SDL cc build does not support {os}; use SDL3_LIB_DIR").into()),
    };
    if os == "windows" && env::var("CARGO_CFG_TARGET_ENV")? != "msvc" {
        return Err("SDL Windows build currently requires the MSVC ABI".into());
    }
    println!("cargo:rerun-if-changed=config");
    for input in ["src", "include"] {
        println!("cargo:rerun-if-changed={SDL}/{input}");
    }
    if os == "linux" {
        println!("cargo:rerun-if-changed=vendor/linux/include");
        println!("cargo:rerun-if-changed=vendor/wayland-generated");
    }
    let mut build = cc::Build::new();
    build
        .include("config")
        .include(format!("{SDL}/include"))
        .include(format!("{SDL}/src"))
        .include(format!("{SDL}/src/video/khronos"))
        .define("SDL_STATIC_LIB", None)
        .define("SDL_VENDOR_INFO", "\"Ara\"")
        .warnings(false)
        .std("c11")
        .opt_level(3);
    if os == "windows" {
        build.flag("/utf-8").define("_CRT_SECURE_NO_WARNINGS", None);
    } else {
        build
            .flag("-fno-strict-aliasing")
            .define("_GNU_SOURCE", None);
    }
    // Apple backends are Objective-C with ARC; they share every C flag above and
    // land in the same archive.
    let mut objc = build.clone();
    objc.flag("-fobjc-arc");
    let mut objc_files = Vec::new();
    for dir in [
        "",
        "atomic",
        "audio",
        "camera",
        "core",
        "cpuinfo",
        "dynapi",
        "events",
        "io",
        "io/generic",
        "filesystem",
        "gpu",
        "joystick",
        "haptic",
        "hidapi",
        "locale",
        "main",
        "main/generic",
        "misc",
        "power",
        "process",
        "render",
        "sensor",
        "stdlib",
        "storage",
        "storage/generic",
        "thread",
        "time",
        "timer",
        "video",
        "video/yuv2rgb",
        "joystick/hidapi",
        "joystick/virtual",
        "haptic/hidapi",
        "tray",
        "tray/dummy",
        "dialog",
        "libm",
    ]
    .iter()
    .chain(dirs.iter())
    {
        add_sources(
            &mut build,
            &mut objc_files,
            &Path::new(SDL).join("src").join(dir),
        )?;
    }
    if os == "windows" {
        build.file(format!("{SDL}/src/thread/generic/SDL_syscond.c"));
        build.file(format!("{SDL}/src/thread/generic/SDL_sysrwlock.c"));
    } else if os == "linux" {
        build.include("vendor/wayland-generated");
        for include in ["", "dbus-1.0", "pipewire-0.3", "spa-0.2", "libdecor-0"] {
            build.include(Path::new("vendor/linux/include").join(include));
        }
        add_sources(
            &mut build,
            &mut objc_files,
            Path::new("vendor/wayland-generated"),
        )?;
    }
    let start = Instant::now();
    if !objc_files.is_empty() {
        objc.files(objc_files);
        build.objects(objc.compile_intermediates());
    }
    build.compile("ara_sdl3");
    if os == "windows" {
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .warnings(false)
            .opt_level(3)
            .flag("/utf-8")
            .define("_CRT_SECURE_NO_WARNINGS", None)
            .define("SDL_STATIC_LIB", None)
            .include("config")
            .include(format!("{SDL}/include"))
            .include(format!("{SDL}/src"))
            .file(format!("{SDL}/src/core/windows/SDL_gameinput.cpp"))
            .file(format!("{SDL}/src/video/windows/SDL_windowsgameinput.cpp"))
            .compile("ara_sdl3_cpp");
    }
    if os == "android" {
        let mut hid = cc::Build::new();
        hid.cpp(true)
            .std("c++11")
            .warnings(false)
            .opt_level(3)
            .include("config")
            .include(format!("{SDL}/include"))
            .include(format!("{SDL}/src"))
            .define("SDL_STATIC_LIB", None)
            .file(format!("{SDL}/src/hidapi/android/hid.cpp"))
            .compile("ara_sdl3_hid");
    }
    eprintln!(
        "SDL 3.4.18 static compilation: {:.2}s (NUM_JOBS={})",
        start.elapsed().as_secs_f64(),
        env::var("NUM_JOBS")?
    );
    let libs: &[&str] = match os.as_str() {
        "windows" => &[
            "kernel32", "user32", "gdi32", "winmm", "imm32", "ole32", "oleaut32", "version",
            "uuid", "advapi32", "setupapi", "shell32",
        ],
        "linux" => &["dl", "pthread", "m", "rt"],
        "android" => &["dl", "log", "android", "OpenSLES"],
        _ => &[],
    };
    for lib in libs {
        println!("cargo:rustc-link-lib={lib}");
    }
    if os == "macos" {
        // System frameworks used by the Cocoa, CoreAudio, IOKit/GameController,
        // haptics, dialog and Metal-view backends; all ship with macOS.
        for framework in [
            "AppKit", "AudioToolbox", "AVFoundation", "Carbon", "Cocoa", "CoreAudio",
            "CoreFoundation", "CoreHaptics", "CoreVideo", "ForceFeedback", "Foundation",
            "GameController", "IOKit", "Metal", "QuartzCore", "UniformTypeIdentifiers",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
    Ok(())
}

/// Add only this source directory; platform backends are selected explicitly above.
/// C files go to `build`; Objective-C (`.m`) files are collected for the ARC build.
fn add_sources(
    build: &mut cc::Build,
    objc: &mut Vec<std::path::PathBuf>,
    dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut files = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    for path in files {
        if path.extension().is_some_and(|ext| ext == "c")
            && path.file_name().is_some_and(|name| name != "pch.c")
        {
            build.file(path);
        } else if path.extension().is_some_and(|ext| ext == "m") {
            objc.push(path);
        }
    }
    Ok(())
}
