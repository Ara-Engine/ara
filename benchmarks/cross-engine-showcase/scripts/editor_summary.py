"""Keep startup and editor markers separate across quiet and call conditions."""
from pathlib import Path
import json,csv,statistics,math,datetime,re
root=Path(r'D:\AraBench');rows=[];startup=[]
def percentile(values,p):return sorted(values)[max(0,math.ceil(len(values)*p)-1)]
def stats(values):return {'p50_ms':percentile(values,.5),'p95_ms':percentile(values,.95),'samples':len(values)} if values else {}
for scene in ['showcase','outpost','hangar']:
    for condition,base in [('quiet-admission',root/'measurements'),('active-discord-call',root/'measurements/active-call')]:
        for engine in ['ara','unity','unreal']:
            folder=base/scene/engine/'editor';row={'scene':scene,'engine':engine,'condition':condition}
            if engine=='ara'and(folder/'responsiveness.json').exists():
                raw=json.loads((folder/'responsiveness.json').read_text());samples=[f['sample']for f in raw['frames']if f['scenario']=='scene-orbit'];row['orbit']=stats([f['frame_interval_ms']for f in samples if f.get('frame_interval_ms')is not None]);row['raw_scenario']=raw['scenarios'].get('scene-orbit');row['play_ms']=[p['clickToFirstGameFrameMs']for p in raw['play']if p.get('clickToFirstGameFrameMs')is not None];row['play_marker']='click to first Game frame observed by editor';row['settle_timed_out']=raw['settleTimedOut'];row['window']='2560×1440; viewport size not logged'
            elif engine=='unity'and(folder/'scene-view-ms.csv').exists():
                row['orbit']=stats([float(x)for x in(folder/'scene-view-ms.csv').read_text().splitlines()if x]);p=list(csv.reader((folder/'play-entry.csv').open()))if(folder/'play-entry.csv').exists()else[];row['play_reload_on_ms']=[float(x[2])for x in p if x[1]=='reload-on'];row['play_reload_off_ms']=[float(x[2])for x in p if x[1]=='reload-off'];row['play_marker']='request to endCameraRendering callback of first Game camera';row['window']=(folder/'viewport.txt').read_text()if(folder/'viewport.txt').exists()else'not logged'
            elif engine=='unreal'and(folder/'scene-view-ms.json').exists():
                row['orbit']=stats(json.loads((folder/'scene-view-ms.json').read_text()));p=json.loads((folder/'play-entry.json').read_text())if(folder/'play-entry.json').exists()else[];row['play_world_ms']=[x['request_to_observed_world_ms']for x in p];row['play_ms']=[x['request_to_next_editor_frame_ms']for x in p];row['play_marker']='request to next Slate tick after observing PIE world; presentation unverified';row['window']='native saved/default editor layout; viewport size not logged'
            if 'orbit'in row:rows.append(row)
    datasets=[('active-discord-call',root/'measurements/active-call'/scene,None),('quiet-admission-continuous',root/'measurements'/scene,None),('quiet-admission-original',root/'measurements'/scene,{'ara':'ara-quiet-60hz','unity':'unity-quiet-60hz'})]
    for condition,base,aliases in datasets:
        for engine in ['ara','unity','unreal']:
            if condition=='quiet-admission-continuous'and engine=='unreal':continue
            for width in ['2560','3840']:
                values=[]
                for repeat in map(str,range(1,6)):
                    run=base/(aliases.get(engine,engine)if aliases else engine)/'high'/width/repeat
                    accepted=run/'accepted.json'
                    if condition=='active-discord-call'and not accepted.exists():continue
                    if engine=='ara'and(run/'player.json').exists():
                        p=json.loads((run/'player.json').read_text())
                        if not p.get('completed'):continue
                        p=p['startup'];values.append({'loading_ms':p.get('loading_frame_ms'),'scene_ms':p.get('full_scene_frame_ms')})
                    elif engine=='unity'and(run/'startup-marker.txt').exists():
                        lines=(run/'startup-marker.txt').read_text().splitlines();a=datetime.datetime.fromisoformat(lines[0]);b=datetime.datetime.fromisoformat(lines[1].split('=',1)[1]);values.append({'scene_callback_ms':(a-b).total_seconds()*1000})
                    elif engine=='unreal':
                        label=json.loads(accepted.read_text())['label']if accepted.exists()else f'{scene}-unreal-high-{width}-{repeat}';session=root/'logs'/label/'session.json';log=session.with_name('stdout.log')
                        if session.exists()and log.exists():
                            info=json.loads(session.read_text());found=re.search(r'\[([\d.]+-[\d.:]+)\].*Engine is initialized\. Leaving',log.read_text(errors='replace'))
                            if found:
                                stamp=datetime.datetime.strptime(found[1],'%Y.%m.%d-%H.%M.%S:%f').replace(tzinfo=datetime.timezone.utc).timestamp();values.append({'engine_initialized_ms':(stamp-info['started_epoch'])*1000})
                if values:
                    med={k:statistics.median([v[k]for v in values if v.get(k)is not None])for k in values[0]if any(v.get(k)is not None for v in values)};spread={k:{'min':min(v[k]for v in values if v.get(k)is not None),'max':max(v[k]for v in values if v.get(k)is not None)}for k in med};startup.append({'scene':scene,'engine':engine,'condition':condition,'width':int(width),'runs':len(values),**med,'spread':spread,'per_run':values})
(root/'report/editor-summary.json').write_text(json.dumps(rows,indent=2));(root/'report/startup-summary.json').write_text(json.dumps(startup,indent=2));print(json.dumps(rows,indent=2))
