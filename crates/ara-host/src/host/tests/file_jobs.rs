//! Source jobs complete only when the file worker acknowledges their transaction.
use super::*;
use ara_doc::file_state::FileState;

#[test]
fn source_job_waits_for_materialization_and_reports_restore_failures() {
    for changed in [false, true] {
        let dir = project(if changed {
            "source-job-conflict"
        } else {
            "source-job-flush"
        });
        let before = b"class Before {}\n";
        let after = "class After {}\n";
        fs::write(dir.join("content/Source.cs"), before).expect("source");
        let mut host = Host::open(
            Project::open(&dir).expect("project"),
            HostConfig {
                debounce: Duration::from_secs(3600),
                watch_every: Duration::from_secs(3600),
                scan_every: Duration::from_secs(3600),
                ..HostConfig::default()
            },
        )
        .expect("host");
        let session = call(
            &mut host,
            "session.begin",
            json!({"client":"source-job","author":"agent:source-job"}),
        )["session"]
            .clone();
        host.flush();
        let release = host.st.files.io.hold();
        let response = call(
            &mut host,
            "cmd.apply",
            json!({"session":session,"doc":"levels/arena.scene","why":"replace source", "commands":[{"cmd":"source.replace","args":{"path":"Source.cs","baseHash":FileState::bytes(before).hash,"text":after}}]}),
        );
        let job = response["job"].as_str().expect("preparation job");
        let deadline = Instant::now() + Duration::from_secs(30);
        while host.store().workspace().file_state("Source.cs")
            != Some(&FileState::bytes(after.as_bytes()))
        {
            host.tick();
            let status = call(
                &mut host,
                "jobs.status",
                json!({"session":session,"job":job}),
            );
            assert_ne!(status["status"], "failed", "preparation failed: {status}");
            assert!(Instant::now() < deadline, "source never committed");
            std::thread::yield_now();
        }
        let status = call(
            &mut host,
            "jobs.status",
            json!({"session":session,"job":job}),
        );
        assert_eq!(
            status["status"], "running",
            "a committed intent is not a completed file write: {status}"
        );
        if changed {
            fs::write(dir.join("content/Source.cs"), b"external change\n")
                .expect("external edit after commit");
        }
        drop(release);
        loop {
            host.tick();
            let status = call(
                &mut host,
                "jobs.status",
                json!({"session":session,"job":job}),
            );
            if status["status"] != "running" {
                if changed {
                    assert_eq!(status["status"], "failed", "{status}");
                    assert!(
                        !status["diagnostics"]
                            .as_array()
                            .expect("diagnostics")
                            .is_empty()
                    );
                    assert_eq!(
                        fs::read(dir.join("content/Source.cs")).expect("external source retained"),
                        b"external change\n"
                    );
                } else {
                    assert_eq!(status["status"], "done", "{status}");
                    assert_eq!(
                        fs::read_to_string(dir.join("content/Source.cs")).expect("durable source"),
                        after
                    );
                }
                break;
            }
            assert!(Instant::now() < deadline, "materialization never completed");
            std::thread::yield_now();
        }
    }
}
