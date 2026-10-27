//! Embeds the Ara game icon (`assets/brand/icons/game/ara-game.ico`) into the Windows executable.
//! Uses `rc.exe` from the Windows SDK that MSVC linking already requires; a no-op elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/icons/game/ara-game.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/icons/game/ara-game.ico");
    embed_resource::compile(
        "../../assets/brand/icons/game/ara-game.rc",
        embed_resource::NONE,
    )
    .manifest_optional()
    .expect("embed the Ara icon resource");
}
