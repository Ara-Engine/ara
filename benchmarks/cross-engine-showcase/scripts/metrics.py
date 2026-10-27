from pathlib import Path
import json,numpy as np,flip_evaluator as flip
from PIL import Image,ImageDraw,ImageFont
from skimage.metrics import structural_similarity
root=Path(r'D:\AraBench\report');rows=[]
for scene in ['showcase','outpost','hangar']:
    folder=root/'images'/scene
    for ref in (folder/'cycles').glob('*.png'):
        if 'preview' in ref.stem:continue
        reference=np.asarray(Image.open(ref).convert('RGB'),dtype=np.float32)/255
        pictures=[]
        for engine in ['ara','unity','unreal','cycles']:
            path=folder/engine/ref.name
            if not path.exists():continue
            im=Image.open(path).convert('RGB');assert im.size==(3840,2160),(path,im.size)
            pictures.append((engine,im))
            im.resize((1920,1080),Image.Resampling.LANCZOS).save(path.with_name(path.stem+'-1080p.jpg'),quality=95)
            if engine=='cycles':continue
            test=np.asarray(im,dtype=np.float32)/255
            error,mean,params=flip.evaluate(reference,test,'LDR',inputsRGB=True,applyMagma=False,parameters={'ppd':67})
            ssim=structural_similarity(reference,test,data_range=1,channel_axis=2)
            rows.append({'scene':scene,'shot':ref.stem,'engine':engine,'flip':float(mean),'ssim':float(ssim),'ppd':67,'width':3840,'height':2160})
        if len(pictures)==4:
            sheet=Image.new('RGB',(3840,2160),(13,14,16));draw=ImageDraw.Draw(sheet)
            for i,(label,im) in enumerate(pictures):
                x=(i%2)*1920;y=(i//2)*1080;sheet.paste(im.resize((1920,1080),Image.Resampling.LANCZOS),(x,y));draw.rectangle((x,y,x+180,y+40),fill=(13,14,16));draw.text((x+16,y+4),label.upper(),fill='white',font=ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf',24))
            sheet.save(folder/(ref.stem+'-comparison.png'))
(root/'quality-metrics.json').write_text(json.dumps(rows,indent=2));print(json.dumps(rows,indent=2))
