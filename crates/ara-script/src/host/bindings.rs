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

/// The engine components exposed to C#, in binding order. The order is part of the
/// ABI: it must match the order the C# bindings were generated with
/// (`csgen::blittable_components`, sorted by name), or the layout hash differs.
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
    /// No bindings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `T` as schema component `name`. Prefer [`bind_component!`](crate::bind_component),
    /// which fills `offsets` with `offset_of!`.
    pub fn component<T: Component>(mut self, name: &str, offsets: &[(&'static str, usize)]) -> Self {
        self.items.push(Binding {
            name: name.to_owned(),
            register: |w| w.register_component::<T>(),
            size: size_of::<T>(),
            align: align_of::<T>(),
            offsets: offsets.to_vec(),
        });
        self
    }

    /// Bound names, in binding order.
    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|b| b.name.as_str()).collect()
    }

    /// Checks every binding against the schema and registers it with the world.
    pub(crate) fn resolve(&self, db: &SchemaDb, world: &mut World) -> Result<Vec<(String, ComponentId, StructLayout)>, Diagnostic> {
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
