//! `ara`: the Ara command-line tool and daemon.
//!
//! - Files, offline: `fmt`, `check [--fix]`, `schema`, `merge` (git merge driver),
//!   `resolve` (settle merge conflicts), `docs --llms`, `init`.
//! - Engine: `serve` (the project daemon), `mcp` (MCP over stdio).
//! - Protocol commands, through the daemon when it runs, else in-process: `do`,
//!   `query`, `inspect`, `history`, `undo`, `redo`, `review`, `accept`, `revert`,
//!   `logs`, `why`. Each runs in its own session attributed to `--author` (default
//!   human).
//!
//! Every command takes `--json`. The project root is `--root`, else the nearest
//! ancestor with `ara.project.json`; without one, commands fail with ARA1901.
//! Protocol commands are attributed to `--author`, else `ARA_AUTHOR`, else the agent
//! running the CLI (Codex, Claude Code), else `human`.
//! Exit codes: 0 success, 1 problems found (errors, conflicts, unformatted files),
//! 2 the command could not run.

mod commands;
mod engine;
mod project;

// Delegates directly to System unless ara-profile/allocation is enabled.
#[global_allocator]
static ALLOCATOR: ara_profile::CountingAllocator<std::alloc::System> =
    ara_profile::CountingAllocator(std::alloc::System);
