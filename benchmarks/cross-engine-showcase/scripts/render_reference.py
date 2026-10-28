import bpy,sys,json,math,struct,argparse,hashlib
from pathlib import Path
from mathutils import Vector
parser=argparse.ArgumentParser(description='Optional Blender 5.1 Cycles reference renderer; outputs never belong in tracked files.')
parser.add_argument('scene',choices=['showcase','outpost','hangar'])
parser.add_argument('--bench-root',type=Path,default=Path(r'D:\AraBench'))
parser.add_argument('--output-dir',type=Path)
parser.add_argument('--linear',action='store_true',help='Unexposed scene-linear Rec.709 EXR for Ara matched-transform scoring')
parser.add_argument('--samples',type=int,default=128)
parser.add_argument('--shot',action='append')
parser.add_argument('--preview',action='store_true')
parser.add_argument('--save-prepared',action='store_true')
options=parser.parse_args(sys.argv[sys.argv.index('--')+1:])
name=options.scene;root=options.bench_root;out=options.output_dir or root/'report'/'images'/name/'cycles';out.mkdir(parents=True,exist_ok=True)
assert bpy.app.version[:2]==(5,1),f'Blender 5.1 required, found {bpy.app.version_string}'
assert (root/'source'/name/(name+'.blend')).is_file(),f'No Blender source exists for {name}; supply it under {root / "source" / name}'
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
scene.cycles.device='GPU';scene.cycles.samples=options.samples;scene.cycles.use_denoising=True
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
scene.render.resolution_percentage=100
if options.save_prepared:
    scene['ara_bench_reference_prepared']=True;tex.image.pack()
    bpy.ops.wm.save_as_mainfile(filepath=str(root/'source'/name/(name+'.blend')))
if options.preview:scene.render.resolution_percentage=25;scene.cycles.samples=16
if options.linear:
    scene.render.image_settings.file_format='OPEN_EXR';scene.render.image_settings.color_mode='RGB';scene.render.image_settings.color_depth='32';scene.render.image_settings.exr_codec='ZIP'
    scene.view_settings.exposure=0
    if hasattr(scene.render.image_settings,'linear_colorspace_settings'):
        scene.render.image_settings.linear_colorspace_settings.name='Linear Rec.709'
else:scene.render.image_settings.file_format='PNG'
shots=[s for s in m['shots'] if not options.shot or s['name'] in options.shot]
for shot in shots[:1] if options.preview else shots:
    scene.camera=bpy.data.objects['Shot_'+shot['name']]
    scene.render.filepath=str(out/(shot['name']+('-preview' if options.preview else '')+('.exr' if options.linear else '.png')))
    bpy.ops.render.render(write_still=True)
if options.linear:
    policy={'format':'ara.lab.reference/1','scene':name,'blender':bpy.app.version_string,'colour_space':'scene_linear_rec709','exposure_baked':False,'radiance_scale':683,'samples':scene.cycles.samples,'denoising':True,'resolution':[scene.render.resolution_x*scene.render.resolution_percentage//100,scene.render.resolution_y*scene.render.resolution_percentage//100],'source_manifest_sha256':hashlib.sha256((root/'source'/name/'manifest.json').read_bytes()).hexdigest()}
    (out/'linear-reference.json').write_text(json.dumps(policy,indent=2)+'\n')
