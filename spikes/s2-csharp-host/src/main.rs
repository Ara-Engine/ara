//! `ara-s2`: M0 spike S2. Hosts .NET 10 from Rust, runs C# systems over bevy_ecs chunks, and
//! measures L1 (EnC delta) and L3 (collectible-ALC module swap) reload latency.
//!
//! Subcommands (see RESULT.md for the full sequence):
//! - `gen [--check]`: generate the C# ABI/component/shape bindings from `schema.rs`.
//! - `bench [--debug-il] [--frames N]`: C# vs Rust ns/entity, single-threaded and on the task pool.
//! - `reload [--rounds N] [--tick-hz N]`: the 50-edit save → first-new-tick latency harness.

mod bench;
mod clr;
mod codegen;
mod edits;
mod filler;
mod harness;
mod sched;
mod schema;
mod world;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("gen") => {
            let ok = codegen::run(rest.iter().any(|a| a == "--check"));
            std::process::exit(if ok { 0 } else { 1 });
        }
        Some("bench") => bench::run(rest),
        Some("reload") => harness::run(rest),
        _ => {
            eprintln!("usage: ara-s2 <gen [--check] | bench [--debug-il] | reload [--rounds N]>");
            std::process::exit(2);
        }
    }
}

/// Value of `--name N` in `args`, or `default`.
pub fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// True if `--name` is present.
pub fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}
