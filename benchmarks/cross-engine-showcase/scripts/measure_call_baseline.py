"""Measure engine-free call load; this is not a benchmark timing run."""
from pathlib import Path
import subprocess,sys,json,re,contextlib,io,os
from call_policy import create
root=Path(r'D:\AraBench');repo=Path(r'D:\Coding\projects\desktop\snengine')
os.environ.update(CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'infra-target'),ARA_CONFIG_DIR=str(root/'profiles/baseline'),ARA_CRASH_DIR=str(root/'profiles/crashes'))
preflight=subprocess.check_output(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'scripts/guard.ps1'),'call-baseline'],text=True)
scan=Path(re.search(r'Preflight clean: (.*)\. ara-dev',preflight).group(1))
log=root/'logs/call-baseline-guarded.log'
# The measurement-only DWM allowance permits observing its idle baseline.
# All other native guard checks remain active; this value is never used for timings.
with log.open('w')as output:
    subprocess.run(['cargo','run','-p','ara-dev','--','gpu','--dwm-allowance-percent','100','--',sys.executable,'-c','import time; time.sleep(34)'],cwd=repo,stdout=output,stderr=subprocess.STDOUT,check=True)
record=json.loads(log.read_text().rsplit('ARA_DEV_GPU_READING ',1)[1].strip().splitlines()[0]);assert record['status']=='valid'
with contextlib.redirect_stdout(io.StringIO()):create(Path(record['report']),scan)
policy=json.loads((root/'report/call-policy.json').read_text())
print(json.dumps({key:policy[key]for key in ['baseline_seconds','dwm_percent','discord_percent','dwm_allowance_percent']},indent=2))
