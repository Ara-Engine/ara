//! Reuse identical finite references across cut/ray-count experiments. Raw native
//! pixels stay binary; report PNGs remain <=1080p. The key covers all fixture data.
use super::fixture::Fixture;
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};
type Pair = (image::RgbaImage, Vec<[f32; 4]>);

pub struct Cache {
    root: Option<PathBuf>,
    size: [u32; 2],
}
impl Cache {
    pub fn new(f: &Fixture, size: [u32; 2], samples: u32) -> Result<Self> {
        let root = std::env::var_os("ARA_B4_REFERENCE_CACHE").map(PathBuf::from);
        let mut hash = Sha256::new();
        hash.update(b"B4-reference-v1-eight-bounces-no-denoise");
        hash.update(bytemuck::bytes_of(&size));
        hash.update(samples.to_le_bytes());
        hash.update(f.ev100.to_le_bytes());
        let camera = f.camera.resolve(size)?;
        hash.update(bytemuck::bytes_of(&camera.view));
        hash.update(bytemuck::bytes_of(&camera.projection));
        for g in &f.scene.geometry {
            hash.update(bytemuck::cast_slice(&g.vertices));
            for lod in &g.lods {
                hash.update(bytemuck::cast_slice(&lod.indices));
            }
        }
        for i in &f.scene.instances {
            hash.update(bytemuck::bytes_of(&i.world));
            hash.update(i.geometry.to_le_bytes());
            hash.update(i.material.to_le_bytes());
            hash.update([u8::from(i.casts_shadows)]);
        }
        hash.update(bytemuck::cast_slice(&f.scene.materials));
        hash.update(bytemuck::cast_slice(&f.scene.lights));
        hash.update(bytemuck::cast_slice(&f.scene.environment));
        hash.update(bytemuck::bytes_of(&f.scene.sun_lux));
        hash.update(bytemuck::bytes_of(&f.scene.sun_direction));
        for (key, _) in &f.textures {
            hash.update(key);
        }
        let key = format!("{:x}", hash.finalize());
        Ok(Self {
            root: root.map(|r| r.join(key)),
            size,
        })
    }
    pub fn read(&self) -> Result<Option<Pair>> {
        let Some(root) = &self.root else {
            return Ok(None);
        };
        if !root.join("complete").exists() {
            return Ok(None);
        }
        let ldr = fs::read(root.join("native.rgba8"))?;
        let hdr = fs::read(root.join("native.rgba32f"))?;
        ensure!(
            ldr.len() == (self.size[0] * self.size[1] * 4) as usize && hdr.len() == ldr.len() * 4,
            "cached reference extent mismatch"
        );
        let image = image::RgbaImage::from_raw(self.size[0], self.size[1], ldr)
            .expect("validated RGBA extent");
        let radiance = hdr
            .as_chunks::<16>()
            .0
            .iter()
            .map(|p| {
                std::array::from_fn(|i| {
                    f32::from_le_bytes(p[i * 4..i * 4 + 4].try_into().expect("four bytes"))
                })
            })
            .collect();
        Ok(Some((image, radiance)))
    }
    pub fn write(&self, image: &image::RgbaImage, data: &[[f32; 4]]) -> Result<()> {
        if let Some(root) = &self.root {
            fs::create_dir_all(root)?;
            fs::write(root.join("native.rgba8"), image.as_raw())?;
            fs::write(root.join("native.rgba32f"), bytemuck::cast_slice(data))?;
            fs::write(
                root.join("complete"),
                b"finite reference; not certified converged\n",
            )?;
        }
        Ok(())
    }
}
