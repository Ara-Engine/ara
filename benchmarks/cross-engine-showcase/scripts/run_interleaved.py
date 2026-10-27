"""Interleave one guarded packaged run per engine; extend noisy blocks to five repeats."""
import os,sys,subprocess,json,math,statistics,time,shutil,re
from pathlib import Path
from measurement import read
from call_policy import flags,validate
root=Path(r'D:\AraBench');repo=Path(r'D:\Coding\projects\desktop\snengine');scene=sys.argv[1] if len(sys.argv)>1 else 'showcase'
os.environ.update(CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'infra-target'),ARA_CONFIG_DIR=str(root/'profiles/call-timings'),ARA_CRASH_DIR=str(root/'profiles/crashes'),VK_LOADER_LAYERS_DISABLE='~implicit~',ARABENCH_PRESENTMON='0')
os.environ['UE-LocalDataCachePath']=str(root/'unreal-ddc')
base=root/'measurements/active-call'/scene;history=root/'logs'/('interleaved-'+scene+'.jsonl')
def announce(item):
    with history.open('a')as file:file.write(json.dumps(item)+'\n')
    print(json.dumps(item),flush=True)
def one(engine,tier,width,height,repeat):
    out=base/engine/tier/str(width)/str(repeat);out.mkdir(parents=True,exist_ok=True);label=f'call-{scene}-{engine}-{tier}-{width}-{repeat}'
    if (out/'accepted.json').exists():return read(engine,out)
    os.environ.update(ARA_BENCH_MANIFEST=str(root/'source'/scene/'manifest.json'),ARA_BENCH_VISIBLE='1',ARA_BENCH_SECONDS='20',ARA_BENCH_MODE='timing')
    framecount=8000 if scene=='showcase' else 4000
    for attempt in range(3):
        actual=out if attempt==0 else out/('attempt-'+str(attempt+1));actual.mkdir(exist_ok=True)
        currentlabel=label if attempt==0 else label+'-attempt'+str(attempt+1)
        if engine=='ara':args=[root/'builds/ara'/scene/'build/ara-player.exe','--size',f'{width}x{height}','--quality',tier,'--ray-tracing=false','--gpu-cache',root/'profiles/ara-gpu-cache','--perf-output',actual/'player.json','--perf-wait-ready','--perf-warmup','180','--perf-ready-timeout-ms','120000']
        elif engine=='unity':args=[root/'builds/unity'/scene/'AraBench.exe','-screen-width',width,'-screen-height',height,'-screen-fullscreen','0','-popupwindow','-force-d3d12','-logFile',actual/'player.log','--bench-output',actual,'--bench-mode','timing','--bench-tier',tier]
        else:
            binary=root/'builds/unreal'/scene/'Windows/AraBench/Binaries/Win64/AraBench.exe';q=2 if tier=='high' else 3;scale=66.666667 if tier=='high' else 77.
            commands=','.join([f'sg.{key} {q}' for key in ['ViewDistanceQuality','AntiAliasingQuality','ShadowQuality','GlobalIlluminationQuality','ReflectionQuality','PostProcessQuality','TextureQuality','EffectsQuality','FoliageQuality','ShadingQuality']]+[f'r.ScreenPercentage {scale}','r.VSync 0','t.MaxFPS 0','r.DynamicRes.OperationMode 0','r.RayTracing 0','r.Lumen.HardwareRayTracing 0','r.EyeAdaptation.LensAttenuation 0.65','r.GPUStatsEnabled 1','r.GPUCsvStatsEnabled 1',f'csvprofile frames={framecount}'])
            args=[binary,'/Game/Maps/'+scene,'-windowed','-borderless','-forceres',f'-ResX={width}',f'-ResY={height}','-unattended','-NoCrashDialog','-NoSplash','-NoSound','-stdout','-FullStdOutLogOutput','-log='+str(actual/'player.log'),'-UserDir='+str(actual/'profile'),'-csvCompression=0','-ExitAfterCsvProfiling','-ExecCmds='+commands]
        subprocess.run(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'scripts/guard.ps1'),currentlabel],check=True,stdout=subprocess.DEVNULL)
        log=root/'logs'/(currentlabel+'-guarded.log');started=time.time();announce({'event':'start','scene':scene,'engine':engine,'tier':tier,'width':width,'repeat':repeat,'attempt':attempt+1,'epoch':started})
        with log.open('w')as file:
            result=subprocess.run(['cargo','run','-p','ara-dev','--','gpu',*flags(),'--','python',str(root/'scripts/session.py'),'--label',currentlabel,'--timeout','300','--',*map(str,args)],cwd=repo,stdout=file,stderr=subprocess.STDOUT)
        if result.returncode:raise RuntimeError(str(log)+' refused or failed; batch stopped')
        guard=validate(log)
        if engine=='unreal':
            captures=[p for p in actual.rglob('*.csv')if p.name!='frames.csv'];assert len(captures)==1,[str(p)for p in captures];shutil.copyfile(captures[0],actual/'frames.csv')
        try:metrics=read(engine,actual)
        except ValueError as error:
            duration=re.search(r'CSV covers ([\d.]+)s',str(error))
            if engine!='unreal' or not duration:raise
            framecount=math.ceil(framecount*30/float(duration[1])/500)*500;announce({'event':'short-csv-retained-unscored','label':currentlabel,'next_frame_count':framecount});continue
        if actual!=out:
            # Canonical rows point to the accepted attempt; retain the earlier short CSV.
            shutil.copyfile(actual/'frames.csv',out/'frames.csv')
        (out/'metrics.json').write_text(json.dumps(metrics,indent=2));(out/'accepted.json').write_text(json.dumps({**guard,'label':currentlabel,'attempt':attempt+1,'camera_mode':'continuous interpolation','settings':{'tier':tier,'output':[width,height],'hardware_ray_tracing':False,'vsync':False}},indent=2))
        announce({'event':'accepted','label':currentlabel,'gpu_p50':metrics['gpu_p50'],'gpu_p95':metrics['gpu_p95'],'wall_p95':metrics['wall_p95']});return metrics
    raise RuntimeError('Unable to get a full timing window: '+label)
def noisy(rows):
    reasons=[]
    for engine,values in rows.items():
        for key,limit in [('gpu_p50',.05),('gpu_p95',.05),('wall_p95',.10),('one_percent_low_fps',.10)]:
            data=[v[key]for v in values];spread=(max(data)-min(data))/statistics.median(data)
            if spread>limit:reasons.append({'engine':engine,'metric':key,'range_fraction':spread,'threshold':limit})
    return reasons
for tier in ['high','epic']:
    for width,height in [(2560,1440),(3840,2160)]:
        rows={engine:[]for engine in ['ara','unity','unreal']}
        for repeat in range(1,4):
            for engine in rows:
                rows[engine].append(one(engine,tier,width,height,repeat))
                if '--single' in sys.argv:sys.exit(0)
        reasons=noisy(rows);announce({'event':'variance-check','tier':tier,'width':width,'extra_repeats':bool(reasons),'reasons':reasons})
        if reasons:
            for repeat in [4,5]:
                for engine in rows:rows[engine].append(one(engine,tier,width,height,repeat))
        summary={'scene':scene,'tier':tier,'width':width,'repeats':len(rows['ara']),'extra_repeat_reasons':reasons,'engine_order':['ara','unity','unreal'],'condition':'active-discord-call'}
        (base/(tier+'-'+str(width)+'-block.json')).write_text(json.dumps(summary,indent=2))
announce({'event':'complete','scene':scene})
