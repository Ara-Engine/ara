//! Which scene entity drew each renderer channel. The renderer writes one channel per
//! drawn instance (its ID texture holds `index + 1` into `Renderer::entities`), so one
//! entity can own several channels: a model's primitives (`stable_id(address)` plus the
//! primitive index) and a terrain's tiles (`terrain::tile_render_id`, one per chunk).
//! Selection outlines, hover and picking go through this map, so an entity reads as one
//! shape, and a terrain as one object rather than loose chunks.
use super::render_id;
use ara_doc::prefab::Resolved;
use serde_json::Value;
use std::collections::HashMap;

/// The most primitives a model draw adds to its entity's render ID.
const PRIMITIVES: u64 = 256;

/// Channel → entity for one renderer entity list and one resolved scene.
#[derive(Default)]
pub struct Channels {
    /// Scene entity addresses, in address order.
    pub addresses: Vec<String>,
    /// Each address's parent (index into `addresses`).
    pub parents: Vec<Option<u32>>,
    /// Per channel (`Renderer::entities` index): the entity that drew it.
    pub owners: Vec<Option<u32>>,
    /// Per channel: what a click or hover acts on (the owner; a terrain chunk's Terrain).
    pub targets: Vec<Option<u32>>,
    /// Per channel: drawn by terrain (ground). Everything standing on the ground is in
    /// front of it, so its outline only follows the sky, the void and other ground.
    pub ground: Vec<bool>,
    /// Renderer entity ID → target, for picking.
    by_id: HashMap<u64, u32>,
    key: u64,
}

