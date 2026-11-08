//! Engine components C# views in place: Rust `#[repr(C)]` bevy components whose
//! layout matches their Schema DB type's canonical layout.

use ara_diag::Diagnostic;
use ara_schema::{SchemaDb, StructLayout};
use bevy_ecs::component::{Component, ComponentId, StorageType};
use bevy_ecs::world::World;

use crate::codes;

/// One bound engine component.
struct Binding {
    name: String,
    register: fn(&mut World) -> ComponentId,
    size: usize,
    align: usize,
    /// `(rust field name, offset)`.
    offsets: Vec<(&'static str, usize)>,
}

/// Checked native ECS views used by C#, in binding order. Other engine components
/// use generated codecs through the simulation registry. Native order is part of the
/// ABI: it must match the order the C# bindings were generated with, or the layout
/// hash differs. Use `builtin` for Ara.Runtime's checked-in native views.
#[derive(Default)]
pub struct EngineBindings {
    items: Vec<Binding>,
}

/// `current_value` → `currentValue` (the derive's field naming).
fn camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

impl EngineBindings {
    /// Additional checked views keep their existing codec indices and wire format.
    /// The selected C layouts have no padding and contain no owned/drop fields.
    pub(crate) fn accessor_views() -> Self {
        let bindings =
            crate::bind_component!(Self::new(), ara_physics::PhysicsId as "PhysicsId" { value });
        let bindings = crate::bind_component!(bindings, ara_physics::PhysicsPose as "PhysicsPose" { position, rotation });
        let bindings = crate::bind_component!(bindings, ara_physics::Velocity as "Velocity" { linear, angular });
        let bindings = crate::bind_component!(bindings, ara_physics::CharacterController as "CharacterController" {
            radius, height, crouch_height, max_slope_deg, step_height, stick_down, padding, max_strength, mass, layer
        });
        crate::bind_component!(bindings, ara_physics::VehicleTelemetry as "VehicleTelemetry" { wheels_in_contact, engine_rpm, gear })
    }

    /// Built-in native views shared by host startup and checked-in C# generation.
    /// A blittable schema alone does not guarantee a Rust component's C layout.
    pub fn builtin() -> Self {
        crate::bind_component!(Self::new(), ara_components::Transform as "Transform" {
            position, rotation, scale
        })
    }

    /// No bindings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `T` as schema component `name`. Prefer [`bind_component!`](crate::bind_component),
    /// which fills `offsets` with `offset_of!`.
    pub fn component<T: Component>(
        mut self,
        name: &str,
        offsets: &[(&'static str, usize)],
    ) -> Self {
        self.items.push(Binding {
            name: name.to_owned(),
            register: |w| w.register_component::<T>(),
            size: size_of::<T>(),
            align: align_of::<T>(),
            offsets: offsets.to_vec(),
        });
        self
    }

    /// Native view names, in binding order.
    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|b| b.name.as_str()).collect()
    }

    /// Checks every binding against the schema and registers it with the world.
    pub(crate) fn resolve(
        &self,
        db: &SchemaDb,
        world: &mut World,
    ) -> Result<Vec<(String, ComponentId, StructLayout)>, Diagnostic> {
        self.items
            .iter()
            .map(|b| {
                let layout = db
                    .struct_layout(&b.name)
                    .map_err(|e| Diagnostic::error(codes::NOT_BLITTABLE, e).record(&b.name))?;
                let mismatch = |what: String| {
                    Diagnostic::error(codes::LAYOUT_MISMATCH, format!("engine component `{}`: {what}", b.name)).record(&b.name)
                };
                if (b.size, b.align) != (layout.size, layout.align) {
                    return Err(mismatch(format!(
                        "Rust size/align {}/{} but the schema layout is {}/{}; make it #[repr(C)] with fields in schema order",
                        b.size, b.align, layout.size, layout.align
                    )));
                }
                for f in &layout.fields {
                    match b.offsets.iter().find(|(n, _)| camel(n) == f.name) {
                        Some((_, off)) if *off == f.offset => {}
                        Some((n, off)) => return Err(mismatch(format!("field `{n}` at {off}, schema says {}", f.offset))),
                        None => return Err(mismatch(format!("no Rust field for schema field `{}`", f.name))),
                    }
                }
                let id = (b.register)(world);
                let table = world
                    .components()
                    .get_info(id)
                    .is_some_and(|i| i.storage_type() == StorageType::Table);
                if !table {
                    return Err(Diagnostic::error(
                        codes::SPARSE_COMPONENT,
                        format!("engine component `{}` must use table storage to be viewed by C#", b.name),
                    ));
                }
                Ok((b.name.clone(), id, layout))
            })
            .collect()
    }
}

/// Binds a `#[repr(C)]` bevy component to its schema name, with field offsets from
/// `offset_of!`:
///
/// ```ignore
/// let engine = bind_component!(EngineBindings::new(), EcsTransform as "Transform" { position, rotation, scale });
/// ```
#[macro_export]
macro_rules! bind_component {
    ($bindings:expr, $ty:ty as $name:literal { $($field:ident),* $(,)? }) => {
        $bindings.component::<$ty>($name, &[$((stringify!($field), ::core::mem::offset_of!($ty, $field))),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_views_validate_rust_offsets_and_match_the_generated_abi() {
        let schema = ara_runtime::schema(&[]).expect("engine schema");
        let bindings = EngineBindings::builtin();
        let native = bindings
            .resolve(&schema, &mut World::new())
            .expect("built-in Rust views match their schema layouts");
        let layouts: Vec<_> = native.into_iter().map(|(_, _, layout)| layout).collect();
        let generated = crate::csgen::engine_layouts(&schema, &bindings.names())
            .expect("native view schemas have blittable layouts");
        assert_eq!(&generated[..layouts.len()], layouts);
        assert!(
            generated.len() > layouts.len(),
            "accessor contracts participate in the ABI hash"
        );
        assert!(
            crate::csgen::blittable_components(&schema).len() > bindings.names().len(),
            "schema-only render components must not become unbound native views"
        );
    }

    #[test]
    fn accessor_views_are_checked_padding_free_and_leave_the_native_prefix_unchanged() {
        let schema = ara_runtime::schema(&[]).expect("engine schema");
        assert_eq!(EngineBindings::builtin().names(), ["Transform"]);
        let views = EngineBindings::accessor_views()
            .resolve(&schema, &mut World::new())
            .expect("physics C layouts match canonical layouts");
        assert_eq!(views.len(), 5);
        for (_, _, layout) in views {
            let mut end = 0;
            for field in &layout.fields {
                assert_eq!(
                    field.offset, end,
                    "{} has no uninitialized padding",
                    layout.name
                );
                end += field.layout.size;
            }
            assert_eq!(end, layout.size, "{} has no trailing padding", layout.name);
        }
    }

    #[test]
    fn a_mismatched_accessor_layout_is_refused_before_a_pointer_is_exposed() {
        #[derive(Component)]
        #[repr(C)]
        struct WrongVelocity {
            linear: [f32; 3],
            angular: [f32; 2],
        }
        let schema = ara_runtime::schema(&[]).expect("engine schema");
        let bindings = crate::bind_component!(EngineBindings::new(), WrongVelocity as "Velocity" { linear, angular });
        let error = bindings
            .resolve(&schema, &mut World::new())
            .expect_err("stale native layout");
        assert_eq!(error.code, codes::LAYOUT_MISMATCH);
    }
}
