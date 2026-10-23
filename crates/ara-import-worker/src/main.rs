//! Disposable below-normal-priority process hosting the builtin asset importers.
fn main() -> std::process::ExitCode {
    match ara_importers::registry().and_then(|registry| ara_asset::serve_worker(&registry)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
