"""Publish original 4K captures; keep small previews embedded in the report."""
from pathlib import Path
import subprocess,json,sys
root=Path(r'D:\AraBench');scene=sys.argv[1]if len(sys.argv)>1 else'showcase';dest=root/'report/image-links.json';links=json.loads(dest.read_text())if dest.exists()else{}
manifest=json.loads((root/'source'/scene/'manifest.json').read_text())
for shot in manifest['shots']:
    for engine in ['ara','unity','unreal','cycles']:
        key='/'.join([scene,engine,shot['name']]);path=root/'report/images'/scene/engine/(shot['name']+'.png')
        if key in links:continue
        data=json.loads(subprocess.check_output(['pnpm.cmd','dlx','@snowy7/snist','media','upload',str(path),'--alt',scene.title()+' '+engine.title()+' '+shot['name']+' original 4K','--json'],text=True))
        links[key]=data['asset']['rawUrl'];dest.write_text(json.dumps(links,indent=2));print('PUBLISHED',key,flush=True)
