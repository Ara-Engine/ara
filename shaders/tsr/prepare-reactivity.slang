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

// Ara: TSR pass 3, from ffx_fsr3upscaler_prepare_reactivity_pass.hlsl (with FSR's
// shading change pass evaluated inline, tsr-signals.slangh SampleShadingChange):
// disocclusion, shading change, motion divergence and the accumulation counter
// (render size), and new locks at the display pixels thin features land on.
#include "tsr.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_common.h"
#include "tsr-signals.slangh"
#include "ffx/fsr3upscaler/ffx_fsr3upscaler_prepare_reactivity.h"
[shader("compute")]
[numthreads(8,8,1)]
void tsrPrepareReactivityMain(int2 iDispatchThreadId:SV_DispatchThreadID){tsrBegin();PrepareReactivity(iDispatchThreadId);}
