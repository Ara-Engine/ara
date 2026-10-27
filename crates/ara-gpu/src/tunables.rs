//! Dev-only typed slots and an upload-on-change uniform block. No global state,
//! synchronization, allocation or name lookup is required when render code reads a key.
use crate::{Buffer, GpuDevice, HeapIndex, vk};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, marker::PhantomData};

/// Control-channel values; the declared type cannot change during a player session.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// Floating-point scalar.
    F32(f32),
    /// Signed integer.
    Int(i32),
    /// Boolean (uniform encoding is zero or one).
    Bool(bool),
    /// Four-component vector, also suitable for RGB plus an unused component.
    Vec([f32; 4]),
}
mod sealed {
    pub trait Parameter {}
}
impl Value {
    fn words(self) -> [u32; 4] {
        match self {
            Self::F32(v) => [v.to_bits(), 0, 0, 0],
            Self::Int(v) => [v as u32, 0, 0, 0],
            Self::Bool(v) => [u32::from(v), 0, 0, 0],
            Self::Vec(v) => v.map(f32::to_bits),
        }
    }
    fn valid(self, min: f32, max: f32) -> bool {
        match self {
            Self::F32(v) => v.is_finite() && (min..=max).contains(&v),
            Self::Int(v) => (min..=max).contains(&(v as f32)),
            Self::Bool(_) => true,
            Self::Vec(v) => v.iter().all(|v| v.is_finite() && (min..=max).contains(v)),
        }
    }
}

/// Types a render module may declare. Implementations are the four control types.
pub trait Parameter: Copy + sealed::Parameter {
    /// Encode a typed default.
    fn encode(self) -> Value;
    /// Decode a validated slot of the same type.
    fn decode(value: Value) -> Option<Self>;
}
macro_rules! parameter {
    ($ty:ty,$case:ident) => {
        impl sealed::Parameter for $ty {}
        impl Parameter for $ty {
            fn encode(self) -> Value {
                Value::$case(self)
            }
            fn decode(value: Value) -> Option<Self> {
                if let Value::$case(value) = value {
                    Some(value)
                } else {
                    None
                }
            }
        }
    };
}
parameter!(f32, F32);
parameter!(i32, Int);
parameter!(bool, Bool);
parameter!([f32; 4], Vec);

/// A typed registry-local slot. Keep it in the render pass that declared it.
#[derive(Debug)]
pub struct Key<T> {
    index: usize,
    _type: PhantomData<T>,
}
impl<T> Copy for Key<T> {}
impl<T> Clone for Key<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Key<T> {
    /// `uint4` offset used by experimental shaders in the uniform block.
    pub fn slot(self) -> u32 {
        self.index as u32
    }
}
#[derive(Serialize)]
struct Entry {
    name: String,
    slot: u32,
    value: Value,
    min: f32,
    max: f32,
}

