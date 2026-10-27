using System;
using System.IO;
using UnityEditor;
using UnityEditor.Build.Reporting;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Rendering;
using UnityEngine.Rendering.Universal;

public static class BenchBuild
{
    static void Set(UnityEngine.Object o,string field,int value) { var s=new SerializedObject(o); var p=s.FindProperty(field); if(p==null)throw new Exception("Missing field: "+field);p.intValue=value;s.ApplyModifiedPropertiesWithoutUndo(); }
    static void SetFloat(UnityEngine.Object o,string field,float value) {var s=new SerializedObject(o);s.FindProperty(field).floatValue=value;s.ApplyModifiedPropertiesWithoutUndo();}
    static void SetBool(UnityEngine.Object o,string field,bool value) {var s=new SerializedObject(o);s.FindProperty(field).boolValue=value;s.ApplyModifiedPropertiesWithoutUndo();}
    public static void Build()
    {
        string name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        if(AssetDatabase.IsValidFolder("Assets/Settings"))AssetDatabase.DeleteAsset("Assets/Settings");
        Directory.CreateDirectory("Assets/Settings");
        var data=ScriptableObject.CreateInstance<UniversalRendererData>();
        data.postProcessData=AssetDatabase.LoadAssetAtPath<PostProcessData>("Packages/com.unity.render-pipelines.universal/Runtime/Data/PostProcessData.asset");
        Set(data,"m_RenderingMode",2); // Forward+; local lights are not capped per object.
        AssetDatabase.CreateAsset(data,"Assets/Settings/Renderer.asset");
        var aoType=typeof(UniversalRenderPipeline).Assembly.GetType("UnityEngine.Rendering.Universal.ScreenSpaceAmbientOcclusion",true);
        var ao=(ScriptableRendererFeature)ScriptableObject.CreateInstance(aoType);
        ao.name="SSAO";AssetDatabase.AddObjectToAsset(ao,data);data.rendererFeatures.Add(ao);
        SetFloat(ao,"m_Settings.Intensity",1);SetFloat(ao,"m_Settings.Radius",0.3f);
        var high=UniversalRenderPipelineAsset.Create(data);
        high.supportsHDR=true;high.msaaSampleCount=1;high.renderScale=2f/3;
        SetBool(high,"m_MainLightShadowsSupported",true);high.mainLightShadowmapResolution=2048;high.shadowCascadeCount=4;high.shadowDistance=100;
        SetBool(high,"m_AdditionalLightShadowsSupported",true);high.additionalLightsShadowmapResolution=2048;SetBool(high,"m_SoftShadowsSupported",true);
        high.supportsCameraDepthTexture=true;high.supportsCameraOpaqueTexture=true;
        AssetDatabase.CreateAsset(high,"Assets/Settings/High.asset");
        var epic=UnityEngine.Object.Instantiate(high);epic.renderScale=.77f;epic.mainLightShadowmapResolution=4096;epic.additionalLightsShadowmapResolution=4096;epic.shadowDistance=150;
        AssetDatabase.CreateAsset(epic,"Assets/Settings/Ultra.asset");
        GraphicsSettings.defaultRenderPipeline=high;
        typeof(UniversalRenderPipeline).Assembly.GetType("UnityEngine.Rendering.Universal.UniversalRenderPipelineGlobalSettings",true).GetMethod("Ensure",System.Reflection.BindingFlags.NonPublic|System.Reflection.BindingFlags.Static).Invoke(null,new object[]{true});
        // Explicit two quality entries; the player chooses High or Ultra from its CLI.
        var quality=new SerializedObject(AssetDatabase.LoadAllAssetsAtPath("ProjectSettings/QualitySettings.asset")[0]);
        var qs=quality.FindProperty("m_QualitySettings");qs.arraySize=2;quality.ApplyModifiedPropertiesWithoutUndo();
        QualitySettings.SetQualityLevel(0);QualitySettings.renderPipeline=high;QualitySettings.vSyncCount=0;
        QualitySettings.SetQualityLevel(1);QualitySettings.renderPipeline=epic;QualitySettings.vSyncCount=0;QualitySettings.SetQualityLevel(0);
        var scene=EditorSceneManager.NewScene(NewSceneSetup.EmptyScene,NewSceneMode.Single);
        AssetDatabase.ImportAsset("Assets/"+name+".glb",ImportAssetOptions.ForceUpdate|ImportAssetOptions.ForceSynchronousImport);
        var model=AssetDatabase.LoadAssetAtPath<GameObject>("Assets/"+name+".glb");
        if(model==null)throw new Exception("glTFast did not import source GLB");
        var geometry=(GameObject)PrefabUtility.InstantiatePrefab(model);geometry.name="Shared Blender geometry";
        // glTFast reflects X; this adapter's common Unity convention reflects Z.
        geometry.transform.rotation=Quaternion.Euler(0,180,0);
        var m=JsonUtility.FromJson<BenchManifest>(File.ReadAllText("Assets/Resources/"+name+".json"));
        float exposure=1f/(1.2f*Mathf.Pow(2,m.ev100));
        foreach(var spec in m.lights) {
            var go=new GameObject(spec.name);var l=go.AddComponent<Light>();
            l.type=spec.kind=="directional"?LightType.Directional:spec.kind=="spot"?LightType.Spot:LightType.Point;
            go.transform.position=BenchRun.Position(spec.position);
            // Reflect the RH glTF rotation into Unity's left-handed coordinate system.
            var q=Quaternion.AngleAxis(spec.rotation[0],Vector3.right)*Quaternion.AngleAxis(spec.rotation[1],Vector3.up)*Quaternion.AngleAxis(spec.rotation[2],Vector3.forward);
            var f=q*Vector3.back;var u=q*Vector3.up;
            go.transform.rotation=Quaternion.LookRotation(new Vector3(f.x,f.y,-f.z),new Vector3(u.x,u.y,-u.z));
            l.color=new Color(spec.color[0],spec.color[1],spec.color[2]);
            // URP uses relative intensity. Pre-expose photometric lighting at the shared EV.
            l.intensity=spec.intensity*exposure/Mathf.PI;
            if(l.type!=LightType.Directional)l.intensity/=4*Mathf.PI;
            l.range=spec.range;l.spotAngle=spec.outerAngle*2;l.innerSpotAngle=spec.innerAngle*2;l.shadows=LightShadows.Soft;
            if(l.type==LightType.Directional)RenderSettings.sun=l;
        }
        var envPath="Assets/environment.hdr";
        var envImporter=(TextureImporter)AssetImporter.GetAtPath(envPath);
        envImporter.textureShape=TextureImporterShape.TextureCube;envImporter.generateCubemap=TextureImporterGenerateCubemap.Cylindrical;envImporter.sRGBTexture=false;envImporter.SaveAndReimport();
        var env=AssetDatabase.LoadAssetAtPath<Cubemap>(envPath);
        var skyMaterial=new Material(Shader.Find("Skybox/Cubemap"));skyMaterial.SetTexture("_Tex",env);skyMaterial.SetFloat("_Exposure",exposure);AssetDatabase.CreateAsset(skyMaterial,"Assets/Settings/Sky.asset");RenderSettings.skybox=skyMaterial;
        RenderSettings.ambientMode=AmbientMode.Custom;
        var sh=new SphericalHarmonicsL2();const int count=2048;
        for(int i=0;i<count;i++) {float y=1-2*(i+.5f)/count;float a=i*2.39996323f;float r=Mathf.Sqrt(1-y*y);var direction=new Vector3(r*Mathf.Cos(a),y,r*Mathf.Sin(a));var horizon=new Color(.6f,.55f,.5f);var end=y>=0?new Color(.5f,.48f,.45f):new Color(.25f,.22f,.2f);float mix=y>=0?Mathf.Pow(y,.6f):Mathf.Min(-y*4,1);sh.AddDirectionalLight(direction,Color.Lerp(horizon,end,mix)*900*exposure,4*Mathf.PI/count);}
        RenderSettings.ambientProbe=sh;
        float ambient=900*exposure;
        RenderSettings.ambientSkyColor=new Color(.5f,.48f,.45f)*ambient;
        RenderSettings.ambientEquatorColor=new Color(.6f,.55f,.5f)*ambient;
        RenderSettings.ambientGroundColor=new Color(.25f,.22f,.2f)*ambient;
        RenderSettings.defaultReflectionMode=DefaultReflectionMode.Custom;RenderSettings.customReflectionTexture=env;RenderSettings.reflectionIntensity=exposure;RenderSettings.fog=false;
        var camera=new GameObject("Benchmark Camera").AddComponent<Camera>();camera.tag="MainCamera";camera.nearClipPlane=.05f;camera.farClipPlane=2000;
        camera.clearFlags=CameraClearFlags.Skybox;camera.backgroundColor=new Color(.6f,.55f,.5f)*ambient;
        var cd=camera.GetUniversalAdditionalCameraData();cd.renderPostProcessing=true;cd.antialiasing=AntialiasingMode.TemporalAntiAliasing;cd.antialiasingQuality=AntialiasingQuality.High;
        var vol=new GameObject("Fixed EV and filmic").AddComponent<Volume>();vol.isGlobal=true;
        var profile=ScriptableObject.CreateInstance<VolumeProfile>();AssetDatabase.CreateAsset(profile,"Assets/Settings/Post.asset");vol.sharedProfile=profile;
        var tone=profile.Add<Tonemapping>(true);tone.mode.value=TonemappingMode.ACES;
        var bloom=profile.Add<Bloom>(true);bloom.intensity.value=.04f;bloom.threshold.value=1;
        var color=profile.Add<ColorAdjustments>(true);color.postExposure.value=0;
        var run=new GameObject("Benchmark").AddComponent<BenchRun>();run.sceneName=name;run.view=camera;
        BenchRun.SetView(camera,m.path[0]);
        EditorSceneManager.SaveScene(scene,"Assets/"+name+".unity");
        PlayerSettings.colorSpace=ColorSpace.Linear;PlayerSettings.enableFrameTimingStats=true;
        PlayerSettings.SetScriptingBackend(UnityEditor.Build.NamedBuildTarget.Standalone,ScriptingImplementation.Mono2x);
        PlayerSettings.SetGraphicsAPIs(BuildTarget.StandaloneWindows64,new[]{UnityEngine.Rendering.GraphicsDeviceType.Direct3D12});
        PlayerSettings.fullScreenMode=FullScreenMode.FullScreenWindow;PlayerSettings.defaultScreenWidth=2560;PlayerSettings.defaultScreenHeight=1440;PlayerSettings.runInBackground=true;
        SavePost();EditorUtility.SetDirty(data);EditorUtility.SetDirty(high);EditorUtility.SetDirty(epic);AssetDatabase.SaveAssets();
        if(Environment.GetEnvironmentVariable("ARABENCH_PREPARE_ONLY")!="1")Package();
    }
    public static void Package() {
        FixQuality();
        string name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        EditorSceneManager.OpenScene("Assets/"+name+".unity");
        FixExposure();FixSky();SavePost();EditorSceneManager.SaveOpenScenes();AssetDatabase.SaveAssets();
        string destination=Environment.GetEnvironmentVariable("ARABENCH_BUILD_OUTPUT")??(@"D:\AraBench\builds\unity\"+name+@"\AraBench.exe");
        var result=BuildPipeline.BuildPlayer(new BuildPlayerOptions{scenes=new[]{"Assets/"+name+".unity"},locationPathName=destination,target=BuildTarget.StandaloneWindows64,options=BuildOptions.None});
        if(result.summary.result!=BuildResult.Succeeded)throw new Exception("Build failed: "+result.summary.result);
        File.WriteAllText(@"D:\AraBench\logs\unity-build-summary.txt",result.summary.totalTime+"\nbytes="+result.summary.totalSize);
    }
    static void SavePost() {
        var profile=AssetDatabase.LoadAssetAtPath<VolumeProfile>("Assets/Settings/Post.asset");
        if(profile.components.Count==0) {var t=profile.Add<Tonemapping>(true);t.mode.value=TonemappingMode.ACES;var b=profile.Add<Bloom>(true);b.intensity.value=.04f;b.threshold.value=1;var c=profile.Add<ColorAdjustments>(true);c.postExposure.value=0;}
        foreach(var component in profile.components) {if(!AssetDatabase.Contains(component))AssetDatabase.AddObjectToAsset(component,profile);EditorUtility.SetDirty(component);}
        EditorUtility.SetDirty(profile);
        var renderer=AssetDatabase.LoadAssetAtPath<UniversalRendererData>("Assets/Settings/Renderer.asset");
        renderer.postProcessData=AssetDatabase.LoadAssetAtPath<PostProcessData>("Packages/com.unity.render-pipelines.universal/Runtime/Data/PostProcessData.asset");
        if(renderer.postProcessData==null)throw new Exception("URP post-process resources were not loaded");EditorUtility.SetDirty(renderer);
    }
    public static void FixExposure() {
        FixQuality();
        string name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        var m=JsonUtility.FromJson<BenchManifest>(File.ReadAllText("Assets/Resources/"+name+".json"));
        GraphicsSettings.lightsUseLinearIntensity=true;
        foreach(var spec in m.lights) {
            var go=GameObject.Find(spec.name);var light=go.GetComponent<Light>();
            light.color=new Color(spec.color[0],spec.color[1],spec.color[2]).gamma;
            light.intensity=spec.intensity/Mathf.PI;if(light.type!=LightType.Directional)light.intensity/=4*Mathf.PI;
        }
        SavePost();var profile=AssetDatabase.LoadAssetAtPath<VolumeProfile>("Assets/Settings/Post.asset");
        profile.TryGet<ColorAdjustments>(out var color);color.postExposure.value=-m.ev100-Mathf.Log(1.2f,2);color.postExposure.overrideState=true;EditorUtility.SetDirty(color);EditorUtility.SetDirty(profile);
        EditorSceneManager.MarkAllScenesDirty();
    }
    public static void FixQuality() {
        // URP also respects the engine-wide switches inherited from the empty project.
        // Its first two default levels disable shadows and one reduces texture resolution.
        for(int i=0;i<2;i++) {
            QualitySettings.SetQualityLevel(i,true);QualitySettings.shadows=UnityEngine.ShadowQuality.All;
            QualitySettings.shadowResolution=i==0?UnityEngine.ShadowResolution.High:UnityEngine.ShadowResolution.VeryHigh;
            QualitySettings.shadowDistance=i==0?100:150;QualitySettings.globalTextureMipmapLimit=0;
            QualitySettings.anisotropicFiltering=AnisotropicFiltering.ForceEnable;QualitySettings.lodBias=1;
            QualitySettings.maximumLODLevel=0;QualitySettings.realtimeReflectionProbes=true;QualitySettings.pixelLightCount=8;QualitySettings.vSyncCount=0;
        }
        QualitySettings.SetQualityLevel(0,true);
        var settings=AssetDatabase.LoadAllAssetsAtPath("ProjectSettings/QualitySettings.asset")[0];
        var serialized=new SerializedObject(settings);var levels=serialized.FindProperty("m_QualitySettings");
        levels.GetArrayElementAtIndex(0).FindPropertyRelative("name").stringValue="High";
        levels.GetArrayElementAtIndex(1).FindPropertyRelative("name").stringValue="Ultra";
        serialized.ApplyModifiedPropertiesWithoutUndo();EditorUtility.SetDirty(settings);
    }
    public static void FixSky() {
        string name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        var m=JsonUtility.FromJson<BenchManifest>(File.ReadAllText("Assets/Resources/"+name+".json"));
        var sky=AssetDatabase.LoadAssetAtPath<Material>("Assets/Settings/Sky.asset");float exposure=1/(1.2f*Mathf.Pow(2,m.ev100));
        sky.shader=Shader.Find("AraBench/PhysicalSky");sky.SetFloat("_Exposure",1);EditorUtility.SetDirty(sky);RenderSettings.skybox=sky;RenderSettings.ambientMode=AmbientMode.Skybox;
        // Native half-float pixels avoid importer-dependent HDR normalization.
        const int size=32;string cubePath="Assets/Settings/PhysicalEnvironment.asset";
        var cube=AssetDatabase.LoadAssetAtPath<Cubemap>(cubePath);
        if(cube==null){cube=new Cubemap(size,TextureFormat.RGBAHalf,true);AssetDatabase.CreateAsset(cube,cubePath);}
        for(int face=0;face<6;face++)for(int y=0;y<size;y++)for(int x=0;x<size;x++) {
            float a=(x+.5f)/size*2-1,b=(y+.5f)/size*2-1;Vector3 d;
            switch(face){case 0:d=new Vector3(1,-b,-a);break;case 1:d=new Vector3(-1,-b,a);break;case 2:d=new Vector3(a,1,b);break;case 3:d=new Vector3(a,-1,-b);break;case 4:d=new Vector3(a,-b,1);break;default:d=new Vector3(-a,-b,-1);break;}
            float elevation=d.normalized.y;float blend=elevation>=0?Mathf.Pow(elevation,.6f):Mathf.Min(-elevation*4,1);
            var color=Color.Lerp(new Color(.6f,.55f,.5f),elevation>=0?new Color(.5f,.48f,.45f):new Color(.25f,.22f,.2f),blend)*900;color.a=1;cube.SetPixel((CubemapFace)face,x,y,color);
        }
        cube.Apply(true,false);EditorUtility.SetDirty(cube);RenderSettings.customReflectionTexture=cube;RenderSettings.reflectionIntensity=1;
        File.WriteAllText(@"D:\AraBench\logs\unity-sky-exposure.txt","linear="+exposure+"\nzenith pixel="+cube.GetPixel(CubemapFace.PositiveY,16,16)+"\nformat="+cube.format);AssetDatabase.SaveAssets();
    }
}
