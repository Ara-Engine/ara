//! Headless interaction and CPU frame measurements through the native Hub App.
use ara_hub::{
    capture,
    connection::Event,
    model::{Call, Hub, Intent, Prefs},
    native::App,
};
use ara_ui::input::InputEvent;
use serde_json::json;
use std::time::Instant;

fn setup() -> (App, Hub) {
    let mut hub = Hub::new(Prefs::default(), "target/hub-tests".into(), None);
    capture::seed(&mut hub);
    let mut app = App::new([1440., 900.], Vec::new()).expect("app");
    app.rebuild(&mut hub, capture::NOW).expect("initial frame");
    (app, hub)
}

fn center(app: &App, id: &str) -> [f32; 2] {
    let r = app.ui.nodes[app.ui.find(id).expect(id)].rect;
    [r.x + r.w / 2., r.y + r.h / 2.]
}

fn status_snapshot(hub: &Hub) -> serde_json::Value {
    json!({"projects":hub.status.values().map(|s| json!({
        "project":{"id":s.project.id,"name":s.project.name,"path":s.project.path,"lastOpened":s.project.last_opened,"thumbnail":s.project.thumbnail,"engineVersion":s.project.engine_version},
        "state":s.state,"pid":s.pid,"port":s.port,"headless":s.headless
    })).collect::<Vec<_>>()})
}

#[test]
fn button_release_after_a_service_refresh_activates_on_first_try() {
    let (mut app, mut hub) = setup();
    let at = center(&app, "more-project-3");
    app.pointer(InputEvent::PointerMove(at)).expect("move");
    app.pointer(InputEvent::PointerDown(at)).expect("press");
    // A slow frame lets the refresh land between press and release.
    hub.event(Event::Reply {
        call: Call::Status,
        result: Ok(status_snapshot(&hub)),
    });
    app.rebuild(&mut hub, capture::NOW)
        .expect("refresh while pressed");
    app.pointer(InputEvent::PointerUp(at)).expect("release");
    assert_eq!(app.actions(), [Intent::Menu(Some("project-3".into()))]);
}

#[test]
fn notification_dismiss_lands_on_first_try() {
    let (mut app, mut hub) = setup();
    hub.event(Event::Reply {
        call: Call::Close {
            project: "project-3".into(),
        },
        result: Err(ara_proto::RpcError {
            code: -32000,
            message: "fixture failure".into(),
            data: None,
        }),
    });
    app.rebuild(&mut hub, capture::NOW).expect("toast");
    let id = hub.notices.last().expect("notice").id;
    let at = center(&app, &format!("dismiss-{id}"));
    app.pointer(InputEvent::PointerMove(at)).expect("move");
    app.pointer(InputEvent::PointerDown(at)).expect("press");
    app.pointer(InputEvent::PointerUp(at)).expect("release");
    assert_eq!(app.actions(), [Intent::Dismiss(id)]);
    hub.intent(Intent::Dismiss(id));
    app.rebuild(&mut hub, capture::NOW).expect("dismiss view");
    assert!(app.ui.find(&format!("dismiss-{id}")).is_none());
    assert!(app.frame().expect("present dismissal"));
}

#[test]
fn dismissed_banners_release_their_layout_space() {
    let (mut app, mut hub) = setup();
    hub.event(Event::Offline {
        message: "fixture offline".into(),
        log: "target/hub-tests/missing.log".into(),
    });
    app.rebuild(&mut hub, capture::NOW).expect("banners");
    app.frame().expect("banner frame");
    let before = center(&app, "open-project-3")[1];
    hub.intent(Intent::DismissBanner("offline".into()));
    app.rebuild(&mut hub, capture::NOW)
        .expect("dismiss offline");
    assert!(app.frame().expect("present dismissal"));
    let after = center(&app, "open-project-3")[1];
    assert!(
        after < before - 50.,
        "banner space must collapse: {before} -> {after}"
    );

    let mut fresh = App::new(app.size, Vec::new()).expect("fresh app");
    fresh
        .rebuild(&mut hub, capture::NOW)
        .expect("same final model");
    assert_eq!(
        center(&app, "open-project-3"),
        center(&fresh, "open-project-3"),
        "reconciled layout must match a fresh layout"
    );
}

