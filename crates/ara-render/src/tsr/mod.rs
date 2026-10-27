//! Ara TSR: a Slang port of AMD FSR 3.1.4's temporal upscaler (FidelityFX SDK
//! v1.1.4, MIT; `shaders/tsr/`), selectable beside the TAAU resolve (`taaMain`).
//!
//! The renderer declares this pass chain in place of `TemporalResolve` when
//! [`Upscaler::Tsr`] is selected: prepare inputs (with FSR's luma pyramid folded in),
//! the shading change pyramid, prepare reactivity (with FSR's shading change pass
//! evaluated inline), luma instability and accumulate, then RCAS on FSR's path. The
//! result is the renderer's display history slot exactly as `taaMain` writes it
//! (RGBA16F, pre-exposed by the applied exposure), so exposure, bloom, grading, the
//! reflection pyramid and captures read it unchanged.
//!
//! Two accumulate paths (`tuning::Path`):
//! - Ara (default): TSR's colour history is the renderer's own display history pair
//!   (w holds FSR's lock), reprojected with a Catmull-Rom; this frame's samples are
//!   weighted by a display-pixel Gaussian that matches area-sampled truth (FSR's
//!   Lanczos over-sharpens it), and there is no RCAS;
//! - FSR: FSR's own history pair, Lanczos reprojection and upsampling, RCAS.
//!
//! Beyond FSR, Ara feeds engine signals (`shaders/tsr/tsr-signals.slangh`): exact
//! entity-ID history validity refines FSR's depth disocclusion, and reactive,
//! composition and still-transparency masks come from the opaque-only copy forward
//! transparency is drawn over (`look::refract`), SSR confidence and the host's
//! composite coverage (`Renderer::finish_reactive`).
//!
//! Development knobs (environment, read once when the renderer is created;
//! `tuning.rs`), all record constants or pass selections, so sweeps need no recook:
//! - `ARA_UPSCALER=tsr|taau`: the upscaler (default TAAU);
//! - `ARA_TSR_PATH=ara|fsr` (ara); `ARA_TSR_SHARPNESS` 0..1 (0: no RCAS; above 0
//!   selects the FSR path);
//! - `ARA_TSR_KERNEL_STATIC` (5.56) / `ARA_TSR_KERNEL_MOVING` (2.78): Gaussian
//!   spread (display px⁻²) for settled and for new or moving pixels;
//!   `ARA_TSR_KERNEL_GAIN` (1): current-frame weight scale (accumulation rate);
//! - `ARA_TSR_STILL_TRANSPARENCY` (0.5): current-frame weight on still transparency;
//! - `ARA_TSR_NEW_SURFACE` (1), `ARA_TSR_SAME_SURFACE` (4), `ARA_TSR_REFLECTION`
//!   (0.25), `ARA_TSR_COMPOSITION` (0.5): engine signal weights;
//! - `ARA_TSR_AUTOGEN_SCALE` (1), `_THRESHOLD` (0.2), `_VALUE` (0.9),
//!   `ARA_TSR_PARALLAX` (1 render px): reactive autogen;
//! - `ARA_TSR_ACCUMULATION` (1/3), `ARA_TSR_MIN_DISOCCLUSION` (-1/3),
//!   `ARA_TSR_VELOCITY` (1), `ARA_TSR_REACTIVENESS` (1): FSR's context values;
//! - `ARA_TSR_SHADING_CHANGE` (0: the pyramid pass skipped, equal fidelity and
//!   steadier in the Showcase sweep; 1 re-enables FSR's), `ARA_TSR_LUMA_INSTABILITY`
//!   (1; 0 skips the luma instability pass, clearly less stable);
//! - `ARA_TSR_IDS` (1; 0 drops the entity-ID signal and its scratch traffic);
//! - `ARA_TSR_PREEXPOSED` (0): the HDR input already holds radiance × the applied
//!   exposure (the pre-exposure contract); otherwise TSR applies it.
//!
//! Images are allocated on first use at the view's render and display sizes,
//! reallocated (with a history reset) when either size or the path changes, and
//! released when TAAU is selected again: seven bindless storage-image slots on the
//! Ara path, eight on FSR's.
mod constants;
mod tuning;

use crate::graph::{Graph, PassId, ResourceId, State};
use anyhow::{Result, ensure};
use ara_gpu::{
    Access, Buffer, Device, GpuDevice, Image, Pipeline, Queue, Resource, RootConstants, vk,
    vulkan::{Command, Recorder},
};
use constants::TsrConstants;
use glam::Mat4;
use std::path::Path;
use tuning::{Path as AccumulatePath, Tuning};

