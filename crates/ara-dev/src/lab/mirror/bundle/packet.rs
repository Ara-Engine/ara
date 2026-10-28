//! One staged tar stream carries artifacts and its durable import request.
use super::*;

pub(super) fn archive(job: &Path, files: &[&str], import: &Value) -> Result<PathBuf> {
    super::super::super::session::write(&job.join("incoming.json"), import)?;
    let request = json!({"tool":"_lab_import","arguments":import});
    // The CLI consumes exactly one NDJSON frame from stdin.
    let mut frame = serde_json::to_vec(&request)?;
    frame.push(b'\n');
    fs::write(job.join("request.json"), frame)?;
    let names = files
        .iter()
        .map(|p| format!("data/{p}\n"))
        .collect::<String>()
        + "incoming.json\nrequest.json\n";
    fs::write(job.join("files.txt"), names)?;
    let archive = job.join("changed.tar");
    let mut tar = Command::new("tar");
    tar.arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(job)
        .arg("-T")
        .arg(job.join("files.txt"))
        .stdin(Stdio::null());
    super::super::super::build::background(&mut tar);
    ensure!(
        crate::process::run(&mut tar, Some(Instant::now() + Duration::from_secs(120)))?.success(),
        "cannot archive cooked changes"
    );
    Ok(archive)
}

pub(super) fn receiver(config: &Config, inbox: &str, activate: bool) -> String {
    let base = transport::quote(&format!("{}/target/ara-dev/lab", config.repo));
    let destination = transport::quote(&format!("{}/target/ara-dev/lab/{inbox}", config.repo));
    let exe = transport::quote(&format!("{}/debug/ara-dev", config.build_dir));
    let finish = if activate {
        format!("{exe} lab rpc < \"$inbox/request.json\"\n")
    } else {
        "printf '%s\\n' '{\"received\":true}'\n".into()
    };
    let receive = format!(
        "{}base={base}\ninbox={destination}\nmkdir -p \"$inbox\"\ntar -xf - -C \"$inbox\"\ntest -d \"$inbox/data\" || mkdir -p \"$inbox/data\"\ncp \"$inbox/incoming.json\" \"$base/mirror/incoming.new.json\"\nmv -f \"$base/mirror/incoming.new.json\" \"$base/mirror/incoming.json\"\n{finish}",
        transport::prelude(config)
    );
    format!(
        "set -eu\n/usr/bin/pmset -g batt | grep -q \"Now drawing from 'AC Power'\" || {{ echo 'ARA_LAB_MAC_BATTERY: connect the charger; import refused' >&2; exit 1; }}\nexec /usr/bin/caffeinate -dimsu /bin/sh -c {}\n",
        transport::quote(&receive)
    )
}
