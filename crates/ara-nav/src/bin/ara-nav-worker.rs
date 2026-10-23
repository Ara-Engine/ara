//! Low-priority native navigation bake worker, spawned by `BakeJob`.
fn main() -> std::process::ExitCode {
    match ara_nav::worker_main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
