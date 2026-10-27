"""Read one accepted 20-second engine measurement without mixing timer scopes."""
from pathlib import Path
import json,csv,statistics,math
from unreal_metrics import read as unreal_read
def quantile(a,p):return sorted(a)[max(0,math.ceil(len(a)*p)-1)]
def read(engine,folder):
    folder=Path(folder)
    if engine=='ara':
        p=json.loads((folder/'player.json').read_text());assert p.get('completed'),'Ara run incomplete'
        wall=p.get('raw_wall_ms',p['cpu'].get('frame_samples_ms',[]));gpu=p['gpu_span_ms'];cpu=p['cpu']['work_ms']
        return {'gpu_p50':gpu['p50'],'gpu_p95':gpu['p95'],'cpu_p50':cpu['p50'],'cpu_p95':cpu['p95'],'wall_p50':quantile(wall,.5),'wall_p95':quantile(wall,.95),'one_percent_low_fps':1000/statistics.mean(sorted(wall,reverse=True)[:max(1,math.ceil(len(wall)*.01))]),'hitches_over_16ms':sum(t>16 for t in wall),'frames':len(wall),'first_frame_ms':p['startup'].get('loading_frame_ms'),'full_scene_ms':p['startup'].get('full_scene_frame_ms')}
    if engine=='unreal':return unreal_read(folder/'frames.csv')
    frames=list(csv.DictReader((folder/'frames.csv').open()));assert frames,'Empty Unity capture'
    gpu=[float(f['gpu_ms']) for f in frames if float(f['gpu_ms'])>0];wall=[float(f['wall_ms']) for f in frames];cpu=[float(f['cpu_main_ms']) for f in frames];assert len(gpu)>=.98*len(frames),'Unity GPU timestamp coverage below 98%'
    return {'gpu_p50':quantile(gpu,.5),'gpu_p95':quantile(gpu,.95),'cpu_p50':quantile(cpu,.5),'cpu_p95':quantile(cpu,.95),'wall_p50':quantile(wall,.5),'wall_p95':quantile(wall,.95),'one_percent_low_fps':1000/statistics.mean(sorted(wall,reverse=True)[:max(1,math.ceil(len(wall)*.01))]),'hitches_over_16ms':sum(t>16 for t in wall),'frames':len(wall),'gpu_coverage':len(gpu)/len(frames),'window_seconds':sum(wall)/1000}
