//! Register the same asynchronous asset authority for offline and daemon commands.
use std::sync::Arc;

pub(crate) fn register(host: &mut ara_host::Host) -> Result<(), ara_diag::Diagnostic> {
    let root = host.root();
    let config = ara_play_host::PlayConfig::for_project(root);
    let worker = ara_asset::ProcessWorker::new(config.import_worker)
        .with_memo(Some(&root.join(".ara/cache")));
    host.register(Box::new(ara_asset_host::AssetService::new(
        worker,
        Arc::new(|path, required| {
            ara_update::check_space(path, required)
                .map(|_| ())
                .map_err(|error| ara_asset::Error::Invalid(error.to_string()))
        }),
    )))
}
