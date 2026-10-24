//! Embeds the Ara icon (`assets/brand/ara.ico`) and makes every DLL import of the
//! setup executable load from System32 only, so a DLL planted beside a downloaded
//! setup is never picked up. Uses `rc.exe` from the Windows SDK; a no-op elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/ara.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/ara.ico");
    embed_resource::compile("../../assets/brand/ara.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embed the Ara icon resource");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // LOAD_LIBRARY_SEARCH_SYSTEM32.
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}
