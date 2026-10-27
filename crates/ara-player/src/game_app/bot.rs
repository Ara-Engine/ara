//! Headless co-op driver: observes the real replica and sends ordinary player actions.
use super::*;
use ara_components::Transform;
use ara_runtime::components::PlayerControl;
use ara_sim::Identity;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs::OpenOptions};
mod avoidance;
mod capture;
mod metrics;

pub(super) struct Bot {
    directory: Option<PathBuf>,
    last_input: u64,
    last_report: u64,
    reported: bool,
    inventory_shown: bool,
    inventory_closed: bool,
    inventory_retry: u64,
    threat_test_done: bool,
    captures: BTreeSet<String>,
    won_at: Option<u64>,
    began: Instant,
    capture: Option<capture::Worker>,
}

pub(super) fn component(runtime: &ara_runtime::Runtime, id: &str, name: &str) -> Value {
    runtime
        .play()
        .session()
        .entity(id)
        .and_then(|e| ara_runtime_script::read(runtime.world(), e, name))
        .unwrap_or(Value::Null)
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Bot {
    pub(super) fn profile_hash(&self, world: &World) -> Result<()> {
        let cost = metrics::hash_cost(world)?;
        eprintln!("Outpost hash cost: {cost}");
        if let Some(directory) = &self.directory {
            std::fs::create_dir_all(directory)?;
            std::fs::write(
                directory.join("hash-cost.json"),
                ara_json::to_canonical_string(&cost),
            )?;
        }
        Ok(())
    }
    pub(super) fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            last_input: 0,
            last_report: 0,
            reported: false,
            inventory_shown: false,
            inventory_closed: false,
            inventory_retry: 0,
            threat_test_done: false,
            captures: BTreeSet::new(),
            won_at: None,
            began: Instant::now(),
            capture: None,
        }
    }
    pub(super) fn drive(&mut self, world: &mut World) -> Result<()> {
        if matches!(&world.runtime,Play::Host(h) if h.dedicated()) {
            return Ok(());
        }
        let tick = world.runtime.tick();
        if tick == self.last_input {
            return Ok(());
        }
        self.last_input = tick;
        let player = world.runtime.player();
        let Some((identity, position)) = world.runtime.world().iter_entities().find_map(|e| {
            e.get::<PlayerControl>().filter(|c| c.player == player.0)?;
            Some((
                e.get::<Identity>()?.0.clone(),
                e.get::<Transform>()?.position,
            ))
        }) else {
            return Ok(());
        };
        let mission = component(&world.runtime, "e_run", "SalvageRun");
        if mission["won"] == true {
            self.won_at.get_or_insert(tick);
        }
        let survivor = component(&world.runtime, &identity, "Survivor");
        self.threat_test_done |= mission["revives"].as_u64().unwrap_or(0) > 0;
        let won = mission["won"] == true || mission["lost"] == true;
        let carried = ["circuits", "cells", "radios"]
            .into_iter()
            .map(|f| survivor[f].as_u64().unwrap_or(0))
            .sum::<u64>();
        let quota =
            mission["salvaged"].as_u64().unwrap_or(0) >= mission["goal"].as_u64().unwrap_or(8);
        let mut goal = [96., 0., 85.];
        if !quota || carried == 0 {
            let door = component(&world.runtime, "e_gamedoor0", "SharedDoor");
            if door["open"] != true {
                goal = [98., 0., 103.];
            } else if component(&world.runtime, "e_gamedoor1", "SharedDoor")["open"] != true {
                goal = [104., 0., 92.5];
            } else {
                let mut caches: Vec<_> = world
                    .runtime
                    .world()
                    .iter_entities()
                    .filter_map(|e| {
                        let id = &e.get::<Identity>()?.0;
                        if !id.starts_with("e_gamecache") {
                            return None;
                        }
                        let index = id.trim_start_matches("e_gamecache").parse::<u64>().ok()?;
                        if component(&world.runtime, id, "SalvageCache")["taken"] == true {
                            return None;
                        }
                        Some((
                            !world.runtime.networked() || index % 4 == (player.0 - 1) % 4,
                            id.clone(),
                            e.get::<Transform>()?.position,
                        ))
                    })
                    .collect();
                caches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                if let Some((_, _, position)) = caches.first() {
                    goal = *position;
                }
            }
        } else if mission["activePlayers"].as_u64().unwrap_or(0)
            < if world.runtime.networked() { 4 } else { 1 }
            || !self.views_ready(world)
        {
            goal = [96., 0., 91.5];
        }
        if carried > 0 && self.inventory_closed && !self.captures.contains("door") {
            goal = [99.5, 0., 100.];
        }
        // One scout deliberately tests real drone damage after all inspection
        // captures are complete. The crew must rescue it using ordinary held E.
        let threat_drill = world.runtime.networked()
            && player.0 == 3
            && carried > 0
            && !self.threat_test_done
            && self.views_ready(world);
        if threat_drill {
            goal = [100., 0., 100.];
        }
        // Revive a nearby teammate through the same held E action as a human.
        let mut revive = false;
        for e in world.runtime.world().iter_entities() {
            if !e.contains::<PlayerControl>() {
                continue;
            }
            let Some(id) = e.get::<Identity>() else {
                continue;
            };
            let teammate = component(&world.runtime, &id.0, "Survivor");
            if teammate["active"] == true
                && teammate["health"] == 0
                && id.0 != identity
                && let Some(transform) = e.get::<Transform>()
            {
                goal = transform.position;
                revive = distance(position, goal) < 2.;
                break;
            }
        }
        let mut next = goal;
        if let Some(mesh) = world
            .runtime
            .world()
            .get_resource::<ara_nav::NavWorld>()
            .and_then(|n| n.mesh(0))
            && let Ok(mut query) = mesh.query()
            && let Ok(path) = query.path(position, goal, &Default::default())
            && let Some(corner) = path
                .corners
                .iter()
                .find(|c| distance(position, c.position) > 0.75)
        {
            next = corner.position;
        }
        for entity in world.runtime.world().iter_entities() {
            let Some(identity) = entity.get::<Identity>() else {
                continue;
            };
            if !identity.0.starts_with("e_gamedoor") {
                continue;
            }
            let Some(transform) = entity.get::<Transform>() else {
                continue;
            };
            let Some(collider) = entity.get::<ara_physics::Collider>() else {
                continue;
            };
            if distance(position, transform.position) < 5. {
                next = avoidance::around_box(position, next, goal, transform, collider, 0.55);
            }
        }
        let stopped = won || revive || distance(position, goal) < 1.3 || survivor["health"] == 0;
        let axis = |delta: f32| {
            if delta.abs() < 0.05 {
                0.
            } else {
                delta.signum()
            }
        };
        let movement = if stopped {
            [0., 0.]
        } else {
            [-axis(next[0] - position[0]), axis(next[2] - position[2])]
        };
        let use_button = !won && !threat_drill && (revive || tick % 24 < 6);
        let mut inputs = vec![
            Injection {
                player,
                action: "Move".into(),
                value: ActionValue::Axis2(movement),
                hold_ticks: 12,
            },
            Injection {
                player,
                action: "Sprint".into(),
                value: ActionValue::Button(true),
                hold_ticks: 12,
            },
            Injection {
                player,
                action: "Interact".into(),
                value: ActionValue::Button(use_button),
                hold_ticks: 12,
            },
        ];
        let inventory_open = survivor["inventoryOpen"] == true;
        self.inventory_shown |= inventory_open;
        self.inventory_closed =
            self.inventory_shown && self.captures.contains("inventory") && !inventory_open;
        let want_inventory = !self.captures.contains("inventory");
        if carried > 0 && inventory_open != want_inventory && tick >= self.inventory_retry {
            self.inventory_retry = tick + 120;
            inputs.push(Injection {
                player,
                action: "Inventory".into(),
                value: ActionValue::Button(true),
                hold_ticks: 1,
            });
        }
        world.runtime.input(0, inputs)
    }

    pub(super) fn observe(
        &mut self,
        world: &mut World,
        _presenter: &mut Presenter,
        frame: &mut RenderFrame,
        route: &[String],
    ) -> Result<bool> {
        if let Some(worker) = &mut self.capture
            && let Some(stage) = worker.completed()?
        {
            self.captures.insert(stage);
        }
        let tick = world.runtime.tick();
        let player = if matches!(&world.runtime,Play::Host(h) if h.dedicated()) {
            0
        } else {
            world.runtime.player().0
        };
        for event in world.events() {
            if let ara_play::Event::Diagnostic { diagnostic } = &event {
                anyhow::ensure!(
                    diagnostic["severity"] != "error",
                    "C# gameplay diagnostic: {diagnostic}"
                );
            }
        }
        let mission = component(&world.runtime, "e_run", "SalvageRun");
        anyhow::ensure!(
            mission["lost"] != true,
            "Outpost bot run lost at tick {tick}: {mission}"
        );
        anyhow::ensure!(
            tick < 18_000 && self.began.elapsed() < Duration::from_secs(240),
            "Outpost bot timed out at tick {tick}"
        );
        let doors: Vec<_> = (0..2)
            .map(|i| component(&world.runtime, &format!("e_gamedoor{i}"), "SharedDoor"))
            .collect();
        let survivors: serde_json::Map<_, _> = world
            .runtime
            .world()
            .iter_entities()
            .filter_map(|e| {
                e.get::<PlayerControl>()?;
                let id = e.get::<Identity>()?;
                Some((id.0.clone(), component(&world.runtime, &id.0, "Survivor")))
            })
            .collect();
        let peers_done = self.directory.as_ref().is_none_or(|directory| {
            survivors
                .values()
                .filter(|s| s["active"] == true && s["player"].as_u64() != Some(player))
                .all(|s| {
                    let Some(slot) = s["player"].as_u64() else {
                        return false;
                    };
                    std::fs::read(directory.join(format!("player-{slot}.json")))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .is_some_and(|report| report["completed"] == true)
                })
        });
        let inventory_open = survivors
            .values()
            .any(|s| s["player"].as_u64() == Some(player) && s["inventoryOpen"] == true);
        let at_door_view = world.runtime.world().iter_entities().any(|entity| {
            entity
                .get::<PlayerControl>()
                .is_some_and(|c| c.player == player)
                && entity
                    .get::<Transform>()
                    .is_some_and(|t| distance(t.position, [99.5, 0., 100.]) < 1.8)
        });
        let reviving = survivors
            .values()
            .any(|s| s["player"].as_u64() == Some(player) && s["prompt"] == 3);
        let mut report = json!({"player":player,"tick":tick,"mission":mission,"doors":doors,"survivors":survivors,"route":route,
            "captures":self.captures,"net":world.runtime.stats(),"fault":world.runtime.fault(),"navigationErrors":world.runtime.navigation_errors()});
        report["positions"] = world
            .runtime
            .world()
            .iter_entities()
            .filter_map(|entity| {
                entity.get::<PlayerControl>()?;
                Some((
                    entity.get::<Identity>()?.0.clone(),
                    json!(entity.get::<Transform>()?.position),
                ))
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        if let Some(directory) = &self.directory {
            std::fs::create_dir_all(directory)?;
            if !self.reported || tick >= self.last_report + 30 || mission["won"] == true {
                self.reported = true;
                self.last_report = tick;
                write_report(&directory.join(format!("player-{player}.json")), &report)?;
            }
            // Authorities keep ticking while clients capture. Capture the host only
            // after the final state reached every peer, so uploads cannot stall a run.
            let client = matches!(world.runtime, Play::Mirror(..));
            let stage = if mission["won"] == true
                && (client || peers_done && self.won_at.is_some_and(|first| tick >= first + 90))
            {
                Some("win")
            } else if player != 0 && reviving && !self.captures.contains("revive") {
                Some("revive")
            } else if player != 0 && tick > 120 && !self.captures.contains("hud") {
                Some("hud")
            } else if player != 0 && inventory_open && !self.captures.contains("inventory") {
                Some("inventory")
            } else if player != 0
                && self.inventory_closed
                && !inventory_open
                && at_door_view
                && doors[0]["open"] == true
                && !self.captures.contains("door")
            {
                Some("door")
            } else {
                None
            };
            if let Some(stage) =
                stage.filter(|_| self.capture.as_ref().is_none_or(|w| !w.pending()))
            {
                let lock = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(directory.join("capture.lock"))?;
                if lock.try_lock().is_ok() {
                    if self.capture.is_none() {
                        self.capture = Some(capture::Worker::new(
                            directory.join(format!("gpu-{player}")),
                        )?);
                    }
                    self.capture
                        .as_mut()
                        .context("capture worker")?
                        .start(world, frame, player, stage, directory, lock)?;
                }
            }
        }
        if mission["won"] == true {
            let first = *self.won_at.get_or_insert(tick);
            if tick >= first + 90
                && self
                    .directory
                    .as_ref()
                    .is_none_or(|_| self.captures.contains("win"))
            {
                if !matches!(world.runtime, Play::Mirror(..)) {
                    let live_hash = world.runtime.play().session().state_hash()?;
                    let replay = world.runtime.play_mut().finish_recording()?;
                    anyhow::ensure!(
                        !replay.ticks.is_empty(),
                        "networked run did not record any boundaries"
                    );
                    if let Some(directory) = &self.directory {
                        std::fs::write(directory.join("authority.replay"), replay.to_bytes()?)?;
                    }
                    let hash = world.runtime.replay(&replay)?;
                    anyhow::ensure!(
                        hash == live_hash,
                        "offline replay differs from the final live state"
                    );
                    let portable = ara_sim::Replay::from_bytes(&replay.to_bytes()?)?;
                    let repeated = world.runtime.replay(&portable)?;
                    anyhow::ensure!(repeated == hash, "repeated offline replay diverged");
                    report["replayVerified"] = json!(true);
                    report["determinismPasses"] = json!(2);
                    report["replayCheckpointEvery"] = json!(replay.hash_interval.get());
                    report["replayTicks"] = json!(replay.ticks.len());
                    report["replayHash"] = json!(hash.to_string());
                }
                report["captures"] = json!(self.captures);
                report["completed"] = json!(true);
                if let Some(directory) = &self.directory {
                    write_report(&directory.join(format!("player-{player}.json")), &report)?;
                }
                println!("ARA_OUTPOST_RESULT {report}");
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn returned_to_menu(&self, player: u64, route: &[String]) -> Result<()> {
        if let Some(directory) = &self.directory {
            let path = directory.join(format!("player-{player}.json"));
            let mut report: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            report["route"] = json!(route);
            report["returnedToMenu"] = json!(player != 0);
            write_report(&path, &report)?;
        }
        Ok(())
    }

    fn views_ready(&self, world: &World) -> bool {
        let Some(directory) = &self.directory else {
            return true;
        };
        world
            .runtime
            .world()
            .iter_entities()
            .filter_map(|e| {
                let control = e.get::<PlayerControl>()?;
                let id = e.get::<Identity>()?;
                (component(&world.runtime, &id.0, "Survivor")["active"] == true)
                    .then_some(control.player)
            })
            .all(|player| {
                std::fs::read(directory.join(format!("player-{player}.json")))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .is_some_and(|report| {
                        ["hud", "inventory", "door"].into_iter().all(|stage| {
                            report["captures"]
                                .as_array()
                                .is_some_and(|captures| captures.iter().any(|c| c == stage))
                        })
                    })
            })
    }
}

fn write_report(path: &std::path::Path, report: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(report)?;
    let scratch = path.with_extension("tmp");
    std::fs::write(&scratch, bytes)?;
    for attempt in 0..100 {
        match std::fs::rename(&scratch, path) {
            Ok(()) => return Ok(()),
            Err(error) if error.raw_os_error() == Some(32) && attempt < 99 => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("replace bot report {}", path.display()));
            }
        }
    }
    Ok(())
}
