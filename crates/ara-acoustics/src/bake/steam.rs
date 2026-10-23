//! Steam Audio 4.8.1 parametric reverb baking, loaded at runtime from an explicit path.
//!
//! Optional cook step: when the runtime is absent, cells bake with `GeometricBaker`.
//! Steam supplies RT60 per probe; room size and level come from a geometric pass.
//! Native handles never leave this module and are released in reverse order.

use super::{GeometricBaker, ReverbBaker, ReverbProbe};
use crate::{geometry::MeshScene, math::V3};
use ara_audio::{AudioError, Result};
use libloading::Library;
use std::{
    ffi::{c_char, c_void},
    path::Path,
    ptr,
};

type Handle = *mut c_void;

#[repr(C)]
struct ContextSettings {
    version: u32,
    log: Option<unsafe extern "C" fn(i32, *const c_char)>,
    allocate: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
    free: Option<unsafe extern "C" fn(*mut c_void)>,
    simd: i32,
    flags: i32,
}
#[repr(C)]
struct SceneSettings {
    kind: i32,
    closest_hit: *const c_void,
    any_hit: *const c_void,
    batched_closest_hit: *const c_void,
    batched_any_hit: *const c_void,
    user_data: *mut c_void,
    embree: Handle,
    radeon_rays: Handle,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Material {
    absorption: [f32; 3],
    scattering: f32,
    transmission: [f32; 3],
}
#[repr(C)]
struct StaticMeshSettings {
    vertices_len: i32,
    triangles_len: i32,
    materials_len: i32,
    vertices: *mut [f32; 3],
    triangles: *mut [i32; 3],
    material_indices: *mut i32,
    materials: *mut Material,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Sphere {
    center: [f32; 3],
    radius: f32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct BakedDataIdentifier {
    kind: i32,
    variation: i32,
    influence: Sphere,
}
#[repr(C)]
struct ReflectionsBakeParams {
    scene: Handle,
    probe_batch: Handle,
    scene_type: i32,
    identifier: BakedDataIdentifier,
    flags: i32,
    rays: i32,
    diffuse_samples: i32,
    bounces: i32,
    simulated_duration: f32,
    saved_duration: f32,
    order: i32,
    threads: i32,
    ray_batch_size: i32,
    irradiance_min_distance: f32,
    bake_batch_size: i32,
    opencl: Handle,
    radeon_rays: Handle,
}

const SUCCESS: i32 = 0;
const VERSION_4_8_1: u32 = 0x0004_0801;
const SCENE_DEFAULT: i32 = 0;
const BAKE_PARAMETRIC: i32 = 1 << 1;
/// IPL_BAKEDDATATYPE_REFLECTIONS with IPL_BAKEDDATAVARIATION_REVERB.
const REVERB: BakedDataIdentifier = BakedDataIdentifier {
    kind: 0,
    variation: 0,
    influence: Sphere {
        center: [0.0; 3],
        radius: 0.0,
    },
};

type ContextCreate = unsafe extern "C" fn(*mut ContextSettings, *mut Handle) -> i32;
type Release = unsafe extern "C" fn(*mut Handle);
type SceneCreate = unsafe extern "C" fn(Handle, *mut SceneSettings, *mut Handle) -> i32;
type SceneCommit = unsafe extern "C" fn(Handle);
type MeshCreate = unsafe extern "C" fn(Handle, *mut StaticMeshSettings, *mut Handle) -> i32;
type MeshAdd = unsafe extern "C" fn(Handle, Handle);
type BatchCreate = unsafe extern "C" fn(Handle, *mut Handle) -> i32;
type BatchAdd = unsafe extern "C" fn(Handle, Sphere);
type BatchCommit = unsafe extern "C" fn(Handle);
type BatchCount = unsafe extern "C" fn(Handle) -> i32;
type BatchReverb = unsafe extern "C" fn(Handle, *mut BakedDataIdentifier, i32, *mut f32);
type Bake = unsafe extern "C" fn(
    Handle,
    *mut ReflectionsBakeParams,
    Option<unsafe extern "C" fn(f32, *mut c_void)>,
    *mut c_void,
);

struct Api {
    context_create: ContextCreate,
    context_release: Release,
    scene_create: SceneCreate,
    scene_release: Release,
    scene_commit: SceneCommit,
    mesh_create: MeshCreate,
    mesh_release: Release,
    mesh_add: MeshAdd,
    batch_create: BatchCreate,
    batch_release: Release,
    batch_add: BatchAdd,
    batch_commit: BatchCommit,
    batch_count: BatchCount,
    batch_reverb: BatchReverb,
    bake: Bake,
    _library: Library,
}

fn symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T> {
    // SAFETY: every caller names a Steam Audio 4.8.1 export with its exact C signature,
    // and `Api` keeps the library loaded for as long as the copied pointers live.
    unsafe { library.get::<T>(name).map(|s| *s) }.map_err(|e| AudioError::Backend(e.to_string()))
}

impl Api {
    fn load(path: &Path) -> Result<Self> {
        // SAFETY: the path is an explicitly supplied, trusted Steam Audio runtime.
        let lib = unsafe { Library::new(path) }.map_err(|e| AudioError::Backend(e.to_string()))?;
        Ok(Self {
            context_create: symbol(&lib, b"iplContextCreate\0")?,
            context_release: symbol(&lib, b"iplContextRelease\0")?,
            scene_create: symbol(&lib, b"iplSceneCreate\0")?,
            scene_release: symbol(&lib, b"iplSceneRelease\0")?,
            scene_commit: symbol(&lib, b"iplSceneCommit\0")?,
            mesh_create: symbol(&lib, b"iplStaticMeshCreate\0")?,
            mesh_release: symbol(&lib, b"iplStaticMeshRelease\0")?,
            mesh_add: symbol(&lib, b"iplStaticMeshAdd\0")?,
            batch_create: symbol(&lib, b"iplProbeBatchCreate\0")?,
            batch_release: symbol(&lib, b"iplProbeBatchRelease\0")?,
            batch_add: symbol(&lib, b"iplProbeBatchAddProbe\0")?,
            batch_commit: symbol(&lib, b"iplProbeBatchCommit\0")?,
            batch_count: symbol(&lib, b"iplProbeBatchGetNumProbes\0")?,
            batch_reverb: symbol(&lib, b"iplProbeBatchGetReverb\0")?,
            bake: symbol(&lib, b"iplReflectionsBakerBake\0")?,
            _library: lib,
        })
    }
}

/// Bake quality knobs, mirroring `IPLReflectionsBakeParams`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SteamBakeSettings {
    /// Rays traced per probe.
    pub rays: u32,
    /// Diffuse samples per hit.
    pub diffuse_samples: u32,
    /// Reflections followed per ray. Long, hard rooms need ~100+ to cover a 60 dB decay.
    pub bounces: u32,
    /// Simulated impulse response length in seconds.
    pub duration: f32,
    /// Worker threads Steam Audio may use.
    pub threads: u32,
    /// Probe sphere radius in meters.
    pub probe_radius: f32,
}

impl Default for SteamBakeSettings {
    fn default() -> Self {
        Self {
            rays: 8192,
            diffuse_samples: 64,
            bounces: 128,
            duration: 3.0,
            threads: 2,
            probe_radius: 1.0,
        }
    }
}

/// Steam Audio parametric reverb baker. Load once per cook worker.
pub struct SteamBaker {
    api: Api,
    context: Handle,
    /// Bake quality.
    pub settings: SteamBakeSettings,
    /// Supplies mean free path, volume and absorption statistics.
    pub statistics: GeometricBaker,
}

impl Drop for SteamBaker {
    fn drop(&mut self) {
        if !self.context.is_null() {
            // SAFETY: the context was created by this library, which is still loaded,
            // and every object created from it has been released.
            unsafe { (self.api.context_release)(&mut self.context) };
        }
    }
}

/// Owned native handles released in reverse creation order.
struct Bakery<'a> {
    api: &'a Api,
    scene: Handle,
    mesh: Handle,
    batch: Handle,
}

impl Drop for Bakery<'_> {
    fn drop(&mut self) {
        // SAFETY: each handle is null or was created by `api` and is released exactly once.
        unsafe {
            if !self.batch.is_null() {
                (self.api.batch_release)(&mut self.batch);
            }
            if !self.mesh.is_null() {
                (self.api.mesh_release)(&mut self.mesh);
            }
            if !self.scene.is_null() {
                (self.api.scene_release)(&mut self.scene);
            }
        }
    }
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == SUCCESS {
        Ok(())
    } else {
        Err(AudioError::Backend(format!(
            "Steam Audio {what} failed ({status})"
        )))
    }
}

