"""Author Ara through typed CLI commands and freeze using the production exporter."""
from pathlib import Path
import subprocess,json,shutil,os,sys,math
root=Path(r'D:\AraBench');bin=root/'ara-target'/'release';name=sys.argv[1] if len(sys.argv)>1 else 'showcase';project=root/'ara'/name
os.environ['ARA_CONFIG_DIR']=str(root/'profiles'/'prepare');os.environ['ARA_CRASH_DIR']=str(root/'profiles'/'crashes');os.environ['ARA_IMPORT_WORKER']=str(bin/'ara-import-worker.exe')
def call(*args):
    print('RUN',*map(str,args),flush=True);subprocess.run(list(map(str,args)),check=True)
if not (project/'ara.project.json').exists():call(bin/'ara.exe','init',project,'--name','AraBench '+name)
(project/'assets').mkdir(exist_ok=True);shutil.copyfile(root/'source'/name/(name+'.glb'),project/'assets'/'scene.glb')
cli=[bin/'ara.exe','--root',project,'--author','human','--json']
if not (project/'assets'/'scene.glb.import.json').exists():call(*cli,'do','import.initialize','-p','source=assets/scene.glb','-p','id=as_benchmark','--why','Register the single shared Blender export')
m=json.loads((root/'source'/name/'manifest.json').read_text())
def camera_euler(view):
    f=[b-a for a,b in zip(view['position'],view['target'])];length=math.sqrt(sum(v*v for v in f));f=[v/length for v in f]
    right=[-f[2],0,f[0]];length=math.sqrt(sum(v*v for v in right));right=[v/length for v in right]
    up=[right[1]*f[2]-right[2]*f[1],right[2]*f[0]-right[0]*f[2],right[0]*f[1]-right[1]*f[0]]
    return [math.degrees(math.atan2(f[1],-f[2])),math.degrees(math.asin(-f[0])),math.degrees(math.atan2(-up[0],right[0]))]
environment={k:m['environment'][k] for k in ['sky','horizon','ground','intensity']}
for k in ['sky','horizon','ground']:environment[k]=environment[k][:3]+[1]
environment['exposure']=1/(1.2*2**m['ev100'])
commands=[{'cmd' :'entity.create','args':{'id':'e_geometry','name':'Shared Blender geometry','components':{'Transform':{},'MeshRenderer':{'model':'@assets/scene.glb#as_benchmark'}}}}]
for i,l in enumerate(m['lights']):
    light={'kind':l['kind'],'color':l['color']+[1],'intensity':l['intensity']}
    if l['kind']!='directional':light.update(range=l['range'],outerAngle=l['outerAngle'],innerAngle=l['innerAngle'])
    commands.append({'cmd':'entity.create','args':{'id':'e_light'+str(i),'name':l['name'],'components':{'Transform':{'position':l['position'],'rotation':l['rotation']},'Light':light}}})
commands.extend([
{'cmd':'entity.create','args':{'id':'e_camera','name':'Benchmark Camera','components':{'Transform':{'position':m['path'][0]['position'],'rotation':camera_euler(m['path'][0])},'Camera':{'fov':m['path'][0]['fov']}}}},
{'cmd':'entity.create','args':{'id':'e_environment','name':'Common environment and fixed EV','components':{'Environment':environment,'PostProcessVolume':{'overrides':[{'module':'exposure','exposure':'manual','ev100':m['ev100']},{'module':'toneMapping','toneMapping':'filmic'},{'module':'bloom','bloom':.04}]}}}}
])
request=root/'scripts'/('ara-author-'+name+'.json');request.write_text(json.dumps({'commands':commands,'doc':'levels/main.scene','why':'Author the common Blender benchmark geometry, lights and fixed exposure'}))
if 'e_geometry' not in json.loads((project/'levels'/'main.scene').read_text())['records']:call(*cli,'do','--args','@'+str(request))
call(*cli,'fmt',project/'levels'/'main.scene');call(*cli,'check')
template=root/'ara-runtime.zip'
if not template.exists():call(bin/'ara-export.exe','runtime-template','--player',bin/'ara-player.exe','--engine',root/'ara-source','--output',template)
pin=json.loads(subprocess.check_output(list(map(str,[bin/'ara-export.exe','pin','--platform','windows','--arch','x64','--config','release','--file',template,'--symbols',bin/'ara_player.pdb'])),text=True))
preset=root/'scripts'/('ara-'+name+'.export.json');preset.write_text(json.dumps({'$schema':'ara://schema/export/1','platform':'windows','config':'release','scriptProject':'','assembly':'AraBench','content':'levels','playerTemplate':'runtime','templates':[pin]}))
call(bin/'ara-export.exe','preset-fmt',preset)
call(bin/'ara-export.exe','export','--project',project,'--preset',preset,'--output',root/'builds'/'ara'/name,'--cache',root/'ara-export-cache')
