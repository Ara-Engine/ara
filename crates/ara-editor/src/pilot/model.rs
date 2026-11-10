//! Capture camera ownership and lower a view pose through its actual parent matrix.
use super::*;
use ara_doc::Op;
use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use std::collections::{BTreeMap, BTreeSet};

struct Node {
    record: Value,
    parent: Option<String>,
    source: (String, String),
}
pub(super) struct Guard {
    pub doc: String,
    pub entity: String,
    pub name: String,
    pub revision: u64,
    nodes: BTreeMap<String, Node>,
    owned: BTreeSet<(String, String)>,
    parent: Mat4,
    local_scale: Vec3,
    world: Mat4,
    world_scale: Vec3,
    fov: f32,
}

fn relative_error(a: Mat4, b: Mat4) -> f32 {
    a.to_cols_array()
        .into_iter()
        .zip(b.to_cols_array())
        .map(|(a, b)| (a - b).abs() / a.abs().max(1.))
        .fold(0., f32::max)
}
fn decomposition(matrix: Mat4) -> ara_ui::Result<(Vec3, Quat, Vec3)> {
    if !matrix.is_finite() || matrix.determinant().abs() < 1e-12 {
        return Err("Pilot requires finite, nonzero camera and parent scales".into());
    }
    let (scale, rotation, position) = matrix.to_scale_rotation_translation();
    if !scale.is_finite()
        || !rotation.is_finite()
        || !position.is_finite()
        || relative_error(
            matrix,
            Mat4::from_scale_rotation_translation(scale, rotation, position),
        ) > 0.0005
    {
        return Err(
            "Pilot cannot represent this sheared camera transform; use a parent with uniform scale"
                .into(),
        );
    }
    Ok((scale, rotation, position))
}
pub(super) fn view_rotation(view: &crate::state::Camera) -> ara_ui::Result<Quat> {
    if !view.is_valid() || !view.target.is_finite() || !view.position().is_finite() {
        return Err("Pilot requires a finite view and a positive navigation radius".into());
    }
    if view.projection != crate::state::CameraProjection::Perspective {
        return Err(
            "Pilot supports perspective Cameras; switch the Scene view to Perspective".into(),
        );
    }
    let (right, up, forward) = view
        .basis()
        .ok_or("Pilot view has no finite viewing direction")?;
    let rotation = Quat::from_mat3(&Mat3::from_cols(right, up, -forward)).normalize();
    if !rotation.is_finite() {
        return Err("Pilot view orientation is not finite".into());
    }
    Ok(rotation)
}

