using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using UnityEngine;
using UnityEngine.Rendering;
using UnityEngine.Rendering.Universal;

[Serializable] public class BenchView { public string name; public float time; public float[] position; public float[] target; public float fov; }
[Serializable] public class BenchLight { public string name,kind,unit; public float[] position,rotation,color; public float intensity,range,outerAngle,innerAngle; }
[Serializable] public class BenchManifest { public string scene; public float ev100; public BenchView[] shots,path; public BenchLight[] lights; }

/// Drives the same Blender-authored samples in timing and fixed-step capture modes.
public sealed class BenchRun : MonoBehaviour
{
    public string sceneName="showcase";
    public Camera view;
    public BenchManifest manifest;
    public static Vector3 Position(float[] p) => new Vector3(p[0],p[1],-p[2]);
    public static void SetView(Camera c, BenchView v) { c.transform.position=Position(v.position); c.transform.LookAt(Position(v.target)); c.fieldOfView=v.fov; }
    public static void SetPath(Camera c,BenchView[] path,double elapsed) {
        double cursor=elapsed*60;int i=Math.Min(path.Length-1,(int)cursor),j=Math.Min(path.Length-1,i+1);float fraction=(float)(cursor-Math.Floor(cursor));
        c.transform.position=Vector3.Lerp(Position(path[i].position),Position(path[j].position),fraction);
        c.transform.LookAt(Vector3.Lerp(Position(path[i].target),Position(path[j].target),fraction));c.fieldOfView=Mathf.Lerp(path[i].fov,path[j].fov,fraction);
    }
    static string Arg(string key,string fallback) { var a=Environment.GetCommandLineArgs(); int i=Array.IndexOf(a,key); return i>=0&&i+1<a.Length?a[i+1]:fallback; }
    readonly FrameTiming[] timings=new FrameTiming[1];
    readonly List<string> rows=new List<string>(20000);
    ulong lastStamp;
    long lastCpu;
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int hgt,uint flags);
    IEnumerator Start()
    {
        #if UNITY_EDITOR
        if(Environment.GetEnvironmentVariable("ARABENCH_EDITOR_PROBE")=="1")yield break;
        #endif
        manifest=JsonUtility.FromJson<BenchManifest>(Resources.Load<TextAsset>(sceneName).text);
        QualitySettings.vSyncCount=0; Application.targetFrameRate=-1; Application.runInBackground=true;
        #if !UNITY_EDITOR
        // Oversized borderless swapchains keep native 4K rendering on this 1440p desktop.
        int width=int.Parse(Arg("-screen-width","2560")),height=int.Parse(Arg("-screen-height","1440"));
        Screen.SetResolution(width,height,FullScreenMode.Windowed);
        yield return null;
        SetWindowPos(Process.GetCurrentProcess().MainWindowHandle,IntPtr.Zero,0,0,width,height,0x0040);
        yield return null;
        #endif
        string mode=Arg("--bench-mode","timing"), output=Arg("--bench-output",@"D:\AraBench\measurements\unity"), tier=Arg("--bench-tier","high");
        Directory.CreateDirectory(output);
        if(tier=="epic") QualitySettings.SetQualityLevel(1,true); else QualitySettings.SetQualityLevel(0,true);
        SetView(view,manifest.path[0]);
        yield return new WaitForEndOfFrame();
        var startup=new { note="EndOfFrame callback is a CPU submission marker, not proof of OS presentation" };
        File.WriteAllText(Path.Combine(output,"startup-marker.txt"),DateTime.UtcNow.ToString("O")+"\nprocess-start="+Process.GetCurrentProcess().StartTime.ToUniversalTime().ToString("O")+"\n"+startup.note);
        // Let uploads, shader variants and temporal history settle before steady timing.
        for(int i=0;i<180;i++) yield return new WaitForEndOfFrame();
        File.WriteAllText(Path.Combine(output,"effects.txt"),"resolution="+Screen.width+"x"+Screen.height+"\nrenderScale="+((UniversalRenderPipelineAsset)QualitySettings.renderPipeline).renderScale+"\nAA="+view.GetUniversalAdditionalCameraData().antialiasing+"\nTone="+VolumeManager.instance.stack.GetComponent<Tonemapping>().mode.value+"\nBloom="+VolumeManager.instance.stack.GetComponent<Bloom>().intensity.value+"\nProbeSystem="+((UniversalRenderPipelineAsset)QualitySettings.renderPipeline).lightProbeSystem+"\nreflection="+RenderSettings.customReflectionTexture+"\nreflectionIntensity="+RenderSettings.reflectionIntensity);
        if(mode=="shots")
        {
            foreach(var shot in manifest.shots) {
                SetView(view,shot);
                for(int j=0;j<60;j++) yield return new WaitForEndOfFrame();
                ScreenCapture.CaptureScreenshot(Path.Combine(output,shot.name+".png"));
                yield return new WaitForEndOfFrame();
            }
        }
        else if(mode=="video")
        {
            Time.captureFramerate=60;
            for(int i=0;i<1200;i++) {
                SetView(view,manifest.path[i]); yield return new WaitForEndOfFrame();
                var image=ScreenCapture.CaptureScreenshotAsTexture();File.WriteAllBytes(Path.Combine(output,i.ToString("D5")+".png"),image.EncodeToPNG());Destroy(image);
                if(Environment.GetEnvironmentVariable("ARABENCH_STREAM_VIDEO")=="1"&&i>3) {
                    var progress=Path.Combine(output,"stream-progress.txt");var wait=Stopwatch.StartNew();
                    while(true) {
                        int consumed=0;try {if(File.Exists(progress))int.TryParse(File.ReadAllText(progress),out consumed);}catch(IOException) {}
                        if(consumed>=i-2)break;if(wait.Elapsed.TotalSeconds>30)throw new Exception("Video encoder stopped consuming frames");System.Threading.Thread.Sleep(5);
                    }
                }
            }
            Time.captureFramerate=0;
        }
        else
        {
            rows.Add("elapsed_s,wall_ms,gpu_ms,cpu_frame_ms,cpu_main_ms,cpu_render_ms,present_wait_ms,frame_stamp");
            double start=Time.realtimeSinceStartupAsDouble; lastCpu=Stopwatch.GetTimestamp();
            while(Time.realtimeSinceStartupAsDouble-start<20) {
                double elapsed=Time.realtimeSinceStartupAsDouble-start;
                SetPath(view,manifest.path,elapsed);
                FrameTimingManager.CaptureFrameTimings();
                yield return new WaitForEndOfFrame();
                long now=Stopwatch.GetTimestamp(); double wall=(now-lastCpu)*1000.0/Stopwatch.Frequency;lastCpu=now;
                if(FrameTimingManager.GetLatestTimings(1,timings)>0&&timings[0].frameStartTimestamp!=lastStamp) {
                    var t=timings[0];lastStamp=t.frameStartTimestamp;
                    rows.Add(string.Join(",",elapsed.ToString("R",CultureInfo.InvariantCulture),wall.ToString("R",CultureInfo.InvariantCulture),t.gpuFrameTime.ToString("R",CultureInfo.InvariantCulture),t.cpuFrameTime.ToString("R",CultureInfo.InvariantCulture),t.cpuMainThreadFrameTime.ToString("R",CultureInfo.InvariantCulture),t.cpuRenderThreadFrameTime.ToString("R",CultureInfo.InvariantCulture),t.cpuMainThreadPresentWaitTime.ToString("R",CultureInfo.InvariantCulture),lastStamp.ToString()));
                }
            }
            File.WriteAllLines(Path.Combine(output,"frames.csv"),rows);
            File.WriteAllText(Path.Combine(output,"device.txt"),SystemInfo.graphicsDeviceName+"\n"+SystemInfo.graphicsDeviceType+"\n"+Screen.width+"x"+Screen.height+"\nFTM="+FrameTimingManager.IsFeatureEnabled()+"\nMSAA="+QualitySettings.antiAliasing+"\nvsync="+QualitySettings.vSyncCount);
        }
        yield return new WaitForEndOfFrame();
        #if UNITY_EDITOR
        UnityEditor.EditorApplication.isPlaying=false;
        #else
        Application.Quit();
        #endif
    }
}