#[test]
fn queued_menu_and_item_clicks_work_even_after_a_slow_frame() {
    for delay in [
        std::time::Duration::ZERO,
        std::time::Duration::from_millis(100),
    ] {
        let (mut app, mut hub) = setup();
        app.frame().expect("initial present");
        std::thread::sleep(delay);
        let at = center(&app, "more-project-3");
        for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
            app.dispatch_pointer(&mut hub, event, capture::NOW)
                .expect("menu click");
        }
        assert_eq!(hub.menu.as_deref(), Some("project-3"));
        let at = center(&app, "menu-headless");
        for event in [InputEvent::PointerDown(at), InputEvent::PointerUp(at)] {
            app.dispatch_pointer(&mut hub, event, capture::NOW)
                .expect("item click");
        }
        assert!(hub.menu.is_none());
        assert_eq!(
            hub.take_calls(),
            [Call::Open {
                project: "project-3".into(),
                headless: true
            }]
        );
        assert!(app.frame().expect("present queued changes"));
        assert!(!app.frame().expect("idle"));
    }
}

#[test]
fn idle_frames_do_not_request_gpu_submission() {
    let (mut app, mut hub) = setup();
    assert!(app.frame().expect("initial present"));
    for _ in 0..120 {
        assert!(!hub.tick(Instant::now()), "idle model tick");
        app.rebuild(&mut hub, capture::NOW).expect("equal refresh");
        assert!(!app.frame().expect("idle frame"));
        assert_eq!(
            app.work.restyled + app.work.layout_passes + app.work.shaped + app.work.paint_nodes,
            0
        );
    }
}

#[test]
fn theme_swap_keeps_text_and_layout_and_restyles_only_once() {
    let (mut app, mut hub) = setup();
    app.frame().expect("initial frame");
    let shapes = app.text.shapes;
    for _ in 0..4 {
        hub.intent(Intent::ToggleTheme);
        app.rebuild(&mut hub, capture::NOW).expect("token swap");
        assert!(app.frame().expect("theme present"));
        assert_eq!(app.text.shapes, shapes, "theme keeps shaped paragraphs");
        assert_eq!(app.work.shaped, 0);
        assert_eq!(app.work.layout_passes, 0);
        assert_eq!(
            app.work.restyled,
            app.ui.nodes.len(),
            "one restyle per node"
        );
        assert!(!app.frame().expect("idle after swap"));
        assert_eq!(app.work.restyled, 0);
    }
}

#[test]
fn shorter_filtered_list_clamps_retained_scroll() {
    let (mut app, mut hub) = setup();
    app.size = [900., 600.];
    app.rebuild(&mut hub, capture::NOW).expect("small viewport");
    let scroll = app.ui.find("scroll-projects").expect("scroll");
    app.ui
        .scroll_to(scroll, [0., 10_000.])
        .expect("scroll down");
    app.frame().expect("scrolled frame");
    let id = ara_ui::source::stable_id("scroll-projects");
    assert!(matches!(app.ui.widgets.get(&id),Some(ara_ui::widgets::Widget::Scroll(p)) if p[1]>0.));
    hub.intent(Intent::Search("cour".into()));
    app.rebuild(&mut hub, capture::NOW).expect("filtered list");
    app.frame().expect("filtered frame");
    assert!(matches!(app.ui.widgets.get(&id),Some(ara_ui::widgets::Widget::Scroll(p)) if p[1]==0.));
    let button = &app.ui.nodes[app.ui.find("open-project-3").expect("filtered project")];
    assert!(
        button.rect.intersects(button.clip),
        "remaining project stays visible"
    );
}

#[test]
fn confirmations_expire_and_errors_and_undo_have_useful_lifetimes() {
    let (_, mut hub) = setup();
    hub.event(Event::Reply {
        call: Call::Remove {
            project: "project-3".into(),
        },
        result: Ok(json!({"removed":"project-3"})),
    });
    let confirmation = hub.notices.last().expect("undo confirmation").id;
    hub.event(Event::Reply {
        call: Call::Close {
            project: "project-4".into(),
        },
        result: Err(ara_proto::RpcError {
            code: -32000,
            message: "fixture error".into(),
            data: None,
        }),
    });
    hub.event(Event::Reply {
        call:Call::Create {template:"blank".into(),name:"Created".into(),path:Some("target/created".into()),engine:None,dry_run:false,revision:0},
        result:Ok(json!({"id":"created","name":"Created","path":"target/created","engineVersion":"0.0.1-alpha"})),
    });
    let informational = hub.notices.last().expect("created confirmation").id;
    let now = Instant::now();
    assert!(
        hub.tick(now + std::time::Duration::from_secs(9)),
        "ordinary informational toasts expire after eight seconds"
    );
    assert!(!hub.notices.iter().any(|notice| notice.id == informational));
    assert!(
        hub.notices.iter().any(|notice| notice.id == confirmation),
        "undo stays available longer"
    );
    assert!(hub.tick(now + std::time::Duration::from_secs(21)));
    assert!(!hub.notices.iter().any(|n| n.id == confirmation));
    assert!(
        hub.notices.iter().any(|n| n.is_error()),
        "errors remain dismissible until acknowledged"
    );
}