impl Guard {
    pub(super) fn capture(
        editor: &Editor,
        resolved: &ara_doc::prefab::Resolved,
        entity: &str,
    ) -> ara_ui::Result<Self> {
        let doc = &editor.state.doc;
        let selected = resolved
            .get(entity)
            .ok_or("The selected Camera was deleted")?;
        if selected.record.contains_key("FollowCamera") {
            return Err(
                "Pilot does not edit FollowCamera rigs; select a Camera without a follow rig"
                    .into(),
            );
        }
        if !selected.record.contains_key("Transform") {
            return Err("Pilot requires a Camera with a Transform component".into());
        }
        let lens: ara_runtime::components::Camera = serde_json::from_value(
            selected
                .record
                .get("Camera")
                .ok_or("Select an entity with a Camera component")?
                .clone(),
        )?;
        if !lens.fov.is_finite() || !crate::state::Camera::FOV_RANGE.contains(&lens.fov) {
            return Err("This Camera's field of view is outside the Scene view's supported 10–120 degree range".into());
        }
        let world = crate::world::world_transform(resolved, entity)
            .ok_or("Camera world transform is unavailable")?;
        let (world_scale, _, _) = decomposition(world)?;
        let (local_scale, _, _) = decomposition(crate::world::local_transform(&selected.record))?;
        let parent = match selected.parent.as_deref() {
            Some(parent) => crate::world::world_transform(resolved, parent)
                .ok_or("The Camera's parent was deleted")?,
            None => Mat4::IDENTITY,
        };
        decomposition(parent)?;
        let mut nodes = BTreeMap::new();
        let mut owned = BTreeSet::new();
        let mut current = Some(entity);
        while let Some(address) = current {
            if nodes.contains_key(address) {
                return Err("The Camera's hierarchy contains a cycle".into());
            }
            let node = resolved
                .get(address)
                .ok_or("A Camera ancestor was deleted")?;
            let source = (
                node.source.0.as_str().to_owned(),
                node.source.1.as_str().to_owned(),
            );
            owned.insert(source.clone());
            if let Some(instance) = address.split('/').next()
                && editor
                    .host
                    .store()
                    .workspace()
                    .doc(doc)
                    .is_some_and(|document| document.record(instance).is_some())
            {
                owned.insert((doc.clone(), instance.to_owned()));
            }
            nodes.insert(
                address.to_owned(),
                Node {
                    record: json!(node.record),
                    parent: node.parent.clone(),
                    source,
                },
            );
            current = node.parent.as_deref();
        }
        Ok(Self {
            doc: doc.clone(),
            entity: entity.into(),
            name: selected
                .record
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(entity)
                .into(),
            revision: editor.host.store().revision(),
            nodes,
            owned,
            parent,
            local_scale,
            world,
            world_scale,
            fov: lens.fov,
        })
    }
    /// Revalidate only after a Host revision changes, never on every idle frame.
    pub(super) fn validate(
        &self,
        editor: &Editor,
        resolved: &ara_doc::prefab::Resolved,
        since: u64,
    ) -> ara_ui::Result<()> {
        if editor.state.doc != self.doc {
            return Err("Pilot stopped because the open Scene changed".into());
        }
        let store = editor.host.store();
        for transaction in store.since(since) {
            let cancelled = store.reverted_by(&transaction.id).is_some()
                || transaction
                    .reverts
                    .as_ref()
                    .and_then(|id| store.txn(id))
                    .is_some_and(|original| original.result > self.revision);
            if cancelled {
                continue;
            }
            if transaction.ops.iter().any(|op| self.owned.iter().any(|(doc,id)|op.doc().as_str()==doc&&op.id().as_str()==id)
                || matches!(op,Op::Document{doc,..} if self.owned.iter().any(|(owned,_)|doc.as_str()==owned))) {
                return Err("Pilot stopped because the Camera or an ancestor was edited; pilot it again to use the new pose".into());
            }
        }
        for (address, before) in &self.nodes {
            let now = resolved
                .get(address)
                .ok_or("Pilot stopped because the Camera or an ancestor was deleted")?;
            if before.record.as_object() != Some(&now.record)
                || now.parent != before.parent
                || now.source.0.as_str() != before.source.0
                || now.source.1.as_str() != before.source.1
            {
                return Err(
                    "Pilot stopped because the Camera's resolved pose or source changed".into(),
                );
            }
        }
        Ok(())
    }
    pub(super) fn aligned_view(
        &self,
        current: crate::state::Camera,
    ) -> ara_ui::Result<crate::state::Camera> {
        view_rotation(&current)?;
        let (_, rotation, position) = decomposition(self.world)?;
        let mut aligned = current;
        aligned.look_from(position, rotation * Vec3::NEG_Z);
        aligned.set_fov(self.fov);
        if relative_error(
            Mat4::from_quat(view_rotation(&aligned)?),
            Mat4::from_quat(rotation),
        ) > 0.0001
        {
            return Err("Pilot cannot preserve this Camera's roll or vertical orientation in the Scene navigation view".into());
        }
        Ok(aligned)
    }
    pub(super) fn commands(&self, view: &crate::state::Camera) -> ara_ui::Result<Vec<Value>> {
        let rotation = view_rotation(view)?;
        let world =
            Mat4::from_scale_rotation_translation(self.world_scale, rotation, view.position());
        let local = self.parent.inverse() * world;
        let (_, rotation, position) = decomposition(local)?;
        let rebuilt = Mat4::from_scale_rotation_translation(self.local_scale, rotation, position);
        if relative_error(local, rebuilt) > 0.0005 {
            return Err("Pilot cannot preserve scale under this parent at the requested angle; use a parent with uniform scale".into());
        }
        let mut commands = Vec::new();
        if relative_error(self.world, world) > 0.00001 {
            let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
            commands.push(json!({"cmd":"transform.set","args":{"entities":[self.entity],"position":position.to_array(),"rotation":[x.to_degrees(),y.to_degrees(),z.to_degrees()]}}));
        }
        if (view.fov_y_degrees - self.fov).abs() > 0.00001 {
            commands.push(json!({"cmd":"property.set","args":{"entity":self.entity,"path":"Camera.fov","value":view.fov_y_degrees}}));
        }
        Ok(commands)
    }
}
