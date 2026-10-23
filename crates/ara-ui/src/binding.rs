//! Typed subscriptions are pushed by change notifications, never polled each frame.
use crate::{Result, Ui};
use std::{collections::BTreeMap, marker::PhantomData};
/// A field's numeric identity is shared with generated C# descriptors.
#[derive(Debug)]
pub struct BindingKey<T> {
    id: u32,
    marker: PhantomData<fn() -> T>,
}
impl<T> Copy for BindingKey<T> {}
impl<T> Clone for BindingKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> BindingKey<T> {
    /// Construct from a schema-generated field ID.
    pub const fn new(id: u32) -> Self {
        Self {
            id,
            marker: PhantomData,
        }
    }
    /// Data-oriented ID for the managed ABI.
    pub fn id(self) -> u32 {
        self.id
    }
}
/// A host implements this for each field type exposed by its model schema.
pub trait ReadBinding<T> {
    /// Read a changed field at the UI/simulation snapshot boundary.
    fn read(&self, key: BindingKey<T>) -> Result<T>;
}
/// UI actions are queued for the host to validate and dispatch at its simulation boundary.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Action {
    /// Registered action name.
    pub name: String,
    /// Stable source control identity.
    pub node: u64,
    /// Edited value or drag payload.
    pub value: serde_json::Value,
}
/// Supported strongly typed binding destinations.
pub trait BindingValue: Sized {
    /// Apply a notified value to one retained control.
    fn apply(self, ui: &mut Ui, node: usize) -> Result<()>;
}
impl BindingValue for String {
    fn apply(self, ui: &mut Ui, node: usize) -> Result<()> {
        ui.set_text(node, &self)
    }
}
impl BindingValue for bool {
    fn apply(self, ui: &mut Ui, node: usize) -> Result<()> {
        ui.set_checked(node, self)
    }
}
impl BindingValue for f64 {
    fn apply(self, ui: &mut Ui, node: usize) -> Result<()> {
        ui.set_value(node, self)
    }
}
/// One value-type subscription index; model fields are read only when explicitly notified.
pub struct Bindings<T> {
    subscribers: BTreeMap<u32, Vec<u64>>,
    marker: PhantomData<fn() -> T>,
}
impl<T> Default for Bindings<T> {
    fn default() -> Self {
        Self {
            subscribers: BTreeMap::new(),
            marker: PhantomData,
        }
    }
}
impl<T: BindingValue + Clone> Bindings<T> {
    /// Initialize a subscription once, validating the destination widget immediately.
    pub fn subscribe(
        &mut self,
        key: BindingKey<T>,
        node: u64,
        model: &impl ReadBinding<T>,
        ui: &mut Ui,
    ) -> Result<()> {
        let index = ui.by_id(node).ok_or("ARA4043: binding node missing")?;
        model.read(key)?.apply(ui, index)?;
        let list = self.subscribers.entry(key.id).or_default();
        if !list.contains(&node) {
            list.push(node);
        }
        Ok(())
    }
    /// Batch a set of changed fields; duplicate notifications are coalesced.
    pub fn changed(
        &self,
        keys: impl IntoIterator<Item = BindingKey<T>>,
        model: &impl ReadBinding<T>,
        ui: &mut Ui,
    ) -> Result<usize> {
        let keys: std::collections::BTreeSet<_> = keys.into_iter().map(|k| k.id).collect();
        let mut count = 0;
        for key in keys {
            if let Some(nodes) = self.subscribers.get(&key) {
                let value = model.read(BindingKey::new(key))?;
                for id in nodes {
                    if let Some(i) = ui.by_id(*id) {
                        value.clone().apply(ui, i)?;
                        count += 1;
                    }
                }
            }
        }
        Ok(count)
    }
    /// Remove subscriptions when a model/owner is detached.
    pub fn unsubscribe(&mut self, node: u64) {
        for list in self.subscribers.values_mut() {
            list.retain(|id| *id != node);
        }
    }
}