impl Channels {
    /// Map `channels` (`Renderer::entities`) onto `entities` (the open scene at store
    /// `revision`). Cheap when neither changed: true when the map was rebuilt.
    pub fn update(
        &mut self,
        entities: &Resolved,
        revision: u64,
        channels: &[(u64, String)],
    ) -> bool {
        let key = channels
            .iter()
            .map(|(id, _)| *id)
            .chain([revision, channels.len() as u64])
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, id| {
                (hash ^ id).wrapping_mul(0x0000_0100_0000_01b3)
            });
        if key == self.key && !self.addresses.is_empty() {
            return false;
        }
        self.key = key;
        self.addresses = entities.keys().cloned().collect();
        let index: HashMap<&str, u32> = self
            .addresses
            .iter()
            .enumerate()
            .map(|(n, a)| (a.as_str(), n as u32))
            .collect();
        self.parents = entities
            .values()
            .map(|e| e.parent.as_deref().and_then(|p| index.get(p).copied()))
            .collect();
        let mut exact = HashMap::new();
        let mut models = Vec::new();
        let mut tiles = HashMap::new();
        for (n, (address, entity)) in entities.iter().enumerate() {
            let n = n as u32;
            exact.insert(render_id(address), n);
            if entity.record.contains_key("MeshRenderer") {
                models.push((render_id(address), n));
            }
            if let Some(chunk) = entity.record.get("TerrainChunk") {
                let at = |key: &str| chunk.get(key).and_then(Value::as_i64).unwrap_or(0) as i32;
                tiles.insert(ara_runtime::terrain::tile_render_id(at("x"), at("z")), n);
            }
        }
        models.sort_unstable();
        // A terrain chunk's clicks and hovers go to its Terrain: the terrain is one object.
        let target = |owner: u32| {
            let entity = &entities[&self.addresses[owner as usize]];
            match self.parents[owner as usize] {
                Some(parent)
                    if entity.record.contains_key("TerrainChunk")
                        && entities[&self.addresses[parent as usize]]
                            .record
                            .contains_key("Terrain") =>
                {
                    parent
                }
                _ => owner,
            }
        };
        self.owners = channels
            .iter()
            .map(|(id, _)| {
                exact
                    .get(id)
                    .or_else(|| tiles.get(id))
                    .copied()
                    .or_else(|| {
                        // A model part: its entity's ID plus the primitive index.
                        let at = models.partition_point(|(base, _)| base <= id);
                        let (base, owner) = *models.get(at.checked_sub(1)?)?;
                        (id - base < PRIMITIVES).then_some(owner)
                    })
            })
            .collect();
        self.targets = self.owners.iter().map(|o| o.map(target)).collect();
        self.ground = self
            .owners
            .iter()
            .map(|o| {
                o.is_some_and(|o| {
                    let record = &entities[&self.addresses[o as usize]].record;
                    record.contains_key("TerrainChunk") || record.contains_key("Terrain")
                })
            })
            .collect();
        self.by_id = channels
            .iter()
            .zip(&self.targets)
            .filter_map(|((id, _), target)| Some((*id, (*target)?)))
            .collect();
        true
    }

    /// Digest of the inputs of the last rebuild (changes when the map does).
    pub fn key(&self) -> u64 {
        self.key
    }

    /// The entity a click on renderer entity `id` selects (`Renderer::entity_at`).
    pub fn target_of(&self, id: u64) -> Option<&str> {
        self.by_id
            .get(&id)
            .map(|n| self.addresses[*n as usize].as_str())
    }

    /// Whether `entity` (an address index) is in `selected` or below one that is.
    pub fn within(&self, entity: u32, selected: &[bool]) -> bool {
        let mut at = Some(entity);
        // Bounded walk: a malformed parent cycle cannot hang the frame.
        for _ in 0..64 {
            let Some(n) = at else {
                return false;
            };
            if selected.get(n as usize).copied().unwrap_or(false) {
                return true;
            }
            at = self.parents[n as usize];
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_doc::{AssetId, RecordId, prefab::ResolvedEntity};
    use serde_json::json;

    fn entity(record: Value, parent: Option<&str>) -> ResolvedEntity {
        ResolvedEntity {
            record: record.as_object().cloned().unwrap_or_default(),
            parent: parent.map(str::to_owned),
            order: None,
            source: (
                AssetId::parse("sc_test1234").expect("asset ID"),
                RecordId::parse("e_test").expect("record ID"),
            ),
        }
    }

    #[test]
    fn terrain_tiles_and_model_parts_map_to_their_entities() {
        let mut scene = Resolved::new();
        scene.insert("e_terrain".into(), entity(json!({"Terrain": {}}), None));
        for (x, z) in [(0, 0), (1, 0)] {
            scene.insert(
                format!("e_chunk{x}z{z}"),
                entity(json!({"TerrainChunk": {"x": x, "z": z}}), Some("e_terrain")),
            );
        }
        scene.insert("e_crate".into(), entity(json!({"MeshRenderer": {}}), None));
        let tile = |x, z| {
            (
                ara_runtime::terrain::tile_render_id(x, z),
                "tile".to_owned(),
            )
        };
        let channels = vec![
            tile(0, 0),
            tile(1, 0),
            (render_id("e_crate"), "crate".to_owned()),
            (render_id("e_crate") + 2, "crate part".to_owned()),
            (42, "foliage".to_owned()),
        ];
        let mut map = Channels::default();
        assert!(map.update(&scene, 1, &channels));
        assert!(
            !map.update(&scene, 1, &channels),
            "unchanged inputs keep the map"
        );
        let name = |n: Option<u32>| n.map(|n| map.addresses[n as usize].as_str());
        assert_eq!(name(map.owners[0]), Some("e_chunk0z0"));
        assert_eq!(
            name(map.targets[0]),
            Some("e_terrain"),
            "a chunk acts as its terrain"
        );
        assert_eq!(name(map.targets[1]), Some("e_terrain"));
        assert_eq!(
            name(map.owners[3]),
            Some("e_crate"),
            "a part's primitive offset"
        );
        assert_eq!(map.owners[4], None);
        assert_eq!(map.ground, [true, true, false, false, false]);
        assert_eq!(map.target_of(channels[1].0), Some("e_terrain"));
        let terrain = map
            .addresses
            .iter()
            .position(|a| a == "e_terrain")
            .expect("terrain");
        let mut selected = vec![false; map.addresses.len()];
        selected[terrain] = true;
        assert!(
            map.within(map.owners[0].expect("owner"), &selected),
            "chunks are within it"
        );
        assert!(!map.within(map.owners[2].expect("owner"), &selected));
    }
}
