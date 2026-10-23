use ara_color::OutputMode;
use ara_gpu::vk;
pub(crate) fn select_format(
    formats: &[vk::SurfaceFormatKHR],
    mode: OutputMode,
) -> Option<vk::SurfaceFormatKHR> {
    let space = match mode {
        OutputMode::Sdr => vk::ColorSpaceKHR::SRGB_NONLINEAR,
        OutputMode::Hdr10 => vk::ColorSpaceKHR::HDR10_ST2084_EXT,
        OutputMode::ScRgb => vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
    };
    let preferred: &[vk::Format] = match mode {
        OutputMode::Sdr => &[vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM],
        OutputMode::Hdr10 => &[
            vk::Format::A2B10G10R10_UNORM_PACK32,
            vk::Format::A2R10G10B10_UNORM_PACK32,
        ],
        OutputMode::ScRgb => &[vk::Format::R16G16B16A16_SFLOAT],
    };
    preferred.iter().find_map(|format| {
        formats
            .iter()
            .find(|f| {
                f.color_space == space && (f.format == *format || f.format == vk::Format::UNDEFINED)
            })
            .map(|_| vk::SurfaceFormatKHR {
                format: *format,
                color_space: space,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hdr_selection_requires_matching_transfer_and_precision() {
        let formats = [
            vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_UNORM,
                color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            },
            vk::SurfaceFormatKHR {
                format: vk::Format::A2B10G10R10_UNORM_PACK32,
                color_space: vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            },
            vk::SurfaceFormatKHR {
                format: vk::Format::R16G16B16A16_SFLOAT,
                color_space: vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
            },
        ];
        assert_eq!(select_format(&formats, OutputMode::Sdr), Some(formats[0]));
        assert_eq!(select_format(&formats, OutputMode::Hdr10), Some(formats[1]));
        assert_eq!(select_format(&formats, OutputMode::ScRgb), Some(formats[2]));
        assert!(select_format(&formats[..1], OutputMode::Hdr10).is_none());
    }
}
