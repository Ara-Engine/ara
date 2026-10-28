// This file is part of the FidelityFX SDK.
//
// Copyright (C) 2024 Advanced Micro Devices, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files(the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and /or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions :
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

// Ara: TSR pass 2, from ffx_fsr3upscaler_shading_change_pyramid_pass.hlsl: SPD over
// the signed luma change against last frame. The shading change pass reads mips 0
// to 2 only, so SPD stops there (identical values; no cross-workgroup tail).
#define TSR_SPD_MIPS 3
#include "tsr.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_common.h"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_shading_change_pyramid.h"
[shader("compute")]
[numthreads(256,1,1)]
void tsrShadingChangePyramidMain(uint3 WorkGroupId:SV_GroupID,uint LocalThreadIndex:SV_GroupIndex){tsrBegin();ComputeShadingChangePyramid(WorkGroupId,LocalThreadIndex);}