/// The raster renderer's temporal upscaler (`Renderer::set_upscaler`). Both resolve
/// into the same display history; switching keeps the post chain and captures
/// unchanged, and switching to TSR restarts its accumulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Upscaler {
    /// Ara's temporal antialiasing and upscaling (`taaMain`): one pass, a
    /// luminance-weighted Gaussian reconstruction with variance clipping.
    #[default]
    Taau,
    /// Ara TSR: FSR 3.1.4's temporal upscaler (reactivity, shading change, luma
    /// instability and thin-feature locks) with Ara's engine signals.
    Tsr,
}

impl Upscaler {
    /// The `ARA_UPSCALER` environment variable (`tsr` or `taau`, any case) for A/B
    /// runs; unset or anything else selects TAAU.
    pub fn from_env() -> Self {
        match std::env::var("ARA_UPSCALER") {
            Ok(value) if value.eq_ignore_ascii_case("tsr") => Self::Tsr,
            _ => Self::Taau,
        }
    }
}

// TSR images, in the record's index rows (`TsrConstants::sampled`/`storage`).
/// R16F render size: luma instability (FSR's INTERMEDIATE_FP16x1).
const INTERMEDIATE: usize = 0;
/// R16F, two render widths: this frame's and last frame's luma.
const LUMA: usize = 1;
/// RG16F atlas: shading change mips 0 to 2 and the half-size farthest depth
/// (`pyramid_extent`).
const PYRAMID: usize = 2;
/// Unused row slot (accumulate prepares colour in group memory).
const UNUSED: usize = 3;
/// R8, two render widths: the accumulation counter, this frame's and last frame's.
const ACCUMULATION: usize = 4;
/// RGBA8 render size: reactive, disocclusion, shading change and accumulation.
const MASKS: usize = 5;
/// R8 display size: new thin-feature locks, consumed (zeroed) by accumulate.
const LOCKS: usize = 6;
/// RGBA16F, two render widths: the last four frames' luma.
const LUMA_HISTORY: usize = 7;
/// RGBA16F, two display widths: FSR's accumulated colour and lock (FSR path only).
const UPSCALED: usize = 8;
const IMAGES: usize = 9;
/// Scratch words per render pixel (tsr.slangh `tsrScratch` regions): dilated motion
/// (two halves), reconstructed previous depth, masks, and the dilated surface (entity
/// ID and depth) of this and last frame.
const SCRATCH_WORDS: u64 = 7;

/// Render pixels the scratch regions hold: whole 8x8 tiles (tsr.slangh `tsrIndex`
/// stores them tile by tile).
fn scratch_pixels(render: [u32; 2]) -> u32 {
    render[0].div_ceil(8) * render[1].div_ceil(8) * 64
}

/// Format and extent of each image for `render` and `display` sizes.
fn image_desc(image: usize, render: [u32; 2], display: [u32; 2]) -> (vk::Format, [u32; 2]) {
    let pair = |[w, h]: [u32; 2]| [w * 2, h];
    match image {
        INTERMEDIATE => (vk::Format::R16_SFLOAT, render),
        LUMA => (vk::Format::R16_SFLOAT, pair(render)),
        PYRAMID => (vk::Format::R16G16_SFLOAT, pyramid_extent(render)),
        ACCUMULATION => (vk::Format::R8_UNORM, pair(render)),
        MASKS => (vk::Format::R8G8B8A8_UNORM, render),
        LOCKS => (vk::Format::R8_UNORM, display),
        LUMA_HISTORY => (vk::Format::R16G16B16A16_SFLOAT, pair(render)),
        _ => (vk::Format::R16G16B16A16_SFLOAT, pair(display)),
    }
}

/// FSR's half render size (`ShadingChangeRenderSize`, the SPD mip 0), at least one.
fn half(render: [u32; 2]) -> [u32; 2] {
    render.map(|v| (v / 2).max(1))
}

/// The pyramid atlas (tsr.slangh `tsrPyramidSize`): mip 0 and the farthest-depth mip
/// stacked on the left, mips 1 and 2 stacked to the right.
fn pyramid_extent(render: [u32; 2]) -> [u32; 2] {
    let mip = |level: u32| half(render).map(|v| (v >> level).max(1));
    [
        mip(0)[0] + mip(1)[0],
        (mip(0)[1] * 2).max(mip(1)[1] + mip(2)[1]),
    ]
}

