# ara-color
Display colour contracts shared by native presentation and render extraction.
OutputSettings defaults to SDR; HDR modes are explicit and must match the actual
swapchain. Linear inputs use BT.709 primaries with 1 = paper white. scRGB uses
80 nits per unit; HDR10 uses BT.2020 and absolute ST2084 PQ. Never gamma-encode
scRGB, or apply an SDR transfer after PQ. Tone mapping precedes output encoding.
Reference math is allocation-free; GPU implementations must match the tests.
Test/check/clippy/fmt with cargo -p ara-color. CARGO_BUILD_JOBS=2.
