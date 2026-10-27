Shader "AraBench/PhysicalSky" {
    Properties { _Exposure ("Linear physical exposure", Float) = 0.001627604 }
    SubShader {
        Tags { "Queue"="Background" "RenderType"="Background" "PreviewType"="Skybox" }
        Cull Off ZWrite Off
        Pass {
            HLSLPROGRAM
            #pragma vertex vert
            #pragma fragment frag
            #include "UnityCG.cginc"
            float _Exposure;
            struct Input { float4 vertex:POSITION; };
            struct Output { float4 position:SV_POSITION; float3 direction:TEXCOORD0; };
            Output vert(Input v) { Output o;o.position=UnityObjectToClipPos(v.vertex);o.direction=v.vertex.xyz;return o; }
            float4 frag(Output i):SV_Target {
                float y=normalize(i.direction).y;
                float3 horizon=float3(.6,.55,.5);
                float3 c=y>=0?lerp(horizon,float3(.5,.48,.45),pow(y,.6)):lerp(horizon,float3(.25,.22,.2),min(-y*4,1));
                return float4(c*900*_Exposure,1);
            }
            ENDHLSL
        }
    }
}