/// One TSR dispatch in the frame graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TsrPass {
    /// Zeroes state FSR clears: on allocation the luma, accumulation and lock
    /// images, on every reset the accumulation (`TsrFrame::clear`).
    Clear,
    PrepareInputs,
    ShadingChangePyramid,
    PrepareReactivity,
    LumaInstability,
    Accumulate,
    /// FSR path only.
    Rcas,
}

struct Pipelines {
    prepare_inputs: Pipeline,
    /// Prepare inputs without a motion target: motion from depth and last frame's
    /// view-projection.
    prepare_inputs_reproject: Pipeline,
    shading_change_pyramid: Pipeline,
    prepare_reactivity: Pipeline,
    luma_instability: Pipeline,
    /// The Ara path's accumulate.
    accumulate: Pipeline,
    /// FSR's accumulate (its own history pair, then RCAS).
    accumulate_fsr: Pipeline,
    rcas: Pipeline,
    clear: Pipeline,
}

/// Size-dependent storage, with each resource's last recorded state.
struct Resources {
    render: [u32; 2],
    display: [u32; 2],
    /// Allocated for FSR's path (with `UPSCALED`).
    fsr: bool,
    images: [Option<Image>; IMAGES],
    sampled: [u32; IMAGES],
    storage: [u32; IMAGES],
    states: [State; IMAGES],
    /// Per render pixel in 8x8 tiles (`SCRATCH_WORDS`, `scratch_pixels`): dilated
    /// motion, the reconstructed previous nearest depth (u32 atomics), the packed
    /// masks and the dilated surface (entity ID, depth) of this and last frame.
    scratch: Buffer,
    scratch_index: u32,
    scratch_state: State,
    /// The per-frame record (host-visible, `upload_queued`).
    record: Buffer,
    record_index: u32,
    /// Freshly allocated: the next frame clears and resets.
    fresh: bool,
}

/// What carries over between frames (FSR's context state).
#[derive(Clone, Copy, Default)]
struct History {
    /// The last completed frame ran TSR on these resources.
    valid: bool,
    jitter: [f32; 2],
    render: [u32; 2],
    display: [u32; 2],
    frame_index: f32,
    /// FSR's smoothed jitter phase count (one step per frame towards the target).
    phases: f32,
    /// This frame's half of every ping-pong pair.
    slot: u32,
    /// The selection epoch (`Tsr::epoch`) the history belongs to.
    epoch: u32,
}

/// A frame whose chain is planned (`begin`) but not yet completed: its record and
/// the history it leaves once recorded (`complete`).
struct TsrFrame {
    constants: TsrConstants,
    path: AccumulatePath,
    /// Prepare inputs reprojects from depth (no motion target).
    reproject: bool,
    /// The shading change pyramid and luma instability passes run.
    shading_change: bool,
    luma_instability: bool,
    /// Images the clear pass zeroes this frame; empty without one.
    clear: Vec<usize>,
    /// The history after this frame.
    next: History,
}

/// The renderer's per-frame TSR inputs (render pixels, bindless indices).
pub(crate) struct FrameInputs {
    pub render: [u32; 2],
    pub display: [u32; 2],
    /// `Frame.upscale.xy`: this frame's sample offset (+y down).
    pub upscale: [f32; 2],
    /// The camera's unjittered (perspective) projection.
    pub projection: Mat4,
    /// The Halton jitter's phase count (`temporal_phases`).
    pub phases: u32,
    /// The renderer's history is invalid or the capture is deterministic.
    pub reset: bool,
    /// The frame's exposure argument (the exposure state's fallback).
    pub exposure: f32,
    /// Sampled reversed-Z depth.
    pub depth: u32,
    /// Sampled native motion target; `None` reprojects from depth.
    pub motion: Option<u32>,
    /// The display frame record (storage buffer; matrices for reprojection).
    pub display_frame: u32,
    /// The look's exposure state (storage buffer).
    pub exposure_state: u32,
    /// Sampled entity IDs (render size, R32 float entity index).
    pub ids: u32,
    /// The lit opaque scene copied before forward transparency this frame
    /// (`look::refract` level 0, sampled) and its valid render-pixel rect.
    pub opaque: Option<(u32, [u32; 4])>,
    /// The SSR trace this frame (half resolution, sampled; a = confidence).
    pub reflections: Option<u32>,
}

/// Indices known only when the chain is declared (`Tsr::upload`).
pub(crate) struct Bindings {
    /// The colour TSR resolves (sampled): HDR, a composite, or depth of field's.
    pub colour: u32,
    /// The composited HDR before depth of field (sampled), compared with the
    /// opaque copy.
    pub composited: u32,
    /// The display history slot TSR writes (storage).
    pub output: u32,
    /// Last frame's display history slot (sampled; the Ara path's colour history).
    pub history: Option<u32>,
    /// The host's reactive coverage (sampled, render size).
    pub reactive: Option<u32>,
    /// The opaque copy may be read (declared in this graph).
    pub opaque: bool,
    /// The SSR trace may be read (declared in this graph).
    pub reflections: bool,
}