mod assets;
mod relaxed;

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
    /// Who protocol commands are attributed to: `human` or `agent:<name>` (default:
    /// `ARA_AUTHOR`, else the agent running this CLI, else `human`).
    #[arg(long, global = true)]
    author: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check installed editor prerequisites without a project.
    Doctor,
    /// Local performance regression checks (no daemon or project required).
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Create a project: ara.project.json, AGENTS.md, .gitattributes, .gitignore,
    /// .mcp.json and a starter scene.
    #[command(visible_alias = "new")]
    Init {
        /// Directory (default: the current one).
        dir: Option<PathBuf>,
        /// Project name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
        /// Configure the repository's merge driver even for a nested project.
        #[arg(long)]
        configure_git: bool,
    },
    /// Inspect or migrate the project's on-disk layout.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
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
        /// Apply every diagnostic's fix when it has exactly one (renames, values),
        /// then check again.
        #[arg(long)]
        fix: bool,
        /// Documents to check (default: every project document).
        files: Vec<PathBuf>,
    },
    /// Show the Schema DB: types, document kinds and commands; with names, each one's
    /// fields, units and defaults (`ara schema Light entity.create`).
    Schema {
        /// Types, commands or methods to describe.
        names: Vec<String>,
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
    /// Settle the conflicts `ara merge` left in a document: `ara resolve levels/a.scene --pick e_b2:name=theirs` (or `--ours` / `--theirs` for all).
    Resolve {
        /// The conflicted document.
        file: PathBuf,
        /// Keep our side of every conflict not picked.
        #[arg(long, conflicts_with = "theirs")]
        ours: bool,
        /// Keep their side of every conflict not picked.
        #[arg(long)]
        theirs: bool,
        /// One conflict: `<record>:<path>=ours|theirs|base` (`<record>=…` for a whole
        /// record). Repeatable.
        #[arg(long = "pick")]
        picks: Vec<String>,
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
    /// Serve global MCP through Ara Hub; --root retains per-project mode.
    Mcp,
    /// Manage the user-level Ara Hub and global MCP installation.
    Hub {
        #[command(subcommand)]
        command: commands::hub::HubCommand,
    },
    /// Run one command as a transaction: `ara do property.set -p entity=name:Crate -p path=Light.intensity -p value=500 --why "Dimmer crate light"`.
    Do {
        /// Command name (`entity.create`; list them with `ara schema`). Without one,
        /// `--args` is a whole batch: `{commands: [{cmd, args, as}], why, doc}`.
        command: Option<String>,
        /// Document to edit (default: the one holding the entities the arguments
        /// name, else the project's main scene). `outpost` is enough.
        #[arg(long)]
        doc: Option<String>,
        /// Arguments: a JSON object, `@file.json`, or `-` for stdin. Quotes are
        /// optional (`{entity: e_8b21, value: [1, 0, 0, 1]}`), so shells that strip
        /// them (PowerShell) are fine.
        #[arg(long)]
        args: Option<String>,
        /// One argument: `key=value`. Values are JSON (`[0,1,0]`, `true`) or text; a
        /// list may skip quotes (`entities=[e_1,e_2]`); `a.b=1` nests. Repeatable.
        #[arg(long = "param", short = 'p')]
        params: Vec<String>,
        /// Symbolic ID for the record the command creates (`$crate`).
        #[arg(long = "as")]
        symbol: Option<String>,
        /// Why, in a sentence, shown in review (required for agents).
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
        /// Document (default: the one holding `entities`, else the main scene).
        #[arg(long)]
        doc: Option<String>,
        /// Entities to show in full: IDs or names (`e_8b21`, `name:Crate`, `Crate`).
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
    /// Apply shipped C# analyzer fixes through undoable typed source commands.
    Fix(commands::fix::FixArgs),
    /// Run the project or a scene headless, returning results, logs and diagnostics.
    Run(commands::headless::RunArgs),
    /// Render a scene or camera offscreen into a PNG; never opens a window.
    Capture(commands::headless::CaptureArgs),
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
    /// Show diagnostics from every subsystem, grouped by code (`--all`: one by one).
    Logs(commands::why::LogsArgs),
    /// Explain a problem: `ara why name:Crate not visible`, `ara why ARA1302`,
    /// `ara why "is the frame slow"`. Ranked causes with evidence and runnable fixes.
    Why(commands::why::WhyArgs),
}

/// Drops `null` fields so optional params stay absent.
fn params(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(o.into_iter().filter(|(_, v)| !v.is_null()).collect()),
        other => other,
    }
}

/// Who protocol commands are attributed to without `--author`: `ARA_AUTHOR`, else
/// the agent CLI whose shell runs us (Codex first: it may run inside Claude Code),
/// else `human`.
fn default_author() -> String {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if let Ok(a) = std::env::var("ARA_AUTHOR")
        && !a.is_empty()
    {
        a
    } else if set("CODEX_THREAD_ID") {
        "agent:codex".into()
    } else if set("CLAUDECODE") {
        "agent:claude-code".into()
    } else {
        "human".into()
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
        } => {
            let parsed = match protocol::args(args.as_deref(), &kv) {
                Ok(a) => a,
                Err(d) => return Some(Err(d)),
            };
            let batch = match command {
                Some(cmd) => protocol::Batch {
                    commands: json!([params(json!({ "cmd": cmd, "args": parsed, "as": symbol }))]),
                    doc: None,
                    why: None,
                },
                None => match protocol::batch(parsed) {
                    Ok(b) => b,
                    Err(d) => return Some(Err(d)),
                },
            };
            Call {
                method: "cmd.apply",
                params: params(json!({
                    "doc": doc.or(batch.doc),
                    "why": why.or(batch.why),
                    "dryRun": dry_run,
                    "commands": batch.commands,
                })),
                summary: protocol::applied,
            }
        }
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
        Command::Logs(args) => args.call(),
        Command::Why(args) => args.call(),
        Command::Run(args) => args.call(),
        Command::Fix(args) => args.call(),
        Command::Capture(args) => args.call(),
        _ => return None,
    }))
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Unify authored files and migrate legacy references, settings and module manifests.
    Migrate {
        /// Preview all moves and reference rewrites without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Destination for old evidence/tests/helpers (default: sibling <project>-support).
        #[arg(long)]
        support_dir: Option<PathBuf>,
    },
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
        Command::Doctor => Ok(commands::doctor::doctor(json)),
        Command::Profile {
            command:
                ProfileCommand::Compare {
                    baseline,
                    current,
                    relative,
                    absolute_ns,
                },
        } => {
            let baseline = ara_profile::read_summary(&baseline).map_err(|e| io(e.to_string()))?;
            let current = ara_profile::read_summary(&current).map_err(|e| io(e.to_string()))?;
            let report = ara_profile::compare(&baseline, &current, relative, absolute_ns)
                .map_err(|e| io(e.into()))?;
            if json {
                let value = serde_json::to_value(&report).map_err(|e| io(e.to_string()))?;
                print!("{}", ara_json::to_canonical_string(&value));
            } else {
                print!("{}", report.table());
            }
            Ok(report.passed)
        }
        Command::Init {
            dir,
            name,
            configure_git,
        } => commands::init(dir.as_deref(), name.as_deref(), configure_git, json).map_err(io),
        Command::Project {
            command:
                ProjectCommand::Migrate {
                    dry_run,
                    support_dir,
                },
        } => commands::project::migrate(
            &project::root(root_flag, &[])?,
            support_dir.as_deref(),
            dry_run,
            json,
        ),
        Command::Fmt { check, files } => {
            commands::fmt(&open(&files)?, &files, check, json).map_err(io)
        }
        Command::Check { fix, files } => {
            commands::check(&open(&files)?, &files, fix, json).map_err(io)
        }
        Command::Resolve {
            file,
            ours,
            theirs,
            picks,
        } => {
            let all = if ours {
                Some("ours")
            } else {
                theirs.then_some("theirs")
            };
            let project = open(std::slice::from_ref(&file))?;
            commands::resolve(&project, &file, all, &picks, json).map_err(io)
        }
        Command::Schema { names, out } => {
            commands::schema(&open(&[])?, &names, json, out.as_deref()).map_err(io)
        }
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
        Command::Mcp => match root_flag {
            Some(root) => commands::mcp(&project::root(Some(root), &[])?).map_err(io),
            None => commands::hub::global_mcp().map_err(io),
        },
        Command::Hub { command } => match commands::hub::hub(command) {
            Ok(result) => Ok(result),
            Err(error) => {
                engine::print_diags(&error.diagnostics(), json);
                Ok(false)
            }
        },
        other => {
            let root = project::root(root_flag, &[])?;
            match protocol_call(other) {
                Some(Ok(call)) => {
                    let author = cli.author.unwrap_or_else(default_author);
                    protocol::run(&root, &author, json, call).map_err(io)
                }
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
    match ara_crash::monitor_from_args() {
        Ok(true) => return ExitCode::SUCCESS,
        Ok(false) => {}
        Err(error) => {
            eprintln!("Ara crash monitor: {error:#}");
            return ExitCode::from(2);
        }
    }
    let cli = Cli::parse();
    let _capture = if matches!(
        &cli.command,
        Command::Hub {
            command: commands::hub::HubCommand::Serve { .. }
        }
    ) {
        ara_crash::desktop::start("ara-hub-service", false)
            .map_err(|error| eprintln!("ARA-EXP-CRASH-004: {error:#}"))
            .ok()
    } else {
        None
    };
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
