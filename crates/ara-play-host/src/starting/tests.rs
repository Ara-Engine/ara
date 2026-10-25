use super::*;
use std::sync::mpsc;

fn held_initialization(mut input: Input, reporter: &Reporter) -> Outcome {
    let _ = reporter.report("assets", 3, 17);
    let _ = input
        .service
        .assets_init
        .take()
        .expect("held asset initializer")
        .recv_timeout(Duration::from_secs(5));
    let result = reporter
        .check()
        .map(|()| Value::Null)
        .map_err(|message| fail(codes::PLAY, message));
    Outcome {
        service: input.service,
        result,
        catalog: None,
    }
}

#[test]
#[allow(clippy::unwrap_used)] // Fixture setup and assertions stay at their call sites.
fn status_reports_start_progress_and_stop_never_joins_pending_initialization() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-play-host/background-start");
    let config = PlayConfig::for_project(&root);
    let mut service = PlayService::new(&root, config.clone());
    let mut candidate = PlayService::new(&root, config);
    let (release, gate) = mpsc::channel();
    candidate.assets_init = Some(gate);
    let mut worker = Worker::spawn(Instant::now(), held_initialization).unwrap();
    worker
        .send(Input {
            service: candidate,
            pending: Pending {
                id: "play:1".into(),
                scene: "sc_pending".into(),
                number: 1,
                began: Instant::now(),
                request: serde_json::from_value(json!({"session":"test","background":true}))
                    .unwrap(),
                workspace: ara_doc::Workspace::default(),
                revision: 0,
                cancelled: false,
            },
        })
        .map_err(|_| "startup input rejected")
        .unwrap();
    service.starter = Some(Active {
        id: "play:1".into(),
        scene: "sc_pending".into(),
        worker,
    });
    let began = Instant::now();
    loop {
        let status = service.status();
        let row = &status["worlds"][0];
        if row["progress"]["phase"] == "assets" {
            assert_eq!(row["status"], "starting");
            assert_eq!(row["progress"]["completed"], 3);
            assert_eq!(row["progress"]["total"], 17);
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(service.stop("play:1").unwrap()["status"], "stopping");
    assert_eq!(service.status()["worlds"][0]["status"], "stopping");
    assert!(
        !service.starter.as_ref().unwrap().worker.finished(),
        "Stop returned while initialization was still held"
    );
    release.send(Err("fixture released".into())).unwrap();
    let outcome = service
        .starter
        .take()
        .unwrap()
        .worker
        .finish()
        .unwrap()
        .unwrap();
    assert!(outcome.result.is_err());
    assert!(outcome.service.worlds.is_empty());
}