/// Graph resources the TSR chain reads and writes besides its own.
pub(crate) struct Io {
    /// The HDR input (or the depth-of-field composite).
    pub colour: ResourceId,
    pub depth: ResourceId,
    pub motion: Option<ResourceId>,
    /// The display history slot TSR writes.
    pub output: ResourceId,
    /// Last frame's display history slot (read on the Ara path).
    pub history: Option<ResourceId>,
    /// The look's exposure state.
    pub exposure: ResourceId,
    /// Entity IDs.
    pub ids: ResourceId,
    /// Signal inputs prepare inputs reads, each once: the opaque copy, the
    /// composited HDR when it differs from `colour`, the SSR trace and the host's
    /// reactive coverage.
    pub signals: Vec<ResourceId>,
}

/// Graph identities of TSR's resources in one frame.
pub(crate) struct TsrIds {
    images: [Option<ResourceId>; IMAGES],
    scratch: ResourceId,
}

/// One view's TSR state: images, histories and the record. The renderer holds the
/// active view's (`Tsr::view`); secondary views swap theirs in while they render.
#[derive(Default)]
pub(crate) struct TsrView {
    resources: Option<Resources>,
    history: History,
    frame: Option<TsrFrame>,
}

impl TsrView {
    /// Retires a released view's storage after its last use.
    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        match self.resources {
            Some(resources) => retire(gpu, resources),
            None => Ok(()),
        }
    }
}

/// The renderer's TSR: the selection, tuning and pipelines shared by its views, and
/// the active view's state.
pub(crate) struct Tsr {
    /// The selected upscaler; TSR resources exist only while it is `Tsr`.
    pub upscaler: Upscaler,
    tuning: Tuning,
    pipelines: Option<Pipelines>,
    /// Counts upscaler switches: a view's history from an earlier selection (one
    /// that was not active when it changed) restarts.
    epoch: u32,
    /// The active view's state.
    pub view: TsrView,
}

impl Tsr {
    /// The upscaler from `ARA_UPSCALER` and the tuning from the `ARA_TSR_*` knobs.
    pub(crate) fn from_env() -> Self {
        Self {
            upscaler: Upscaler::from_env(),
            tuning: Tuning::from_env(),
            pipelines: None,
            epoch: 0,
            view: TsrView::default(),
        }
    }

    /// Sets FSR's RCAS sharpness (0 none to 1 most) from the next frame; above zero
    /// selects FSR's path (RCAS needs FSR's own history).
    pub(crate) fn set_sharpness(&mut self, sharpness: f32) {
        if sharpness.is_finite() {
            self.tuning.sharpness = sharpness.clamp(0., 1.);
        }
    }

    /// Whether this frame's chain reads the renderer's previous display history
    /// (the Ara path's colour history).
    pub(crate) fn reads_history(&self) -> bool {
        self.view
            .frame
            .as_ref()
            .is_some_and(|f| f.path == AccumulatePath::Ara)
    }

    /// Selects the upscaler; TSR's accumulation restarts when it is next used.
    pub(crate) fn select(&mut self, upscaler: Upscaler) {
        if upscaler != self.upscaler {
            self.upscaler = upscaler;
            self.epoch = self.epoch.wrapping_add(1);
            self.invalidate();
        }
    }

    /// Forgets TSR's histories: a frame of the primary view resolved without it.
    pub(crate) fn invalidate(&mut self) {
        self.view.history.valid = false;
        self.view.frame = None;
    }

    /// Releases the images while TAAU is selected (they hold bindless slots).
    pub(crate) fn release_unused(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        if self.upscaler == Upscaler::Tsr {
            return Ok(());
        }
        std::mem::take(&mut self.view).destroy(gpu)
    }

    /// Loads the cooked pipelines on first use (the renderer's cohort) and
    /// allocates the images for `render` and `display` and the tuning's path,
    /// replacing them when any of those changed.
    pub(crate) fn prepare(
        &mut self,
        gpu: &mut GpuDevice,
        artifacts: &Path,
        content_key: &str,
        render: [u32; 2],
        display: [u32; 2],
    ) -> Result<()> {
        if self.pipelines.is_none() {
            self.pipelines = Some(load_pipelines(gpu, artifacts, content_key)?);
        }
        let fsr = self.tuning.path() == AccumulatePath::Fsr;
        if self
            .view
            .resources
            .as_ref()
            .is_some_and(|r| r.render == render && r.display == display && r.fsr == fsr)
        {
            return Ok(());
        }
        if let Some(old) = self.view.resources.take() {
            retire(gpu, old)?;
        }
        self.invalidate();
        self.view.resources = Some(allocate(gpu, render, display, fsr)?);
        Ok(())
    }

