using System;
using System.IO;
using System.Text;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Rendering;
using UnityEngine.Rendering.Universal;

public static class BenchDiagnostics {
    public static void Shadows() {
        var name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        EditorSceneManager.OpenScene("Assets/"+name+".unity");
        var text=new StringBuilder();
        foreach(var l in UnityEngine.Object.FindObjectsByType<Light>(FindObjectsSortMode.None)) {
            var b=l.bakingOutput;text.AppendLine(l.name+" type="+l.lightmapBakeType+" baked="+b.isBaked+" mixed="+b.mixedLightingMode+" mask="+b.occlusionMaskChannel+" shadows="+l.shadows);
        }
        foreach(var r in UnityEngine.Object.FindObjectsByType<MeshRenderer>(FindObjectsSortMode.None)) {
            text.AppendLine(r.name+" casting="+r.shadowCastingMode+" receive="+r.receiveShadows);
            foreach(var m in r.sharedMaterials)text.AppendLine("  "+m.name+" shader="+m.shader.name+" shadowpass="+m.FindPass("ShadowCaster"));
        }
        File.WriteAllText(@"D:\AraBench\logs\unity-shadow-audit.txt",text.ToString());
        if(Environment.GetEnvironmentVariable("ARABENCH_FIX_REALTIME")=="1") {
            foreach(var l in UnityEngine.Object.FindObjectsByType<Light>(FindObjectsSortMode.None)) {
                l.lightmapBakeType=LightmapBakeType.Realtime;
                l.bakingOutput=new LightBakingOutput{isBaked=false,lightmapBakeType=LightmapBakeType.Realtime,mixedLightingMode=MixedLightingMode.IndirectOnly,occlusionMaskChannel=-1,probeOcclusionLightIndex=-1};
            }
            EditorSceneManager.MarkAllScenesDirty();EditorSceneManager.SaveOpenScenes();AssetDatabase.SaveAssets();BenchBuild.Package();
        }
    }
}
