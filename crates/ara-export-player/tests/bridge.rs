//! Optional smoke test against a real exported C# image and pack.
#![cfg(feature = "dev-bridge")]
#![allow(clippy::unwrap_used)]
use std::{
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires ARA_EXPORT_ARTIFACTS/windows from ara-export end-to-end test"]
fn exported_dev_runtime_serves_authenticated_desktop_observability() {
    let root =
        PathBuf::from(std::env::var_os("ARA_EXPORT_ARTIFACTS").unwrap()).join("windows/build");
    let dir = tempfile::tempdir().unwrap();
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            if e.path().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    copy(&root, dir.path());
    let mut launch: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("ara-launch.json")).unwrap())
            .unwrap();
    launch["config"] = "dev".into();
    std::fs::write(
        dir.path().join("ara-launch.json"),
        serde_json::to_vec(&launch).unwrap(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let token = "temporary-integration-token-32-bytes";
    let mut child = Command::new(env!("CARGO_BIN_EXE_ara-export-smoke"))
        .args(["--headless", "--root"])
        .arg(dir.path())
        .env("ARA_BRIDGE_TOKEN", token)
        .env("ARA_BRIDGE_PORT", address.port().to_string())
        .env("ARA_BRIDGE_WAIT_MS", "1500")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let mut client = loop {
        match ara_bridge::DeviceClient::connect(
            SocketAddr::from(([127, 0, 0, 1], address.port())),
            token.into(),
        ) {
            Ok(c) => break c,
            Err(_) => {
                assert!(started.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    assert_eq!(
        client.request(ara_bridge::Command::Hello).unwrap()["protocol"],
        1
    );
    loop {
        let logs = client.logs(0).unwrap();
        if logs["latest"].as_u64().unwrap() > 0 {
            assert_eq!(logs["logs"][0]["fields"]["verified"], true);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        client
            .screenshot()
            .unwrap_err()
            .to_string()
            .contains("ARA-EXP-BRG-010")
    );
    drop(client);
    assert!(child.wait().unwrap().success());
}
