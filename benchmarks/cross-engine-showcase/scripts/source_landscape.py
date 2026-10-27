"""Author the outdoor landscape once as portable, static Blender geometry."""
import bpy,json,math,random
import numpy as np
from pathlib import Path
from mathutils import Vector,Matrix

def terrain(project,axis,imports,import_model):
    records=json.loads((project/'terrain/outpost.terrain').read_text())['records'];header=records['h_grid'];origin=header['origin'];spacing=header['spacing'];cells=header['cells'];side=cells*header['max']['x']+1
    xs=origin[0]+np.arange(side)*spacing;zs=origin[1]+np.arange(side)*spacing;x,z=np.meshgrid(xs,zs);height=np.zeros_like(x,dtype=np.float32)
    # The source consists only of raise and paint gestures. Replay its sculpt once.
    for key,stroke in sorted(records.items(),key=lambda item:(item[1].get('sequence',0),item[0])):
        if 'dabs' not in stroke:continue
        operation=stroke.get('operation','raise')
        if operation=='paint':continue
        if operation!='raise':raise RuntimeError('Unsupported source terrain operation: '+operation)
        for dab in stroke['dabs']:
            d=((x-dab['center'][0])**2+(z-dab['center'][1])**2)/dab['radius']**2
            height+=np.maximum(1-d,0)**2*dab['amount']
    print('LANDSCAPE_HEIGHT_READY',side,flush=True)
    vertices=[(float(px),-float(pz),float(py)) for px,py,pz in zip(x.flat,height.flat,z.flat)]
    faces=[(j*side+i,(j+1)*side+i,(j+1)*side+i+1,j*side+i+1) for j in range(side-1)for i in range(side-1)]
    mesh=bpy.data.meshes.new('Shared landscape');mesh.from_pydata(vertices,[],faces);mesh.update();obj=bpy.data.objects.new('Landscape sculpt',mesh);bpy.context.scene.collection.objects.link(obj)
    for asset in ['dry_ground_rocks','rocky_terrain_02']:
        mat=bpy.data.materials.new('Landscape '+asset);mat.use_nodes=True;nodes=mat.node_tree.nodes;links=mat.node_tree.links;p=nodes.get('Principled BSDF');p.inputs['Metallic'].default_value=0
        for suffix in ['diff','nor_gl','arm']:
            path=project/'assets/third_party'/asset/(asset+'_'+suffix+'_2k.jpg');tex=nodes.new('ShaderNodeTexImage');tex.image=bpy.data.images.load(str(path),check_existing=True)
            if suffix=='diff':links.new(tex.outputs['Color'],p.inputs['Base Color'])
            else:
                tex.image.colorspace_settings.name='Non-Color'
                if suffix=='nor_gl':
                    normal=nodes.new('ShaderNodeNormalMap');links.new(tex.outputs['Color'],normal.inputs['Color']);links.new(normal.outputs['Normal'],p.inputs['Normal'])
                else:
                    separate=nodes.new('ShaderNodeSeparateColor');links.new(tex.outputs['Color'],separate.inputs['Color']);links.new(separate.outputs['Green'],p.inputs['Roughness'])
        mesh.materials.append(mat)
    uv=mesh.uv_layers.new(name='UVMap');dz,dx=np.gradient(height,spacing);slope=np.sqrt(dx*dx+dz*dz)
    average=(slope[:-1,:-1]+slope[1:,:-1]+slope[:-1,1:]+slope[1:,1:])/4
    mesh.polygons.foreach_set('material_index',(average>.55).astype(np.int32).ravel())
    mesh.polygons.foreach_set('use_smooth',np.ones(len(faces),dtype=np.bool_))
    coordinates=np.stack((x.ravel(),z.ravel()),axis=1)/4
    uv.data.foreach_set('uv',coordinates[np.asarray(faces,dtype=np.int32).ravel()].astype(np.float32).ravel())
    print('LANDSCAPE_MESH_READY',len(faces)*2,flush=True)
    def sample(px,pz):
        ix=min(side-2,max(0,int((px-origin[0])/spacing)));iz=min(side-2,max(0,int((pz-origin[1])/spacing)));fx=(px-xs[ix])/spacing;fz=(pz-zs[iz])/spacing
        h=float(height[iz,ix]*(1-fx)*(1-fz)+height[iz,ix+1]*fx*(1-fz)+height[iz+1,ix]*(1-fx)*fz+height[iz+1,ix+1]*fx*fz)
        return h,math.degrees(math.atan(float(slope[iz,ix])))
    foliage=json.loads((project/'terrain/outpost.foliage').read_text())['records'];clear=[r for k,r in foliage.items() if k.startswith('x_')];total=0;counts={}
    for key,rule in sorted(foliage.items()):
        if not key.startswith('r_') or not rule.get('enabled',True):continue
        prefab=json.loads((project/rule['asset'].split('#')[0].lstrip('@')).read_text())['records'];templates=[]
        for part,record in prefab.items():
            if 'MeshRenderer'in record:templates.extend(import_model(record,'Scatter template '+key,Matrix.Identity(4)))
        rng=random.Random(rule['seed']);wanted=max(8,min(120,int(rule['density']*2000)));placed=0
        for attempt in range(wanted*200):
            if placed>=wanted:break
            px=rng.uniform(-50,240);pz=rng.uniform(-50,240);py,angle=sample(px,pz)
            if any((px-c['center'][0])**2+(pz-c['center'][1])**2<c['radius']**2 for c in clear):continue
            if not(rule['height'][0]<=py<=rule['height'][1] and rule['slope'][0]<=angle<=rule['slope'][1]):continue
            scale=rng.uniform(*rule['scale']);matrix=Matrix.Translation(axis.to_3x3()@Vector((px,py,pz)))@Matrix.Rotation(rng.uniform(0,2*math.pi),4,'Z')@Matrix.Diagonal(Vector((scale,scale,scale,1)))
            for template in templates:
                clone=template.copy();clone.data=template.data;clone.name=key+'_'+str(placed)+'__'+template.name;bpy.context.scene.collection.objects.link(clone);clone.matrix_world=matrix@template.matrix_world
            placed+=1
        for template in templates:bpy.data.objects.remove(template,do_unlink=True)
        counts[key]=placed;total+=placed
    imports.append({'entity':'landscape','source':'source sculpt replay in Blender; static textured mesh','triangles':len(faces)*2,'foliage':'one fixed seeded Blender scatter, not native foliage systems','instances':total,'speciesCounts':counts})
    return sample
