"""Reject clipped/missing fixed-camera renders before any timing batch is accepted."""
import sys,json
from pathlib import Path
import numpy as np
from PIL import Image
root=Path(r'D:\AraBench');scene=sys.argv[1];engine=sys.argv[2]
m=json.loads((root/'source'/scene/'manifest.json').read_text());rows=[]
for shot in m['shots']:
    path=root/'report'/'images'/scene/engine/(shot['name']+'.png');im=Image.open(path).convert('RGB');assert im.size==(3840,2160),(path,im.size)
    a=np.asarray(im.resize((480,270)),dtype=np.float32);clipped=float(np.mean(np.min(a,axis=2)>250));std=float(np.std(a));mean=float(np.mean(a))
    assert clipped<.8 and std>10 and 15<mean<245,('Invalid rendered image',str(path),clipped,std,mean)
    rows.append({'path':str(path),'clipped_white_fraction':clipped,'std_8bit':std,'mean_8bit':mean})
(root/'report'/'images'/scene/engine/'verification.json').write_text(json.dumps(rows,indent=2));print(json.dumps(rows,indent=2))
