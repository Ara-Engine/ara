"""Record the installed Unreal preset definitions rather than guessing defaults."""
from pathlib import Path
import json
root=Path(r'D:\AraBench');source=Path(r'C:\Program Files\Epic Games\UE_5.7\Engine\Config\BaseScalability.ini');sections={};current=None
for line in source.read_text(encoding='utf-8-sig').splitlines():
    line=line.strip()
    if line.startswith('[')and line.endswith(']'):current=line[1:-1];sections[current]={}
    elif current and '='in line and not line.startswith(';'):
        key,value=line.split('=',1);sections[current][key.strip()]=value.strip()
picked={name:values for name,values in sections.items()if name.rsplit('@',1)[-1]in['2','3']and name.split('@')[0]in['AntiAliasingQuality','ShadowQuality','GlobalIlluminationQuality','ReflectionQuality','PostProcessQuality','TextureQuality','EffectsQuality','ShadingQuality']}
(root/'report/unreal-scalability.json').write_text(json.dumps({'engine':'5.7.4','source':str(source),'sections':picked},indent=2))
print('Saved',len(picked),'native scalability sections')
