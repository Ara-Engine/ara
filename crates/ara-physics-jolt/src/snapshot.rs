use crate::{CharacterContactKey, ContactKey, JoltBackend};
use ara_jolt_sys as sys;
use ara_physics::*;
use std::collections::BTreeMap;

/// Trusted Jolt state. Private construction prevents untrusted bytes reaching RestoreState.
/// Transfer authoritative ECS state over the network; retain native snapshots locally.
#[derive(Clone)]
pub struct JoltSnapshot {
    bytes: Vec<u8>,
    structure: u64,
    fingerprint: u64,
    config: PhysicsConfig,
    deterministic: bool,
    contacts: BTreeMap<ContactKey, (bool, ContactEvent)>,
    character_contacts: BTreeMap<CharacterContactKey, CharacterContactEvent>,
}
impl JoltSnapshot {
    /// Native byte length, excluding the small Rust contact/configuration metadata.
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }
    /// Diagnostic view of the pinned native format; not a portable asset/wire encoding.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl JoltBackend {
    pub(crate) fn save_snapshot(&mut self) -> Result<JoltSnapshot> {
        let mut len = 0;
        // SAFETY: exclusive live world and writable length. The shim retains its
        // buffer until the next save; copy it immediately without another native call.
        let data = unsafe { sys::ara_world_save(self.ptr(), &mut len) };
        if data.is_null() {
            return Err(Self::error());
        }
        // SAFETY: the successful native save returned len valid bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        let mut config = self.config.clone();
        config.threads = 0; // Execution topology must not alter physics state.
        Ok(JoltSnapshot {
            bytes,
            structure: self.structure,
            fingerprint: self.fingerprint,
            config,
            deterministic: self.deterministic(),
            contacts: self.contacts.clone(),
            character_contacts: self.character_contacts.clone(),
        })
    }
    pub(crate) fn restore_snapshot(&mut self, snapshot: &JoltSnapshot) -> Result<()> {
        let mut config = self.config.clone();
        config.threads = 0;
        if self.structure != snapshot.structure
            || self.fingerprint != snapshot.fingerprint
            || config != snapshot.config
            || self.deterministic() != snapshot.deterministic
        {
            return Err(PhysicsError("snapshot topology, configuration or build differs; reconstruct the identical world first".into()));
        }
        // SAFETY: the opaque snapshot is constructible only by native save. All
        // topology/configuration checks precede mutation; checksum is checked natively.
        Self::ok(unsafe {
            sys::ara_world_restore(self.ptr(), snapshot.bytes.as_ptr(), snapshot.bytes.len())
        })?;
        self.contacts.clone_from(&snapshot.contacts);
        self.character_contacts
            .clone_from(&snapshot.character_contacts);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupted_checkpoint_is_rejected_before_mutation() {
        let mut world = PhysicsWorld::<JoltBackend>::new(PhysicsConfig {
            max_bodies: 16,
            ..Default::default()
        })
        .expect("valid fixture");
        let mut snapshot = world.backend_mut().save().expect("valid checkpoint");
        snapshot.bytes[8] ^= 1;
        assert!(world.backend_mut().restore(&snapshot).is_err());
        assert_eq!(world.tick(), 0);
        world
            .step()
            .expect("world remains usable after rejected corruption");
    }
}
