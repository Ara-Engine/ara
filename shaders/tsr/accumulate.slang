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

// Ara: TSR pass 5, from ffx_fsr3upscaler_accumulate_pass.hlsl: reprojects and
// rectifies the display-size history, upsamples this frame's samples and
// accumulates them, with locks for thin features. tsrAccumulateMain is the Ara path
// (TSR_ARA_HISTORY: the renderer's display history, Catmull-Rom reprojection,
// display-pixel Gaussian upsampling, no RCAS); accumulate-fsr.slang defines
// TSR_FSR_PATH for FSR's (its own history pair, Lanczos, RCAS after).
// Ara: it also clears the reconstructed previous depth after prepare reactivity,
// its only reader (render pixels under display threads), for the next frame's splat.
#ifdef TSR_FSR_PATH
#define TSR_ACCUMULATE tsrAccumulateFsrMain
#else
#define TSR_ARA_HISTORY 1
#define TSR_ACCUMULATE tsrAccumulateMain
#endif
#include "tsr.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_common.h"
#include "tsr-signals.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_sample.h"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_upsample.h"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_reproject.h"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_accumulate.h"
[shader("compute")]
[numthreads(8,8,1)]
void TSR_ACCUMULATE(int2 iDispatchThreadId:SV_DispatchThreadID){
    tsrBegin();
    Accumulate(iDispatchThreadId);
    SetReconstructedDepth(iDispatchThreadId,0);
}
