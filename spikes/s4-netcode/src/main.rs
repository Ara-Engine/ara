//! Ara S4: headless multiplayer experiment, not an engine API.
#[cfg(feature = "network")]
mod adapter;
#[cfg(feature = "network")]
mod conditioner;
mod gameplay;
#[cfg(feature = "network")]
mod harness;
mod local;
#[cfg_attr(not(feature = "network"), allow(dead_code))]
mod schema;
#[cfg(feature = "network")]
mod session;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("local") => println!("{}", serde_json::to_string_pretty(&local::run(600)?)?),
        Some("bench") => println!("{}", serde_json::to_string_pretty(&local::benchmark())?),
        #[cfg(feature = "network")]
        Some("server") => session::server(std::path::Path::new(args.get(2).ok_or("missing runtime directory")?))?,
        #[cfg(feature = "network")]
        Some("client") => session::client(std::path::Path::new(args.get(2).ok_or("missing runtime directory")?), args.get(3).ok_or("missing player")?.parse()?)?,
        #[cfg(feature = "network")]
        Some("replay") => println!("{}", session::replay_file(std::path::Path::new(args.get(2).ok_or("missing replay path")?))?),
        #[cfg(feature = "network")]
        Some("harness") => {
            let mode = args.get(2).map_or("dedicated4", String::as_str);
            let impaired = args.get(3).is_none_or(|value| value != "clean");
            let seed = args.get(4).map_or(Ok(0x54a4_u64), |value| value.parse())?;
            let output = args.get(5).map_or_else(|| std::path::PathBuf::from(format!("artifacts/{mode}-{}.json", if impaired { "impaired" } else { "clean" })), std::path::PathBuf::from);
            let report = harness::run(mode, impaired, seed, &output)?;
            println!("{mode}: PASS; {} remote clients, 600 ticks, hash {}, replay/privacy/convergence/late-join PASS; report {}", report.clients.len(), report.server.state_hash, output.display());
        }
        _ => return Err("usage: ara-s4-netcode local | bench | harness [dedicated3|dedicated4|dedicated4-live|listen] [impaired|clean] [seed] [report.json] | replay <file> | server <dir> | client <dir> <player>".into()),
    }
    Ok(())
}
