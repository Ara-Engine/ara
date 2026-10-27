use super::*;
use ara_play::{Control, Controlled, View, ViewCamera};
use ara_play_host::editor_control::{ViewPose, property_ops};
use ara_runtime::components::{FollowCamera, PlayerControl};
use ara_schema::EntityRef;
use glam34::{EulerRot, Mat3, Quat, Vec3};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct PendingControl {
    pub id: Value,
    pub tick: u64,
    pub result: Controlled,
    pub refresh: Vec<String>,
}
fn diagnostics(problems: Vec<ara_diag::Diagnostic>) -> anyhow::Error {
    anyhow!(
        "{}",
        problems
            .iter()
            .map(|problem| problem.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    )
}
fn pose(camera: &ViewCamera) -> Result<ViewPose> {
    let position = Vec3::from_array(camera.position);
    let target = Vec3::from_array(camera.target);
    let forward = (target - position)
        .try_normalize()
        .context("Spectator camera target must differ from its position")?;
    if !position.is_finite()
        || !forward.is_finite()
        || !camera.fov_y_degrees.is_finite()
        || !(10. ..=120.).contains(&camera.fov_y_degrees)
    {
        bail!("Spectator camera requires finite positions and a 10–120 degree FOV");
    }
    let up = if forward.y.abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let right = forward.cross(up).normalize();
    let up = right.cross(forward);
    let rotation = Quat::from_mat3(&Mat3::from_cols(right, up, -forward));
    let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
    Ok(ViewPose {
        position: camera.position,
        rotation: [x.to_degrees(), y.to_degrees(), z.to_degrees()],
        fov_y_degrees: camera.fov_y_degrees,
    })
}
impl<R> World<R> {
    pub(super) fn apply_editor_view(&self, frame: &mut RenderFrame) {
        if let Some(pose) = self.editor_view.pose() {
            let rotation = Quat::from_euler(
                EulerRot::XYZ,
                pose.rotation[0].to_radians(),
                pose.rotation[1].to_radians(),
                pose.rotation[2].to_radians(),
            );
            frame.camera = Some(ara_runtime::extract::CameraView {
                position: pose.position,
                forward: (rotation * Vec3::NEG_Z).to_array(),
                fov: pose.fov_y_degrees,
            });
        }
    }
}
impl World {
    pub(crate) fn spectating(&self) -> bool {
        self.editor_view.pose().is_some()
    }
    /// Advance held navigation on a fixed 60 Hz presentation clock. Only typed
    /// navigation ops change; ECS movement/actions remain owned by simulation.
    pub(crate) fn navigate(
        &mut self,
        input: crate::input::Navigation,
        elapsed: Duration,
    ) -> Result<()> {
        let Some(mut current) = self.editor_view.pose() else {
            return Ok(());
        };
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            current.rotation[0].to_radians(),
            current.rotation[1].to_radians(),
            current.rotation[2].to_radians(),
        );
        let mut forward = rotation * Vec3::NEG_Z;
        if input.look != [0.; 2] {
            let yaw = forward.x.atan2(-forward.z) + input.look[0].to_radians();
            let pitch = forward.y.asin().clamp(-1.553_343, 1.553_343) - input.look[1].to_radians();
            let pitch = pitch.clamp(-1.553_343, 1.553_343);
            forward = Vec3::new(
                yaw.sin() * pitch.cos(),
                pitch.sin(),
                -yaw.cos() * pitch.cos(),
            );
        }
        self.navigation_remainder += elapsed.min(Duration::from_millis(250));
        let step = Duration::from_nanos(1_000_000_000 / 60);
        let ticks = (self.navigation_remainder.as_nanos() / step.as_nanos()).min(15) as u32;
        self.navigation_remainder = self.navigation_remainder.saturating_sub(step * ticks);
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let movement =
            (right * input.movement[0] + Vec3::Y * input.movement[1] + forward * input.movement[2])
                .normalize_or_zero();
        let distance =
            self.editor_view.navigation_speed() * if input.fast { 3. } else { 1. } * ticks as f32
                / 60.;
        if input.look == [0.; 2] && (ticks == 0 || movement == Vec3::ZERO) {
            return Ok(());
        }
        current.position = (Vec3::from_array(current.position) + movement * distance).to_array();
        let camera = ViewCamera {
            position: current.position,
            target: (Vec3::from_array(current.position) + forward).to_array(),
            fov_y_degrees: current.fov_y_degrees,
        };
        self.editor_view
            .set(Some(pose(&camera)?))
            .map_err(diagnostics)?;
        Ok(())
    }
    /// Navigation-only typed ops change extraction, never an authored Camera or Transform.
    pub(crate) fn set_editor_view(&mut self, view: View) -> Result<Value> {
        if let Some(speed) = view.navigation_speed {
            self.editor_view
                .set_navigation_speed(speed)
                .map_err(diagnostics)?;
        }
        self.navigation_remainder = Duration::ZERO;
        let camera = match view.camera {
            Some(camera) => Some(camera),
            None if view.capture_current => {
                let mut frame = RenderFrame::default();
                self.runtime.extract(&mut frame);
                let camera = frame.camera.context(
                    "The running world has no gameplay camera; supply an explicit spectator view",
                )?;
                Some(ViewCamera {
                    position: camera.position,
                    target: (Vec3::from_array(camera.position) + Vec3::from_array(camera.forward))
                        .to_array(),
                    fov_y_degrees: camera.fov.clamp(10., 120.),
                })
            }
            None => None,
        };
        let revision = self
            .editor_view
            .set(camera.as_ref().map(pose).transpose()?)
            .map_err(diagnostics)?;
        Ok(
            json!({"mode":if camera.is_some(){"spectator"}else{"gameplay"},"revision":revision,"camera":camera,"navigationSpeed":self.editor_view.navigation_speed()}),
        )
    }
    /// One input owner relinquishes its old actor(s) and acquires the selected
    /// controllable actor through one validated set of typed command ops.
    pub(crate) fn transfer_control(
        &mut self,
        id: Value,
        control: Control,
    ) -> Result<PendingControl> {
        self.runtime.authoritative_only("play.control")?;
        if control.owner == 0 {
            bail!("A control owner must identify a real player");
        }
        if !self.pending_controls.is_empty() || !self.pending.is_empty() {
            bail!(
                "The previous runtime edit is awaiting its boundary; wait for its Play job to finish"
            );
        }
        let session = self.runtime.play().session();
        let mut actors = BTreeMap::new();
        let mut rigs = BTreeMap::new();
        for entity in session.world().iter_entities() {
            let Some(identity) = entity.get::<ara_sim::Identity>() else {
                continue;
            };
            if let Some(control) = entity.get::<PlayerControl>() {
                actors.insert(identity.0.clone(), control.clone());
            }
            if let Some(rig) = entity.get::<FollowCamera>() {
                rigs.insert(identity.0.clone(), rig.clone());
            }
        }
        if let Some(target) = &control.entity {
            let target = actors
                .get(target)
                .context("Possess requires an entity with PlayerControl")?;
            if target.player != 0 && target.player != control.owner {
                bail!("The selected actor is controlled by another player; eject that owner first");
            }
        }
        let mut previous = Vec::new();
        let mut sets = Vec::new();
        let mut refresh = BTreeSet::new();
        for (identity, actor) in &actors {
            if actor.player == control.owner && control.entity.as_deref() != Some(identity.as_str())
            {
                previous.push(identity.clone());
                refresh.insert(identity.clone());
                sets.push(Set {
                    entity: identity.clone(),
                    path: "PlayerControl.player".into(),
                    value: Some(json!(0)),
                });
            }
        }
        if let Some(target) = &control.entity {
            sets.push(Set {
                entity: target.clone(),
                path: "PlayerControl.player".into(),
                value: Some(json!(control.owner)),
            });
            refresh.insert(target.clone());
            for (identity, rig) in &rigs {
                if rig.player == control.owner {
                    sets.push(Set {
                        entity: identity.clone(),
                        path: "FollowCamera.target".into(),
                        value: Some(json!(EntityRef(target.clone()))),
                    });
                    sets.push(Set {
                        entity: target.clone(),
                        path: "PlayerControl.camera".into(),
                        value: Some(json!(EntityRef(identity.clone()))),
                    });
                    break;
                }
            }
        }
        let workspace = self.runtime.play().session().projection().workspace();
        let ops = property_ops(self.runtime.schema(), workspace, self.scene.as_str(), &sets)
            .map_err(diagnostics)?;
        self.runtime
            .patch_edits(ops.iter().map(ara_doc::Op::to_edit).collect())?;
        let tick = self.runtime.tick() + 1;
        Ok(PendingControl {
            id,
            tick,
            result: Controlled {
                applied_at_tick: tick,
                owner: control.owner,
                entity: control.entity,
                previous,
            },
            refresh: refresh.into_iter().collect(),
        })
    }
    /// Update network ownership from the ECS state after the authoritative control boundary.
    pub(crate) fn adopted_control(&mut self, control: &PendingControl) -> Result<()> {
        for identity in &control.refresh {
            self.runtime.refresh_control_owner(identity)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
