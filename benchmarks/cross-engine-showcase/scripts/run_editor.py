import os,sys,subprocess,shutil
from pathlib import Path
from call_policy import flags,validate
root=Path(r'D:\AraBench');repo=Path(r'D:\Coding\projects\desktop\snengine');engine=sys.argv[1];scene=sys.argv[2]
out=(root/'measurements/active-call'if(root/'report/call-policy.json').exists()else root/'measurements')/scene/engine/'editor';out.mkdir(parents=True,exist_ok=True)
os.environ.update(CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'infra-target'),ARA_CONFIG_DIR=str(root/'profiles'/('editor-'+engine)),ARA_CRASH_DIR=str(root/'profiles'/'crashes'),VK_LOADER_LAYERS_DISABLE='~implicit~',ARABENCH_PRESENTMON='0')
if engine=='ara':
    source=root/'ara'/scene;project=root/'editor-projects'/scene
    if not project.exists():shutil.copytree(source,project,ignore=shutil.ignore_patterns('.ara'))
    os.environ.update(ARA_PLAYER=str(root/'ara-target'/'release'/'ara-player.exe'),ARA_IMPORT_WORKER=str(root/'ara-target'/'release'/'ara-import-worker.exe'),ARA_SHADER_DIR=str(root/'ara-source'),ARA_BENCH_MANIFEST=str(root/'source'/scene/'manifest.json'),ARA_BENCH_EDITOR_ONLY='1',ARA_BENCH_EDITOR_ORBIT='1',ARA_EDITOR_MEMO_CHECK='0')
    args=[root/'ara-target'/'release'/'ara-editor.exe',project,'--verify-responsiveness','--settle-imports','before','--size','2560x1440','--output',out]
elif engine=='unity':
    os.environ.update(ARABENCH_SCENE=scene,ARABENCH_EDITOR_PROBE='1',ARABENCH_EDITOR_OUTPUT=str(out))
    args=[r'C:\Program Files\Unity\Hub\Editor\6000.6.6f1\Editor\Unity.exe','-projectPath',root/'unity','-executeMethod','BenchEditor.Run','-logFile',root/'logs'/(scene+'-unity-editor.log')]
else:raise RuntimeError(engine)
label=scene+'-'+engine+'-editor'
subprocess.run(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'scripts'/'guard.ps1'),label],check=True)
with (root/'logs'/(label+'-guarded.log')).open('w') as log:
    subprocess.run(['cargo','run','-p','ara-dev','--','gpu',*(flags()if(root/'report/call-policy.json').exists()else[]),'--','python',str(root/'scripts'/'session.py'),'--label',label,'--timeout','300','--',*map(str,args)],cwd=repo,stdout=log,stderr=subprocess.STDOUT,check=True)
if(root/'report/call-policy.json').exists():validate(root/'logs'/(label+'-guarded.log'))
print('EDITOR_COMPLETE',out)
