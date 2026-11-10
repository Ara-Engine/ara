//! Checkout SDK discovery must not depend on Cargo's executable directory.
mod support;

use ara_script::project::{self, ToolPaths};
use std::{path::PathBuf, process::Command};

#[test]
fn external_executables_find_source_tools_but_packaged_engines_keep_their_boundary() {
    const CASE: &str = "ARA_TOOL_PATHS_CASE";
    if let Some(case) = std::env::var_os(CASE) {
        let project = PathBuf::from(std::env::var_os("ARA_TOOL_PATHS_PROJECT").expect("project"));
        let tools = ToolPaths::locate(&project);
        if case == "packaged" {
            assert!(tools.is_none(), "packaged SDK cannot borrow checkout tools");
            let error = project::inventory(&project).expect_err("missing packaged tools");
            assert_eq!(error.code, ara_script::codes::COMPILE_SERVER);
            assert!(error.message.contains("ARA_DOTNET_ARTIFACTS"));
            assert!(error.message.contains("artifacts/bin"));
        } else {
            let tools = tools.expect("source, override or installed tools");
            let expected =
                PathBuf::from(std::env::var_os("ARA_TOOL_PATHS_EXPECTED").expect("expected"));
            assert_eq!(
                tools.compile_server,
                expected
                    .canonicalize()
                    .expect("canonical bin")
                    .join("Ara.CompileServer/release/Ara.CompileServer.dll")
            );
        }
        return;
    }

    support::build();
    let root = std::env::temp_dir().join(format!("ara-tool-paths-{}", std::process::id()));
    let source_bin = support::dotnet().join("artifacts/bin");
    for case in ["checkout", "packaged", "override", "installed"] {
        let directory = root.join(case);
        let project = directory.join("project");
        let authored = project.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(&authored).expect("external project");
        std::fs::write(authored.join("Script.cs"), "class Script { }").expect("source");
        let exe = directory.join(if cfg!(windows) { "probe.exe" } else { "probe" });
        std::fs::copy(std::env::current_exe().expect("test executable"), &exe)
            .expect("external executable");
        if case != "checkout" {
            std::fs::write(directory.join("engine.json"), "{}").expect("packaged marker");
        }
        let installed = directory.join("dotnet/artifacts/bin");
        if case == "installed" {
            for file in [
                "Ara.Host/release/Ara.Host.dll",
                "Ara.CompileServer/release/Ara.CompileServer.dll",
            ] {
                let destination = installed.join(file);
                std::fs::create_dir_all(destination.parent().expect("tool directory"))
                    .expect("installed tool directory");
                std::fs::copy(source_bin.join(file), destination).expect("installed tool");
            }
        }
        let mut child = Command::new(exe);
        child
            .args([
                "--exact",
                "external_executables_find_source_tools_but_packaged_engines_keep_their_boundary",
                "--nocapture",
            ])
            .current_dir(&directory)
            .env(CASE, case)
            .env("ARA_TOOL_PATHS_PROJECT", &project)
            .env(
                "ARA_TOOL_PATHS_EXPECTED",
                if case == "installed" {
                    &installed
                } else {
                    &source_bin
                },
            )
            .env_remove("ARA_DOTNET_ARTIFACTS");
        if matches!(case, "override" | "installed") {
            child.env("ARA_DOTNET_ARTIFACTS", &source_bin);
        }
        let output = child.output().expect("external test process");
        assert!(
            output.status.success(),
            "{case}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
