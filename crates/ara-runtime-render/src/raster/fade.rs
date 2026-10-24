//! Short material-only arrivals; no geometry replacement when the fade finishes.
use super::*;
use ara_render::instanced::{Changes, Scene};
use std::collections::BTreeSet;

const FRAMES: u8 = 12;
struct Fade {
    asset: String,
    material: usize,
    alpha: f32,
    mode: u32,
    remaining: u8,
    root: bool,
}
#[derive(Default)]
pub(super) struct Fades {
    active: Vec<Fade>,
    changes: Changes,
}
impl Fades {
    pub fn is_active(&self) -> bool {
        !self.active.is_empty()
    }
    pub fn rebind(&mut self, prepared: &Prepared, arrivals: &BTreeSet<String>) {
        let phases = self
            .active
            .iter()
            .map(|fade| (fade.asset.to_owned(), fade.remaining))
            .collect();
        self.active.clear();
        self.begin_with(prepared, arrivals, &phases);
    }
    pub fn begin(&mut self, prepared: &Prepared, arrivals: &BTreeSet<String>) {
        if !arrivals.is_empty() {
            self.begin_with(prepared, arrivals, &Default::default());
        }
    }
    fn begin_with(
        &mut self,
        prepared: &Prepared,
        arrivals: &BTreeSet<String>,
        phases: &std::collections::BTreeMap<String, u8>,
    ) {
        let scene = &prepared.scene;
        for (index, instance) in scene.instances.iter().enumerate() {
            let Some(asset) = prepared.model_source(index) else {
                continue;
            };
            let remaining = if arrivals.contains(asset) {
                FRAMES
            } else if let Some(&remaining) = phases.get(asset) {
                remaining
            } else {
                continue;
            };
            let material = instance.material;
            if self.active.iter().any(|fade| fade.material == material) {
                continue;
            }
            let source = &scene.materials[material].pbr;
            self.active.push(Fade {
                asset: asset.to_owned(),
                material,
                alpha: source.base_color[3],
                mode: source.flags[0],
                remaining,
                root: index < prepared.root_instances(),
            });
        }
    }
    /// A newly synchronized source snapshot carries authored alpha again. Retain
    /// its edits, then reapply the current presentation weight until the next draw.
    pub fn refresh(&mut self, scene: &mut Scene) {
        for fade in &mut self.active {
            if !fade.root {
                continue;
            }
            let material = &mut scene.materials[fade.material].pbr;
            fade.alpha = material.base_color[3];
            fade.mode = material.flags[0];
            apply(fade, material, false);
        }
    }
    pub fn apply(&mut self, scene: &mut Scene, renderer: &mut Renderer) -> Result<()> {
        if self.active.is_empty() {
            return Ok(());
        }
        self.changes.clear();
        for fade in &mut self.active {
            apply(fade, &mut scene.materials[fade.material].pbr, true);
            self.changes.materials.push(fade.material);
        }
        renderer.update_native_changed(scene, &self.changes)?;
        Ok(())
    }
    pub fn presented(&mut self) {
        for fade in &mut self.active {
            fade.remaining = fade.remaining.saturating_sub(1);
        }
        self.active.retain(|fade| fade.remaining > 0);
    }
    pub fn finish(&mut self, scene: &mut Scene, renderer: &mut Renderer) -> Result<()> {
        for fade in &mut self.active {
            fade.remaining = 0;
        }
        self.apply(scene, renderer)?;
        self.active.clear();
        Ok(())
    }
}
fn apply(fade: &Fade, material: &mut ara_render::scene::Material, advancing: bool) {
    let remaining = if advancing {
        fade.remaining
    } else {
        fade.remaining.saturating_add(1).min(FRAMES)
    };
    if remaining <= 1 {
        material.base_color[3] = fade.alpha;
        material.flags[0] = fade.mode;
    } else {
        let weight = 0.18 + 0.82 * f32::from(FRAMES - remaining) / f32::from(FRAMES - 1);
        // glTF OPAQUE ignores its authored alpha. Fade from a real opacity even
        // when such a material happened to store zero in baseColorFactor.a.
        material.base_color[3] = if fade.mode == 0 {
            weight
        } else {
            fade.alpha * weight
        };
        material.flags[0] = 2;
    }
}