/// Sole owner of named declarations, typed values and the optional GPU uniform.
#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
    names: BTreeMap<String, usize>,
    uniform: Option<(Buffer, HeapIndex)>,
    dirty: bool,
    generation: u64,
}
impl Registry {
    /// Declare once during render setup. Repeated declarations preserve live values.
    pub fn declare<T: Parameter>(
        &mut self,
        name: &str,
        default: T,
        min: f32,
        max: f32,
    ) -> Result<Key<T>> {
        ensure!(
            !name.is_empty() && min.is_finite() && max.is_finite() && min <= max,
            "invalid tunable declaration"
        );
        let index = if let Some(index) = self.names.get(name) {
            ensure!(
                T::decode(self.entries[*index].value).is_some(),
                "tunable {name} was declared with a different type"
            );
            *index
        } else {
            ensure!(
                self.entries.len() < 256 && default.encode().valid(min, max),
                "tunable capacity or default out of range"
            );
            let index = self.entries.len();
            self.entries.push(Entry {
                name: name.into(),
                slot: index as u32,
                value: default.encode(),
                min,
                max,
            });
            self.names.insert(name.into(), index);
            self.dirty = true;
            index
        };
        Ok(Key {
            index,
            _type: PhantomData,
        })
    }
    /// Obtain a typed key once at setup/control time; type mismatch is diagnosed.
    pub fn key<T: Parameter>(&self, name: &str) -> Result<Key<T>> {
        let index = *self
            .names
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown tunable {name}"))?;
        ensure!(
            T::decode(self.entries[index].value).is_some(),
            "tunable {name} type mismatch"
        );
        Ok(Key {
            index,
            _type: PhantomData,
        })
    }
    /// Direct owned slot read: no allocation, clone, lock or name lookup.
    pub fn get<T: Parameter>(&self, key: Key<T>) -> T {
        T::decode(self.entries[key.index].value)
            .expect("typed key belongs to its declaring registry")
    }
    /// Validate the complete update before applying any part of it.
    pub fn set(&mut self, changes: &BTreeMap<String, Value>) -> Result<()> {
        for (name, value) in changes {
            let index = *self
                .names
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("unknown tunable {name}"))?;
            let old = &self.entries[index];
            ensure!(
                std::mem::discriminant(&old.value) == std::mem::discriminant(value)
                    && value.valid(old.min, old.max),
                "tunable {name}: type mismatch or value outside {}..{}",
                old.min,
                old.max
            );
        }
        for (name, value) in changes {
            self.entries[self.names[name]].value = *value;
        }
        self.dirty |= !changes.is_empty();
        if !changes.is_empty() {
            self.generation += 1;
        }
        Ok(())
    }
    /// Definitions and current values, for discovery over the control channel.
    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({"generation":self.generation,"parameters":self.entries,"uniform_heap_index":self.uniform.map(|(_,h)|h.index)})
    }
    /// Current GPU table index. Created only when the lab pushes a parameter update.
    pub fn uniform(&self) -> Option<HeapIndex> {
        self.uniform.map(|(_, h)| h)
    }
    /// Declare this buffer as ShaderRead in an experimental pass that uses its slots.
    pub fn buffer(&self) -> Option<Buffer> {
        self.uniform.map(|(buffer, _)| buffer)
    }
    fn upload(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let buffer = if let Some((buffer, _)) = self.uniform {
            buffer
        } else {
            let buffer = gpu.create_buffer(
                256 * 16,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                true,
            )?;
            let index = gpu.storage_buffer(buffer)?;
            self.uniform = Some((buffer, index));
            buffer
        };
        let mut block = [[0u32; 4]; 256];
        for (slot, entry) in block.iter_mut().zip(&self.entries) {
            *slot = entry.value.words();
        }
        gpu.upload_queued(buffer, bytemuck::cast_slice(&block))?;
        self.dirty = false;
        Ok(())
    }
}
impl GpuDevice {
    /// Dev-only registry owned by this device.
    pub fn lab_params(&self) -> &Registry {
        &self.lab_params
    }
    /// Declare render-module parameters or apply a validated control update.
    pub fn lab_params_mut(&mut self) -> &mut Registry {
        &mut self.lab_params
    }
    /// Queue one safe GPU upload after changes. Normal frames never call this.
    pub fn flush_lab_params(&mut self) -> Result<()> {
        let mut registry = std::mem::take(&mut self.lab_params);
        let result = registry.upload(self);
        self.lab_params = registry;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn type_and_range_errors_leave_the_whole_update_unchanged() -> Result<()> {
        let mut registry = Registry::default();
        let a = registry.declare("a", 1_f32, 0., 2.)?;
        registry.declare("count", 2_i32, 0., 10.)?;
        registry.declare("on", true, 0., 1.)?;
        let vector = registry.declare("colour", [1_f32; 4], 0., 2.)?;
        assert!(
            registry
                .set(&BTreeMap::from([
                    ("a".into(), Value::F32(1.5)),
                    ("count".into(), Value::F32(3.))
                ]))
                .is_err()
        );
        assert_eq!(registry.get(a), 1.);
        assert!(
            registry
                .set(&BTreeMap::from([("a".into(), Value::F32(f32::NAN))]))
                .is_err()
        );
        registry.set(&BTreeMap::from([("colour".into(), Value::Vec([0.5; 4]))]))?;
        assert_eq!(registry.get(vector), [0.5; 4]);
        assert!(registry.key::<bool>("count").is_err());
        Ok(())
    }
}
