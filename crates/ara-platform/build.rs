fn main() {
    println!("cargo:rerun-if-env-changed=SDL3_LIB_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let dir = std::env::var("SDL3_LIB_DIR")
            .unwrap_or_else(|_| format!("{}/native", env!("CARGO_MANIFEST_DIR")));
        println!("cargo:rustc-link-search=native={dir}");
        // Link explicitly: ara-audio enables sdl3-sys `no-link`, and workspace feature
        // unification would otherwise drop sdl3-sys's own link directive.
        println!("cargo:rustc-link-lib=dylib=SDL3");
    }
}
