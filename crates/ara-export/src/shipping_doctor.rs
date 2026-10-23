//! Optional packaging/capture setup, scoped to the selected platform and preset.
use crate::{
    ExportPreset, Platform,
    doctor::{Check, Status},
    tools::Toolchain,
};
use serde_json::json;
use std::path::Path;

pub(crate) fn host(platform: Option<Platform>, tools: &Toolchain) -> Vec<Check> {
    let mut checks = Vec::new();
    if platform.is_none_or(|p| p == Platform::Windows) {
        let tool = crate::distribution::signtool();
        checks.push(Check { code: "ARA-EXP-SIGN-002".into(), status: if tool.is_some() { Status::Ok } else { Status::Warning }, message: "Optional Authenticode signtool (unsigned portable builds and installers are supported)".into(), context: json!({"path":tool}), fixes: vec!["Set ARA_SIGNTOOL to Windows SDK bin/<version>/x64/signtool.exe".into()] });
    }
    if platform.is_none_or(|p| matches!(p, Platform::Linux | Platform::Android)) {
        let tool = crate::symbols::objcopy(tools);
        checks.push(Check { code: "ARA-EXP-CRASH-007".into(), status: if tool.is_some() { Status::Ok } else { Status::Warning }, message: "LLVM objcopy for archiving then stripping DWARF at export (not a Cargo prerequisite)".into(), context: json!({"path":tool}), fixes: vec!["Set ARA_OBJCOPY to an installed LLVM/NDK llvm-objcopy executable".into()] });
    }
    checks.push(Check { code: "ARA-EXP-CRASH-004".into(), status: Status::Ok, message: "Native crash capture uses a shipped out-of-process monitor; upload is disabled by default".into(), context: json!({"platforms":["windows","linux"],"upload_default":false}), fixes: vec![] });
    checks
}
pub(crate) fn preset(project: &Path, preset: &ExportPreset) -> Vec<Check> {
    let mut checks = Vec::new();
    if preset.crash.upload_endpoint.is_some() {
        let ready = crate::tools::which("curl").is_some();
        checks.push(Check {
            code: "ARA-EXP-CRASH-003".into(),
            status: if ready { Status::Ok } else { Status::Warning },
            message: "Optional consented crash upload requires curl on the player host".into(),
            context: json!({"curl_on_export_host":ready,"upload_automatic":false}),
            fixes: if ready {
                vec![]
            } else {
                vec!["Install curl on hosts where players explicitly opt into crash upload".into()]
            },
        });
    }
    if preset.platform == Platform::Windows {
        let ready = preset.templates.iter().all(|pin| pin.symbols.is_some());
        checks.push(Check { code: "ARA-EXP-CRASH-005".into(), status: if ready { Status::Ok } else { Status::Error }, message: "Each Windows template must pin its matching native PDB; GUID/age is checked at export".into(), context: json!({"pdb_pins_present":ready}), fixes: vec!["ara-export pin --platform windows --arch x64 --file <player.exe> --symbols <player.pdb>".into()] });
    }
    for (name, pin, code) in [
        (
            "Installer stub",
            preset.installer_stub.as_ref(),
            "ARA-EXP-INSTALL-005",
        ),
        (
            "AppImage runtime",
            preset.appimage_runtime.as_ref(),
            "ARA-EXP-APPIMAGE-001",
        ),
    ] {
        if let Some(pin) = pin {
            let path = Path::new(&pin.source);
            let local = if path.is_absolute() {
                path.to_path_buf()
            } else {
                project.join(path)
            };
            let ready = if pin.source.starts_with("https://") {
                true
            } else {
                std::fs::read(local)
                    .is_ok_and(|bytes| blake3::hash(&bytes).to_hex().as_str() == pin.hash)
            };
            checks.push(Check { code: code.into(), status: if ready { Status::Ok } else { Status::Error }, message: format!("{name} artifact source/hash"), context: json!({"source":pin.source,"download_on_cache_miss":pin.source.starts_with("https://")}), fixes: vec![format!("Provide a local or HTTPS {name} file and its BLAKE3 pin in the preset")] });
        }
    }
    if let Some(signing) = &preset.windows_signing {
        let ready = std::env::var(&signing.certificate_env)
            .is_ok_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        checks.push(Check { code: "ARA-EXP-SIGN-003".into(), status: if ready { Status::Ok } else { Status::Error }, message: "Windows signing certificate reference (values are never logged)".into(), context: json!({"environment_present_and_valid":ready}), fixes: vec!["Set the configured certificate environment variable to its certificate-store thumbprint".into()] });
    }
    checks
}
