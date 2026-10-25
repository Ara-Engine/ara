use super::*;
#[test]
#[ignore = "requires built ara-hub, ara-hub-updater and ara-setup; runs real tiny installed packages"]
fn installed_packages_replace_a_stale_hub_and_launch_new_build_through_shortcuts() {
    exercise().expect("installed Hub upgrade smoke");
}
fn exercise() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = root.join("target/hub-build-package-tests");
    fs::create_dir_all(&target)?;
    let scratch = tempfile::tempdir_in(&target)?;
    let directory = std::path::absolute(scratch.path())?;
    let env = Environment {
        installed: directory.join("engine"),
        directory,
    };
    for folder in ["config", "local", "roaming", "crashes", "empty-layers"] {
        fs::create_dir_all(env.directory.join(folder))?;
    }
    let bins = PathBuf::from(
        std::env::var_os("ARA_TEST_BIN_DIR")
            .context("set ARA_TEST_BIN_DIR to the built executables")?,
    );
    let hub = env.directory.join("hub-package");
    let engine = env.directory.join("engine-package");
    fs::create_dir_all(hub.join("bootstrap"))?;
    fs::create_dir_all(engine.join("assets/brand"))?;
    fs::copy(
        root.join("assets/brand/ara.ico"),
        engine.join("assets/brand/ara.ico"),
    )?;
    let compiler = env.directory.join("fixture.ps1");
    fs::write(
        &compiler,
        r#"param($Output)
$ErrorActionPreference='Stop'
Add-Type -OutputAssembly $Output -OutputType ConsoleApplication -TypeDefinition 'public static class FixtureAra { public static int Main(string[] args) { if(args.Length > 0 && args[0] == "--hold") { System.IO.File.WriteAllText(System.Environment.GetEnvironmentVariable("ARA_CONFIG_DIR") + "/ready", "ready"); while(!System.IO.File.Exists(System.Environment.GetEnvironmentVariable("ARA_CONFIG_DIR") + "/stop")) System.Threading.Thread.Sleep(20); } return 0; } }'
"#,
    )?;
    run(
        env.command("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(compiler)
            .arg(engine.join("ara.exe")),
        &env.directory.join("fixture.log"),
    )?;
    for name in ["ara-hub.exe", "ara-hub-updater.exe"] {
        fs::copy(bins.join(name), engine.join(name))?;
        fs::copy(bins.join(name), hub.join(name))?;
    }
    for entry in fs::read_dir(&bins)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
        {
            let name = path.file_name().context("DLL name")?;
            fs::copy(&path, engine.join(name))?;
            fs::copy(&path, hub.join(name))?;
        }
    }
    let info = env.directory.join("compiled-hub.json");
    run(
        env.command(engine.join("ara-hub.exe"))
            .arg("--build-info")
            .arg(&info),
        &env.directory.join("compiled-hub.log"),
    )?;
    let mut marker: Value = serde_json::from_slice(&fs::read(info)?)?;
    marker["format"] = json!("ara.hub/1");
    let version = env!("CARGO_PKG_VERSION");
    support::json(&hub.join("hub.json"), &marker)?;
    support::json(&engine.join("hub-launcher.json"), &marker)?;
    for name in ["paths.ps1", "hub-installation.ps1"] {
        fs::copy(root.join("release").join(name), hub.join(name))?;
        fs::copy(root.join("release").join(name), engine.join(name))?;
    }
    fs::write(
        hub.join("install.ps1"),
        include_str!("../../../hub-install.ps1"),
    )?;
    fs::write(
        engine.join("standalone-hub-install.ps1"),
        include_str!("../../../hub-install.ps1"),
    )?;
    fs::write(
        engine.join("install.ps1"),
        format!(
            "{}\n{}",
            fs::read_to_string(root.join("release/install.ps1"))?,
            include_str!("../../../engine-hub.ps1")
        ),
    )?;
    support::json(
        &engine.join("engine.json"),
        &json!({"format":"ara.engine/1","version":version}),
    )?;
    support::json(
        &engine.join("hub-service.json"),
        &json!({"version":version,"build":"fixture"}),
    )?;
    fs::copy(
        engine.join("hub-service.json"),
        hub.join("bootstrap/service.json"),
    )?;
    fs::copy(engine.join("ara.exe"), hub.join("bootstrap/ara.exe"))?;
    for source in [&hub, &engine] {
        fs::write(
            source.join("THIRD_PARTY_NOTICES.txt"),
            "Synthetic installed-build smoke",
        )?;
    }
    let files: Vec<_> = support::files(&engine)?.iter().map(|p|json!({"path":p.strip_prefix(&engine).expect("engine file").to_string_lossy().replace('\\',"/")})).collect();
    support::json(
        &engine.join("files.json"),
        &json!({"version":version,"files":files}),
    )?;
    for (source, name) in [
        (&hub, format!("ara-hub-{version}-windows-x64")),
        (&engine, support::name(version)),
    ] {
        let zip = env.directory.join(format!("{name}.zip"));
        crate::archive::zip(source, &zip)?;
        fs::write(
            env.directory.join(format!("{name}-setup.exe")),
            ara_export_installer::append(&fs::read(bins.join("ara-setup.exe"))?, &fs::read(zip)?)?,
        )?;
    }
    seed(&env, &env.directory, version)?;
    let current = managed(&env);
    let old_hash = ara_update::sha256_file(&current.join("ara-hub.exe"))?;
    let lock = fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(
            current
                .parent()
                .context("parent")?
                .join(".ara-hub-update.lock"),
        )?;
    lock.try_lock()?;
    let blocked = run(
        env.command(
            env.directory
                .join(format!("{}-setup.exe", support::name(version))),
        )
        .arg("--destination")
        .arg(&env.installed)
        .args(["--silent", "--no-system-integration"]),
        &env.directory.join("blocked-install.log"),
    );
    ensure!(
        blocked.is_err(),
        "parallel Hub update must block engine registration"
    );
    ensure!(
        ara_update::sha256_file(&current.join("ara-hub.exe"))? == old_hash,
        "locked update changed Hub"
    );
    drop(lock);
    // File locks and a running program must also preserve the old launcher.
    let held = fs::File::options()
        .read(true)
        .open(current.join("ara-hub.exe"))?;
    let retry = |log: &str| {
        run(
            env.command("powershell.exe")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(env.installed.join("install.ps1"))
                .args(["-RegisterOnly", "-Silent", "-NoSystemIntegration"]),
            &env.directory.join(log),
        )
    };
    ensure!(
        retry("held-file.log").is_err(),
        "a held file must block replacement"
    );
    drop(held);
    fs::copy(env.installed.join("ara.exe"), current.join("held.exe"))?;
    let mut held_process = OwnedProcess(
        env.command(current.join("held.exe"))
            .arg("--hold")
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !env.directory.join("config/ready").exists() {
        ensure!(
            held_process.0.try_wait()?.is_none() && Instant::now() < deadline,
            "fixture process did not become ready"
        );
        thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        retry("running-hub.log").is_err(),
        "a running Hub process must block replacement"
    );
    fs::write(env.directory.join("config/stop"), "stop")?;
    held_process.0.wait()?;
    fs::remove_file(current.join("held.exe"))?;
    retry("retry-register.log")?;
    verify(&env)?;
    // Installing the same engine later must preserve a newer standalone Hub.
    let mut newer = marker;
    newer["build"]["generation"] = json!(
        newer["build"]["generation"]
            .as_u64()
            .context("generation")?
            + 1
    );
    newer["build"]["commit"] = json!("f".repeat(40));
    support::json(&current.join("hub.json"), &newer)?;
    retry("keep-newer.log")?;
    ensure!(
        serde_json::from_slice::<Value>(&fs::read(current.join("hub.json"))?)? == newer,
        "engine setup downgraded a newer Hub"
    );
    run(
        env.command(
            env.directory
                .join(format!("ara-hub-{version}-windows-x64-setup.exe")),
        )
        .arg("--destination")
        .arg(&current)
        .args(["--silent", "--replace", "--no-system-integration"]),
        &env.directory.join("keep-newer-standalone.log"),
    )?;
    ensure!(
        serde_json::from_slice::<Value>(&fs::read(current.join("hub.json"))?)? == newer,
        "standalone setup downgraded a newer Hub"
    );
    Ok(())
}
