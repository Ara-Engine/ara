//! Embeds the Ara game icon (`assets/brand/icons/game/ara-game.ico`) into the Windows executable.
//! Uses `rc.exe` from the Windows SDK that MSVC linking already requires; a no-op elsewhere.
//! Also reserves an 8 MB main-thread stack on Windows (see below).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/icons/game/ara-game.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/icons/game/ara-game.ico");
    embed_resource::compile(
        "../../assets/brand/icons/game/ara-game.rc",
        embed_resource::NONE,
    )
    .manifest_optional()
    .expect("embed the Ara icon resource");
    // Optimized startup frames (run, standalone::run, startup::load) take ~0.9 MB, and the
    // driver's Vulkan instance creation then overran Windows' default 1 MB main stack.
    // Reserve 8 MB like macOS/Linux; pages are committed only as they are used. Exported
    // games are this executable, so they inherit the reserve.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
}
