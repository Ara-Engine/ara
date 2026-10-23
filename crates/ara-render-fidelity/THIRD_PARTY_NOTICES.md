# AMD FidelityFX Super Resolution 1

`shaders/fsr/ffx_a.h` and `ffx_fsr1.h` are unmodified upstream headers from
<https://github.com/GPUOpen-Effects/FidelityFX-FSR>, commit
`a21ffb8f6c13233ba336352bdff293894c706575` (master when fetched).
Copyright (c) 2021 Advanced Micro Devices, Inc. All rights reserved.
The complete MIT license is retained in `shaders/fsr/LICENSE.txt` and in headers.

SHA-256:
- `ffx_a.h`: `f60e2722fcd13989523b9164d776ab382b3692791767f3bf8bb19967f763f3fb`
- `ffx_fsr1.h`: `93c3922362ea7fc99cbcc698ca30c98de4f8c246d1fbb0b09e015ddef38ce3a5`

`upscale.slang` supplies engine callbacks and float-only RCAS constants. The
unused packed-half constant is zero, so no Float16/Int16 device features are
required by this float-only shader path. No NRD, RTXGI or Streamline code is copied.
