//! Content hashes remain independent of resource addresses and edit history.
use crate::scene::Scene;
use sha2::{Digest, Sha256};
pub(super) fn scene_hash(scene: &Scene) -> String {
    let mut hash = Sha256::new();
    textures(&mut hash, scene);
    hash.update(bytemuck::cast_slice(&scene.materials));
    for mesh in &scene.meshes {
        hash.update(mesh.entity.to_le_bytes());
        hash.update((mesh.name.len() as u64).to_le_bytes());
        hash.update(mesh.name.as_bytes());
        hash.update(bytemuck::cast_slice(&mesh.vertices));
        hash.update(bytemuck::cast_slice(&mesh.indices));
        hash.update((mesh.material as u64).to_le_bytes());
    }
    hash.update(bytemuck::cast_slice(&scene.lights));
    hash.update(bytemuck::bytes_of(&scene.sun_direction));
    hash.update(bytemuck::bytes_of(&scene.sun_lux));
    environment(&mut hash, scene);
    format!("{:x}", hash.finalize())
}
pub(super) fn static_hash(scene: &Scene) -> String {
    let mut hash = Sha256::new();
    textures(&mut hash, scene);
    environment(&mut hash, scene);
    format!("{:x}", hash.finalize())
}
fn textures(hash: &mut Sha256, scene: &Scene) {
    for texture in &scene.textures {
        hash.update(texture.size[0].to_le_bytes());
        hash.update(texture.size[1].to_le_bytes());
        hash.update([u8::from(texture.srgb)]);
        hash.update(&texture.rgba);
    }
}
fn environment(hash: &mut Sha256, scene: &Scene) {
    hash.update(scene.environment_size[0].to_le_bytes());
    hash.update(scene.environment_size[1].to_le_bytes());
    hash.update(bytemuck::cast_slice(&scene.environment));
}
