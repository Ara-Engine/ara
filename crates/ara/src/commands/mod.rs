//! Subcommand implementations. Each returns `Ok(true)` on success, `Ok(false)` when
//! it found problems (exit 1) and `Err` when it could not run (exit 2).

mod check;
mod docs;
mod edit;
mod fmt;
pub mod hub;
mod init;
mod merge;
pub mod protocol;
mod resolve;
mod schema;
mod serve;
pub mod why;

pub use check::check;
pub use docs::docs;
pub use fmt::fmt;
pub use init::init;
pub use merge::merge;
pub use resolve::resolve;
pub use schema::schema;
pub use serve::{mcp, serve};

use ara_diag::{Diagnostic, Severity};
use serde_json::{Value, json};

/// Prints diagnostics as text blocks to stdout.
fn print_text(diags: &[Diagnostic]) {
    for d in diags {
        println!("{d}\n");
    }
}

/// "1 file" / "3 files".
fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

/// Counts errors and warnings.
fn tally(diags: &[Diagnostic]) -> (usize, usize) {
    let count = |s: Severity| diags.iter().filter(|d| d.severity == s).count();
    (count(Severity::Error), count(Severity::Warning))
}

/// `{ "files", "errors", "warnings", "diagnostics" }`.
fn json_report(files: usize, diags: &[Diagnostic]) -> Value {
    let (errors, warnings) = tally(diags);
    json!({ "files": files, "errors": errors, "warnings": warnings, "diagnostics": diags })
}