impl SteamBaker {
    /// Loads Steam Audio 4.8.1 from `path` (`phonon.dll`, `libphonon.so`, `libphonon.dylib`).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let api = Api::load(path.as_ref())?;
        let mut settings = ContextSettings {
            version: VERSION_4_8_1,
            log: None,
            allocate: None,
            free: None,
            simd: 3,
            flags: 0,
        };
        let mut context = ptr::null_mut();
        // SAFETY: settings and the output handle are valid for the duration of the call.
        let status = unsafe { (api.context_create)(&mut settings, &mut context) };
        check(status, "context")?;
        Ok(Self {
            api,
            context,
            settings: SteamBakeSettings::default(),
            statistics: GeometricBaker {
                rays: 256,
                bounces: 16,
                ..GeometricBaker::default()
            },
        })
    }

    fn reverb_times(&self, scene: &MeshScene, positions: &[V3]) -> Result<Vec<[f32; 3]>> {
        let s = self.settings;
        if s.rays == 0
            || s.bounces == 0
            || s.duration.is_nan()
            || s.duration <= 0.0
            || s.probe_radius.is_nan()
            || s.probe_radius <= 0.0
        {
            return Err(AudioError::Invalid("Steam bake settings"));
        }
        let mut vertices: Vec<[f32; 3]> = scene.vertices().to_vec();
        let mut triangles = Vec::with_capacity(scene.triangle_count());
        let mut indices = Vec::with_capacity(scene.triangle_count());
        for (t, material) in scene.triangles() {
            triangles.push(t.map(|i| i as i32));
            indices.push(material as i32);
        }
        let mut materials: Vec<Material> = scene
            .materials()
            .iter()
            .map(|m| Material {
                absorption: m.absorption,
                scattering: m.scattering,
                transmission: m.transmission,
            })
            .collect();
        let too_big = |n: usize| i32::try_from(n).is_err();
        if triangles.is_empty() || too_big(vertices.len()) || too_big(triangles.len()) {
            return Err(AudioError::Invalid("Steam bake scene size"));
        }
        let mut bakery = Bakery {
            api: &self.api,
            scene: ptr::null_mut(),
            mesh: ptr::null_mut(),
            batch: ptr::null_mut(),
        };
        let mut scene_settings = SceneSettings {
            kind: SCENE_DEFAULT,
            closest_hit: ptr::null(),
            any_hit: ptr::null(),
            batched_closest_hit: ptr::null(),
            batched_any_hit: ptr::null(),
            user_data: ptr::null_mut(),
            embree: ptr::null_mut(),
            radeon_rays: ptr::null_mut(),
        };
        let mut mesh_settings = StaticMeshSettings {
            vertices_len: vertices.len() as i32,
            triangles_len: triangles.len() as i32,
            materials_len: materials.len() as i32,
            vertices: vertices.as_mut_ptr(),
            triangles: triangles.as_mut_ptr(),
            material_indices: indices.as_mut_ptr(),
            materials: materials.as_mut_ptr(),
        };
        // SAFETY: all pointers reference live, correctly sized buffers owned by this frame
        // for the duration of each call; Steam copies mesh data on creation. Handles are
        // owned by `bakery` and released on every exit path.
        unsafe {
            check(
                (self.api.scene_create)(self.context, &mut scene_settings, &mut bakery.scene),
                "scene",
            )?;
            check(
                (self.api.mesh_create)(bakery.scene, &mut mesh_settings, &mut bakery.mesh),
                "static mesh",
            )?;
            (self.api.mesh_add)(bakery.mesh, bakery.scene);
            (self.api.scene_commit)(bakery.scene);
            check(
                (self.api.batch_create)(self.context, &mut bakery.batch),
                "probe batch",
            )?;
            for p in positions {
                (self.api.batch_add)(
                    bakery.batch,
                    Sphere {
                        center: *p,
                        radius: s.probe_radius,
                    },
                );
            }
            (self.api.batch_commit)(bakery.batch);
            if (self.api.batch_count)(bakery.batch) as usize != positions.len() {
                return Err(AudioError::Backend(
                    "Steam probe batch size mismatch".into(),
                ));
            }
            let mut params = ReflectionsBakeParams {
                scene: bakery.scene,
                probe_batch: bakery.batch,
                scene_type: SCENE_DEFAULT,
                identifier: REVERB,
                flags: BAKE_PARAMETRIC,
                rays: s.rays as i32,
                diffuse_samples: s.diffuse_samples as i32,
                bounces: s.bounces as i32,
                simulated_duration: s.duration,
                saved_duration: s.duration,
                order: 0,
                threads: s.threads.max(1) as i32,
                ray_batch_size: 1,
                irradiance_min_distance: 1.0,
                bake_batch_size: 1,
                opencl: ptr::null_mut(),
                radeon_rays: ptr::null_mut(),
            };
            (self.api.bake)(self.context, &mut params, None, ptr::null_mut());
            let mut identifier = REVERB;
            let mut out = Vec::with_capacity(positions.len());
            for i in 0..positions.len() {
                let mut times = [0.0_f32; 3];
                (self.api.batch_reverb)(
                    bakery.batch,
                    &mut identifier,
                    i as i32,
                    times.as_mut_ptr(),
                );
                out.push(times);
            }
            Ok(out)
        }
    }
}

impl ReverbBaker for SteamBaker {
    fn name(&self) -> String {
        "steam-audio/4.8.1".into()
    }

    fn bake(&mut self, scene: &MeshScene, positions: &[V3]) -> Result<Vec<ReverbProbe>> {
        let mut probes = self.statistics.bake(scene, positions)?;
        let times = self.reverb_times(scene, positions)?;
        for (probe, rt60) in probes.iter_mut().zip(times) {
            // Probes Steam could not fill (no hits) keep the geometric estimate.
            if rt60.iter().all(|t| t.is_finite() && *t > 0.0) {
                probe.rt60 = rt60.map(|t| t.clamp(0.05, 20.0));
            }
        }
        Ok(probes)
    }
}
