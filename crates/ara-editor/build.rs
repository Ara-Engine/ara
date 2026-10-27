//! Embeds the Ara engine icon (`assets/brand/icons/engine/ara-engine.ico`) into the Windows executable.
//! Uses `rc.exe` from the Windows SDK that MSVC linking already requires; a no-op elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/icons/engine/ara-engine.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/icons/engine/ara-engine.ico");
    embed_resource::compile(
        "../../assets/brand/icons/engine/ara-engine.rc",
        embed_resource::NONE,
    )
    .manifest_optional()
    .expect("embed the Ara icon resource");
}
