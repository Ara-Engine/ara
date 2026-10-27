"""Build the cross-engine source once; all engine adapters consume these exports."""
import bpy, json, math, sys
from pathlib import Path
from mathutils import Matrix, Vector, Euler

ROOT = Path(r'D:\AraBench')
REPO = Path(r'D:\Coding\projects\desktop\snengine')
name = sys.argv[sys.argv.index('--') + 1] if '--' in sys.argv else 'showcase'
project = REPO / 'samples' / ('showcase' if name == 'showcase' else 'outpost')
source = json.loads((project / 'levels' / (name + '.scene')).read_text())
look = json.loads((project / 'look' / (name + '.look.json')).read_text())
out = ROOT / 'source' / name
out.mkdir(parents=True, exist_ok=True)
bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
# glTF is right-handed Y-up; Blender is right-handed Z-up.
axis = Matrix(((1,0,0,0),(0,0,-1,0),(0,1,0,0),(0,0,0,1)))
def ara_matrix(t):
    p = Vector(t.get('position', [0,0,0]))
    x,y,z=[math.radians(x) for x in t.get('rotation',[0,0,0])]
    r = Matrix.Rotation(x,4,'X') @ Matrix.Rotation(y,4,'Y') @ Matrix.Rotation(z,4,'Z')
    s = Matrix.Diagonal(Vector(t.get('scale',[1,1,1]) + [1]))
    return axis @ Matrix.Translation(p) @ r @ s @ axis.inverted()
def bpos(p): return axis.to_3x3() @ Vector(p)
imports=[]
model_cache={}
def entity_matrix(key):
    record=source['records'][key];local=ara_matrix(record.get('Transform',{}))
    return entity_matrix(record['parent'])@local if record.get('parent') else local

def import_model(rec,key,placement):
    renderer=rec.get('MeshRenderer',{});model=renderer.get('model')
    if not model:
        if not renderer:return []
        if renderer.get('shape')!='plane':raise RuntimeError('Unsupported procedural source: '+key)
        bpy.ops.mesh.primitive_plane_add(size=2);o=bpy.context.object;o.name=key
        size=renderer.get('size',[1,1,1]);o.scale=(size[0]/2,size[2]/2,1)
        bpy.ops.object.transform_apply(location=False,rotation=False,scale=True);o.matrix_world=placement
        mat=bpy.data.materials.new(key+' material');mat.use_nodes=True;p=mat.node_tree.nodes['Principled BSDF'];p.inputs['Base Color'].default_value=renderer.get('color',[1,1,1,1]);p.inputs['Roughness'].default_value=.7
        p.inputs['Emission Color'].default_value=(*renderer.get('emissive',[0,0,0]),1);p.inputs['Emission Strength'].default_value=1;o.data.materials.append(mat)
        return [o]
    path = project / model.split('#')[0].lstrip('@')
    if not path.is_file(): raise RuntimeError('Missing source: ' + str(path))
    cache_key=(str(path),str(renderer.get('node','')),str(renderer.get('primitive','')))
    if cache_key in model_cache:
        made=[]
        for data,local,label in model_cache[cache_key]:
            o=bpy.data.objects.new(key+'__'+label,data);bpy.context.scene.collection.objects.link(o);o.matrix_world=placement@local;made.append(o)
        imports.append({'entity':key,'source':str(path),'meshes':len(made),'linkedGeometry':True})
        return made
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=str(path), import_pack_images=True, disable_bone_shape=True)
    objects = set(bpy.data.objects) - before
    for o in objects:
        o.animation_data_clear()
        if o.type == 'ARMATURE':
            o.data.pose_position='REST'
    bpy.context.view_layer.update()
    selected=objects;offset=Matrix.Identity(4)
    if renderer.get('node'):
        node=str(renderer['node']);matches=sorted([o for o in objects if o.name==node or o.name.rsplit('.',1)[0]==node],key=lambda o:(o.name!=node,o.name))
        if not matches:raise RuntimeError('Source node not found: '+key+' / '+node)
        pivot=matches[0];selected={pivot,*pivot.children_recursive};offset=Matrix.Translation(-pivot.matrix_world.translation)
    if renderer.get('primitive') is not None:raise RuntimeError('Primitive selection requires an explicit common export: '+key)
    made=[];cached=[]
    # Bake the evaluated rest geometry; engines get identical static content.
    for o in list(objects):
        if o.type == 'MESH' and o in selected:
            dg = bpy.context.evaluated_depsgraph_get()
            mesh = bpy.data.meshes.new_from_object(o.evaluated_get(dg), depsgraph=dg)
            local=offset@o.matrix_world;world=placement@local;label=o.name
            o.modifiers.clear(); o.data=mesh; o.parent=None; o.matrix_world=world
            o.name=key+'__'+o.name
            made.append(o)
            cached.append((mesh,local,label))
    mesh_count=len(made)
    for o in list(objects):
        if o not in made: bpy.data.objects.remove(o, do_unlink=True)
    imports.append({'entity':key,'source':str(path),'meshes':mesh_count})
    model_cache[cache_key]=cached
    return made

