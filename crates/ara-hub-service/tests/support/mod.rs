//! Cleanup uses existing discovery only: it never starts a service to stop one.
use ara_proto::Endpoint;
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub fn shutdown(directory: &Path) -> Result<(), String> {
    ara_hub_service::validate_development_config(directory).map_err(|error| error.to_string())?;
    let discovery = directory.join("hub");
    let mut failure = None;
    if let Some(mut hub) = ara_rpc::Client::for_project(&discovery) {
        if let Ok(projects) = hub.call("hub.projects", json!({})) {
            for project in projects["projects"].as_array().into_iter().flatten() {
                if let Err(error) = hub.call("hub.close", json!({"project":project["id"]})) {
                    failure = Some(error.to_string());
                }
            }
        }
        if let Err(error) = hub.call("hub.shutdown", json!({})) {
            failure = Some(error.to_string());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while ara_rpc::DaemonInfo::read(&discovery).is_some() {
        if Instant::now() >= deadline {
            return Err(format!("test Hub did not stop: {}", directory.display()));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    failure.map_or(Ok(()), Err)
}
