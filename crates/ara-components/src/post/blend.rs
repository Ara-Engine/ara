//! Per-camera blending of post-process volumes (Unity's volume stack): start from
//! [`PostValues::default`], visit volumes in ascending priority (ties in entity ID
//! order) and move each overridden property toward the volume's value by its weight
//! at the camera. Numbers and colours interpolate; enums and the lookup table switch
//! as soon as a volume has any weight.

use super::{PostModule, PostOverride, PostProcessVolume, VolumeShape};
use crate::render::{ExposureMode, SceneLook, ToneMapping};
use ara_schema::{AssetRef, Color};
use glam::{Affine3A, Quat, Vec3};

/// A post-process volume as extracted for one frame: the component and its entity's
/// world pose (the shape's frame). `SceneLook::volumes` keeps them in entity ID order.
#[derive(Debug, Clone, PartialEq)]
pub struct PostVolume {
    /// The authored volume.
    pub volume: PostProcessVolume,
    /// World position of the entity, in metres.
    pub translation: [f32; 3],
    /// World rotation of the entity (unit quaternion, x y z w).
    pub rotation: [f32; 4],
    /// World scale of the entity: the box's edge lengths and the sphere's diameters, in metres.
    pub scale: [f32; 3],
}

impl PostVolume {
    /// The volume's weight at `camera` (a world position in metres): its `weight` inside
    /// the shape (everywhere for a global volume), falling linearly to 0 at
    /// `blendDistance` outside. A degenerate (zero-scale) box or sphere has no weight.
    pub fn weight_at(&self, camera: [f32; 3]) -> f32 {
        let camera = Vec3::from_array(camera);
        let v = &self.volume;
        let weight = if v.weight.is_finite() {
            v.weight.clamp(0., 1.)
        } else {
            0.
        };
        if v.shape == VolumeShape::Global {
            return weight;
        }
        let world = Affine3A::from_scale_rotation_translation(
            Vec3::from_array(self.scale),
            Quat::from_array(self.rotation).normalize(),
            Vec3::from_array(self.translation),
        );
        let determinant = world.matrix3.determinant();
        if !determinant.is_finite() || determinant.abs() < 1e-12 {
            return 0.;
        }
        let local = world.inverse().transform_point3(camera);
        let closest = match v.shape {
            VolumeShape::Box => local.clamp(Vec3::splat(-0.5), Vec3::splat(0.5)),
            // The nearest point on the unit sphere's surface, mapped back to world
            // space: exact for uniform scale, close for mild ellipsoids.
            _ if local.length() <= 0.5 => local,
            _ => local * (0.5 / local.length()),
        };
        if closest == local {
            return weight;
        }
        let distance = world.transform_point3(closest).distance(camera);
        if v.blend_distance > 0. && v.blend_distance.is_finite() {
            weight * (1. - distance / v.blend_distance).max(0.)
        } else {
            0.
        }
    }
}

/// A preset-relative setting a volume may override (AO radius, SSR distance): the
/// final value is `preset × keep + add`, so overrides at any weight compose exactly
/// before the renderer knows the preset's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresetBlend {
    /// Share of the preset's value that remains.
    pub keep: f32,
    /// Overridden contribution, in the setting's unit.
    pub add: f32,
}
impl Default for PresetBlend {
    fn default() -> Self {
        Self { keep: 1., add: 0. }
    }
}
impl PresetBlend {
    /// The value over the preset's own.
    pub fn over(self, preset: f32) -> f32 {
        preset * self.keep + self.add
    }
    fn toward(&mut self, target: Option<f32>, w: f32) {
        if let Some(t) = target.filter(|t| t.is_finite()) {
            self.keep *= 1. - w;
            self.add = self.add * (1. - w) + t * w;
        }
    }
}

