//! `ara`: the Ara command-line tool and daemon.
//!
//! - Files, offline: `fmt`, `check`, `schema`, `merge` (git merge driver),
//!   `docs --llms`, `init`.
//! - Engine: `serve` (the project daemon), `mcp` (MCP over stdio).
//! - Protocol commands, through the daemon when it runs, else in-process: `do`,
//!   `query`, `inspect`, `history`, `undo`, `redo`, `review`, `accept`, `revert`,
//!   `logs`. Each runs in its own session attributed to `--author` (default human).
//!
//! Every command takes `--json`. The project root is `--root`, else the nearest
//! ancestor with `ara.project.json`; without one, commands fail with ARA1901.
//! Exit codes: 0 success, 1 problems found (errors, conflicts, unformatted files),
//! 2 the command could not run.

mod commands;
mod engine;
mod project;

use std::path::PathBuf;
use std::process::ExitCode;

use ara_diag::{Diagnostic, codes};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use commands::protocol::{self, Call};

/// Ara command-line tool.
#[derive(Parser)]
#[command(name = "ara", version, about)]
struct Cli {
    /// Project root (default: the nearest ancestor with `ara.project.json`).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Print results as JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Who protocol commands are attributed to: `human` or `agent:<name>`.
    #[arg(long, global = true, default_value = "human")]
    author: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Local performance regression checks (no daemon or project required).
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Create a project: ara.project.json, AGENTS.md, .gitattributes, .gitignore,
    /// .mcp.json and a starter scene.
    Init {
        /// Directory (default: the current one).
        dir: Option<PathBuf>,
        /// Project name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
    },
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
        /// Documents to check (default: every project document).
        files: Vec<PathBuf>,
    },
    /// Show the Schema DB: types, document kinds and commands.
    Schema {
        /// Write `reflection.json` and JSON Schemas into this directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Three-way merge of a document; writes the result over `ours` (git: `%O %A %B`).
    Merge {
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
    /// Generate docs for this engine version: llms.txt, llms-full.txt, per-type pages.
    Docs {
        /// The llms.txt format (the only one so far).
        #[arg(long, required = true)]
        llms: bool,
        /// Output directory (default: `.ara/docs`).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Run the project daemon (Engine Protocol on WebSocket, MCP at /mcp).
    Serve {
        /// Port on 127.0.0.1 (default: any free one, written to `.ara/daemon.json`).
        #[arg(long)]
        port: Option<u16>,
        /// Also serve the Engine Protocol on stdin/stdout.
        #[arg(long)]
        stdio: bool,
    },
    /// Serve MCP on stdio (uses the daemon, or becomes it).
    Mcp,
    /// Run one command as a transaction: `ara do entity.create --doc levels/a.scene --param name=Crate`.
    Do {
        /// Command name (`entity.create`; list them with `ara schema`).
        command: String,
        /// Document to edit.
        #[arg(long)]
        doc: String,
        /// Arguments as a JSON object.
        #[arg(long)]
        args: Option<String>,
        /// One argument: `key=value` (JSON value or string; `a.b=1` nests). Repeatable.
        #[arg(long = "param", short = 'p')]
        params: Vec<String>,
        /// Symbolic ID for the record the command creates (`$crate`).
        #[arg(long = "as")]
        symbol: Option<String>,
        /// Why, shown in review.
        #[arg(long)]
        why: Option<String>,
        /// Show the diff without applying.
        #[arg(long)]
        dry_run: bool,
    },
    /// Find entities: `ara query "Light name:Lamp*"`.
    Query {
        /// Compact query (`Light`, `!Light`, `name:Crate*`, `Light.intensity>500`, ...).
        q: Option<String>,
        /// Only this document.
        #[arg(long)]
        doc: Option<String>,
        /// Field paths to show, comma-separated.
        #[arg(long, value_delimiter = ',')]
        fields: Vec<String>,
        /// Page size.
        #[arg(long)]
        limit: Option<u32>,
        /// Cursor from the previous page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show a document's hierarchy, or entities in full.
    Inspect {
        /// Document.
        #[arg(long)]
        doc: String,
        /// Entities to show in full.
        entities: Vec<String>,
        /// Hierarchy depth.
        #[arg(long)]
        depth: Option<u32>,
    },
    /// List transactions, newest first.
    History {
        /// Only this document.
        #[arg(long)]
        doc: Option<String>,
        /// Only transactions by this author (`human`, `agent:claude`).
        #[arg(long = "by")]
        by: Option<String>,
        /// How many.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Undo `--author`'s latest transaction, or the given one.
    Undo {
        /// Transaction (`tx_...`).
        txn: Option<String>,
    },
    /// Redo `--author`'s most recently undone transaction.
    Redo,
    /// List agent changes awaiting review.
    Review,
    /// Accept agent changes: a transaction, an entity's changes, or `--all`.
    Accept {
        /// Transaction (`tx_...`).
        txn: Option<String>,
        /// Only this entity's changes.
        #[arg(long)]
        entity: Option<String>,
        /// Every unreviewed transaction.
        #[arg(long)]
        all: bool,
    },
    /// Revert agent changes: a transaction or an entity's changes.
    Revert {
        /// Transaction (`tx_...`).
        txn: Option<String>,
        /// Only this entity's changes.
        #[arg(long)]
        entity: Option<String>,
    },
    /// Show diagnostics from the engine's log.
    Logs {
        /// Only entries after this cursor.
        #[arg(long)]
        since: Option<u64>,
        /// Only these codes (`ARA1101`). Repeatable.
        #[arg(long = "code")]
        codes: Vec<String>,
        /// Minimum severity: error, warning, info.
        #[arg(long)]
        severity: Option<String>,
        /// Only entries about this entity.
        #[arg(long)]
        entity: Option<String>,
        /// How many.
        #[arg(long)]
        limit: Option<u32>,
    },
}

/// Drops `null` fields so optional params stay absent.
fn params(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.into_iter().filter(|(_, v)| !v.is_null()).collect()),
        other => other,
    }
}

/// The protocol call for a protocol command; `None` for file and engine commands.
fn protocol_call(command: Command) -> Option<Result<Call, Vec<Diagnostic>>> {
    let summary = protocol::pretty;
    Some(Ok(match command {
        Command::Do {
            command,
            doc,
            args,
            params: kv,
            symbol,
            why,
            dry_run,
        } => Call {
            method: "cmd.apply",
            params: params(json!({
                "doc": doc,
                "why": why,
                "dryRun": dry_run,
                "commands": [params(json!({
                "cmd": command,
                "args": match protocol::args(args.as_deref(), &kv) {
                    Ok(a) => a,
                    Err(d) => return Some(Err(d)),
                },
                "as": symbol,
            }))],
            })),
            summary: protocol::applied,
        },
        Command::Query {
            q,
            doc,
            fields,
            limit,
            cursor,
        } => Call {
            method: "query",
            params: params(json!({
                "q": q, "doc": doc, "limit": limit, "cursor": cursor,
                "fields": (!fields.is_empty()).then_some(fields),
            })),
            summary,
        },
        Command::Inspect {
            doc,
            entities,
            depth,
        } => Call {
            method: "inspect",
            params: params(json!({
                "doc": doc, "depth": depth,
                "entities": (!entities.is_empty()).then_some(entities),
            })),
            summary,
        },
        Command::History { doc, by, limit } => Call {
            method: "history.list",
            params: params(json!({ "doc": doc, "author": by, "limit": limit })),
            summary,
        },
        Command::Undo { txn } => Call {
            method: "history.undo",
            params: params(json!({ "txn": txn })),
            summary: protocol::undone,
        },
        Command::Redo => Call {
            method: "history.redo",
            params: json!({}),
            summary: protocol::undone,
        },
        Command::Review => Call {
            method: "review.list",
            params: json!({}),
            summary,
        },
        Command::Accept { txn, entity, all } => Call {
            method: "review.accept",
            params: params(json!({ "txn": txn, "entity": entity, "all": all.then_some(true) })),
            summary: protocol::reviewed,
        },
        Command::Revert { txn, entity } => Call {
            method: "review.revert",
            params: params(json!({ "txn": txn, "entity": entity })),
            summary: protocol::reviewed,
        },
        Command::Logs {
            since,
            codes,
            severity,
            entity,
            limit,
        } => Call {
            method: "logs",
            params: params(json!({
                "since": since, "severity": severity, "entity": entity, "limit": limit,
                "codes": (!codes.is_empty()).then_some(codes),
            })),
            summary,
        },
        _ => return None,
    }))
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Compare saved JSON summaries; exit 1 for regressions or incomplete evidence.
    Compare {
        /// Saved baseline summary.
        baseline: PathBuf,
        /// Current summary.
        current: PathBuf,
        /// Allowed fractional p95 increase.
        #[arg(long, default_value_t = 0.05)]
        relative: f64,
        /// Allowed absolute p95 increase in nanoseconds.
        #[arg(long, default_value_t = 1000)]
        absolute_ns: u64,
    },
}

fn run(cli: Cli) -> Result<bool, Diagnostic> {
    let json = cli.json;
    let root_flag = cli.root.as_deref();
    let open = |files: &[PathBuf]| project::open(root_flag, files);
    let io = |msg: String| Diagnostic::error(codes::IO, msg);
    match cli.command {
        Command::Profile { command: ProfileCommand::Compare { baseline, current, relative, absolute_ns } } => {
            let baseline = ara_profile::read_summary(&baseline).map_err(|e| io(e.to_string()))?;
            let current = ara_profile::read_summary(&current).map_err(|e| io(e.to_string()))?;
            let report = ara_profile::compare(&baseline, &current, relative, absolute_ns).map_err(|e| io(e.into()))?;
            if json {
                let value = serde_json::to_value(&report).map_err(|e| io(e.to_string()))?;
                print!("{}", ara_json::to_canonical_string(&value));
            } else { print!("{}", report.table()); }
            Ok(report.passed)
        }
        Command::Init { dir, name } => {
            commands::init(dir.as_deref(), name.as_deref(), json).map_err(io)
        }
        Command::Fmt { check, files } => {
            commands::fmt(&open(&files)?, &files, check, json).map_err(io)
        }
        Command::Check { files } => commands::check(&open(&files)?, &files, json).map_err(io),
        Command::Schema { out } => commands::schema(&open(&[])?, json, out.as_deref()).map_err(io),
        Command::Merge {
            output,
            base,
            ours,
            theirs,
        } => {
            let files = [base, ours, theirs];
            let project = open(&files)?;
            let [base, ours, theirs] = &files;
            commands::merge(&project, [base, ours, theirs], output.as_deref(), json).map_err(io)
        }
        Command::Docs { llms: _, out } => {
            commands::docs(&open(&[])?, out.as_deref(), json).map_err(io)
        }
        Command::Serve { port, stdio } => {
            commands::serve(&project::root(root_flag, &[])?, port, stdio, json).map_err(io)
        }
        Command::Mcp => commands::mcp(&project::root(root_flag, &[])?).map_err(io),
        other => {
            let root = project::root(root_flag, &[])?;
            match protocol_call(other) {
                Some(Ok(call)) => protocol::run(&root, &cli.author, json, call).map_err(io),
                Some(Err(diags)) => {
                    engine::print_diags(&diags, json);
                    Ok(false)
                }
                None => Err(io("not a protocol command".into())),
            }
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(d) => {
            engine::print_diags(&[d], json);
            ExitCode::from(2)
        }
    }
}
