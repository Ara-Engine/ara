"""Keep conditions separate and report between-run spread as well as the median."""
from pathlib import Path
import json,statistics
from measurement import read
root=Path(r'D:\AraBench');rows=[]
for scene in ['showcase','outpost','hangar']:
    datasets=[
        ('active-discord-call',{e:root/'measurements/active-call'/scene/e for e in ['ara','unity','unreal']}),
        ('quiet-admission-original',{'ara':root/'measurements'/scene/'ara-quiet-60hz','unity':root/'measurements'/scene/'unity-quiet-60hz','unreal':root/'measurements'/scene/'unreal'}),
        ('quiet-admission-continuous',{'ara':root/'measurements'/scene/'ara','unity':root/'measurements'/scene/'unity'}),
    ]
    for condition,engines in datasets:
        for engine,base in engines.items():
            for tier in ['high','epic']:
                for width in ['2560','3840']:
                    runs=[]
                    for repeat in range(1,6):
                        folder=base/tier/width/str(repeat)
                        if condition=='active-discord-call' and not(folder/'accepted.json').exists():continue
                        if not(folder/('player.json'if engine=='ara'else'frames.csv')).exists():continue
                        try:
                            path=folder/'metrics.json';values=json.loads(path.read_text())if path.exists()else read(engine,folder);runs.append({'repeat':repeat,'folder':str(folder),**values})
                        except (ValueError,AssertionError)as error:print('REJECT',folder,error)
                    if not runs:continue
                    keys=set().union(*(r.keys()for r in runs))-{'repeat','folder'}
                    med={key:statistics.median([r[key]for r in runs if r.get(key)is not None]) for key in keys if any(r.get(key)is not None for r in runs)}
                    spread={}
                    for key in ['gpu_p50','gpu_p95','cpu_p50','cpu_p95','wall_p50','wall_p95','one_percent_low_fps','hitches_over_16ms']:
                        data=[r[key]for r in runs];mean=statistics.mean(data);spread[key]={'min':min(data),'max':max(data),'sample_cv_percent':statistics.stdev(data)/mean*100 if len(data)>1 and mean else 0.}
                    camera='60 Hz pose steps'if condition=='quiet-admission-original'and engine in ['ara','unity']else'continuous interpolation'
                    rows.append({'scene':scene,'engine':engine,'tier':tier,'width':int(width),'condition':condition,'camera_mode':camera,'runs':len(runs),**med,'hitches_total':sum(r['hitches_over_16ms']for r in runs),'spread':spread,'per_run':runs})
(root/'report/runtime-summary.json').write_text(json.dumps(rows,indent=2));print(json.dumps([{k:r[k]for k in ['scene','engine','tier','width','condition','runs','gpu_p50','gpu_p95']}for r in rows],indent=2))
