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

// Ara: TSR pass 1, from ffx_fsr3upscaler_prepare_inputs_pass.hlsl. Dilates depth,
// motion and entity IDs (render size), splats the reconstructed previous nearest
// depth and writes this frame's luma. Ara folds in more:
// - FSR's luma pyramid, whose only consumed output is the half-size farthest depth:
//   each 8x8 group averages its 2x2 blocks through group memory (SPD's mip 0);
// - FSR's prepared colour (YCoCg, pre-exposed) for accumulate's nine taps;
// - Ara's reactive and composition masks (tsr-signals.slangh).
// tsrPrepareInputsMain reads the native motion target; prepare-inputs-reproject.slang
// defines TSR_REPROJECT for frames without one (motion from depth and last frame's
// view-projection).
#ifdef TSR_REPROJECT
#define TSR_PREPARE_INPUTS tsrPrepareInputsReprojectMain
#else
#define TSR_PREPARE_INPUTS tsrPrepareInputsMain
#endif
#include "tsr.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_common.h"
#include "tsr-signals.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_prepare_inputs.h"
[shader("compute")]
[numthreads(8,8,1)]
void TSR_PREPARE_INPUTS(int2 iDispatchThreadId:SV_DispatchThreadID){
    tsrBegin();
    PrepareInputs(iDispatchThreadId);
    tsrStorePrepared(iDispatchThreadId);
    tsrPrepareMasks(iDispatchThreadId);
    GroupMemoryBarrierWithGroupSync();
    int2 local=iDispatchThreadId&7;
    if(all((local&1)==0)){
        float farthest=(gsTsrFarthest[local.y][local.x]+gsTsrFarthest[local.y][local.x+1]
            +gsTsrFarthest[local.y+1][local.x]+gsTsrFarthest[local.y+1][local.x+1])*0.25;
        StoreFarthestDepthMip1(FfxUInt32x2(iDispatchThreadId/2),farthest);
    }
}
