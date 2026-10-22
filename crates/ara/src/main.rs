//! `ara`: the Ara command-line tool.
//!
//! Every engine workflow gets a subcommand here. The kernel set works offline on
//! project files:
//! - `ara fmt <files>`: rewrite documents canonically (repairs references);
//! - `ara check [files]`: schema, reference and prefab validation (`--json`);
//! - `ara schema`: dump the Schema DB (`--json` for `reflection.json`, `--out` for
//!   JSON Schemas);
//! - `ara merge <base> <ours> <theirs>`: semantic three-way merge (git merge driver).
//!
//! Exit codes: 0 success, 1 problems found (errors, conflicts, unformatted files),
//! 2 the command could not run.

mod commands;
mod project;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Ara command-line tool.
#[derive(Parser)]
#[command(name = "ara", version, about)]
struct Cli {
    /// Project root (default: nearest ancestor with `ara.project.json`, else the
    /// current directory).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rewrite documents in canonical form, repairing references.
    Fmt {
        /// Only report files that would change.
        #[arg(long)]
        check: bool,
        /// Documents to format.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Validate documents against the schema, references and prefabs.
    Check {
        /// Print diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Documents to check (default: every project document).
        files: Vec<PathBuf>,
    },
    /// Show the Schema DB: types, document kinds and commands.
    Schema {
        /// Print `reflection.json`.
        #[arg(long)]
        json: bool,
        /// Write `reflection.json` and JSON Schemas into this directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Three-way merge of a document; writes the result over `ours` (git: `%O %A %B`).
    Merge {
        /// Print conflicts as JSON.
        #[arg(long)]
        json: bool,
        /// Write the result here instead of over `ours`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Common ancestor.
        base: PathBuf,
        /// Our version (overwritten with the result).
        ours: PathBuf,
        /// Their version.
        theirs: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let project = match project::Project::open(cli.root.as_deref()) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(2);
        }
    };
    let result = match cli.command {
        Command::Fmt { check, files } => commands::fmt(&project, &files, check),
        Command::Check { json, files } => commands::check(&project, &files, json),
        Command::Schema { json, out } => commands::schema(&project, json, out.as_deref()),
        Command::Merge {
            json,
            output,
            base,
            ours,
            theirs,
        } => commands::merge(&project, [&base, &ours, &theirs], output.as_deref(), json),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(2)
        }
    }
}
