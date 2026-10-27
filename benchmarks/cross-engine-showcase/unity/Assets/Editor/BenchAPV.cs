using System;
using System.IO;
using System.Reflection;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Rendering;
using UnityEngine.Rendering.Universal;

public static class BenchAPV
{
    public static void Bake() {
        string name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        var scene=EditorSceneManager.OpenScene("Assets/"+name+".unity");
        BenchBuild.FixExposure();BenchBuild.FixSky();
        foreach(string path in new[]{"Assets/Settings/High.asset","Assets/Settings/Ultra.asset"}) {
            var asset=AssetDatabase.LoadAssetAtPath<UniversalRenderPipelineAsset>(path);var s=new SerializedObject(asset);s.FindProperty("m_LightProbeSystem").intValue=1;s.ApplyModifiedPropertiesWithoutUndo();
        }
        foreach(var renderer in UnityEngine.Object.FindObjectsByType<MeshRenderer>(FindObjectsSortMode.None)) {GameObjectUtility.SetStaticEditorFlags(renderer.gameObject,StaticEditorFlags.ContributeGI);renderer.receiveGI=ReceiveGI.LightProbes;}
        foreach(var light in UnityEngine.Object.FindObjectsByType<Light>(FindObjectsSortMode.None))light.lightmapBakeType=LightmapBakeType.Mixed;
        var oldVolume=GameObject.Find("Adaptive Probe Volume");if(oldVolume!=null)UnityEngine.Object.DestroyImmediate(oldVolume);
        var volume=new GameObject("Adaptive Probe Volume").AddComponent<ProbeVolume>();volume.mode=ProbeVolume.Mode.Local;volume.size=new Vector3(22,10,22);volume.transform.position=new Vector3(0,3,0);
        AssetDatabase.DeleteAsset("Assets/Settings/ProbeBakingSet.asset");
        var set=ScriptableObject.CreateInstance<ProbeVolumeBakingSet>();
        typeof(ProbeVolumeBakingSet).GetMethod("SetDefaults",BindingFlags.NonPublic|BindingFlags.Instance).Invoke(set,null);
        AssetDatabase.CreateAsset(set,"Assets/Settings/ProbeBakingSet.asset");
        string guid=AssetDatabase.AssetPathToGUID(scene.path);
        typeof(ProbeVolumeBakingSet).GetMethod("AddScene",BindingFlags.NonPublic|BindingFlags.Instance).Invoke(set,new object[]{guid,null});set.SetSceneBaking(guid,true);
        ProbeReferenceVolume.instance.SetActiveBakingSet(set);
        var lighting=new LightingSettings();lighting.bakedGI=true;lighting.realtimeGI=false;lighting.mixedBakeMode=MixedLightingMode.IndirectOnly;lighting.lightmapper=LightingSettings.Lightmapper.ProgressiveCPU;lighting.lightmapResolution=4;lighting.indirectSampleCount=128;lighting.lightmapMaxSize=512;Lightmapping.lightingSettings=lighting;
        EditorSceneManager.SaveScene(scene);AssetDatabase.SaveAssets();
        bool result=Lightmapping.Bake();
        File.WriteAllText(@"D:\AraBench\logs\unity-apv-"+name+".txt","Lightmapping.Bake="+result+"\nBakingSet="+AssetDatabase.GetAssetPath(set));
        EditorSceneManager.SaveScene(scene);AssetDatabase.SaveAssets();
        if(!result)throw new Exception("Command-line APV bake failed");
    }
}
