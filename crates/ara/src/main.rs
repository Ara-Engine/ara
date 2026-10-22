//! `ara`: the Ara command-line tool.
//!
//! Every engine workflow (check, test, run, export, agent access) gets a subcommand
//! here, starting in M1 (see docs/plan/milestones.md). Until then it only reports
//! its version.

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => println!("ara {version}"),
        _ => {
            println!("ara {version} (pre-M0)");
            println!("No commands yet. See docs/plan/milestones.md.");
        }
    }
}
