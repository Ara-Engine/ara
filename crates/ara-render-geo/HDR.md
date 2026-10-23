# Native HDR verification

`ara-color::OutputSettings` is reflected and defaults to SDR. Paper white is
203 nits by default (80–500), peak 1000 nits (at least paper white). Exposed linear
BT.709 enters `DisplayOutput`; its hue-preserving highlight shoulder runs before
encoding. A creative PBR Neutral/AgX operator can precede this pass. Fidelity is
unchanged this round; its SDR output must **not** be fed to this linear-input API.

HDR10 selects A2B10G10R10/A2R10G10B10 UNORM and HDR10_ST2084, transforms BT.709
to BT.2020 and applies absolute PQ. The intermediate is RGBA16F containing PQ
values; native blitting quantizes to the 10-bit swapchain. scRGB selects RGBA16F
and EXTENDED_SRGB_LINEAR; 1 = 80 nits, so paper-white 203 = 2.5375. It preserves
negative components and values above 1. Output mode mismatches fail explicitly.
No silent HDR-to-SDR fallback. Changing mode recreates the swapchain.

Headless GPU tests compare SDR/PQ/scRGB values with the CPU reference. Native
tests report the *queried* surface format and color space in `captures/hdr`.
An SDR PNG proxy cannot establish actual HDR brightness or display correctness.

Manual HDR display check (pending until a human actually performs it):

1. Enable Windows **Use HDR** on an HDR monitor; disable Auto HDR/night-light
   and other color transforms. Set the monitor's calibrated peak in settings.
2. With `CARGO_BUILD_JOBS=2`, `VULKAN_SDK=C:\VulkanSDK\1.4.363.0`, run
   `cargo run -p ara-render-geo --example hdr -- --mode=hdr10 --frames=1800`.
   Confirm the availability JSON says HDR10_ST2084 and a 10-bit UNORM format.
3. Repeat with `--mode=scrgb`. White left of the ramp's quarter mark represents
   paper white. The right side must remain brighter, with no SDR clipping plateau,
   hue jumps, banding beyond expected 10-bit quantization, or double transfer.
4. Compare 100/203/300-nit paper white. A meter should measure corresponding
   diffuse white, with highlights rolling toward the configured peak. Compare
   PQ and scRGB at the same settings. Record monitor, Windows HDR state, measured
   white/peak and observer result; do not call the manual gate passed without it.
5. Repeat default `--mode=sdr` with Windows HDR disabled. Verify resize/recreate
   behavior in the existing platform windowed gate. The ramp example is fixed-size.

References: Vulkan `VkColorSpaceKHR` and Microsoft Windows Advanced Color scRGB
(80 nits per unit). The chosen transfer is independent of camera exposure.
