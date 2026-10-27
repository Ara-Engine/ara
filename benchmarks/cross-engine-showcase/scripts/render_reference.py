import bpy,sys,json,math,struct
from pathlib import Path
from mathutils import Vector
name=sys.argv[sys.argv.index('--')+1];root=Path(r'D:\AraBench');out=root/'report'/'images'/name/'cycles';out.mkdir(parents=True,exist_ok=True)
bpy.ops.wm.open_mainfile(filepath=str(root/'source'/name/(name+'.blend')))
scene=bpy.context.scene;m=json.loads((root/'source'/name/'manifest.json').read_text())
for spec in m['lights']:
    light=bpy.data.objects[spec['name']];x,y,z=[math.radians(v) for v in spec['rotation']]
    forward=Vector((-math.sin(y),math.cos(x)*math.cos(y),math.sin(x)*math.cos(y)))
    light.location=Vector((spec['position'][0],-spec['position'][2],spec['position'][1]))
    light.rotation_euler=forward.to_track_quat('-Z','Y').to_euler();light.data.normalize=True
print('SOURCE_VISIBLE_MESHES',sum(o.type=='MESH' and not o.hide_render for o in scene.objects))
print('SOURCE_HIDDEN_MESHES',[(o.name,len(o.data.polygons)) for o in scene.objects if o.type=='MESH' and o.hide_render])
with (root/'source'/name/(name+'.glb')).open('rb') as file:
    file.read(12);length,kind=struct.unpack('<II',file.read(8));gltf=json.loads(file.read(length))
exported={n.get('name')for n in gltf['nodes']if 'mesh'in n}
audit={'source_mesh_objects':sum(o.type=='MESH'for o in scene.objects),'exported_mesh_nodes':len(exported),'source_meshes_without_exported_node':[{'name':o.name,'vertices':len(o.data.vertices),'polygons':len(o.data.polygons),'hidden_viewport':o.hide_get(),'hidden_render':o.hide_render}for o in scene.objects if o.type=='MESH'and o.name not in exported],'light_forward_blender':{spec['name']:list(bpy.data.objects[spec['name']].rotation_euler.to_matrix()@Vector((0,0,-1)))for spec in m['lights']}}
(out/'reference-audit.json').write_text(json.dumps(audit,indent=2));print('REFERENCE_AUDIT',json.dumps(audit))
world=scene.world;world.use_nodes=True;nodes=world.node_tree.nodes;nodes.clear()
tex=nodes.new('ShaderNodeTexEnvironment');environment=root/'source'/name/'environment.hdr';tex.image=bpy.data.images.load(str(environment if environment.exists() else root/'source/environment.hdr'))
bg=nodes.new('ShaderNodeBackground');bg.inputs['Strength'].default_value=1/683
output=nodes.new('ShaderNodeOutputWorld');world.node_tree.links.new(tex.outputs['Color'],bg.inputs['Color']);world.node_tree.links.new(bg.outputs['Background'],output.inputs['Surface'])
prefs=bpy.context.preferences.addons['cycles'].preferences
prefs.compute_device_type='CUDA';prefs.get_devices()
for device in prefs.devices:device.use=device.type=='CUDA'
print('DEVICES',[(d.name,d.type,d.use) for d in prefs.devices])
scene.cycles.device='GPU';scene.cycles.samples=128;scene.cycles.use_denoising=True
scene.view_settings.exposure=math.log2(683/(1.2*2**m['ev100']))
# glTF emissive RGB is radiance in the common photometric contract, not W/m2.
if not scene.get('ara_bench_reference_prepared',False):
    for mat in bpy.data.materials:
        if mat.use_nodes:
            for node in mat.node_tree.nodes:
                if node.type=='BSDF_PRINCIPLED':node.inputs['Emission Strength'].default_value/=683
for spec in m['lights']:
    if spec['kind']=='spot':
        light=bpy.data.objects[spec['name']].data
        outer=math.radians(spec['outerAngle']);inner=math.radians(spec['innerAngle'])
        light.spot_blend=(math.cos(inner)-math.cos(outer))/(1-math.cos(outer))
scene.render.resolution_x=3840;scene.render.resolution_y=2160
if '--save-prepared' in sys.argv:
    scene['ara_bench_reference_prepared']=True;tex.image.pack()
    bpy.ops.wm.save_as_mainfile(filepath=str(root/'source'/name/(name+'.blend')))
if '--preview' in sys.argv:scene.render.resolution_percentage=25;scene.cycles.samples=16
for shot in m['shots'][:1] if '--preview' in sys.argv else m['shots']:
    scene.camera=bpy.data.objects['Shot_'+shot['name']]
    scene.render.filepath=str(out/(shot['name']+('-preview' if '--preview' in sys.argv else '')+'.png'))
    bpy.ops.render.render(write_still=True)
