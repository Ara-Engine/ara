"""Derive a narrowly scoped call policy from an engine-free 30-second reading."""
from pathlib import Path
import json,math,statistics,shutil
root=Path(r'D:\AraBench')
def percentile(values,p):return sorted(values)[max(0,math.ceil(len(values)*p)-1)]
def create(path,process_scan=None):
    raw=json.loads(path.read_text());samples=raw['reading']['samples'][3:33]
    assert len(samples)>=29,'Thirty idle samples are required'
    def values(name):return [sum(p['percent'] for p in s['raw'] if p['name'].lower()==name) for s in samples]
    def stats(a):return {'mean':statistics.mean(a),'p50':percentile(a,.5),'p95':percentile(a,.95),'min':min(a),'max':max(a)}
    dwm=stats(values('dwm.exe'));discord=stats(values('discord.exe'))
    known={str(p['pid'])+':'+str(p['birth']):{'name':p['name'],'peak_percent':0.} for s in samples for p in s['raw']}
    for s in samples:
        for p in s['raw']:known[str(p['pid'])+':'+str(p['birth'])]['peak_percent']=max(known[str(p['pid'])+':'+str(p['birth'])]['peak_percent'],p['percent'])
    names={}
    if process_scan:
        text=Path(process_scan).read_text(encoding='utf-8-sig');start=next(i for i,line in enumerate(text.splitlines())if line.strip()=='[');items=json.loads('\n'.join(text.splitlines()[start:]));names={str(p['ProcessId']):p['Name'] for p in items}
    policy={'condition':'active-discord-call','policy_revision':'1.1-baseline-process-birth','baseline_start_unix_ms':samples[0]['interval_start_unix_ms'],'baseline_seconds':(samples[-1]['unix_ms']-samples[0]['interval_start_unix_ms'])/1000,'dwm_percent':dwm,'discord_percent':discord,'dwm_allowance_percent':math.ceil((dwm['p95']+5)*10)/10,'margin_percentage_points':5,'foreign_threshold_percent':10,'foreign_sustained_samples':5,'new_gpu_process_threshold_percent':.5,'new_gpu_process_sustained_samples':2,'baseline_processes':known,'baseline_all_process_names':names,'baseline_process_scan':str(process_scan),'baseline_reading':str(path),'note':'Only DWM allowance is raised; Discord remains charged by the unchanged foreign-sum guard. Games still invalidate on presence. New GPU process identities are checked after each run. An existing process is not new merely because it had zero GPU activity during the idle sample.'}
    shutil.copyfile(path,root/'report/call-baseline-raw.json');(root/'report/call-policy.json').write_text(json.dumps(policy,indent=2));print(json.dumps(policy,indent=2))
def flags():
    policy=json.loads((root/'report/call-policy.json').read_text());return ['--dwm-allowance-percent',str(policy['dwm_allowance_percent'])]
def validate(log):
    text=log.read_text(errors='replace');marker='ARA_DEV_GPU_READING ';assert marker in text,'Missing continuous GPU-monitor report'
    record=json.loads(text.rsplit(marker,1)[1].strip().splitlines()[0]);assert record['status']=='valid',record.get('status')
    policy=json.loads((root/'report/call-policy.json').read_text());known=policy['baseline_processes'];counts={};bad=[]
    for sample in record['reading']['samples']:
        seen=set()
        for p in sample['raw']:
            key=str(p['pid'])+':'+str(p['birth'])
            existed_at_baseline=policy.get('baseline_all_process_names',{}).get(str(p['pid']))==p['name'] and p['birth']>0 and (p['birth']/10_000_000-11644473600)*1000<policy['baseline_start_unix_ms']
            if key not in known and not existed_at_baseline and p['name'].lower() not in ['system','csrss.exe'] and p['percent']>policy['new_gpu_process_threshold_percent']:
                seen.add(key);counts[key]=counts.get(key,0)+1
                if counts[key]>=policy['new_gpu_process_sustained_samples']:bad.append(p)
        counts={key:value for key,value in counts.items() if key in seen}
    if bad:raise RuntimeError('New foreign GPU process detected: '+json.dumps(bad))
    import hashlib
    return {'status':'valid','condition':policy['condition'],'policy_revision':policy['policy_revision'],'policy_sha256':hashlib.sha256((root/'report/call-policy.json').read_bytes()).hexdigest(),'dwm_allowance_percent':policy['dwm_allowance_percent'],'baseline_file':'call-policy.json','gpu_report':record.get('report'),'new_gpu_process_check':'passed'}
if __name__=='__main__':
    import sys
    create(Path(sys.argv[1]),Path(sys.argv[2]) if len(sys.argv)>2 else None)
