"""Read DXGI adapter LUIDs to identify which device owns the GPU counter load."""
import ctypes as c,json
from ctypes import wintypes as w
class GUID(c.Structure):_fields_=[('a',w.DWORD),('b',w.WORD),('d',w.WORD),('e',c.c_ubyte*8)]
class LUID(c.Structure):_fields_=[('low',w.DWORD),('high',w.LONG)]
class DESC(c.Structure):_fields_=[('description',w.WCHAR*128),('vendor',w.UINT),('device',w.UINT),('subsys',w.UINT),('revision',w.UINT),('video',c.c_size_t),('system',c.c_size_t),('shared',c.c_size_t),('luid',LUID),('flags',w.UINT)]
def method(obj,index,restype,*args):return c.WINFUNCTYPE(restype,c.c_void_p,*args)(c.cast(obj,c.POINTER(c.POINTER(c.c_void_p))).contents[index])
iid=GUID(0x770aae78,0xf26f,0x4dba,(c.c_ubyte*8)(0xa8,0x29,0x25,0x3c,0x83,0xd1,0xb3,0x87));factory=c.c_void_p();hr=c.windll.dxgi.CreateDXGIFactory1(c.byref(iid),c.byref(factory));assert hr==0,hr
rows=[]
try:
    for index in range(8):
        adapter=c.c_void_p();hr=method(factory,12,c.c_long,w.UINT,c.POINTER(c.c_void_p))(factory,index,c.byref(adapter))
        if hr:break
        try:
            desc=DESC();hr=method(adapter,10,c.c_long,c.POINTER(DESC))(adapter,c.byref(desc));assert hr==0,hr
            rows.append({'name':desc.description,'luid':f'0x{desc.luid.high&0xffffffff:08X}_0x{desc.luid.low:08X}','videoBytes':desc.video})
        finally:method(adapter,2,w.ULONG)(adapter)
finally:method(factory,2,w.ULONG)(factory)
print(json.dumps(rows,indent=2))