for key,rec in source['records'].items():
    import_model(rec,key,entity_matrix(key))

height_sample=None
if name=='outpost':
    sys.path.insert(0,str(Path(__file__).parent))
    from source_landscape import terrain
    height_sample=terrain(project,axis,imports,import_model)

# Preserve the material studio and compound geometry, but use one common light contract.
sun_rec=next((r for r in source['records'].values() if r.get('Light',{}).get('kind')=='directional'),None)
sun_t=sun_rec.get('Transform',{}) if sun_rec else {'rotation':[-35,-25,0]}
sun_lux={'showcase':2200.0,'outpost':1500.0,'hangar':0.25}[name]
ev={'showcase':9.0,'outpost':9.0,'hangar':7.0}[name]
sun_color=[1.0,0.95,0.88] if name=='showcase' else sun_rec['Light'].get('color',[1,1,1])[:3]
environment=next((r['Environment'] for r in source['records'].values() if 'Environment'in r),{})
environment={k:environment.get(k,v) for k,v in {'sky':[.5,.48,.45,1],'horizon':[.6,.55,.5,1],'ground':[.25,.22,.2,1],'intensity':900}.items()}
lights=[{'name':'Sun','kind':'directional','position':[0,10,0],'rotation':sun_t.get('rotation',[-35,-25,0]),'color':sun_color,'intensity':sun_lux,'unit':'lux','range':0}]
for key, r in source['records'].items():
    l=r.get('Light',{})
    if l and l.get('kind')!='directional':
        lights.append({'name':key,'kind':l.get('kind','point'),'position':r.get('Transform',{}).get('position',[0,0,0]),'rotation':r.get('Transform',{}).get('rotation',[0,0,0]),'color':l.get('color',[1,1,1])[:3],'intensity':l.get('intensity',1000),'unit':'lumens','range':l.get('range',10),'outerAngle':l.get('outerAngle',45),'innerAngle':l.get('innerAngle',25)})
for spec in lights:
    data=bpy.data.lights.new(spec['name'], 'SUN' if spec['kind']=='directional' else ('SPOT' if spec['kind']=='spot' else 'POINT'))
    o=bpy.data.objects.new(spec['name'],data); scene.collection.objects.link(o)
    o.location=bpos(spec['position'])
    x,y,z=[math.radians(v) for v in spec['rotation']]
    forward=bpos([-math.sin(y),math.sin(x)*math.cos(y),-math.cos(x)*math.cos(y)])
    o.rotation_euler=forward.to_track_quat('-Z','Y').to_euler()
    data.color=spec['color']
    data.normalize=True
    # A documented photometric-to-radiometric approximation for a neutral reference.
    # White-light luminous efficacy is fixed to 683 lm/W for every reference light.
    data.energy=spec['intensity']/683
    if data.type=='SUN': data.angle=math.radians(0.526)
    if data.type=='SPOT': data.spot_size=2*math.radians(spec['outerAngle']); data.spot_blend=1-spec['innerAngle']/spec['outerAngle']
    if data.type!='SUN': data.shadow_soft_size=0.05

