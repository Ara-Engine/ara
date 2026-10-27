import os,sys,subprocess,json,shutil,time
from pathlib import Path
from call_policy import flags,validate
root=Path(r'D:\AraBench');repo=Path(r'D:\Coding\projects\desktop\snengine');engine=Path(r'C:\Program Files\Epic Games\UE_5.7\Engine');mode=sys.argv[1];name=sys.argv[2] if len(sys.argv)>2 else 'showcase'
os.environ.update(ARABENCH_SCENE=name,CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'infra-target'),ARA_CONFIG_DIR=str(root/'profiles'/'unreal'),ARA_CRASH_DIR=str(root/'profiles'/'crashes'),ARABENCH_PRESENTMON='0')
os.environ['UE-LocalDataCachePath']=str(root/'unreal-ddc')
def run(label,args,guard=True,timeout=1800,video_directory=None):
    command=['python',str(root/'scripts'/'session.py'),'--label',label,'--timeout',str(timeout),*(['--video-directory',str(video_directory)]if video_directory else[]),'--',*map(str,args)]
    if guard:
        subprocess.run(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'scripts'/'guard.ps1'),label],check=True)
        command=['cargo','run','-p','ara-dev','--','gpu',*(flags()if(root/'report/call-policy.json').exists()else[]),'--',*command]
    with (root/'logs'/(label+'-guarded.log')).open('w') as log:subprocess.run(command,cwd=repo,stdout=log,stderr=subprocess.STDOUT,check=True)
    if guard and(root/'report/call-policy.json').exists():validate(root/'logs'/(label+'-guarded.log'))
    print('COMPLETED',label,flush=True)
if mode=='build':
    import re
    config=root/'unreal/Config/DefaultEngine.ini';content=config.read_text();content=re.sub(r'(GameDefaultMap=|EditorStartupMap=)/Game/Maps/[^\r\n]+',lambda match:match.group(1)+'/Game/Maps/'+name,content);content=re.sub(r'\+MapsToCook=\(FilePath="/Game/Maps/[^"\r\n]+"\)', '+MapsToCook=(FilePath="/Game/Maps/'+name+'")',content);config.write_text(content)
    run('unreal-package-'+name,[engine/'Build'/'BatchFiles'/'RunUAT.bat','BuildCookRun','-project='+str(root/'unreal'/'AraBench.uproject'),'-nop4','-installed','-build','-cook','-stage','-pak','-package','-archive','-archivedirectory='+str(root/'builds'/'unreal'/name),'-platform=Win64','-clientconfig=Development','-map=/Game/Maps/'+name,'-utf8output'],False,2400)
elif mode in ['import','shots','video','editor','finalize']:
    script={'import':'unreal_import.py','shots':'unreal_mrq_stills.py','video':'unreal_movie.py','editor':'unreal_editor.py','finalize':'unreal_finalize.py'}[mode]
    os.environ['ARABENCH_UNREAL_SCRIPT']=str(root/'scripts'/script)
    if mode=='editor'and(root/'report/call-policy.json').exists():os.environ['ARABENCH_EDITOR_OUTPUT']=str(root/'measurements/active-call'/name/'unreal/editor')
    run(name+'-unreal-'+mode,[engine/'Binaries'/'Win64'/'UnrealEditor-Cmd.exe',root/'unreal'/'AraBench.uproject','-ExecutePythonScript='+str(root/'scripts'/'unreal_entry.py'),'-unattended','-nosplash','-NoSound','-stdout','-FullStdOutLogOutput','-log='+str(root/'logs'/(name+'-unreal-'+mode+'.log'))],True,900 if mode=='video' else 300,root/'report/videos'/name/'unreal'if mode=='video'else None)
    text=(root/'logs'/(name+'-unreal-'+mode)/'stdout.log').read_text(errors='replace')
    assert 'ARABENCH_SCRIPT_ERROR' not in text,text[text.find('ARABENCH_SCRIPT_ERROR'):][:2000]
    if mode=='video':
        out=root/'report'/'videos'/name/'unreal'
        result=json.loads((out/'stream-result.json').read_text());assert result['frames']==1200 and result['error']is None,result
        frames=sorted(out.glob('[0-9][0-9][0-9][0-9][0-9].png'))
        subprocess.run(['ffmpeg','-y','-v','warning','-i',str(out/'4k60.mp4'),'-vf','scale=1920:1080','-c:v','libx264','-threads','8','-preset','fast','-crf','18','-pix_fmt','yuv420p',str(out/'1080p60.mp4')],check=True)
        for f in frames:f.unlink()
elif mode=='timing':
    # Find the real packaged game binary rather than the bootstrap launcher.
    candidates=[p for p in (root/'builds'/'unreal'/name).rglob('*.exe') if 'Binaries' in p.parts and not any(x in p.name.lower() for x in ['crash','prereq','bootstrap'])]
    candidates=[p for p in candidates if p.name in ['AraBench.exe','AraBench-Win64-Development.exe','UnrealGame.exe']]
    assert len(candidates)==1,[str(p) for p in candidates];binary=candidates[0]
    for tier,quality,scale in [('high',2,66.666667),('epic',3,77.)]:
        for width,height in [(2560,1440),(3840,2160)]:
            for i in range(1,4):
                label=f'{name}-unreal-{tier}-{width}-{i}';out=root/'measurements'/name/'unreal'/tier/str(width)/str(i);out.mkdir(parents=True,exist_ok=True)
                if (out/'capture-complete.json').exists():continue
                commands=','.join([f'sg.{key} {quality}' for key in ['ViewDistanceQuality','AntiAliasingQuality','ShadowQuality','GlobalIlluminationQuality','ReflectionQuality','PostProcessQuality','TextureQuality','EffectsQuality','FoliageQuality','ShadingQuality']]+[f'r.ScreenPercentage {scale}','r.VSync 0','t.MaxFPS 0','r.DynamicRes.OperationMode 0','r.RayTracing 0','r.Lumen.HardwareRayTracing 0','r.EyeAdaptation.LensAttenuation 0.65','r.GPUStatsEnabled 1','r.GPUCsvStatsEnabled 1','csvprofile frames=8000'])
                began=time.time()
                run(label,[binary,'/Game/Maps/'+name,'-windowed','-borderless','-forceres',f'-ResX={width}',f'-ResY={height}','-unattended','-NoCrashDialog','-NoSplash','-NoSound','-stdout','-FullStdOutLogOutput','-log='+str(out/'player.log'),'-UserDir='+str(out/'profile'),'-csvCompression=0','-ExitAfterCsvProfiling','-ExecCmds='+commands],True,300)
                captures=[p for p in out.rglob('*.csv') if p.name!='frames.csv']
                if not captures:captures=[p for p in (root/'builds'/'unreal'/name).rglob('*.csv') if p.stat().st_mtime>=began]
                assert len(captures)==1,[str(p) for p in captures];shutil.copyfile(captures[0],out/'frames.csv')
                (out/'capture-complete.json').write_text(json.dumps({'binary':str(binary),'tier':tier,'screenPercentage':scale,'width':width,'height':height}))
                if '--single' in sys.argv:sys.exit(0)
else:raise RuntimeError(mode)
