use super::*;
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, Op, Workspace};
use ara_ops::Store;
use ara_schema::Schema;

/// Navigation-only component. It is never projected into the gameplay world.
#[derive(Schema)]
#[ara(component, module = "editor")]
struct PlayViewState {
    /// Use the transient camera instead of the world's authored gameplay camera.
    #[expect(
        dead_code,
        reason = "this reflected field is stored in the typed navigation document"
    )]
    spectator: bool,
    /// Spectator navigation speed in metres per second.
    #[ara(default = 4, range(min = 0.5, max = 50))]
    #[expect(
        dead_code,
        reason = "reflected navigation setting stored in the typed document"
    )]
    navigation_speed: f32,
}
/// Read-only camera pose decoded from the transient op store, never a second mutable state owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewPose {
    /// Eye position in metres.
    pub position: [f32; 3],
    /// Intrinsic XYZ Euler rotation, in degrees (the same convention as Transform).
    pub rotation: [f32; 3],
    /// Vertical field of view in degrees.
    pub fov_y_degrees: f32,
}
/// A typed navigation document owned by one player view. Every camera change is
/// a builtin command transaction; neither authored camera records nor ECS gameplay state is mutated.
pub struct ViewStore {
    registry: Registry,
    store: Store,
    epoch: u64,
}
const DOC: &str = "sc_editorview";
const ENTITY: &str = "e_editorview";
impl ViewStore {
    /// Monotonic navigation document revision, including retained history epochs.
    pub fn revision(&self) -> u64 {
        self.epoch + self.store.revision()
    }
    /// The view's own navigation speed; it never changes gameplay movement settings.
    pub fn navigation_speed(&self) -> f32 {
        self.store
            .workspace()
            .doc(DOC)
            .and_then(|doc| doc.record(ENTITY))
            .and_then(|record| record.get("PlayViewState"))
            .and_then(|state| state.get("navigationSpeed"))
            .and_then(Value::as_f64)
            .unwrap_or(4.) as f32
    }
    /// Change speed through the reflected navigation component's typed property op.
    pub fn set_navigation_speed(&mut self, speed: f32) -> Result<(), Vec<Diagnostic>> {
        if !speed.is_finite() || !(0.5..=50.).contains(&speed) {
            return Err(fail(
                codes::REQUEST,
                "Spectator navigation speed must be between 0.5 and 50 metres per second",
            ));
        }
        self.registry.apply(
            &mut self.store,
            &Batch::new(
                DOC,
                [(
                    "property.set",
                    json!({"entity":ENTITY,"path":"PlayViewState.navigationSpeed","value":speed}),
                )],
            ),
            &Session::human(),
        )?;
        Ok(())
    }
    /// Start a private in-memory navigation document using the engine's existing camera/transform schemas.
    pub fn new(schema: &SchemaDb) -> Result<Self, Vec<Diagnostic>> {
        let mut schema = schema.clone();
        schema.register::<PlayViewState>();
        let registry = Registry::builtin(&mut schema);
        let mut workspace = Workspace::default();
        let id =
            AssetId::parse(DOC).map_err(|problem| fail(codes::REQUEST, problem.to_string()))?;
        workspace.insert("editor-view.scene", Document::new("scene", id));
        let mut store = Store::new(schema, workspace);
        registry.apply(&mut store,&Batch::new(DOC,[("entity.create",json!({"id":ENTITY,"components":{"Transform":{},"Camera":{},"PlayViewState":{"spectator":false}}}))]),&Session::human())?;
        Ok(Self {
            registry,
            store,
            epoch: 0,
        })
    }
    /// Apply an editor camera pose, or return this view to the gameplay camera.
    pub fn set(&mut self, pose: Option<ViewPose>) -> Result<u64, Vec<Diagnostic>> {
        if let Some(pose) = pose
            && (!pose
                .position
                .into_iter()
                .chain(pose.rotation)
                .chain([pose.fov_y_degrees])
                .all(f32::is_finite)
                || !(10. ..=120.).contains(&pose.fov_y_degrees))
        {
            return Err(fail(
                codes::REQUEST,
                "Spectator camera values must be finite, with FOV between 10 and 120 degrees",
            ));
        }
        let mut calls = vec![(
            "property.set",
            json!({"entity":ENTITY,"path":"PlayViewState.spectator","value":pose.is_some()}),
        )];
        if let Some(pose) = pose {
            calls.push((
                "transform.set",
                json!({"entities":[ENTITY],"position":pose.position,"rotation":pose.rotation}),
            ));
            calls.push((
                "property.set",
                json!({"entity":ENTITY,"path":"Camera.fov","value":pose.fov_y_degrees}),
            ));
        }
        self.registry
            .apply(&mut self.store, &Batch::new(DOC, calls), &Session::human())?;
        let revision = self.epoch + self.store.revision();
        // Camera history is not content undo. Bound retained navigation history
        // while preserving its current typed document as the next baseline.
        if self.store.revision() >= 512 {
            self.store = Store::new(self.store.schema().clone(), self.store.workspace().clone());
            self.epoch = revision;
        }
        Ok(revision)
    }
    /// Resolve the active pose from the op document for presentation extraction.
    pub fn pose(&self) -> Option<ViewPose> {
        let record = self.store.workspace().doc(DOC)?.record(ENTITY)?;
        if record.get("PlayViewState")?.get("spectator")?.as_bool() != Some(true) {
            return None;
        }
        let vector = |field: &str| -> Option<[f32; 3]> {
            let Some(values) = record
                .get("Transform")
                .and_then(|transform| transform.get(field))
                .and_then(Value::as_array)
            else {
                return Some([0.; 3]);
            };
            let mut result = [0.; 3];
            for (index, value) in result.iter_mut().enumerate() {
                *value = values.get(index)?.as_f64()? as f32;
            }
            Some(result)
        };
        Some(ViewPose {
            position: vector("position")?,
            rotation: vector("rotation")?,
            fov_y_degrees: record
                .get("Camera")
                .and_then(|camera| camera.get("fov"))
                .and_then(Value::as_f64)
                .unwrap_or(55.) as f32,
        })
    }
}
/// Lower runtime control properties through builtin typed commands. The returned
/// ops retain schema validation, prefab overrides and prev guards before the runtime queues them.
pub fn property_ops(
    schema: &SchemaDb,
    workspace: &Workspace,
    scene: &str,
    sets: &[ara_play::Set],
) -> Result<Vec<Op>, Vec<Diagnostic>> {
    let mut schema = schema.clone();
    let registry = Registry::builtin(&mut schema);
    let mut store = Store::new(schema, workspace.clone());
    let batch = Batch::new(
        scene,
        sets.iter().map(|set| {
            (
                "property.set",
                json!({"entity":set.entity,"path":set.path,"value":set.value}),
            )
        }),
    );
    let result = registry.apply(&mut store, &batch, &Session::human())?;
    Ok(result.ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_camera_changes_use_ops_and_never_write_the_source_workspace()
    -> Result<(), Vec<Diagnostic>> {
        let schema = ara_runtime::schema(&[])
            .map_err(|problem| fail(codes::REQUEST, problem.to_string()))?;
        let mut view = ViewStore::new(&schema)?;
        assert!(view.pose().is_none());
        assert_eq!(view.navigation_speed(), 4.);
        view.set_navigation_speed(16.)?;
        assert_eq!(view.navigation_speed(), 16.);
        assert!(view.set_navigation_speed(f32::NAN).is_err());
        assert!(view.set_navigation_speed(0.).is_err());
        let pose = ViewPose {
            position: [1., 2., 3.],
            rotation: [0., 45., 0.],
            fov_y_degrees: 55.,
        };
        let first = view.set(Some(pose))?;
        assert_eq!(view.pose(), Some(pose));
        assert!(
            view.store
                .history()
                .last()
                .is_some_and(|txn| !txn.ops.is_empty())
        );
        let second = view.set(None)?;
        assert!(second > first);
        assert!(view.pose().is_none());
        Ok(())
    }
}