/// Post-processing for one camera, blended from every volume (`SceneLook::post_at`);
/// `ara-render-fidelity::environment` maps it onto both renderers. The defaults are
/// the engine's: automatic exposure, Filmic tone mapping, light bloom, neutral grading,
/// and every lens effect off.
#[derive(Debug, Clone, PartialEq)]
pub struct PostValues {
    /// Automatic metering or a fixed exposure.
    pub exposure: ExposureMode,
    /// Manual exposure value at ISO 100.
    pub ev100: f32,
    /// Exposure compensation in stops.
    pub compensation: f32,
    /// Darkest automatic exposure.
    pub min_ev100: f32,
    /// Brightest automatic exposure.
    pub max_ev100: f32,
    /// Automatic adaptation rate, per second.
    pub adaptation_speed: f32,
    /// Display transform.
    pub tone_mapping: ToneMapping,
    /// Bloom energy fraction.
    pub bloom: f32,
    /// Neutral colour temperature, in kelvin.
    pub white_balance: f32,
    /// Colour saturation.
    pub saturation: f32,
    /// Contrast around middle grey.
    pub contrast: f32,
    /// Display-space shadow offset (linear RGB).
    pub lift: [f32; 3],
    /// Display-space midtone power (linear RGB; 1 is neutral).
    pub gamma: [f32; 3],
    /// Display-space multiplier (linear RGB).
    pub gain: [f32; 3],
    /// Lookup table texture, when one applies.
    pub lut: Option<AssetRef>,
    /// Share of the lookup table applied.
    pub lut_contribution: f32,
    /// Corner darkening.
    pub vignette: f32,
    /// In-focus distance in metres; 0 is no depth of field.
    pub focus_distance: f32,
    /// Lens f-number.
    pub aperture: f32,
    /// Foreground blur strength.
    pub near_blur: f32,
    /// Background blur strength.
    pub far_blur: f32,
    /// Shutter angle in degrees; 0 is no motion blur.
    pub shutter_angle: f32,
    /// Colour fringe strength; 0 is none.
    pub chromatic_aberration: f32,
    /// Ambient occlusion strength.
    pub ao_intensity: f32,
    /// Ambient occlusion radius over the preset's (metres).
    pub ao_radius: PresetBlend,
    /// Screen-space reflection strength.
    pub reflection_intensity: f32,
    /// Longest screen-space reflection over the preset's (metres).
    pub reflection_distance: PresetBlend,
}
impl Default for PostValues {
    fn default() -> Self {
        Self {
            exposure: ExposureMode::Auto,
            ev100: 10.,
            compensation: 0.,
            min_ev100: -2.,
            max_ev100: 16.,
            adaptation_speed: 2.,
            tone_mapping: ToneMapping::Filmic,
            bloom: 0.04,
            white_balance: 6500.,
            saturation: 1.,
            contrast: 1.,
            lift: [0.; 3],
            gamma: [1.; 3],
            gain: [1.; 3],
            lut: None,
            lut_contribution: 1.,
            vignette: 0.,
            focus_distance: 0.,
            aperture: 2.8,
            near_blur: 1.,
            far_blur: 1.,
            shutter_angle: 0.,
            chromatic_aberration: 0.,
            ao_intensity: 1.,
            ao_radius: PresetBlend::default(),
            reflection_intensity: 1.,
            reflection_distance: PresetBlend::default(),
        }
    }
}

