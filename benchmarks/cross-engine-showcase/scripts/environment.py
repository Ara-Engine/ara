"""Write the exact 64x32 Ara Environment gradient as portable Radiance HDR."""
from pathlib import Path
import math,json,sys
root=Path(r'D:\AraBench\source')
def rgbe(rgb):
    v=max(rgb)
    if v<1e-32:return bytes(4)
    m,e=math.frexp(v);scale=m*256/v
    return bytes([min(255,int(c*scale)) for c in rgb]+[e+128])
name=sys.argv[1] if len(sys.argv)>1 else None
environment=json.loads((root/name/'manifest.json').read_text())['environment'] if name else {}
dest=root/name/'environment.hdr' if name else root/'environment.hdr'
with dest.open('wb') as f:
    f.write(b'#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 32 +X 64\n')
    for y in range(32):
        s=math.sin(math.pi/2-(y+.5)/32*math.pi)
        a=environment.get('horizon',[.6,.55,.5])[:3];b=environment.get('sky',[.5,.48,.45])[:3] if s>=0 else environment.get('ground',[.25,.22,.2])[:3];t=s**.6 if s>=0 else min(-s*4,1)
        c=[(x+(z-x)*t)*environment.get('intensity',900) for x,z in zip(a,b)]
        f.write(rgbe(c)*64)
print(dest)
