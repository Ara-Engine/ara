#![allow(clippy::unwrap_used)]
use std::process::Command;
#[test]
fn compare_cli_exits_for_pass_regression_and_malformed_input_without_project() {
    let dir = std::env::temp_dir().join(format!("ara-profile-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let baseline = dir.join("baseline.json"); let current = dir.join("current.json");
    let p = ara_profile::Profile { version:1, budget_ns:1000,
        frames: [ara_profile::Frame { id:1,duration_ns:100,..Default::default() }].into(), ..Default::default() };
    let summary = p.summary(Default::default()).unwrap();
    ara_profile::write_summary(&baseline,&summary).unwrap(); ara_profile::write_summary(&current,&summary).unwrap();
    let run = || Command::new(env!("CARGO_BIN_EXE_ara")).args(["profile","compare"]).arg(&baseline).arg(&current).arg("--absolute-ns=0").current_dir(&dir).output().unwrap();
    let result = run(); assert!(result.status.success()); assert!(String::from_utf8_lossy(&result.stdout).contains("PASS"));
    let mut slower = summary; slower.cpu_frame.p95 *= 2;
    ara_profile::write_summary(&current,&slower).unwrap(); let result = run();
    assert_eq!(result.status.code(),Some(1)); assert!(String::from_utf8_lossy(&result.stdout).contains("REGRESSION"));
    std::fs::write(&current,"invalid").unwrap(); assert_eq!(run().status.code(),Some(2));
    std::fs::remove_dir_all(dir).unwrap();
}
