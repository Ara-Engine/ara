using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Rendering;

/// Captures rendered Scene-view intervals, then Play request-to-first-Game-camera-frame.
[InitializeOnLoad]
public static class BenchEditor
{
    static readonly List<double> orbit=new List<double>();
    static double start,last;static SceneView sceneView;static BenchManifest manifest;
    static string Output => Environment.GetEnvironmentVariable("ARABENCH_EDITOR_OUTPUT")??@"D:\AraBench\measurements\showcase\unity\editor";
    static BenchEditor() {
        if(SessionState.GetBool("AraBenchEditorActive",false)) {EditorApplication.playModeStateChanged+=PlayState;RenderPipelineManager.endCameraRendering+=Rendered;}
    }
    public static void Run() {
        SessionState.SetBool("AraBenchEditorActive",true);SessionState.SetInt("AraBenchPlayIndex",0);
        SessionState.SetBool("AraBenchOriginalOptions",EditorSettings.enterPlayModeOptionsEnabled);SessionState.SetInt("AraBenchOriginalFlags",(int)EditorSettings.enterPlayModeOptions);
        string name=Environment.GetEnvironmentVariable("ARABENCH_SCENE")??"showcase";
        EditorSceneManager.OpenScene("Assets/"+name+".unity");manifest=JsonUtility.FromJson<BenchManifest>(File.ReadAllText("Assets/Resources/"+name+".json"));
        Directory.CreateDirectory(Output);sceneView=EditorWindow.GetWindow<SceneView>();sceneView.position=new Rect(30,30,1600,1000);sceneView.Show();sceneView.Focus();sceneView.autoRepaintOnSceneChange=true;sceneView.sceneLighting=true;sceneView.sceneViewState.showImageEffects=true;
        sceneView.cameraSettings.fieldOfView=manifest.path[0].fov;
        EditorApplication.playModeStateChanged-=PlayState;EditorApplication.playModeStateChanged+=PlayState;RenderPipelineManager.endCameraRendering-=Rendered;RenderPipelineManager.endCameraRendering+=Rendered;
        SceneView.duringSceneGui-=SceneGui;SceneView.duringSceneGui+=SceneGui;
        start=EditorApplication.timeSinceStartup;last=0;EditorApplication.update+=Orbit;
    }
    static void Orbit() {
        double elapsed=EditorApplication.timeSinceStartup-start;
        if(elapsed>12) {EditorApplication.update-=Orbit;SceneView.duringSceneGui-=SceneGui;File.WriteAllLines(Path.Combine(Output,"scene-view-ms.csv"),orbit.ConvertAll(x=>x.ToString("R",CultureInfo.InvariantCulture)));if(orbit.Count<10)throw new Exception("Scene View did not repaint during the orbit");BeginPlay();return;}
        var sample=manifest.path[Math.Min(1199,(int)(elapsed*60))];Vector3 p=BenchRun.Position(sample.position),t=BenchRun.Position(sample.target);Quaternion q=Quaternion.LookRotation(t-p);
        sceneView.orthographic=false;sceneView.LookAtDirect(t,q,Vector3.Distance(p,t)*Mathf.Sin(sample.fov*Mathf.Deg2Rad/2));sceneView.Repaint();EditorApplication.QueuePlayerLoopUpdate();
    }
    static void SceneGui(SceneView view) {
        if(view!=sceneView||Event.current.type!=EventType.Repaint)return;
        double now=EditorApplication.timeSinceStartup;if(last==0)File.WriteAllText(Path.Combine(Output,"viewport.txt"),view.camera.pixelWidth+"x"+view.camera.pixelHeight+"; SceneView repaint intervals");
        if(now-start>2&&last>0)orbit.Add((now-last)*1000);last=now;
    }
    static void Rendered(ScriptableRenderContext context,Camera camera) {
        double now=EditorApplication.timeSinceStartup;
        if(camera.cameraType==CameraType.Game&&EditorApplication.isPlaying&&SessionState.GetBool("AraBenchAwaitFirstFrame",false)) {
            SessionState.SetBool("AraBenchAwaitFirstFrame",false);
            long begin=long.Parse(SessionState.GetString("AraBenchPlayBegin","0"));double ms=(DateTime.UtcNow.Ticks-begin)/10000.0;
            int index=SessionState.GetInt("AraBenchPlayIndex",0);
            File.AppendAllText(Path.Combine(Output,"play-entry.csv"),index+","+(index<3?"reload-on":"reload-off")+","+ms.ToString("R",CultureInfo.InvariantCulture)+Environment.NewLine);
            EditorApplication.delayCall+=()=>EditorApplication.isPlaying=false;
        }
    }
    static void BeginPlay() {
        int index=SessionState.GetInt("AraBenchPlayIndex",0);
        if(index>=6) {
            EditorSettings.enterPlayModeOptionsEnabled=SessionState.GetBool("AraBenchOriginalOptions",false);EditorSettings.enterPlayModeOptions=(EnterPlayModeOptions)SessionState.GetInt("AraBenchOriginalFlags",0);
            SessionState.SetBool("AraBenchEditorActive",false);EditorApplication.Exit(0);return;
        }
        EditorSettings.enterPlayModeOptionsEnabled=index>=3;EditorSettings.enterPlayModeOptions=EnterPlayModeOptions.DisableDomainReload;
        SessionState.SetString("AraBenchPlayBegin",DateTime.UtcNow.Ticks.ToString());SessionState.SetBool("AraBenchAwaitFirstFrame",true);EditorApplication.isPlaying=true;
    }
    static void PlayState(PlayModeStateChange state) {
        if(state==PlayModeStateChange.EnteredEditMode&&SessionState.GetBool("AraBenchEditorActive",false)) {
            SessionState.SetInt("AraBenchPlayIndex",SessionState.GetInt("AraBenchPlayIndex",0)+1);EditorApplication.delayCall+=BeginPlay;
        }
    }
}
