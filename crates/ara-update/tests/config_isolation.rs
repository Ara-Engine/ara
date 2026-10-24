//! Shared harness guard: all test invocations must explicitly isolate the profile.
#[test]
fn test_config_is_explicit_and_under_target() {
    // `.cargo/config.toml` sets ARA_CONFIG_DIR to target/ara-config for every
    // `cargo test`; create it on first use.
    let directory = std::env::var_os("ARA_CONFIG_DIR")
        .expect("ARA_CONFIG_DIR comes from .cargo/config.toml or the caller");
    std::fs::create_dir_all(&directory).expect("create the scratch ARA_CONFIG_DIR");
    let directory = std::path::PathBuf::from(directory)
        .canonicalize()
        .expect("scratch ARA_CONFIG_DIR exists");
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()
        .expect("workspace target");
    assert!(
        directory.starts_with(&target),
        "tests must resolve ARA_CONFIG_DIR under target/: {}",
        directory.display()
    );
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let profile = std::path::PathBuf::from(appdata).join("Ara");
        if let Ok(profile) = profile.canonicalize() {
            assert!(
                !directory.starts_with(profile),
                "test resolved the real %APPDATA%/Ara"
            );
        }
    }
}
