#![allow(clippy::unwrap_used)]
use ara_graphics_settings::*;

fn scratch() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/graphics-tests");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn candidates_apply_revert_and_reset_without_saving_or_enabling_rt() {
    let mut service = Service::default();
    let original = service.state().applied.clone();
    let mut candidate = original.clone();
    candidate.groups.shadows = Some(Tier::Low);
    candidate.ray_tracing = true;
    service
        .request(Request::Set {
            settings: candidate.clone(),
        })
        .unwrap();
    assert_eq!(service.state().applied, original);
    assert_eq!(service.request(Request::Apply).unwrap().applied, candidate);
    assert!(service.state().restart_required);
    assert!(!service.state().active_ray_tracing);
    assert_eq!(service.request(Request::Revert).unwrap().applied, original);
    assert!(!service.state().restart_required);
    service
        .request(Request::Set {
            settings: candidate,
        })
        .unwrap();
    assert_eq!(
        service.request(Request::ResetToDefaults).unwrap().pending,
        original
    );
    assert_eq!(service.request(Request::Save).unwrap_err().code, STORAGE);
}

#[test]
fn invalid_numbers_and_capabilities_cannot_corrupt_applied_state() {
    let mut candidate = Settings {
        width: 0,
        height: u32::MAX,
        frame_rate_cap: 1,
        texture_budget_mib: Some(u32::MAX),
        resolution_scale: Some(f32::NAN),
        ..Default::default()
    };
    assert_eq!(candidate.validate().len(), 5);
    assert_eq!([candidate.width, candidate.height], [320, 8192]);
    assert_eq!(candidate.frame_rate_cap, 15);
    assert_eq!(candidate.resolution_scale, None);
    assert_eq!(candidate.texture_budget_mib, Some(32768));
    let caps = Capabilities {
        device_known: true,
        device_local_mib: Some(8192),
        ..Default::default()
    };
    assert_eq!(candidate.texture_mib(&caps), 4096);
    let mut service = Service::default();
    service.set_capabilities(caps);
    candidate.ray_tracing = true;
    service
        .request(Request::Set {
            settings: candidate,
        })
        .unwrap();
    let before = service.state().applied.clone();
    assert_eq!(service.request(Request::Apply).unwrap_err().code, INVALID);
    assert_eq!(service.state().applied, before);
    assert_eq!(service.state().revision, 1);
}

#[test]
fn strict_versioned_store_round_trips_and_preserves_future_files() {
    let dir = scratch();
    let path = dir.path().join("graphics.json");
    let defaults = Settings::default();
    let mut store = Store::at(&path);
    assert_eq!(store.load(&defaults).0, defaults);
    let custom = Settings {
        preset: Preset::Epic,
        groups: Groups {
            shadows: Some(Tier::Low),
            ..Default::default()
        },
        ..defaults.clone()
    };
    store.save(&custom).unwrap();
    assert_eq!(store.load(&defaults).0, custom);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n') && !text.contains('\r'));
    for bad in [
        r#"{"version":1,"version":1,"settings":{}}"#,
        r#"{"version":1,"settings":{"preset":"ultra"}}"#,
        r#"{"version":0,"settings":{}}"#,
    ] {
        std::fs::write(&path, bad).unwrap();
        let (loaded, rows) = store.load(&defaults);
        assert_eq!(loaded, defaults);
        assert_eq!(rows[0].code, STORAGE);
    }
    let future = r#"{"version":999,"settings":{"newField":true}}"#;
    std::fs::write(&path, future).unwrap();
    assert_eq!(store.load(&defaults).0, defaults);
    assert!(store.save(&custom).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), future);
}

#[test]
fn save_uses_applied_values_and_auto_keeps_rt_an_explicit_opt_in() {
    let dir = scratch();
    let path = dir.path().join("graphics.json");
    let mut service = Service::new(Settings::default(), Some(Store::at(&path)));
    service
        .request(Request::Set {
            settings: Settings {
                preset: Preset::Epic,
                ..Default::default()
            },
        })
        .unwrap();
    service.request(Request::Save).unwrap();
    assert_eq!(
        Store::at(&path).load(&Settings::default()).0.preset,
        Preset::Auto
    );
    service.request(Request::Apply).unwrap();
    service.request(Request::Save).unwrap();
    assert_eq!(
        Store::at(&path).load(&Settings::default()).0.preset,
        Preset::Epic
    );
    let detected = service.request(Request::AutoDetect).unwrap();
    assert!(!detected.pending.ray_tracing);
    assert_eq!(detected.applied.preset, Preset::Epic);
    for (mib, expected) in [
        (None, Tier::Low),
        (Some(6144), Tier::Medium),
        (Some(8192), Tier::High),
    ] {
        assert_eq!(
            detected.pending.tier(&Capabilities {
                device_local_mib: mib,
                ..Default::default()
            }),
            expected
        );
    }
}
