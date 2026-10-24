//! Renders every window state offscreen (no window opens) to
//! `target/ara-setup-ui/<state>@<scale>x.png` for review, as Windows 11 shows it.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use ara_setup::{
    flow::{Kind, Plan, Step},
    processes::Blocker,
    ui::{
        Action, View,
        paint::{Frame, Pointer, render},
    },
};
use std::path::{Path, PathBuf};

const INSTALLED: &str = r"C:\Users\snowy\AppData\Local\Programs\Ara\0.0.1-alpha";

fn states() -> Vec<(&'static str, View, Pointer)> {
    let plan = Plan {
        kind: Kind::Engine,
        destination: PathBuf::from(INSTALLED),
    };
    let idle = Pointer::default();
    let installing = View::progress(
        Some(&plan),
        Step::Extract {
            file: "resources/ara-render-fidelity/shaders/lumen_screen_probe_gather.spv",
            done: 278_000_000,
            total: 730_000_000,
        },
    );
    let blockers = [
        Blocker {
            pid: 18_244,
            exe: "ara-hub.exe".into(),
            path: PathBuf::from(INSTALLED).join("ara-hub.exe"),
            service: false,
        },
        Blocker {
            pid: 9_120,
            exe: "ara.exe".into(),
            path: PathBuf::from(INSTALLED).join("ara.exe"),
            service: true,
        },
    ];
    let error = anyhow::anyhow!(
        "ARA-EXP-INSTALL-010: registration failed (exit code: 1). The files remain at {INSTALLED}; run install.ps1 -RegisterOnly there to retry\n\nInstall Microsoft Visual C++ v14 Redistributable (x64), then rerun install.ps1 -RegisterOnly"
    );
    vec![
        ("installing", installing, Pointer { tick: 40, ..idle }),
        (
            "registering",
            View::progress(Some(&plan), Step::Register),
            Pointer { tick: 12, ..idle },
        ),
        ("confirm", View::confirm(&plan, "0.0.1-alpha"), idle),
        (
            "confirm-hover",
            View::confirm(&plan, "0.0.1-alpha"),
            Pointer {
                hover: Some(Action::Replace),
                ..idle
            },
        ),
        ("blocked", View::blocked(&blockers), idle),
        ("blocked-unknown", View::blocked(&[]), idle),
        ("error", View::failed(Some(&plan), &error), idle),
        (
            "done",
            View::Done {
                title: "Ara 0.0.1-alpha installed".into(),
                destination: INSTALLED.into(),
                note: "Replaced the previous installation. Open Ara Hub to create a project or try a sample.".into(),
                warning: None,
                launch: Some(PathBuf::from("ara-hub.exe")),
            },
            idle,
        ),
    ]
}

#[test]
fn every_state_renders_to_png() {
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-setup-ui");
    std::fs::create_dir_all(&output).unwrap();
    for (name, view, pointer) in states() {
        for scale in [1.0, 2.0] {
            let image = render(&view, pointer, scale, Frame::Preview).unwrap();
            assert_eq!(
                (image.width, image.height),
                ((480.0 * scale) as u32, (200.0 * scale) as u32)
            );
            let alpha = |x: u32, y: u32| image.rgba[((y * image.width + x) * 4 + 3) as usize];
            assert_eq!(alpha(0, 0), 0, "{name}: rounded corner is transparent");
            assert_eq!(
                alpha(image.width / 2, image.height / 2),
                255,
                "{name}: opaque"
            );
            image::RgbaImage::from_raw(image.width, image.height, image.rgba)
                .unwrap()
                .save(output.join(format!("{name}@{scale}x.png")))
                .unwrap();
        }
    }
}

#[test]
fn buttons_are_hit_where_they_are_painted() {
    let plan = Plan {
        kind: Kind::Hub,
        destination: PathBuf::from(r"C:\Users\snowy\AppData\Local\Programs\Ara Hub"),
    };
    let view = View::confirm(&plan, "0.0.1-alpha");
    let (width, height) = ara_setup::ui::paint::size(1.5);
    let mut surface = ara_setup::ui::paint::Surface::new(width, height).unwrap();
    let hits =
        ara_setup::ui::paint::paint(&mut surface, &view, Pointer::default(), 1.5, Frame::System);
    let actions: Vec<_> = hits.iter().map(|(action, _)| *action).collect();
    assert_eq!(actions, [Action::Replace, Action::Cancel]);
    let (_, replace) = hits[0];
    assert!(replace.x + replace.w <= width as f32 - 24.0 * 1.5 + 0.5);
    assert!(replace.y + replace.h <= height as f32);
}
