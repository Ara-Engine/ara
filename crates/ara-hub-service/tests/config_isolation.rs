#[path = "../../ara-update/tests/config_isolation.rs"]
mod harness;

#[test]
fn config_resolver_uses_the_isolated_harness_directory() {
    let expected = std::path::PathBuf::from(
        std::env::var_os("ARA_CONFIG_DIR").expect("tests require ARA_CONFIG_DIR under target/"),
    );
    std::fs::create_dir_all(&expected).expect("create the scratch ARA_CONFIG_DIR");
    assert_eq!(
        ara_hub_service::config_dir()
            .expect("isolated config")
            .canonicalize()
            .expect("existing scratch directory"),
        expected
            .canonicalize()
            .expect("configured scratch directory")
    );
}
