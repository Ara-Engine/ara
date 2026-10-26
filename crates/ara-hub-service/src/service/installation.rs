//! Installer registration shares the authority and lock without starting a listener.
use super::*;

/// The installed build's registry change, preserving projects and version pins.
#[derive(Clone, Copy)]
pub enum EngineRegistration {
    /// Add or refresh this build's executable paths.
    Register,
    /// Remove only this exact build, refusing open project pins.
    Remove,
}

/// Call a running Hub or briefly open its registry offline. Never spawn a service.
pub fn update_installed_engine(
    config: HubConfig,
    operation: EngineRegistration,
) -> Result<Value, RpcError> {
    crate::validate_development_config(&config.directory).map_err(io_error)?;
    crate::isolation::validate_engine(&config.directory, &config.engine.executable)
        .map_err(io_error)?;
    fs::create_dir_all(&config.directory).map_err(io_error)?;
    let _start = transport::startup_lock(&config.directory)?;
    let method = match operation {
        EngineRegistration::Register => "hub.engine.register",
        EngineRegistration::Remove => "hub.engine.remove",
    };
    let params = json!({"engine":config.engine});
    if let Some(mut client) = Client::for_project(&config.directory.join("hub")) {
        return client.call(method, params);
    }
    // The offline authority holds hub.lock and uses the RPC validation/lifecycle rules.
    // A concurrent legacy startup may refuse this open; it never permits two writers.
    Hub::open_for_installation(config)?.call(method, params)
}