    /// Plans this frame's chain: FSR's per-dispatch constants (jitter, sizes, frame
    /// index, phase count) from `inputs`, after `prepare` for the same sizes. The
    /// colour input and output slot are set by `upload`.
    pub(crate) fn begin(&mut self, inputs: &FrameInputs) -> Result<()> {
        let resources = self
            .view
            .resources
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("TSR resources are prepared before a frame"))?;
        ensure!(
            resources.render == inputs.render && resources.display == inputs.display,
            "TSR resources were prepared for another view size"
        );
        if self.view.history.epoch != self.epoch {
            self.view.history.valid = false;
        }
        let reset = inputs.reset || resources.fresh || !self.view.history.valid;
        // FSR keeps last frame's jitter and sizes across a reset; without a valid
        // history there are none, so this frame's stand in.
        let previous = if self.view.history.valid {
            self.view.history
        } else {
            History {
                render: inputs.render,
                display: inputs.display,
                ..self.view.history
            }
        };
        let jitter = [-inputs.upscale[0], -inputs.upscale[1]];
        let target = inputs.phases as f32;
        let phases = if reset || previous.phases == 0. {
            target
        } else {
            previous.phases + (target - previous.phases).signum()
        };
        let frame_index = if reset { 0. } else { previous.frame_index + 1. };
        let size = |s: [u32; 2]| [s[0] as i32, s[1] as i32];
        let [r, d, pr, pd] = [
            inputs.render,
            inputs.display,
            previous.render,
            previous.display,
        ]
        .map(size);
        let row = |values: &[u32; IMAGES], i: usize| -> [u32; 4] {
            std::array::from_fn(|k| values.get(i * 4 + k).copied().unwrap_or(0))
        };
        let t = &self.tuning;
        let shading_change = t.factors[2] > 0.;
        let mut options = 0;
        if t.luma_instability {
            options |= constants::OPTION_LUMA_INSTABILITY;
        }
        if shading_change {
            options |= constants::OPTION_SHADING_CHANGE;
        }
        if t.ids {
            options |= constants::OPTION_IDS;
        }
        let constants = TsrConstants {
            render_size: [r[0], r[1], pr[0], pr[1]],
            upscale_size: [d[0], d[1], pd[0], pd[1]],
            max_size: [r[0], r[1], d[0], d[1]],
            device_to_view_depth: constants::device_to_view_depth(inputs.projection),
            jitter: [jitter[0], jitter[1], previous.jitter[0], previous.jitter[1]],
            scale: [
                inputs.render[0] as f32 / inputs.display[0] as f32,
                inputs.render[1] as f32 / inputs.display[1] as f32,
                1. / inputs.projection.x_axis.x,
                phases,
            ],
            frame: [frame_index, t.factors[0], t.factors[1], t.factors[2]],
            accumulation: [
                t.accumulation[0],
                t.accumulation[1],
                constants::FRAME_SECONDS,
                1.,
            ],
            // The one pre-exposure switch: 0 multiplies the linear HDR input by the
            // look's applied exposure; 1 reads it as already pre-exposed (VQ's
            // contract). Output and history are pre-exposed by the applied exposure
            // either way, history decoded with last frame's.
            exposure: [f32::from(u8::from(t.pre_exposed)), inputs.exposure, 0., 0.],
            rcas: constants::rcas(t.sharpness),
            inputs: [
                0,
                inputs.depth,
                inputs.motion.unwrap_or(u32::MAX),
                inputs.display_frame,
            ],
            resources: [
                inputs.exposure_state,
                resources.scratch_index,
                0,
                self.view.history.slot,
            ],
            sampled: std::array::from_fn(|i| row(&resources.sampled, i)),
            storage: std::array::from_fn(|i| row(&resources.storage, i)),
            signal_inputs: [
                inputs.ids,
                inputs.opaque.map_or(u32::MAX, |(index, _)| index),
                u32::MAX,
                inputs.reflections.unwrap_or(u32::MAX),
            ],
            opaque_rect: inputs.opaque.map_or([0; 4], |(_, rect)| rect),
            signals: t.signals,
            autogen: t.autogen,
            external: [u32::MAX; 4],
            kernel: t.kernel,
            options: [options, 0, 0, 0],
        };
        let clear = if resources.fresh {
            vec![LUMA, ACCUMULATION, LOCKS]
        } else if reset {
            vec![ACCUMULATION]
        } else {
            Vec::new()
        };
        self.view.frame = Some(TsrFrame {
            constants,
            path: if resources.fsr {
                AccumulatePath::Fsr
            } else {
                AccumulatePath::Ara
            },
            reproject: inputs.motion.is_none(),
            shading_change,
            luma_instability: t.luma_instability,
            clear,
            next: History {
                valid: true,
                jitter,
                render: inputs.render,
                display: inputs.display,
                frame_index,
                phases,
                slot: 1 - self.view.history.slot,
                epoch: self.epoch,
            },
        });
        Ok(())
    }

    /// Whether a planned frame waits to be declared (`begin` ran).
    pub(crate) fn planned(&self) -> bool {
        self.view.frame.is_some()
    }

    /// Uploads the planned record with the indices its graph binds; call before
    /// that graph is submitted.
    pub(crate) fn upload(&mut self, gpu: &mut GpuDevice, bindings: &Bindings) -> Result<()> {
        let (Some(frame), Some(resources)) = (&mut self.view.frame, &self.view.resources) else {
            anyhow::bail!("TSR uploads a planned frame");
        };
        let c = &mut frame.constants;
        c.inputs[0] = bindings.colour;
        c.resources[2] = bindings.output;
        c.signal_inputs[2] = bindings.composited;
        if !bindings.opaque {
            c.signal_inputs[1] = u32::MAX;
        }
        if !bindings.reflections {
            c.signal_inputs[3] = u32::MAX;
        }
        c.external[0] = bindings.reactive.unwrap_or(u32::MAX);
        // Reflections count only with a weight.
        if c.signal_inputs[1] != u32::MAX
            || (c.signal_inputs[3] != u32::MAX && c.signals[2] > 0.)
            || c.external[0] != u32::MAX
        {
            c.options[0] |= constants::OPTION_MASKS;
        }
        c.options[1] = bindings.history.unwrap_or(0);
        gpu.upload_queued(resources.record, bytemuck::bytes_of(c))?;
        Ok(())
    }

    /// Declares the planned chain; `shared` joins every pass (the frame's
    /// conservative bindless reachability). Returns the passes in order.
    pub(crate) fn declare(
        &self,
        graph: &mut Graph,
        io: &Io,
        shared: &[(ResourceId, Access)],
    ) -> Result<(TsrIds, Vec<(PassId, TsrPass)>)> {
        let (Some(frame), Some(resources)) = (&self.view.frame, &self.view.resources) else {
            anyhow::bail!("TSR declares a planned frame");
        };
        let ids: [Option<ResourceId>; IMAGES] = std::array::from_fn(|i| {
            resources.images[i]
                .map(|image| graph.import(Resource::Image(image), resources.states[i]))
        });
        let image = |i: usize| ids[i].ok_or_else(|| anyhow::anyhow!("TSR image {i} is allocated"));
        let scratch = graph.import(Resource::Buffer(resources.scratch), resources.scratch_state);
        let constants = graph.import(
            Resource::Buffer(resources.record),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        );
        let (read, write) = (Access::ShaderRead, Access::Storage);
        let mut stages: Vec<Stage> = Vec::new();
        if !frame.clear.is_empty() {
            let mut uses = frame
                .clear
                .iter()
                .map(|&i| Ok((image(i)?, write)))
                .collect::<Result<Vec<_>>>()?;
            uses.push((scratch, write));
            stages.push(("TSR.Clear", TsrPass::Clear, uses));
        }
        let mut prepare = vec![
            (io.colour, read),
            (io.depth, read),
            (io.ids, read),
            (scratch, write),
            (image(LUMA)?, write),
            (image(PYRAMID)?, write),
        ];
        prepare.extend(io.motion.map(|motion| (motion, read)));
        prepare.extend(io.signals.iter().map(|&signal| (signal, read)));
        stages.push(("TSR.PrepareInputs", TsrPass::PrepareInputs, prepare));
        if frame.shading_change {
            stages.push((
                "TSR.ShadingChangePyramid",
                TsrPass::ShadingChangePyramid,
                vec![
                    (image(LUMA)?, read),
                    (scratch, read),
                    (image(PYRAMID)?, write),
                ],
            ));
        }
        stages.push((
            "TSR.PrepareReactivity",
            TsrPass::PrepareReactivity,
            vec![
                (scratch, read),
                (image(PYRAMID)?, read),
                (image(LUMA)?, read),
                (image(ACCUMULATION)?, write),
                (image(MASKS)?, write),
                (image(LOCKS)?, write),
            ],
        ));
        if frame.luma_instability {
            stages.push((
                "TSR.LumaInstability",
                TsrPass::LumaInstability,
                vec![
                    (scratch, read),
                    (image(MASKS)?, read),
                    (image(LUMA)?, read),
                    (image(PYRAMID)?, read),
                    (image(LUMA_HISTORY)?, write),
                    (image(INTERMEDIATE)?, write),
                ],
            ));
        }
        // Accumulate also clears the reconstructed depth (scratch).
        let mut accumulate = vec![
            (scratch, write),
            (image(PYRAMID)?, read),
            (image(MASKS)?, read),
            (io.colour, read),
            (image(LOCKS)?, write),
        ];
        if frame.luma_instability {
            accumulate.push((image(INTERMEDIATE)?, read));
        }
        match frame.path {
            AccumulatePath::Ara => {
                accumulate.push((io.output, write));
                accumulate.extend(io.history.map(|history| (history, read)));
                stages.push(("TSR.Accumulate", TsrPass::Accumulate, accumulate));
            }
            AccumulatePath::Fsr => {
                accumulate.push((image(UPSCALED)?, write));
                stages.push(("TSR.Accumulate", TsrPass::Accumulate, accumulate));
                stages.push((
                    "TSR.Rcas",
                    TsrPass::Rcas,
                    vec![
                        (image(UPSCALED)?, read),
                        (io.depth, read),
                        (io.output, write),
                    ],
                ));
            }
        }
        let passes = stages
            .into_iter()
            .map(|(name, pass, uses)| {
                let id = graph.pass(
                    name,
                    Queue::Graphics,
                    // Every pass loads the record and the exposure state (tsrBegin).
                    uses.into_iter()
                        .chain([(constants, read), (io.exposure, read)])
                        .chain(shared.iter().copied()),
                );
                (id, pass)
            })
            .collect();
        Ok((
            TsrIds {
                images: ids,
                scratch,
            },
            passes,
        ))
    }

    /// Records one declared dispatch.
    pub(crate) fn record(&self, recorder: &mut Recorder, pass: TsrPass) {
        let (Some(frame), Some(p), Some(resources)) =
            (&self.view.frame, &self.pipelines, &self.view.resources)
        else {
            return;
        };
        let groups =
            |size: [u32; 2], edge: u32| [size[0].div_ceil(edge), size[1].div_ceil(edge), 1];
        let (render, display) = (resources.render, resources.display);
        let (pipeline, dispatch) = match pass {
            TsrPass::Clear => {
                recorder.push(Command::Pipeline(p.clear));
                for &image in &frame.clear {
                    let [width, height] = image_desc(image, render, display).1;
                    let mut words = [0; 8];
                    (words[1], words[2], words[3]) = (resources.storage[image], width, height);
                    recorder.push(Command::Constants(RootConstants { words }));
                    recorder.push(Command::Dispatch(groups([width, height], 8)));
                }
                // The reconstructed previous depth (scratch region 1): 64 words a
                // group, rows of 1024 groups.
                let pixels = scratch_pixels(render);
                let rows = pixels.div_ceil(64).div_ceil(1024);
                let mut words = [0; 8];
                (words[1], words[2], words[3], words[5], words[6]) =
                    (resources.scratch_index, pixels, 1024, 1, pixels);
                recorder.push(Command::Constants(RootConstants { words }));
                recorder.push(Command::Dispatch([1024, rows, 1]));
                return;
            }
            TsrPass::PrepareInputs if frame.reproject => {
                (p.prepare_inputs_reproject, groups(render, 8))
            }
            TsrPass::PrepareInputs => (p.prepare_inputs, groups(render, 8)),
            TsrPass::ShadingChangePyramid => (p.shading_change_pyramid, groups(render, 64)),
            TsrPass::PrepareReactivity => (p.prepare_reactivity, groups(render, 8)),
            TsrPass::LumaInstability => (p.luma_instability, groups(render, 8)),
            TsrPass::Accumulate if frame.path == AccumulatePath::Fsr => {
                (p.accumulate_fsr, groups(display, 8))
            }
            TsrPass::Accumulate => (p.accumulate, groups(display, 8)),
            TsrPass::Rcas => (p.rcas, groups(display, 16)),
        };
        let mut words = [0; 8];
        words[0] = resources.record_index;
        recorder.push(Command::Pipeline(pipeline));
        recorder.push(Command::Constants(RootConstants { words }));
        recorder.push(Command::Dispatch(dispatch));
    }

    /// Records the final states of a compiled frame and, the chain recorded,
    /// advances the history (ping-pong slot, jitter, frame index).
    pub(crate) fn complete(&mut self, ids: &TsrIds, final_states: &[State]) {
        if let Some(resources) = &mut self.view.resources {
            for (state, id) in resources.states.iter_mut().zip(&ids.images) {
                if let Some(id) = id {
                    *state = final_states[id.0];
                }
            }
            resources.scratch_state = final_states[ids.scratch.0];
            resources.fresh = false;
        }
        if let Some(frame) = self.view.frame.take() {
            self.view.history = frame.next;
        }
    }
}

