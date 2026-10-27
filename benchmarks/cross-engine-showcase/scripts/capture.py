"""One guarded capture batch, with timing disabled and explicitly named outputs."""
import subprocess,sys,os
from pathlib import Path
from call_policy import flags,validate
root=Path(r'D:\AraBench');repo=Path(r'D:\Coding\projects\desktop\snengine');engine=sys.argv[1];scene=sys.argv[2];mode=sys.argv[3]
out=root/'report'/('videos' if mode=='video' else 'images')/scene/engine;out.mkdir(parents=True,exist_ok=True)
os.environ.update(CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'infra-target'),ARA_CONFIG_DIR=str(root/'profiles'/('capture-'+engine)),ARA_CRASH_DIR=str(root/'profiles'/'crashes'),VK_LOADER_LAYERS_DISABLE='~implicit~',ARABENCH_PRESENTMON='0')
if engine=='ara':
    os.environ.update(ARA_BENCH_VISIBLE='1',ARA_BENCH_MANIFEST=str(root/'source'/scene/'manifest.json'),ARA_BENCH_MODE=mode,ARA_BENCH_OUTPUT=str(out/'4k60.mp4' if mode=='video' else out))
    player=root/'capture-builds/ara'/scene/'ara-player.exe'if mode=='video'else root/'builds/ara'/scene/'build/ara-player.exe';assert player.exists(),player
    args=[player,'--size','3840x2160','--quality','high','--ray-tracing=false','--frames','2000','--gpu-cache',root/'profiles'/'ara-gpu-cache']
elif engine=='unity':
    player=root/'capture-builds/unity'/scene/'AraBench.exe'if mode=='video'else root/'builds/unity'/scene/'AraBench.exe';assert player.exists(),player
    if mode=='video':os.environ['ARABENCH_STREAM_VIDEO']='1'
    args=[player,'-screen-width','3840','-screen-height','2160','-screen-fullscreen','0','-popupwindow','-force-d3d12','-logFile',root/'logs'/f'{scene}-unity-{mode}.log','--bench-output',out,'--bench-mode',mode,'--bench-tier','high']
elif engine=='cycles':args=[r'C:\Program Files\Blender Foundation\Blender 5.1\blender.exe','-b','-t','8','-P',root/'scripts'/'render_reference.py','--',scene,'--save-prepared']
else:raise RuntimeError(engine)
label=f'{scene}-{engine}-{mode}'
subprocess.run(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'scripts'/'guard.ps1'),label],check=True)
with (root/'logs'/(label+'-guarded.log')).open('w') as log:
    subprocess.run(['cargo','run','-p','ara-dev','--','gpu',*(flags()if(root/'report/call-policy.json').exists()else[]),'--','python',str(root/'scripts'/'session.py'),'--label',label,'--timeout','1800',*(['--video-directory',str(out)]if engine=='unity'and mode=='video'else[]),'--',*map(str,args)],cwd=repo,stdout=log,stderr=subprocess.STDOUT,check=True)
if(root/'report/call-policy.json').exists():(out/'capture-policy.json').write_text(__import__('json').dumps(validate(root/'logs'/(label+'-guarded.log')),indent=2))
if mode=='video':
    subprocess.run(['ffmpeg','-y','-v','warning','-i',str(out/'4k60.mp4'),'-vf','scale=1920:1080','-c:v','libx264','-threads','8','-preset','fast','-crf','18','-pix_fmt','yuv420p',str(out/'1080p60.mp4')],check=True)
    # Delete only exact scratch-frame files created by this capture after both encodes succeed.
    if engine=='unity':
        for path in out.glob('[0-9][0-9][0-9][0-9][0-9].png'):path.unlink()
print('CAPTURE_COMPLETE',out)
