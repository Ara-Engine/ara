"""Import the single Blender export and construct lighting and camera adapters."""
import unreal as u, json, os, math, traceback
from pathlib import Path
root=Path(r'D:\AraBench');name=os.environ.get('ARABENCH_SCENE','showcase')
m=json.loads((root/'source'/name/'manifest.json').read_text())
def pos(p):return u.Vector(p[0]*100,p[2]*100,p[1]*100)
def rot(view):return u.MathLibrary.find_look_at_rotation(pos(view['position']),pos(view['target']))
try:
    levels=u.get_editor_subsystem(u.LevelEditorSubsystem)
    levels.new_level('/Game/Maps/'+name)
    manager=u.InterchangeManager.get_interchange_manager_scripted()
    source=u.InterchangeManager.create_source_data(str(root/'source'/name/(name+'.glb')))
    params=u.ImportAssetParameters();params.is_automated=True;params.replace_existing=True
    assert manager.import_scene('/Game/Imported/'+name,source,params),'glTF scene import failed'
    actors=u.get_editor_subsystem(u.EditorActorSubsystem)
    # Python runs synchronously through import_scene; keep exact imported placements.
    meshes=[a for a in actors.get_all_level_actors() if isinstance(a,u.StaticMeshActor)]
    print('ARABENCH_IMPORTED_MESH_ACTORS',len(meshes))
    for spec in m['lights']:
        cls=u.DirectionalLight if spec['kind']=='directional' else u.SpotLight if spec['kind']=='spot' else u.PointLight
        a=actors.spawn_actor_from_class(cls,pos(spec['position']))
        a.set_actor_label(spec['name'])
        # Convert the common intrinsic XYZ light rotation through its forward vector.
        x,y,z=[math.radians(v) for v in spec['rotation']]
        # Rx*Ry*Rz * (0,0,-1).
        f=[-math.sin(y),math.sin(x)*math.cos(y),-math.cos(x)*math.cos(y)]
        a.set_actor_rotation(u.MathLibrary.find_look_at_rotation(u.Vector(),u.Vector(f[0],f[2],f[1])),False)
        c=a.get_component_by_class(u.LightComponent)
        c.set_mobility(u.ComponentMobility.MOVABLE)
        c.set_light_color(u.LinearColor(*spec['color'],1))
        c.set_intensity(spec['intensity'])
        if isinstance(c,u.LocalLightComponent):
            c.set_editor_property('intensity_units',u.LightUnits.LUMENS)
            c.set_editor_property('attenuation_radius',spec['range']*100)
        if isinstance(c,u.SpotLightComponent):
            c.set_editor_property('inner_cone_angle',spec['innerAngle'])
            c.set_editor_property('outer_cone_angle',spec['outerAngle'])
    pp=actors.spawn_actor_from_class(u.PostProcessVolume,u.Vector())
    pp.set_editor_property('unbound',True)
    settings=pp.get_editor_property('settings')
    overrides={'auto_exposure_method':u.AutoExposureMethod.AEM_MANUAL,'auto_exposure_apply_physical_camera_exposure':False,'auto_exposure_bias':-m['ev100'],'bloom_intensity':.04,'motion_blur_amount':0.0,'dynamic_global_illumination_method':u.DynamicGlobalIlluminationMethod.LUMEN,'reflection_method':u.ReflectionMethod.SCREEN_SPACE,'screen_space_reflection_quality':75.0}
    for k,v in overrides.items():
        settings.set_editor_property('override_'+k,True);settings.set_editor_property(k,v)
    pp.set_editor_property('settings',settings)
    sky=actors.spawn_actor_from_class(u.SkyLight,u.Vector(0,0,1000));sk=sky.get_component_by_class(u.SkyLightComponent)
    sk.set_mobility(u.ComponentMobility.MOVABLE);sk.set_editor_property('real_time_capture',True);sk.set_editor_property('intensity',1.0)
    # Common environment gradient is created as a large unlit sphere by a material.
    tools=u.AssetToolsHelpers.get_asset_tools()
    mat=tools.create_asset('Environment','/Game/Benchmark'if name=='showcase'else'/Game/Benchmark/'+name,u.Material,u.MaterialFactoryNew())
    mat.set_editor_property('two_sided',True);mat.set_editor_property('shading_model',u.MaterialShadingModel.MSM_UNLIT)
    color=u.MaterialEditingLibrary.create_material_expression(mat,u.MaterialExpressionConstant3Vector,0,0)
    color.set_editor_property('constant',u.LinearColor(540,495,450,1))
    u.MaterialEditingLibrary.connect_material_property(color,'',u.MaterialProperty.MP_EMISSIVE_COLOR)
    u.MaterialEditingLibrary.recompile_material(mat)
    sphere=actors.spawn_actor_from_class(u.StaticMeshActor,u.Vector())
    sphere.static_mesh_component.set_static_mesh(u.load_asset('/Engine/BasicShapes/Sphere.Sphere'));sphere.set_actor_scale3d(u.Vector(1000,1000,1000));sphere.static_mesh_component.set_material(0,mat);sphere.static_mesh_component.set_editor_property('cast_shadow',False)
    sky.get_component_by_class(u.SkyLightComponent).recapture_sky()
    camera=actors.spawn_actor_from_class(u.CameraActor,pos(m['path'][0]['position']),rot(m['path'][0]));camera.set_actor_label('Benchmark Camera')
    camera.camera_component.set_editor_property('aspect_ratio',16/9)
    camera.camera_component.set_editor_property('field_of_view',math.degrees(2*math.atan(math.tan(math.radians(m['path'][0]['fov'])/2)*16/9)))
    sequence=tools.create_asset(name+'_flythrough','/Game/Benchmark',u.LevelSequence,u.LevelSequenceFactoryNew())
    sequence.set_display_rate(u.FrameRate(60,1));sequence.set_playback_start(0);sequence.set_playback_end(1200)
    binding=sequence.add_possessable(camera)
    track=binding.add_track(u.MovieScene3DTransformTrack);section=track.add_section();section.set_range(0,1200)
    channels=section.get_all_channels()
    for i,v in enumerate(m['path']):
        p=pos(v['position']);r=rot(v)
        for ch,val in zip(channels,[p.x,p.y,p.z,r.roll,r.pitch,r.yaw,1.,1.,1.]):ch.add_key(u.FrameNumber(i),val,interpolation=u.MovieSceneKeyInterpolation.LINEAR)
    cuts=sequence.add_track(u.MovieSceneCameraCutTrack);cut=cuts.add_section();cut.set_range(0,1200)
    bid=u.MovieSceneObjectBindingID();bid.set_editor_property('guid',binding.get_id());cut.set_camera_binding_id(bid)
    seq_actor=actors.spawn_actor_from_class(u.LevelSequenceActor,u.Vector());seq_actor.set_sequence(sequence)
    playback=seq_actor.get_editor_property('playback_settings');playback.set_editor_property('auto_play',True);playback.set_editor_property('loop_count',u.MovieSceneSequenceLoopCount(-1));seq_actor.set_editor_property('playback_settings',playback)
    for s in m['shots']:
        a=actors.spawn_actor_from_class(u.CameraActor,pos(s['position']),rot(s));a.set_actor_label('Shot_'+s['name']);a.camera_component.set_editor_property('field_of_view',math.degrees(2*math.atan(math.tan(math.radians(s['fov'])/2)*16/9)))
    levels.save_current_level();u.EditorAssetLibrary.save_directory('/Game',False,True)
    (root/'logs'/('unreal-import-'+name+'-success.json')).write_text(json.dumps({'meshActors':len(meshes),'cameraKeys':len(m['path'])}))
except Exception:
    (root/'logs'/('unreal-import-'+name+'-error.txt')).write_text(traceback.format_exc());raise
