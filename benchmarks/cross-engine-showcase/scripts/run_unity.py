"""Use Unity's CLI for preparation, APV baking and packaging, one process at a time."""
import os,subprocess,sys,shutil
from pathlib import Path
from call_policy import flags,validate
root=Path(r'D:\AraBench');scene=sys.argv[2] if len(sys.argv)>2 else 'showcase';mode=sys.argv[1]
os.environ.update(ARABENCH_SCENE=scene,CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'infra-target'),ARA_CONFIG_DIR=str(root/'profiles/unity-build'),ARA_CRASH_DIR=str(root/'profiles/crashes'))
methods={'prepare':'BenchBuild.Build','bake':'BenchAPV.Bake','package':'BenchBuild.Package','package-video':'BenchBuild.Package','audit':'BenchDiagnostics.Shadows','fix-realtime':'BenchDiagnostics.Shadows'}
if mode=='package-video':os.environ['ARABENCH_BUILD_OUTPUT']=str(root/'capture-builds/unity'/scene/'AraBench.exe')
if mode=='prepare':
    environment=root/'source'/scene/'environment.hdr'
    for src,dest in [(root/'source'/scene/(scene+'.glb'),root/'unity/Assets'/(scene+'.glb')),(root/'source'/scene/'manifest.json',root/'unity/Assets/Resources'/(scene+'.json')),(environment if environment.exists() else root/'source/environment.hdr',root/'unity/Assets/environment.hdr')]:shutil.copyfile(src,dest)
    os.environ['ARABENCH_PREPARE_ONLY']='1'
if mode=='fix-realtime':os.environ['ARABENCH_FIX_REALTIME']='1'
label=scene+'-unity-'+mode
subprocess.run(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'scripts/guard.ps1'),label],check=True)
args=[r'C:\Program Files\Unity\Hub\Editor\6000.6.6f1\Editor\Unity.exe','-batchmode','-quit','-projectPath',str(root/'unity'),'-executeMethod',methods[mode],'-logFile',str(root/'logs'/(label+'.log'))]
with (root/'logs'/(label+'-guarded.log')).open('w') as log:
    subprocess.run(['cargo','run','-p','ara-dev','--','gpu',*(flags()if(root/'report/call-policy.json').exists()else[]),'--','python',str(root/'scripts/session.py'),'--label',label,'--timeout','1800','--',*args],cwd=r'D:\Coding\projects\desktop\snengine',stdout=log,stderr=subprocess.STDOUT,check=True)
if(root/'report/call-policy.json').exists():validate(root/'logs'/(label+'-guarded.log'))
print('UNITY_COMPLETE',label)
