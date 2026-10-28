//! Captured argv and policy only: no test invokes the process launcher.
use super::*;

fn paths() -> (PathBuf, PathBuf) {
    let root = std::path::absolute("Ara IDE proof/مرحبا project").unwrap();
    let file = root.join("content/Player Scripts/玩家 Controller.cs");
    (root, file)
}

#[test]
fn configured_code_location_preserves_absolute_unicode_argv_exactly() {
    let (root, file) = paths();
    let configured = root.join("Microsoft VS Code/Code.exe");
    let configured = configured.to_str().unwrap();
    let mut request = IdeRequest::build(&root, &file, Some((37, 12)), configured).unwrap();
    request
        .execute(LaunchPolicy::new(true, true, false), &mut |_| {
            panic!("offscreen IDE request must not launch")
        })
        .unwrap();
    let report = json!(request);
    assert_eq!(report["program"], configured);
    assert_eq!(report["targetFile"], file.to_str().unwrap());
    assert_eq!(report["cwd"], root.to_str().unwrap());
    assert_eq!(
        report["args"],
        json!([
            "--reuse-window",
            "--goto",
            format!("{}:37:12", file.to_str().unwrap())
        ])
    );
    assert_eq!(report["line"], 37);
    assert_eq!(report["column"], 12);
    assert_eq!(report["lineHandling"], "vscode-goto");
    assert_eq!(report["launchSuppressed"], true);
    assert_eq!(report["attempted"], false);
    assert_eq!(report["outcome"], "suppressed");
    assert_eq!(report["suppressionReason"], "offscreen");
    assert!(report["fallback"].is_null());
}

#[test]
fn system_association_keeps_requested_coordinates_but_does_not_claim_delivery() {
    let (root, file) = paths();
    let mut request = IdeRequest::build(&root, &file, Some((9, 4)), "system").unwrap();
    let mut captured = Vec::new();
    request
        .execute(LaunchPolicy::new(false, true, false), &mut |invocation| {
            captured.push((invocation.program.clone(), invocation.args.clone()));
            Ok(())
        })
        .unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].1, [file.to_str().unwrap()]);
    assert_eq!(request.line, Some(9));
    assert_eq!(request.column, Some(4));
    assert_eq!(request.line_handling, "unsupported-system-association");
    assert!(request.attempted);
    assert_eq!(request.outcome, "launched");
    assert!(!request.launch_suppressed);
}

#[test]
fn disabled_and_hidden_module_openers_build_full_request_without_launching() {
    let (root, _) = paths();
    let module = root.join("content/Editor Tools/编译 Module");
    for (policy, reason) in [
        (LaunchPolicy::new(true, true, false), "offscreen"),
        (
            LaunchPolicy::new(false, false, false),
            "disabled-for-session",
        ),
        (
            LaunchPolicy::new(false, true, true),
            "offscreen-environment",
        ),
    ] {
        let mut request = IdeRequest::build(&root, &module, None, "").unwrap();
        assert_eq!(request.outcome, "prepared");
        request
            .execute(policy, &mut |_| panic!("suppressed module must not launch"))
            .unwrap();
        let report = json!(request);
        assert_eq!(report["targetFile"], module.to_str().unwrap());
        assert_eq!(
            report["args"],
            json!([
                "--reuse-window",
                root.to_str().unwrap(),
                module.to_str().unwrap()
            ])
        );
        assert_eq!(report["cwd"], root.to_str().unwrap());
        assert!(report["line"].is_null());
        assert!(report["column"].is_null());
        assert_eq!(report["lineHandling"], "not-requested");
        assert_eq!(report["suppressionReason"], reason);
        assert_eq!(report["attempted"], false);
        assert_eq!(report["fallback"]["attempted"], false);
        assert_eq!(report["fallback"]["launchSuppressed"], true);
    }
}

#[test]
fn default_code_failure_records_the_actual_system_fallback_separately() {
    let (root, file) = paths();
    let mut request = IdeRequest::build(&root, &file, Some((0, 0)), "").unwrap();
    let mut captured = Vec::new();
    request
        .execute(LaunchPolicy::new(false, true, false), &mut |invocation| {
            captured.push((invocation.program.clone(), invocation.args.clone()));
            if captured.len() == 1 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Code not installed",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0].1,
        vec![
            "--reuse-window".to_owned(),
            "--goto".to_owned(),
            format!("{}:1:1", file.to_str().unwrap())
        ]
    );
    assert_eq!(captured[1].1, [file.to_str().unwrap()]);
    assert_eq!(request.outcome, "fallback-launched");
    assert_eq!(request.error.as_deref(), Some("Code not installed"));
    let fallback = request.fallback.as_ref().unwrap();
    assert!(fallback.attempted);
    assert_eq!(fallback.line_handling, "unsupported-system-association");
    assert_eq!(fallback.outcome, "launched");
}

#[test]
fn arbitrary_configured_editor_is_literal_and_never_claims_a_goto_protocol() {
    let (root, file) = paths();
    let configured = root.join("Custom Tools/Editor & Review.exe");
    let configured = configured.to_str().unwrap();
    let mut request = IdeRequest::build(&root, &file, Some((2, 3)), configured).unwrap();
    request
        .execute(LaunchPolicy::new(false, false, false), &mut |_| {
            panic!("disabled custom editor must not launch")
        })
        .unwrap();
    assert_eq!(request.program, configured);
    assert_eq!(request.args, [file.to_str().unwrap()]);
    assert_eq!(request.line_handling, "unsupported-configured-editor");
    assert!(!is_vscode("codex.exe"));
    assert!(!is_vscode("Code Review.exe"));
    assert!(is_vscode("C:\\Program Files\\Microsoft VS Code\\Code.exe"));
}

#[test]
fn request_rejects_relative_paths_before_any_launch_policy_is_applied() {
    let (root, file) = paths();
    assert!(IdeRequest::build(Path::new("relative"), &file, None, "").is_err());
    assert!(IdeRequest::build(&root, Path::new("relative.cs"), None, "").is_err());
}
