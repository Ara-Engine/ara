// Release builds are GUI apps: no console window from the Start menu shortcut.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use ara_hub::native::{self, Options};
use std::path::PathBuf;

const HELP: &str = r"ara-hub [--size WxH] [--dpi SCALE] [--capture DIR] [--config-dir DIR] [--ara PATH]

Ara Hub: projects, templates, engine installs and agent setup.
--capture DIR    render every view in both themes to DIR/<W>x<H>/ and exit
--size WxH       window size in physical pixels (default 1440x900)
--dpi SCALE      fixed UI scale instead of the display's
--config-dir DIR user Hub registry (default %APPDATA%\Ara or ARA_CONFIG_DIR)
--ara PATH       the ara CLI that runs the Hub service (default: beside ara-hub)";

fn options() -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    let value = |args: &mut dyn Iterator<Item = String>, flag: &str| {
        args.next().ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            #[cfg(feature = "rehearsal")]
            "--rehearse-update" => {
                options.rehearse_update = Some(PathBuf::from(value(&mut args, &arg)?))
            }
            "--build-info" => {
                let path = value(&mut args, &arg)?;
                ara_update::write_json(std::path::Path::new(&path), &serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"build":ara_hub::build_identity()})).map_err(|e|e.to_string())?;
                return Ok(None);
            }
            "--service-check" => {
                options.service_check = Some(PathBuf::from(value(&mut args, &arg)?))
            }
            "--startup-probe" => {
                options.startup_probe = Some(PathBuf::from(value(&mut args, &arg)?))
            }
            "--update-health" => {
                options.update_health = Some(PathBuf::from(value(&mut args, &arg)?))
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("ara-hub {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--capture" => options.capture = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--config-dir" => options.config = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--ara" => options.ara = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--size" => {
                let size = value(&mut args, &arg)?;
                let (w, h) = size.split_once('x').ok_or("--size needs WxH")?;
                options.size = [
                    w.parse().map_err(|_| "--size needs WxH")?,
                    h.parse().map_err(|_| "--size needs WxH")?,
                ];
                if options.size.iter().any(|v| !(480..=8192).contains(v)) {
                    return Err("window size must be 480..8192 pixels".into());
                }
            }
            "--dpi" => {
                let dpi: f32 = value(&mut args, &arg)?
                    .parse()
                    .map_err(|_| "--dpi needs a number")?;
                if !dpi.is_finite() || !(0.5..=4.).contains(&dpi) {
                    return Err("--dpi must be 0.5..4".into());
                }
                options.dpi = Some(dpi);
            }
            _ => return Err(format!("unknown option {arg}; see --help")),
        }
    }
    Ok(Some(options))
}

/// Automation modes report errors through their exit status, without desktop dialogs.
fn interactive(options: &Options) -> bool {
    #[cfg(feature = "rehearsal")]
    if options.rehearse_update.is_some()
        || options.update_health.is_some()
        || std::env::var_os("ARA_RELEASE_TEST_API").is_some()
    {
        return false;
    }
    options.capture.is_none() && options.startup_probe.is_none() && options.service_check.is_none()
}

fn main() -> std::process::ExitCode {
    let entered = std::time::Instant::now();
    match ara_crash::monitor_from_args() {
        Ok(true) => return std::process::ExitCode::SUCCESS,
        Ok(false) => {}
        Err(error) => eprintln!("ARA-EXP-CRASH-004: {error:#}"),
    }
    let options = match options() {
        Ok(Some(options)) => options,
        Ok(None) => return std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let interactive = interactive(&options);
    ara_hub_service::startup::record("ui.entry", entered.elapsed());
    let phase = std::time::Instant::now();
    let _crashes = ara_crash::desktop::start("ara-hub", interactive)
        .map_err(|error| eprintln!("ARA-EXP-CRASH-004: {error:#}"))
        .ok();
    ara_hub_service::startup::record("ui.crash_monitor", phase.elapsed());
    match native::run(options) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let message = format!("Ara Hub could not start:\n\n{error:#}");
            eprintln!("{message}");
            if interactive {
                native::fatal(&message);
            }
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_never_uses_desktop_error_dialogs() {
        for options in [
            Options {
                capture: Some("target/hub-captures".into()),
                ..Default::default()
            },
            Options {
                startup_probe: Some("target/hub-startup".into()),
                ..Default::default()
            },
            Options {
                service_check: Some("target/hub-service-check.json".into()),
                ..Default::default()
            },
        ] {
            assert!(!interactive(&options));
        }
        #[cfg(feature = "rehearsal")]
        for options in [
            Options {
                rehearse_update: Some("target/hub-rehearsal.json".into()),
                ..Default::default()
            },
            Options {
                update_health: Some("target/hub-update-health".into()),
                ..Default::default()
            },
        ] {
            assert!(
                !interactive(&options),
                "explicit flags suffice without a test API environment"
            );
        }
    }
}