shots = [s for s in look['shots'] if s['name'] in (['stage','props','materials'] if name=='showcase' else [x['name'] for x in look['shots'][:3]])][:3]
if height_sample is not None and look.get('groundRelative'):
    for shot in shots:
        base=height_sample(shot['position'][0],shot['position'][2])[0]
        shot['position'][1]+=base;shot['target'][1]+=base
def cam(spec,label):
    d=bpy.data.cameras.new(label); o=bpy.data.objects.new(label,d); scene.collection.objects.link(o)
    o.location=bpos(spec['position']); o.rotation_euler=(bpos(spec['target'])-o.location).to_track_quat('-Z','Y').to_euler()
    # Source FOV is vertical; Blender's horizontal sensor fit uses the matching angle.
    d.sensor_fit='HORIZONTAL'; d.angle=2*math.atan(math.tan(math.radians(spec['fov'])/2)*16/9)
    d.clip_start=0.05; d.clip_end=2000
    return o
for s in shots: cam(s,'Shot_'+s['name'])
pathcam=cam(shots[0],'Flythrough')
samples=[]
# An orbit derived once in Blender, sampled at fixed 60 Hz for all adapters.
center=Vector(shots[0]['target']); start=Vector(shots[0]['position']); delta=start-center
for i in range(1200):
    t=i/60; theta=math.radians(-22+44*i/1199)
    p=center+Vector((delta.x*math.cos(theta)+delta.z*math.sin(theta),delta.y+0.12*math.sin(2*math.pi*i/1199),-delta.x*math.sin(theta)+delta.z*math.cos(theta)))
    spec={'position':list(p),'target':list(center),'fov':shots[0]['fov']}
    pathcam.location=bpos(p); pathcam.rotation_euler=(bpos(center)-pathcam.location).to_track_quat('-Z','Y').to_euler()
    pathcam.keyframe_insert('location',frame=i+1); pathcam.keyframe_insert('rotation_euler',frame=i+1)
    samples.append({'time':t,**spec})
scene.frame_start=1; scene.frame_end=1200; scene.render.fps=60; scene.frame_set(1)
scene.camera=pathcam
world=bpy.data.worlds.new('Neutral world'); scene.world=world; world.use_nodes=True
world.node_tree.nodes['Background'].inputs['Color'].default_value=(0.5,0.48,0.45,1)
world.node_tree.nodes['Background'].inputs['Strength'].default_value=0.05
scene.render.engine='CYCLES'; scene.cycles.samples=128; scene.cycles.use_denoising=True
scene.render.resolution_x=3840; scene.render.resolution_y=2160; scene.render.resolution_percentage=100
scene.view_settings.view_transform='AgX'; scene.view_settings.look='AgX - Medium High Contrast'
scene.view_settings.exposure=math.log2(683/(1.2*2**ev)); scene.view_settings.gamma=1
scene.render.image_settings.file_format='PNG'; scene.render.image_settings.color_mode='RGBA'
bpy.ops.wm.save_as_mainfile(filepath=str(out/(name+'.blend')))
# Both exports come from the same static evaluated meshes. Lights/cameras use manifest.
bpy.ops.object.select_all(action='DESELECT')
for o in scene.objects:
    o.select_set(o.type=='MESH')
bpy.ops.export_scene.gltf(filepath=str(out/(name+'.glb')),export_format='GLB',use_selection=True,export_animations=False,export_cameras=False,export_lights=False,export_yup=True)
if '--fbx' in sys.argv:bpy.ops.export_scene.fbx(filepath=str(out/(name+'.fbx')),use_selection=True,object_types={'MESH'},bake_anim=False,axis_forward='-Z',axis_up='Y',path_mode='COPY',embed_textures=False)
manifest={'scene':name,'source':'Blender 5.1 static rest geometry','units':'meters','axes':'right-handed Y-up; cameras look at target','duration':20,'fps':60,'ev100':ev,'sunKelvin':5500,'lights':lights,'shots':shots,'path':samples,'environment':environment,'imports':imports,'triangles':sum(len(p.vertices)-2 for o in scene.objects if o.type=='MESH' for p in o.data.polygons)}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2))
print('ARABENCH_SOURCE '+json.dumps({'scene':name,'triangles':manifest['triangles'],'meshes':sum(o.type=='MESH' for o in scene.objects)}))
