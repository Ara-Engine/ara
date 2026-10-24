//! Shared harness guard: all test invocations must explicitly isolate the profile.
#[test]
fn test_config_is_explicit_and_under_target() {
    let directory = std::env::var_os("ARA_CONFIG_DIR").expect(
        "set ARA_CONFIG_DIR to an existing scratch directory under target/ before running tests",
    );
    let directory = std::path::PathBuf::from(directory)
        .canonicalize()
        .expect("create the scratch ARA_CONFIG_DIR before running tests");
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
