//! Embeds the Ara hub icon (`assets/brand/icons/hub/ara-hub.ico`) into the Windows executable.
//! Uses `rc.exe` from the Windows SDK that MSVC linking already requires; a no-op elsewhere.

fn main() {
    use std::process::Command;
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    for (key, args) in [
        ("ARA_HUB_COMMIT", vec!["rev-parse", "HEAD"]),
        ("ARA_HUB_GENERATION", vec!["rev-list", "--count", "HEAD"]),
    ] {
        println!("cargo:rustc-env={key}={}", git(&args).unwrap_or_default());
    }
    if let Some(path) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &branch])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=../../assets/brand/icons/hub/ara-hub.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/icons/hub/ara-hub.ico");
    embed_resource::compile(
        "../../assets/brand/icons/hub/ara-hub.rc",
        embed_resource::NONE,
    )
    .manifest_optional()
    .expect("embed the Ara icon resource");
}
