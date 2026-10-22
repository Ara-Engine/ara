//! S5 spike: play mode in a child process. One binary, three roles:
//!
//! - `gen-scene --count N --out FILE [--seed S]` writes a deterministic JSON scene;
//! - `host --scene FILE --out REPORT [--reps N] [--patches N] [--suites a,b] [--gpu]` is the
//!   editor host: it loads the edit world and runs the measurement suites;
//! - `probe-timers --out FILE` measures this OS's sleep and socket-timeout precision;
//! - `player --connect URL --token T --name N --role R [--gpu-init]` is the child player the host
//!   spawns (never run by hand).

mod crash;
mod gpu;
mod host;
mod model;
mod player;
mod probe;
mod protocol;
mod registry;
mod rpc;
mod scene;
mod shm;
mod snapshot;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use protocol::Role;

/// `--key value` options plus bare `--flag`s.
struct Args {
    values: HashMap<String, String>,
    flags: Vec<String>,
}

impl Args {
    fn parse(raw: &[String], flags: &[&str]) -> Result<Self> {
        let mut values = HashMap::new();
        let mut set = Vec::new();
        let mut it = raw.iter();
        while let Some(arg) = it.next() {
            let Some(key) = arg.strip_prefix("--") else {
                bail!("unexpected argument {arg}");
            };
            if flags.contains(&key) {
                set.push(key.to_owned());
            } else {
                let value = it
                    .next()
                    .with_context(|| format!("--{key} needs a value"))?;
                values.insert(key.to_owned(), value.clone());
            }
        }
        Ok(Self { values, flags: set })
    }

    fn get(&self, key: &str) -> Result<&str> {
        self.values
            .get(key)
            .map(String::as_str)
            .with_context(|| format!("missing --{key}"))
    }

    fn get_or<T: std::str::FromStr>(&self, key: &str, default: T) -> Result<T> {
        match self.values.get(key) {
            Some(v) => v.parse().ok().with_context(|| format!("bad --{key} {v:?}")),
            None => Ok(default),
        }
    }

    fn flag(&self, key: &str) -> bool {
        self.flags.iter().any(|f| f == key)
    }
}

fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = raw
        .split_first()
        .context("usage: ara-s5-play <gen-scene|host|probe-timers|player> ...")?;
    match command.as_str() {
        "gen-scene" => {
            let args = Args::parse(rest, &[])?;
            let count: usize = args.get_or("count", 5000)?;
            let doc = scene::generate(count, args.get_or("seed", 0x5EED)?);
            let out = PathBuf::from(args.get("out")?);
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&out, serde_json::to_vec_pretty(&doc)?)?;
            println!("wrote {count} entities to {}", out.display());
        }
        "host" => {
            let args = Args::parse(rest, &["gpu"])?;
            let suites = match args.values.get("suites") {
                Some(list) => list.split(',').map(str::to_owned).collect(),
                None => host::SUITES.iter().map(|s| (*s).to_owned()).collect(),
            };
            host::run(host::HostArgs {
                scene: args.get("scene")?.into(),
                out: args.get("out")?.into(),
                reps: args.get_or("reps", 20)?,
                patches: args.get_or("patches", 200)?,
                suites,
                gpu: args.flag("gpu"),
            })?;
        }
        "player" => {
            let args = Args::parse(rest, &["gpu-init"])?;
            let role = match args.get("role")? {
                "standalone" => Role::Standalone,
                "server" => Role::Server,
                "client" => Role::Client,
                other => bail!("unknown role {other}"),
            };
            player::run(player::PlayerArgs {
                connect: args.get("connect")?.to_owned(),
                token: args.get("token")?.to_owned(),
                name: args.get("name")?.to_owned(),
                role,
                gpu_init: args.flag("gpu-init"),
            })?;
        }
        "probe-timers" => {
            let args = Args::parse(rest, &[])?;
            probe::run(args.get("out")?.as_ref())?;
        }
        other => bail!("unknown command {other}"),
    }
    Ok(())
}
