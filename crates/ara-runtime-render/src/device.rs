//! The device the presenter and the editor's scene compositor share.
use anyhow::Result;
use ara_gpu::{GpuDevice, GpuLease};
use std::path::PathBuf;

/// The GPU owner for presentation.
///
/// The device sits in a [`GpuLease`], so its address stays put when this value
/// moves. A renderer built with `Renderer::on_device` holds its own lease: the
/// device stays alive until both have dropped, in either order. On Apple `open`
/// is `GpuDevice::new`, the same constructor the bootstrap renderer used, which
/// is the Metal device.
pub struct PresentationDevice {
    lease: GpuLease,
}

impl PresentationDevice {
    /// Opens the presentation device: hardware ray queries where the GPU has
    /// them, the same device `Renderer::new_hybrid` created. `extensions` are
    /// the window's (empty when headless). `cache` holds the pipeline cache.
    pub fn open(extensions: &[std::ffi::CString], cache: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            lease: GpuLease::new(GpuDevice::new(extensions, cache.into())?),
        })
    }

    /// Adopts a device the caller already opened (a loading handoff).
    pub fn from_device(gpu: GpuDevice) -> Self {
        Self {
            lease: GpuLease::new(gpu),
        }
    }

    /// Another lease on this device. The renderer stores it so the device
    /// outlives this owner.
    pub fn lease(&self) -> GpuLease {
        self.lease.clone()
    }

    /// The device external renderers and the editor compositor record on.
    pub fn gpu(&self) -> &GpuDevice {
        self.lease.get()
    }

    /// The device, mutably, between completed frames.
    ///
    /// The renderer's lease aliases this allocation. Call this when that
    /// renderer is not borrowing the device.
    pub fn gpu_mut(&mut self) -> &mut GpuDevice {
        // SAFETY: `&mut self` is the recording thread's use of this owner
        // between frames. The allocation lives as long as any lease does.
        unsafe { &mut *self.lease.as_mut_ptr() }
    }
}

#[cfg(test)]
mod tests {
    use super::PresentationDevice;
    use ara_frame::scene::{CameraSpec, Material, Mesh, Scene, Texture, Vertex};
    use ara_render::Renderer;

    fn triangle() -> Scene {
        Scene {
            meshes: vec![Mesh {
                entity: 1,
                name: "triangle".into(),
                vertices: vec![
                    Vertex {
                        position: [0.0, 0.0, 0.0],
                        normal: [0.0, 1.0, 0.0],
                        uv: [0.0, 0.0],
                        tangent: [1.0, 0.0, 0.0, 1.0],
                    },
                    Vertex {
                        position: [1.0, 0.0, 0.0],
                        normal: [0.0, 1.0, 0.0],
                        uv: [1.0, 0.0],
                        tangent: [1.0, 0.0, 0.0, 1.0],
                    },
                    Vertex {
                        position: [0.0, 1.0, 0.0],
                        normal: [0.0, 1.0, 0.0],
                        uv: [0.0, 1.0],
                        tangent: [1.0, 0.0, 0.0, 1.0],
                    },
                ],
                indices: vec![0, 1, 2],
                material: 0,
            }],
            materials: vec![Material::default()],
            textures: vec![
                Texture {
                    size: [1, 1],
                    rgba: vec![255, 255, 255, 255],
                    srgb: false,
                },
                Texture {
                    size: [1, 1],
                    rgba: vec![128, 128, 255, 255],
                    srgb: false,
                },
            ],
            lights: Vec::new(),
            sun_direction: glam30::Vec3::Y,
            sun_lux: glam30::Vec3::ONE,
            environment: vec![[1.0; 4]],
            environment_size: [1, 1],
        }
    }

    /// Moves the owner after `Renderer::on_device`, submits a frame, drops the
    /// owner, and polls. The poll is the call that jumped through a freed
    /// dispatch table when the renderer stored a raw pointer and the owner dropped.
    #[test]
    fn a_renderer_polls_after_its_device_owner_is_moved_and_dropped() -> anyhow::Result<()> {
        let cache = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")
            })
            .join("presentation-device-lease");
        let device = PresentationDevice::open(&[], &cache)?;
        let address = device.gpu() as *const ara_gpu::GpuDevice;
        let mut renderer = Renderer::on_device(triangle(), [64, 64], device.lease())?;
        assert_eq!(renderer.gpu() as *const ara_gpu::GpuDevice, address);
        let device = device;
        assert_eq!(device.gpu() as *const ara_gpu::GpuDevice, address);
        assert_eq!(renderer.gpu() as *const ara_gpu::GpuDevice, address);
        let camera = CameraSpec::LookAt {
            position: glam30::Vec3::new(0.0, 0.0, 3.0),
            target: glam30::Vec3::ZERO,
            vertical_fov: 0.9,
        };
        assert!(renderer.submit_scene_frame(|renderer| renderer.render(&camera, 1.0))?);
        drop(device);
        assert_eq!(renderer.gpu() as *const ara_gpu::GpuDevice, address);
        renderer.poll_scene_frame()?;
        renderer.wait_scene_frame()?;
        Ok(())
    }
}
