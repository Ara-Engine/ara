//! Agent-style process lifecycle, socket impairment and first-field convergence checks.
use crate::{
    adapter::{self, Registry},
    conditioner::{DirectionStats, Link},
    gameplay,
    session::{self, ClientReport, Config, ServerReport},
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// All success and error paths kill and reap the processes this harness owns.
struct Children(Vec<Child>);
impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

fn spawn(directory: &Path, role: &str, player: Option<usize>) -> Result<Child> {
    let name = player.map_or_else(|| role.to_owned(), |id| format!("{role}-{id}"));
    let log = fs::File::create(directory.join(format!("{name}.log")))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(role)
        .arg(directory)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    if let Some(player) = player {
        command.arg(player.to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    Ok(command.spawn()?)
}

#[derive(Serialize)]
pub struct ClientMeasurement {
    pub player: usize,
    pub upstream: DirectionStats,
    pub downstream: DirectionStats,
    pub upstream_udp_bytes_per_second: f64,
    pub downstream_udp_bytes_per_second: f64,
    pub state: ClientReport,
    pub converged: bool,
}
#[derive(Serialize)]
pub struct Report {
    pub mode: String,
    pub profile: String,
    pub seed: u64,
    pub runtime_artifacts: PathBuf,
    pub server: ServerReport,
    pub clients: Vec<ClientMeasurement>,
    pub bandwidth_window_seconds: f64,
    pub late_join_pass: bool,
    pub local_replay_match: bool,
    pub ideal_script_outcome_match: bool,
    pub prediction_lead_ticks: u32,
    pub replay_pass: bool,
    pub privacy_pass: bool,
    pub convergence_pass: bool,
    pub measured_wall_seconds: f64,
}

pub fn run(mode: &str, impaired: bool, seed: u64, output: &Path) -> Result<Report> {
    let (listen, clients, late) = match mode {
        "dedicated3" => (false, 3, false),
        "dedicated4" => (false, 4, true),
        "dedicated4-live" => (false, 4, true),
        "listen" => (true, 4, true),
        _ => return Err("expected dedicated3, dedicated4, dedicated4-live or listen".into()),
    };
    let live = mode == "dedicated4-live";
    let prediction_lead_ticks = if live { 6 } else { 12 };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts/runtime")
        .join(format!("{mode}-{seed}-{stamp}"));
    fs::create_dir_all(&directory)?;
    let mut links = (0..clients)
        .map(|id| Link::new(seed.wrapping_add(id as u64 * 997 + 1), impaired))
        .collect::<std::io::Result<Vec<_>>>()?;
    let proxies = links
        .iter()
        .map(Link::address)
        .collect::<std::io::Result<Vec<_>>>()?;
    session::write_json(
        directory.join("config.json"),
        &Config {
            proxies,
            listen,
            clients,
            late,
            seed,
            impaired,
            prediction_lead_ticks,
        },
    )?;
    let mut children = Children(vec![spawn(&directory, "server", None)?]);
    let boot = Instant::now();
    while !directory.join("ready.json").exists() {
        if let Some(status) = children.0[0].try_wait()? {
            return Err(format!("server failed {status}; inspect {}", directory.display()).into());
        }
        if boot.elapsed() > Duration::from_secs(10) {
            return Err("server readiness deadline exceeded".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let address = session::read_json(directory.join("ready.json"))?;
    for link in &mut links {
        link.set_server(address);
    }
    for player in usize::from(listen)..clients - usize::from(late) {
        children.0.push(spawn(&directory, "client", Some(player))?);
    }
    let mut started = None;
    let mut late_started = !late;
    let mut measurement_start = None;
    let mut measurement_end = None;
    loop {
        let instant = Instant::now();
        if boot.elapsed() > Duration::from_secs(35) {
            return Err(
                format!("harness deadline exceeded; inspect {}", directory.display()).into(),
            );
        }
        if started.is_none() && directory.join("started.json").exists() {
            started = Some(instant);
        }
        let elapsed = started
            .map(|time| instant.duration_since(time))
            .unwrap_or_default();
        if !late_started && elapsed >= Duration::from_secs(3) {
            children
                .0
                .push(spawn(&directory, "client", Some(clients - 1))?);
            late_started = true;
        }
        let measure = started.is_some()
            && elapsed >= Duration::from_secs(5)
            && elapsed < Duration::from_secs(9);
        if measure && measurement_start.is_none() {
            measurement_start = Some(instant);
        }
        if !measure && measurement_start.is_some() && measurement_end.is_none() {
            measurement_end = Some(instant);
        }
        for link in &mut links {
            link.pump(instant, measure)?;
        }
        let mut all_done = late_started;
        for child in &mut children.0 {
            match child.try_wait()? {
                Some(status) if !status.success() => {
                    return Err(format!(
                        "child {} failed {status}; inspect {}",
                        child.id(),
                        directory.display()
                    )
                    .into());
                }
                Some(_) => {}
                None => all_done = false,
            }
        }
        if all_done {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let server: ServerReport = session::read_json(directory.join("server.json"))?;
    let registry = Registry::new();
    registry.validate()?;
    let bandwidth_window_seconds = measurement_end
        .ok_or("measurement window did not end")?
        .duration_since(measurement_start.ok_or("measurement window did not start")?)
        .as_secs_f64();
    let mut measurements = Vec::new();
    let mut late_join_pass = !late;
    for (player, link) in links.into_iter().enumerate().skip(usize::from(listen)) {
        let client: ClientReport =
            session::read_json(directory.join(format!("client-{player}.json")))?;
        let expected = registry.project(&server.state, player);
        if let Some(difference) = adapter::first_difference(&expected, &client.state) {
            return Err(format!("client {player}: {difference}").into());
        }
        if client.final_predicted_motor != expected.motors[player] {
            return Err(format!("tick {} client {player} predicted motor", expected.tick).into());
        }
        if late && player == clients - 1 {
            late_join_pass = client.first_baseline_tick >= 180
                && client.baseline_door_open
                && client.baseline_collected_items == 3;
            if !late_join_pass {
                return Err(format!(
                    "late baseline inconsistent at tick {}",
                    client.first_baseline_tick
                )
                .into());
            }
        }
        measurements.push(ClientMeasurement {
            player,
            upstream_udp_bytes_per_second: link.up.measurement_udp_bytes as f64
                / bandwidth_window_seconds,
            downstream_udp_bytes_per_second: link.down.measurement_udp_bytes as f64
                / bandwidth_window_seconds,
            upstream: link.up,
            downstream: link.down,
            state: client,
            converged: true,
        });
    }
    let mut expected_local = gameplay::Simulation::new();
    for tick in 1..=session::TICKS {
        expected_local.step(&gameplay::scripted_frame(tick, clients));
    }
    let local_state = expected_local.capture();
    let mut ideal_script_outcome_match = true;
    for player in 0..clients {
        let mut expected = registry.project(&local_state, player);
        // Receipt latency may shift intent execution; replay checks exact assigned ticks.
        expected.door.changed = server.state.door.changed;
        if let Some(difference) =
            adapter::first_difference(&expected, &registry.project(&server.state, player))
        {
            ideal_script_outcome_match = false;
            if !live {
                return Err(format!("local/{mode} outcome differs: {difference}").into());
            }
        }
    }
    if listen {
        let host = server
            .host_projection
            .as_ref()
            .ok_or("listen host missing endpoint projection")?;
        if let Some(difference) =
            adapter::first_difference(&registry.project(&server.state, 0), host)
        {
            return Err(format!("listen host: {difference}").into());
        }
    }
    let replay_hash = session::replay_file(&directory.join("replay.json"))?;
    if replay_hash != server.state_hash {
        return Err("persisted replay hash mismatch".into());
    }
    if server.nonzero_input_ticks == 0 {
        return Err("network test did not exercise authoritative movement".into());
    }
    if live
        && !measurements
            .iter()
            .any(|measurement| !measurement.state.corrections_mm.is_empty())
    {
        return Err("tight prediction window did not exercise any corrections".into());
    }
    let privacy_pass = measurements.iter().all(|measurement| {
        measurement.state.foreign_inventory_values == 0
            && measurement.state.inventory_values_received > 0
    });
    if !privacy_pass {
        return Err("inventory privacy assertion failed".into());
    }
    let report = Report {
        mode: mode.into(),
        profile: if impaired {
            "100ms RTT / +/-20ms one-way jitter / 5% loss each direction"
        } else {
            "loopback, no impairment"
        }
        .into(),
        seed,
        runtime_artifacts: directory,
        server,
        clients: measurements,
        bandwidth_window_seconds,
        late_join_pass,
        local_replay_match: true,
        ideal_script_outcome_match,
        prediction_lead_ticks,
        replay_pass: true,
        privacy_pass,
        convergence_pass: true,
        measured_wall_seconds: boot.elapsed().as_secs_f64(),
    };
    session::write_json(output, &report)?;
    Ok(report)
}
