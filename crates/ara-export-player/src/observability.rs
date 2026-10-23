//! Optional development observability; compiled out of release templates.
use ara_bridge::{LogRecord, NoHooks, Server, Telemetry};
use std::net::SocketAddr;

/// Starts an authenticated bridge only for dev/profile launch configurations.
/// Tokens/ports arrive through the host's launch environment, never a pack.
pub fn start(config: &str) -> Result<Option<Server>, String> {
    if !matches!(config, "dev" | "profile") {
        return Ok(None);
    }
    let Some(token) = std::env::var("ARA_BRIDGE_TOKEN").ok() else {
        return Ok(None);
    };
    let port = std::env::var("ARA_BRIDGE_PORT")
        .unwrap_or_else(|_| "43871".into())
        .parse::<u16>()
        .map_err(|e| e.to_string())?;
    Server::start(SocketAddr::from(([127, 0, 0, 1], port)), token, NoHooks)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Reads opt-in Android dev bridge settings from app-private `ara-bridge.json`.
/// Push this file through `adb shell run-as PACKAGE` before launching a dev APK.
pub fn start_private(directory: &std::path::Path) -> Result<Option<Server>, String> {
    #[derive(serde::Deserialize)]
    struct Settings {
        token: String,
        port: u16,
    }
    let path = directory.join("ara-bridge.json");
    if !path.is_file() {
        return Ok(None);
    }
    let settings: Settings =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    Server::start(
        SocketAddr::from(([127, 0, 0, 1], settings.port)),
        settings.token,
        NoHooks,
    )
    .map(Some)
    .map_err(|e| e.to_string())
}

/// Publishes a conformance-run result without blocking a simulation tick.
pub fn report(server: &Server, report: &crate::runtime::RunReport) {
    server.publisher().try_publish(Telemetry {
        logs: vec![LogRecord {
            sequence: 0,
            tick: report.ticks,
            level: "info".into(),
            target: "ara-export-player".into(),
            message: "export smoke run completed".into(),
            fields: std::collections::BTreeMap::from([
                ("verified".into(), report.verified.into()),
                ("pack_id".into(), report.pack_id.clone().into()),
                ("cs_mean_ms".into(), report.tick.cs_mean_ms.into()),
            ]),
        }],
        frame_us: vec![],
        gpu_us: vec![],
    });
}
