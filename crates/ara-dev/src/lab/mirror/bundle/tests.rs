use super::*;
#[test]
fn a_streamed_packet_stages_data_and_control_before_publishing_any_shader() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/lab-tests")
        .join(super::super::super::session::id());
    let job = root.join("packet with spaces");
    let shader = "shaders/test/cooked/pipeline.json";
    let signal = "shaders/test/cooked/.ara-shader-reload";
    fs::create_dir_all(job.join("data/shaders/test/cooked"))?;
    fs::write(job.join("data").join(shader), b"new pipeline")?;
    fs::write(job.join("data").join(signal), b"new cohort")?;
    let record =
        json!({"directory":"mirror/inbox/test","variants":{},"claims":{"agent":["shader.slang"]}});
    let archive = packet::archive(&job, &[shader, signal], &record)?;
    let peer = root.join("peer");
    let inbox = peer.join("mirror/inbox/test");
    fs::create_dir_all(&inbox)?;
    fs::create_dir_all(peer.join("shaders/test/cooked"))?;
    fs::write(peer.join(shader), b"last good")?;
    let mut tar = Command::new("tar");
    tar.arg("-xf").arg(archive).arg("-C").arg(&inbox);
    super::super::super::build::background(&mut tar);
    assert!(
        crate::process::run(&mut tar, Some(Instant::now() + Duration::from_secs(10)))?.success()
    );
    assert_eq!(fs::read(peer.join(shader))?, b"last good");
    assert_eq!(
        super::super::super::session::read(&inbox.join("incoming.json"))
            .context("durable record")?,
        record
    );
    // `lab rpc` reads one NDJSON frame, not an entire pretty-printed document.
    let frame = fs::read_to_string(inbox.join("request.json"))?;
    let request: Value = serde_json::from_str(frame.lines().next().context("request frame")?)?;
    assert_eq!(frame.lines().count(), 1);
    assert_eq!(request["tool"], "_lab_import");
    assert_eq!(request["arguments"], record);
    import(&peer, &record)?;
    assert_eq!(fs::read(peer.join(shader))?, b"new pipeline");
    assert_eq!(fs::read(peer.join(signal))?, b"new cohort");
    assert_eq!(
        super::super::super::session::read(&peer.join("claims.json")).context("claims")?,
        record["claims"]
    );
    let empty = root.join("metadata-only");
    fs::create_dir_all(&empty)?;
    assert!(packet::archive(&empty, &[], &record)?.is_file());
    fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn shader_bundle_follows_active_metadata_and_requires_metal() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/lab-tests")
        .join(super::super::super::session::id());
    fs::create_dir_all(root.join("key"))?;
    for name in ["a.spv", "a.metallib", "a.metal.json", "orphan.metallib"] {
        fs::write(root.join("key").join(name), b"artifact")?;
    }
    let obsolete = root.join("f".repeat(64));
    fs::create_dir_all(&obsolete)?;
    fs::write(
        obsolete.join("reflection.json"),
        br#"{"stages":[{"spirv":"missing"}]}"#,
    )?;
    let manifest = json!({"stages":[{"spirv":"key/a.spv","metal":{"metallib":"key/a.metallib","reflection":"key/a.metal.json"}}]});
    super::super::super::session::write(&root.join("pipeline.json"), &manifest)?;
    let mut files = BTreeMap::new();
    shaders(&root, "shaders/cooked", &mut files)?;
    assert_eq!(files.len(), 4);
    assert!(!files.keys().any(|p| p.contains("orphan")));
    super::super::super::session::write(
        &root.join("pipeline.json"),
        &json!({"stages":[{"spirv":"key/a.spv"}]}),
    )?;
    assert!(shaders(&root, "shaders/cooked", &mut files).is_err());
    fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn artifact_paths_are_confined_and_windows_recipes_are_portable() {
    for p in ["/tmp/a", "../a", "a/../../b", "a\\b", "a\nb"] {
        assert!(relative(p).is_err());
    }
    assert!(relative("shaders/crates/a/cooked/a.json").is_ok());
    assert_eq!(
        portable(json!({"crates\\a\\pipelines.json":{"source":"x\\y.slang"}})),
        json!({"crates/a/pipelines.json":{"source":"x/y.slang"}})
    );
}