/// A declared pass: its name, dispatch and resource uses.
type Stage = (&'static str, TsrPass, Vec<(ResourceId, Access)>);

/// The cooked TSR pipelines, loaded with the renderer's content key (a mixed cohort
/// is rejected, as for every renderer pipeline).
fn load_pipelines(gpu: &mut GpuDevice, artifacts: &Path, content_key: &str) -> Result<Pipelines> {
    let mut load = |name: &str| -> Result<Pipeline> {
        let cooked: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
        ensure!(
            cooked.key == content_key,
            "TSR cook is incomplete; recook the pipeline manifest"
        );
        Ok(gpu.create_pipeline(&cooked, artifacts)?)
    };
    Ok(Pipelines {
        prepare_inputs: load("tsr_prepare_inputs")?,
        prepare_inputs_reproject: load("tsr_prepare_inputs_reproject")?,
        shading_change_pyramid: load("tsr_shading_change_pyramid")?,
        prepare_reactivity: load("tsr_prepare_reactivity")?,
        luma_instability: load("tsr_luma_instability")?,
        accumulate: load("tsr_accumulate")?,
        accumulate_fsr: load("tsr_accumulate_fsr")?,
        rcas: load("tsr_rcas")?,
        clear: load("tsr_clear")?,
    })
}

/// Allocates the images (the upscaled pair only for FSR's path), scratch buffer
/// and record for `render` and `display`.
fn allocate(
    gpu: &mut GpuDevice,
    render: [u32; 2],
    display: [u32; 2],
    fsr: bool,
) -> Result<Resources> {
    let mut images = [None; IMAGES];
    let mut sampled = [0; IMAGES];
    let mut storage = [0; IMAGES];
    for (i, slot) in images.iter_mut().enumerate() {
        if i == UNUSED || (i == UPSCALED && !fsr) {
            continue;
        }
        let (format, [width, height]) = image_desc(i, render, display);
        let image = gpu.create_image(
            width,
            height,
            format,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
        )?;
        *slot = Some(image);
        sampled[i] = gpu.sampled_image(image, false)?.index;
        storage[i] = gpu.storage_image(image)?.index;
    }
    let pixels = u64::from(scratch_pixels(render));
    let scratch = gpu.create_buffer(
        pixels * SCRATCH_WORDS * 4,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        false,
    )?;
    let scratch_index = gpu.storage_buffer(scratch)?.index;
    let record = gpu.create_buffer(
        std::mem::size_of::<TsrConstants>() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        true,
    )?;
    let record_index = gpu.storage_buffer(record)?.index;
    let undefined = State {
        access: Access::Undefined,
        queue: Queue::Graphics,
    };
    Ok(Resources {
        render,
        display,
        fsr,
        images,
        sampled,
        storage,
        states: [undefined; IMAGES],
        scratch,
        scratch_index,
        scratch_state: undefined,
        record,
        record_index,
        fresh: true,
    })
}

/// Retires replaced or released storage after its last use.
fn retire(gpu: &mut GpuDevice, resources: Resources) -> Result<()> {
    for image in resources.images.into_iter().flatten() {
        gpu.retire(Resource::Image(image))?;
    }
    gpu.retire(Resource::Buffer(resources.scratch))?;
    gpu.retire(Resource::Buffer(resources.record))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramid_atlas_holds_every_region() {
        for render in [[1707, 960], [1971, 1109], [2560, 1440], [1, 1], [3, 2]] {
            let [w, h] = pyramid_extent(render);
            let mip = |level: u32| half(render).map(|v| (v >> level).max(1));
            // Mip 0 over the farthest depth on the left; mips 1 and 2 on the right.
            assert!(mip(0)[1] * 2 <= h && mip(0)[0] + mip(1)[0] <= w);
            assert!(mip(1)[1] + mip(2)[1] <= h && mip(2)[0] <= mip(1)[0]);
        }
    }
}
