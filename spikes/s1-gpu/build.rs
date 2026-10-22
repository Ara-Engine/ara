fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=tools/SDL3-3.4.18/lib/x64/SDL3.lib");
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo sets the manifest directory");
    println!("cargo:rustc-link-search=native={root}/tools/SDL3-3.4.18/lib/x64");
}
