//! Repeat identical publications to distinguish capture state from transport error.
use super::{
    PatchTransport, Renderer, Result, TransportOptions, gltf_scene, max_white, request,
    worst_region, write_json,
};
use crate::fixtures;
use std::path::Path;

pub(super) struct CaptureRepeat {
    pub valid: bool,
    pub max_white: f32,
    pub region_flip: f64,
}

impl CaptureRepeat {
    pub fn include(&mut self, other: Self) {
        self.valid &= other.valid;
        self.max_white = self.max_white.max(other.max_white);
        self.region_flip = self.region_flip.max(other.region_flip);
    }
}

/// The second identical capture is a control only. A changing control makes the
/// frame's image comparison unsigned; it never replaces the first scored image.
pub(super) fn repeated_capture(
    renderer: &mut Renderer,
    request: &ara_render::capture::CaptureRequest,
) -> Result<(ara_render::capture::Capture, Vec<[f32; 4]>, CaptureRepeat)> {
    let capture = renderer.capture(request)?;
    let (_, hdr) = renderer.lab_radiance()?;
    let repeat = renderer.capture(request)?;
    let (_, repeated_hdr) = renderer.lab_radiance()?;
    let max_white = max_white(&hdr, &repeated_hdr, request.exposure)?;
    let identical = capture.image.as_raw() == repeat.image.as_raw();
    let region_flip = if identical {
        0.
    } else {
        worst_region(
            &ara_pathtrace::flip::flip(
                &capture.image,
                &repeat.image,
                ara_pathtrace::flip::PIXELS_PER_DEGREE,
            )?
            .errors,
        )
    };
    Ok((
        capture,
        hdr,
        CaptureRepeat {
            valid: identical && max_white == 0.,
            max_white,
            region_flip,
        },
    ))
}

/// Reproduce a sleeping publication after an awake scene, without advancing any
/// transport state between the identical-input captures being compared.
pub fn capture_order(out: &Path) -> Result<()> {
    let case = fixtures::cases()
        .into_iter()
        .find(|case| case.name == "sleep-wake")
        .expect("sleep-wake fixture exists");
    let mut fixture = fixtures::create(&case)?;
    fixtures::advance(&mut fixture, &case, 27);
    let active = PatchTransport::new(
        &fixture.scene,
        &fixture.textures,
        fixture
            .bodies
            .iter()
            .map(|(&id, &body)| (id, body))
            .collect(),
        TransportOptions::default(),
        Some(&out.join("charts")),
    )?;
    let mut renderer = Renderer::new_raster(
        gltf_scene::world(&fixture),
        super::SIZE,
        &[],
        out.join("gpu"),
    )?;
    renderer.set_patch_transport(Some(&active), &fixture.scene)?;
    renderer
        .capture(&request(&fixture))?
        .image
        .save(out.join("00-awake.png"))?;

    fixtures::advance(&mut fixture, &case, 28);
    let sleeping = PatchTransport::new(
        &fixture.scene,
        &fixture.textures,
        fixture
            .bodies
            .iter()
            .map(|(&id, &body)| (id, body))
            .collect(),
        TransportOptions::default(),
        Some(&out.join("charts")),
    )?;
    let exact = sleeping.exact_patch_solution()?;
    renderer.replace_scene(gltf_scene::world(&fixture))?;
    renderer.set_patch_transport(Some(&sleeping), &fixture.scene)?;
    let mut previous: Option<(&str, image::RgbaImage, Vec<[f32; 4]>)> = None;
    let mut comparisons = Vec::new();
    for (index, label) in [
        "candidate",
        "same-candidate",
        "fp64-reference",
        "same-reference",
        "candidate-restored",
    ]
    .into_iter()
    .enumerate()
    {
        if index == 2 {
            renderer.set_patch_transport_reference(&sleeping, &fixture.scene, &exact)?;
        } else if index == 4 {
            renderer.set_patch_transport(Some(&sleeping), &fixture.scene)?;
        }
        let capture = renderer.capture(&request(&fixture))?;
        let (_, hdr) = renderer.lab_radiance()?;
        capture
            .image
            .save(out.join(format!("{:02}-{label}.png", index + 1)))?;
        write_json(
            &out.join(format!("{:02}-{label}.json", index + 1)),
            &capture.metadata,
        )?;
        if let Some((old_label, old_image, old_hdr)) = &previous {
            let flip = ara_pathtrace::flip::flip(
                old_image,
                &capture.image,
                ara_pathtrace::flip::PIXELS_PER_DEGREE,
            )?;
            comparisons.push(serde_json::json!({"from":old_label,"to":label,
                "max_white":max_white(old_hdr, &hdr, fixture.exposure)?,
                "worst_region_flip":worst_region(&flip.errors)}));
        }
        previous = Some((label, capture.image, hdr));
    }
    write_json(&out.join("comparisons.json"), &comparisons)?;
    println!("{}", serde_json::to_string_pretty(&comparisons)?);
    Ok(())
}
