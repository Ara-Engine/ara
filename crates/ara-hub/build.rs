//! Embeds the Ara app icon (`assets/brand/ara.ico`) into the Windows executable.
//! Uses `rc.exe` from the Windows SDK that MSVC linking already requires; a no-op elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/ara.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/ara.ico");
    embed_resource::compile("../../assets/brand/ara.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embed the Ara icon resource");
}
