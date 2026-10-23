use anyhow::{Result, ensure};
use ara_vfx::{Effect, Module, Sorting};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec4};

pub(crate) const CONFIG_STRIDE: usize = 192;
pub(crate) const CONTROL_STRIDE: usize = 64;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Config {
    pub partition: [u32; 4], // base, capacity, group base, group count
    pub lists: [u32; 4],     // alive, dead, local prefix, group counts (byte offsets)
    pub scratch: [u32; 4],   // group prefix, event records, routed records, padded count
    pub render: [u32; 4],    // geometry, vertex count, domain, sorting
    pub atlas: [u32; 4],     // grid x/y, event cap, reserved
    pub surface: [f32; 4],   // soft distance, fps, width, overdraw budget
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
    pub light_color: [f32; 4],    // rgb, lumens
    pub light_position: [f32; 4], // xyz,radius
    pub appearance: [u32; 4],     // sheet, blend, alignment, reserved
    pub motion: [f32; 4],         // stretch, shutter, reserved
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Frame {
    pub vp: Mat4,
    pub inverse_vp: Mat4,
    pub camera: Vec4,
    pub right: Vec4,
    pub up: Vec4,
    pub forward: Vec4,
    pub extent: [u32; 4], // width,height,total padded slots,emitter count
    pub clock: [f32; 4],  // dt,exposure,near,unused
    pub inputs: [u32; 4], // config index,depth index,depth enabled,tick
    pub demands: [[u32; 4]; 16],
    pub visuals: [u32; 4], // atlas, background, flags (linear depth/background), reserved
    pub ambient: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_color: [f32; 4],
    pub probe_origin: [f32; 4],
    pub probe_spacing: [f32; 4],
    pub probe_counts: [u32; 4], // xyz, buffer index + 1
}
pub(crate) struct Layout {
    pub configs: Vec<Config>,
    pub total: u32,
    pub list_bytes: u32,
}
impl Layout {
    pub fn new(effect: &Effect) -> Result<Self> {
        effect.validate().map_err(|d| anyhow::anyhow!("{d:?}"))?;
        let mut total = 0;
        let mut groups = 0;
        let mut configs = Vec::new();
        let mut lights = 0;
        for (e, capacity) in effect.emitters.iter().zip(effect.capacities()) {
            let padded = capacity.max(1).next_power_of_two().max(256);
            let mut c = Config::zeroed();
            c.partition = [total, capacity, groups, padded / 256];
            c.scratch[3] = padded;
            c.atlas = [1, 1, effect.quality.events, 0];
            c.motion = [1., 0., 0., 0.];
            c.surface[3] = effect.quality.overdraw * capacity as f32
                / effect.capacities().iter().sum::<u32>().max(1) as f32;
            c.bounds_min = [e.bounds.min[0], e.bounds.min[1], e.bounds.min[2], 0.];
            c.bounds_max = [e.bounds.max[0], e.bounds.max[1], e.bounds.max[2], 0.];
            for m in &e.stacks.render {
                let domain = match &m.module {
                    Module::Appearance {
                        texture,
                        blend,
                        alignment,
                        stretch,
                        shutter,
                    } => {
                        c.appearance = [*texture as u32, *blend as u32, *alignment as u32, 0];
                        c.motion = [*stretch, *shutter, 0., 0.];
                        None
                    }
                    Module::Sprite {
                        domain,
                        soft_distance,
                        sorting,
                    } => {
                        c.render[0] = 0;
                        c.render[1] = 6;
                        c.render[3] = sort(*sorting);
                        c.surface[0] = *soft_distance;
                        Some(*domain)
                    }
                    Module::Ribbon { domain, width } => {
                        c.render[0] = 1;
                        c.render[1] = 6;
                        c.surface[2] = *width;
                        c.render[3] = 2;
                        Some(*domain)
                    }
                    Module::Mesh {
                        domain,
                        triangles,
                        sorting,
                    } => {
                        c.render[0] = 2;
                        c.render[1] = triangles.len() as u32 * 3;
                        c.render[3] = sort(*sorting);
                        Some(*domain)
                    }
                    Module::Flipbook { grid, fps } => {
                        c.atlas[0] = grid[0];
                        c.atlas[1] = grid[1];
                        c.surface[1] = *fps;
                        None
                    }
                    Module::Light {
                        color,
                        lumens,
                        radius,
                    } if lights < effect.quality.lights => {
                        lights += 1;
                        c.light_color = [color[0], color[1], color[2], *lumens];
                        c.light_position = [
                            (e.bounds.min[0] + e.bounds.max[0]) * 0.5,
                            0.7,
                            (e.bounds.min[2] + e.bounds.max[2]) * 0.5,
                            *radius,
                        ];
                        None
                    }
                    _ => None,
                };
                if let Some(d) = domain {
                    c.render[2] = match d {
                        ara_material::Domain::Surface => 0,
                        ara_material::Domain::Ui => 2,
                        _ => 1,
                    };
                }
            }
            configs.push(c);
            total += padded;
            groups += padded / 256;
        }
        // Alive/dead U32 lists, uint4 local prefixes and two uint4 group channels.
        let alive = 0;
        let dead = total * 4;
        let prefix = total * 8;
        let counts = total * 24;
        let group_prefix = counts + groups * 16;
        let events = group_prefix + groups * 16;
        let cap = effect.quality.events;
        let routed = events + configs.len() as u32 * cap * 32;
        let list_bytes = routed + configs.len() as u32 * cap * 16;
        for (i, c) in configs.iter_mut().enumerate() {
            c.lists = [
                alive + c.partition[0] * 4,
                dead + c.partition[0] * 4,
                prefix + c.partition[0] * 16,
                counts + c.partition[2] * 16,
            ];
            c.scratch[0] = group_prefix + c.partition[2] * 16;
            c.scratch[1] = events + i as u32 * cap * 32;
            c.scratch[2] = routed + i as u32 * cap * 16;
        }
        ensure!(
            u64::from(total) * 64 <= 128 * 1024 * 1024,
            "particle storage exceeds native binding range"
        );
        Ok(Self {
            configs,
            total,
            list_bytes,
        })
    }
}
fn sort(s: Sorting) -> u32 {
    match s {
        Sorting::None => 0,
        Sorting::Depth => 1,
        Sorting::Age => 2,
    }
}
