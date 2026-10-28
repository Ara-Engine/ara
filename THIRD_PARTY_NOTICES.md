# Third-party notices

## meshoptimizer 1.3 (meshopt 0.7.0 native sources)

`crates/ara-meshopt-sys/vendor` contains the unmodified native simplifier and
header shipped in meshopt 0.7.0, pinned by hashes in `vendor/REVISION.md`.
Ara's separate shim adds owned preparation/reset contexts. Upstream headers and
the complete MIT notice remain in the vendor directory.

Copyright (c) 2016-2026 Arseny Kapoulkine

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Vendored Lightyear 0.31.0

`crates/ara-net/vendor/lightyear_sync` and `lightyear_transport` contain complete
Lightyear 0.31.0 crate sources by Charles Bournhonesque and contributors, from
[upstream commit 8a926d6](https://github.com/cBournhonesque/lightyear/tree/8a926d6fece6a0cf04c2747de90c01258524fe0d).
The sources are licensed under MIT OR Apache-2.0. Both original license files
are included in each crate directory. The local clock changes and exact diff
are documented in [PATCHES.md](crates/ara-net/vendor/PATCHES.md).

## BC6H format tables (bcdec)

The BC6H layout, partition and anchor tables in
`crates/ara-texture-gpu/shaders/bc6h_tables.slangh` and
`src/bc6h_reference.rs` derive from
[bcdec revision 80859ed](https://github.com/iOrange/bcdec/tree/80859ed3b7afb1c527a2a99d70c61457bea72d0c),
via bcdec_rs 0.2.1. The encoder search/fitting implementation is original Ara code.
The table-bearing files retain the following MIT license:

Copyright (c) 2022 Sergii Kudlai

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## BC7 partition and anchor tables (bc7enc)

The normative BC7 tables in `crates/ara-texture-gpu/shaders/bc7_tables.slangh`
and `src/bc7_reference.rs` derive from Richard Geldreich Jr.'s
[bc7enc revision d6425dc](https://github.com/richgel999/bc7enc_rdo_devel/blob/d6425dc0a5557bab67e2d845e9e462ef39bc4c7b/bc7enc.cpp).
Ara uses the upstream public-domain alternative; original notices remain in both
files. No CPU BC7E encoder is vendored by this change.

This is free and unencumbered software released into the public domain.
Anyone is free to copy, modify, publish, use, compile, sell, or distribute this
software, either in source code form or as a compiled binary, for any purpose,
commercial or non-commercial, and by any means.
In jurisdictions that recognize copyright laws, the author or authors of this
software dedicate any and all copyright interest in the software to the public
domain. We make this dedication for the benefit of the public at large and to
the detriment of our heirs and successors. We intend this dedication to be an
overt act of relinquishment in perpetuity of all present and future rights to
this software under copyright law.
THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
For more information, please refer to <http://unlicense.org/>.

## Outpost v0 game assets

The sample's redistributable art and audio are pinned by URL, BLAKE3 and size in
`samples/assets.lock.json`; `ara-samples fetch` restores the original files and
licence notices. The generated inventory is `samples/THIRD_PARTY_ASSETS.md`.

- **Robot Expressive**, by Tomás Laulhé / Quaternius, CC0-1.0:
  <https://quaternius.com/packs/animatedrobot.html>. The pinned glTF conversion
  includes Don McCurdy's modifications and its original README from three.js
  revision `e06d724a29df5b44c6ad878207974eb09ed7e0d9`.
- **Space Kit**, **Sci-Fi Sounds**, **Interface Sounds**, **Impact Sounds**, and
  **UI Pack Space Expansion**, by Kenney, CC0-1.0: <https://kenney.nl/assets>.
  This includes the drone model, input/reticle icons, Kenney Future fonts and game
  sound effects. Each downloaded archive retains its original `License.txt`.
- Existing Poly Haven compound props, materials and industrial doors, and the
  OpenGameArt ambience/music remain attributed individually in the asset lock and
  `samples/THIRD_PARTY_ASSETS.md` under their recorded permissive licences.

## AMD FidelityFX SDK v1.1.4: FSR 3.1.4 temporal upscaler (Ara TSR)

`shaders/tsr/ffx/` contains GPU headers from the
[AMD FidelityFX SDK v1.1.4](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/tree/v1.1.4)
(`sdk/include/FidelityFX/gpu`): `ffx_common_types.h`, `ffx_core.h`,
`ffx_core_hlsl.h`, `ffx_core_gpu_common.h`, `ffx_core_gpu_common_half.h`,
`ffx_core_portability.h`, `spd/ffx_spd.h`, `fsr1/ffx_fsr1.h` (RCAS) and the FSR 3.1.4
upscaler's `fsr3upscaler/ffx_fsr3upscaler_*.h` pass headers. Ara's edits are marked
`// Ara:` (include paths, loop unrolling, a register-resident sample-set lookup).
`shaders/tsr/tsr.slangh` (the resource callbacks, derived from
`ffx_fsr3upscaler_callbacks_hlsl.h`) and the pass files in `shaders/tsr/` (derived
from `sdk/src/backends/dx12/shaders/fsr3upscaler/*_pass.hlsl`) carry the same notice;
`crates/ara-render/src/tsr/` follows the host constant setup of
`sdk/src/components/fsr3upscaler/ffx_fsr3upscaler.cpp`. Every copied or derived file
retains the following MIT license:

Copyright (C) 2024 Advanced Micro Devices, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files(the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and /or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions :

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