#[test]
fn inline_notifications_have_dismiss_controls() {
    let (mut app, mut hub) = setup();
    app.frame().expect("initial frame");
    hub.event(Event::Offline {
        message: "fixture offline".into(),
        log: "target/hub-tests/missing.log".into(),
    });
    app.rebuild(&mut hub, capture::NOW).expect("offline");
    for id in ["dismiss-offline", "dismiss-update"] {
        let at = center(&app, id);
        app.dispatch_pointer(&mut hub, InputEvent::PointerDown(at), capture::NOW)
            .expect("down");
        app.dispatch_pointer(&mut hub, InputEvent::PointerUp(at), capture::NOW)
            .expect("up");
        assert!(app.ui.find(id).is_none(), "{id} disappears on first click");
        assert!(app.frame().expect("present"));
    }
}

#[test]
fn changed_view_is_presented_without_another_input_event() {
    let (mut app, mut hub) = setup();
    hub.intent(Intent::ToggleTheme);
    app.rebuild(&mut hub, capture::NOW).expect("new theme");
    assert!(
        app.frame().expect("frame"),
        "theme, dismissal and status updates must reach the GPU without waiting for a second input"
    );
}

#[test]
fn unchanged_service_reply_preserves_shaped_text() {
    let (mut app, mut hub) = setup();
    let shapes = app.text.shapes;
    hub.event(Event::Reply {
        call: Call::Status,
        result: Ok(status_snapshot(&hub)),
    });
    app.rebuild(&mut hub, capture::NOW)
        .expect("unchanged refresh");
    assert_eq!(
        app.text.shapes, shapes,
        "unchanged replies must keep the retained text cache"
    );
}

/// Opt-in measurement: writes only to the requested target/ artifact path.
#[test]
#[ignore = "CPU timing measurement; run with ARA_HUB_TIMINGS under target/"]
fn measure_hub_frames() {
    let path =
        std::path::PathBuf::from(std::env::var_os("ARA_HUB_TIMINGS").expect("timing output"));
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()
        .expect("target");
    assert!(
        path.parent()
            .expect("parent")
            .canonicalize()
            .expect("create an output directory under target/ first")
            .starts_with(target)
    );
    let (mut app, mut hub) = setup();
    let mut samples = Vec::new();
    app.frame().expect("initial frame");
    for scenario in ["unchanged", "status-refresh", "status-change", "theme"] {
        let mut elapsed = Vec::new();
        let mut work = ara_ui::Work::default();
        for sample in 0..40 {
            let start = Instant::now();
            if scenario == "theme" {
                hub.intent(Intent::ToggleTheme);
            }
            if scenario == "status-refresh" {
                hub.event(Event::Reply {
                    call: Call::Status,
                    result: Ok(status_snapshot(&hub)),
                });
            }
            if scenario == "status-change" {
                let mut snapshot = status_snapshot(&hub);
                let status = &mut snapshot["projects"][0];
                let open = sample % 2 == 1;
                status["state"] = json!(if open { "open" } else { "closed" });
                status["pid"] = if open {
                    json!(1001)
                } else {
                    serde_json::Value::Null
                };
                status["port"] = if open {
                    json!(40101)
                } else {
                    serde_json::Value::Null
                };
                status["headless"] = if open {
                    json!(false)
                } else {
                    serde_json::Value::Null
                };
                hub.event(Event::Reply {
                    call: Call::Status,
                    result: Ok(snapshot),
                });
            }
            app.rebuild(&mut hub, capture::NOW).expect("frame");
            app.frame().expect("paint frame");
            work += std::mem::take(&mut app.work);
            elapsed.push(start.elapsed().as_secs_f64() * 1000.);
        }
        elapsed.sort_by(f64::total_cmp);
        samples.push(json!({"scenario":scenario,"medianMs":elapsed[20],"p95Ms":elapsed[37],"samplesMs":elapsed,"work":work}));
    }
    let result = json!({"profile": if cfg!(debug_assertions) {"debug"} else {"release"}, "size":[1440,900],"samples":samples});
    std::fs::write(path, ara_json::to_canonical_string(&result)).expect("timing artifact");
    println!("{result}");
}