impl PostValues {
    /// Moves the properties `entry` ticks toward its values by `w` (in (0, 1]). Only
    /// the properties of the entry's module apply: values left in fields of another
    /// group (after switching `module`) stay inert.
    pub fn apply(&mut self, entry: &PostOverride, w: f32) {
        let o = entry;
        match o.module {
            PostModule::Exposure => {
                pick(&mut self.exposure, o.exposure);
                mix(&mut self.ev100, o.ev100, w);
                mix(&mut self.compensation, o.compensation, w);
                mix(&mut self.min_ev100, o.min_ev100, w);
                mix(&mut self.max_ev100, o.max_ev100, w);
                mix(&mut self.adaptation_speed, o.adaptation_speed, w);
            }
            PostModule::ToneMapping => pick(&mut self.tone_mapping, o.tone_mapping),
            PostModule::Bloom => mix(&mut self.bloom, o.bloom, w),
            PostModule::ColorGrading => {
                mix(&mut self.white_balance, o.white_balance, w);
                mix(&mut self.saturation, o.saturation, w);
                mix(&mut self.contrast, o.contrast, w);
                mix_rgb(&mut self.lift, o.lift, w);
                mix_rgb(&mut self.gamma, o.gamma, w);
                mix_rgb(&mut self.gain, o.gain, w);
                if let Some(lut) = &o.lut {
                    self.lut = (!lut.0.is_empty()).then(|| lut.clone());
                }
                mix(&mut self.lut_contribution, o.lut_contribution, w);
            }
            PostModule::Vignette => mix(&mut self.vignette, o.vignette, w),
            PostModule::DepthOfField => {
                mix(&mut self.focus_distance, o.focus_distance, w);
                mix(&mut self.aperture, o.aperture, w);
                mix(&mut self.near_blur, o.near_blur, w);
                mix(&mut self.far_blur, o.far_blur, w);
            }
            PostModule::MotionBlur => mix(&mut self.shutter_angle, o.shutter_angle, w),
            PostModule::ChromaticAberration => {
                mix(&mut self.chromatic_aberration, o.chromatic_aberration, w)
            }
            PostModule::AmbientOcclusion => {
                mix(&mut self.ao_intensity, o.ao_intensity, w);
                self.ao_radius.toward(o.ao_radius, w);
            }
            PostModule::Reflections => {
                mix(&mut self.reflection_intensity, o.reflection_intensity, w);
                self.reflection_distance.toward(o.reflection_distance, w);
            }
        }
    }
}

fn mix(value: &mut f32, target: Option<f32>, w: f32) {
    if let Some(t) = target.filter(|t| t.is_finite()) {
        *value += (t - *value) * w;
    }
}

fn mix_rgb(value: &mut [f32; 3], target: Option<Color>, w: f32) {
    if let Some(Color(t)) = target {
        for (v, t) in value.iter_mut().zip(t) {
            mix(v, Some(t), w);
        }
    }
}

fn pick<T: Copy>(value: &mut T, target: Option<T>) {
    if let Some(t) = target {
        *value = t;
    }
}

impl SceneLook {
    /// Post-processing for a camera at `camera` (a world position in metres): `None`
    /// when the scene has no post-process volume (renderers keep their legacy
    /// exposure), otherwise the defaults with every volume that has weight there
    /// blended in, by priority.
    pub fn post_at(&self, camera: [f32; 3]) -> Option<PostValues> {
        if self.volumes.is_empty() {
            return None;
        }
        let mut values = PostValues::default();
        for volume in self.in_priority_order() {
            let w = volume.weight_at(camera);
            if w > 0. {
                for entry in &volume.volume.overrides {
                    values.apply(entry, w);
                }
            }
        }
        Some(values)
    }

    /// Volumes in blend order, ascending priority with ties in entity ID order,
    /// without allocating (scenes hold a handful of volumes).
    fn in_priority_order(&self) -> impl Iterator<Item = &PostVolume> {
        let volumes = &self.volumes;
        let order = move |a: usize, b: usize| {
            let (pa, pb) = (volumes[a].volume.priority, volumes[b].volume.priority);
            pa.total_cmp(&pb).then(a.cmp(&b))
        };
        let mut last: Option<usize> = None;
        std::iter::from_fn(move || {
            let next = (0..volumes.len())
                .filter(|&i| last.is_none_or(|l| order(i, l).is_gt()))
                .min_by(|&a, &b| order(a, b))?;
            last = Some(next);
            Some(&volumes[next])
        })
    }
}
