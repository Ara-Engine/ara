#![allow(clippy::unwrap_used)] // Pure captured invocations; no process is launched.
use super::*;

#[test]
fn new_and_open_keep_literal_unicode_argv_and_the_same_config() {
    let root = std::path::absolute("target/Hub handoff/مرحبا engine").unwrap();
    let editor = root.join(format!("ara-editor{}", std::env::consts::EXE_SUFFIX));
    let profile = root.join("isolated user profile");
    let project = root.join("玩家 Project");
    for new_project in [false, true] {
        let mut request = LaunchRequest::build(&editor, &profile, &project, new_project).unwrap();
        request
            .execute(true, &mut |_| {
                panic!("suppressed Hub handoff must not launch")
            })
            .unwrap();
        let value = json!(request);
        let mut expected = vec![
            "--config-dir".to_owned(),
            profile.to_str().unwrap().to_owned(),
            "--ara".to_owned(),
            root.join(format!("ara{}", std::env::consts::EXE_SUFFIX))
                .to_str()
                .unwrap()
                .to_owned(),
        ];
        if new_project {
            expected.push("--new-project".into());
        }
        assert_eq!(value["args"], json!(expected));
        assert_eq!(value["cwd"], project.to_str().unwrap());
        assert_eq!(
            value["target"],
            if new_project {
                "newProject"
            } else {
                "projects"
            }
        );
        assert_eq!(value["launchSuppressed"], true);
        assert_eq!(value["attempted"], false);
        assert_eq!(value["outcome"], "suppressed");
    }
}

#[test]
fn missing_companions_are_reported_without_falling_back_to_an_unrelated_hub() {
    let root = std::path::absolute(crate::test_target())
        .unwrap()
        .join(format!("missing-hub-companion-{}", std::process::id()));
    let editor = root.join(format!("ara-editor{}", std::env::consts::EXE_SUFFIX));
    let mut request =
        LaunchRequest::build(&editor, &root.join("profile"), &root.join("project"), true).unwrap();
    assert!(
        request
            .execute(false, &mut |_| panic!("missing companion must not launch"))
            .is_err()
    );
    assert_eq!(request.outcome, "missing-companion");
    assert!(!request.attempted);
    assert!(request.error.is_some());
}
