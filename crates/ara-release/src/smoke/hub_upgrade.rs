//! Exercise same-version launcher replacement without touching the real Start menu.
use super::*;

fn managed(env: &Environment) -> PathBuf {
    env.directory.join("local/Programs/Ara Hub")
}

pub(super) fn seed(env: &Environment, dist: &Path, version: &str) -> Result<()> {
    let hub = managed(env);
    run(
        env.command(dist.join(format!("ara-hub-{version}-windows-x64-setup.exe")))
            .arg("--destination")
            .arg(&hub)
            .args(["--silent", "--no-system-integration"]),
        &env.directory.join("old-hub-install.log"),
    )?;
    let mut marker: Value = serde_json::from_slice(&fs::read(hub.join("hub.json"))?)?;
    // A valid, distinct same-version older preview. PE overlays preserve execution.
    marker["build"] = json!({"commit":"0".repeat(40),"generation":0});
    support::json(&hub.join("hub.json"), &marker)?;
    fs::OpenOptions::new()
        .append(true)
        .open(hub.join("ara-hub.exe"))?
        .write_all(b"older-hub-build-smoke")?;
    fs::write(
        env.directory.join("old-hub-sha256.txt"),
        ara_update::sha256_file(&hub.join("ara-hub.exe"))?,
    )?;
    let script = env.directory.join("seed-hub-shortcuts.ps1");
    fs::write(
        &script,
        r#"param($Hub,$Programs)
$ErrorActionPreference='Stop'
. ([IO.Path]::Combine($Hub,'paths.ps1'))
. ([IO.Path]::Combine($Hub,'hub-installation.ps1'))
New-AraHubShortcut (Join-Path $Programs 'Ara\Ara Hub.lnk') $Hub
New-AraHubShortcut (Join-Path $Programs 'Ara\0.0.1-alpha\Ara Hub.lnk') $Hub
"#,
    )?;
    run(
        env.command("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script)
            .arg(&hub)
            .arg(env.directory.join("shortcuts")),
        &env.directory.join("old-hub-shortcuts.log"),
    )?;
    Ok(())
}

pub(super) fn verify(env: &Environment) -> Result<()> {
    let hub = managed(env);
    let expected: Value =
        serde_json::from_slice(&fs::read(env.installed.join("hub-launcher.json"))?)?;
    let actual: Value = serde_json::from_slice(&fs::read(hub.join("hub.json"))?)?;
    ensure!(
        actual == expected,
        "engine setup left the previous Hub build installed"
    );
    let hash = ara_update::sha256_file(&hub.join("ara-hub.exe"))?;
    ensure!(
        hash == ara_update::sha256_file(&env.installed.join("ara-hub.exe"))?
            && hash != fs::read_to_string(env.directory.join("old-hub-sha256.txt"))?,
        "stale Hub executable survived engine installation"
    );
    // Run the exact production shortcut writer against an explicit scratch
    // Programs directory. -NoSystemIntegration keeps real shell/registry untouched.
    let script = env.directory.join("verify-hub-shortcuts.ps1");
    fs::write(
        &script,
        r#"param($Hub,$Programs,$Output)
$ErrorActionPreference='Stop'
. ([IO.Path]::Combine($Hub,'paths.ps1'))
. ([IO.Path]::Combine($Hub,'hub-installation.ps1'))
Register-AraHubShortcuts $Hub $Programs
$shell=New-Object -ComObject WScript.Shell
@((Join-Path $Programs 'Ara\Ara Hub.lnk'),(Join-Path $Programs 'Ara\0.0.1-alpha\Ara Hub.lnk')) | ForEach-Object { $shell.CreateShortcut($_).TargetPath } | ConvertTo-Json | Set-Content -LiteralPath $Output -Encoding UTF8
"#,
    )?;
    let targets = env.directory.join("hub-shortcut-targets.json");
    run(
        env.command("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script)
            .arg(&hub)
            .arg(env.directory.join("shortcuts"))
            .arg(&targets),
        &env.directory.join("hub-shortcuts.log"),
    )?;
    let bytes = fs::read(targets)?;
    let targets: Vec<PathBuf> =
        serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))?;
    ensure!(targets.len() == 2, "both Hub aliases must be checked");
    for (index, target) in targets.iter().enumerate() {
        ensure!(
            target == &hub.join("ara-hub.exe"),
            "shortcut targets a different launcher: {}",
            target.display()
        );
        let info = env.directory.join(format!("shortcut-build-{index}.json"));
        run(
            env.command(target).arg("--build-info").arg(&info),
            &env.directory.join(format!("shortcut-build-{index}.log")),
        )?;
        let launched: Value = serde_json::from_slice(&fs::read(info)?)?;
        ensure!(
            launched["build"] == expected["build"],
            "shortcut launched the old Hub executable: {launched}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
