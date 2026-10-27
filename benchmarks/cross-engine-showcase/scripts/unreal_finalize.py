import unreal as u, json, os, math,traceback
from pathlib import Path
root=Path(r'D:\AraBench');name=os.environ.get('ARABENCH_SCENE','showcase');m=json.loads((root/'source'/name/'manifest.json').read_text())
try:
    levels=u.get_editor_subsystem(u.LevelEditorSubsystem);levels.load_level('/Game/Maps/'+name)
    actors=u.get_editor_subsystem(u.EditorActorSubsystem)
    folder='/Game/Benchmark'if name=='showcase'else'/Game/Benchmark/'+name
    hdr=root/'source'/name/'environment.hdr';hdr=hdr if hdr.exists()else root/'source/environment.hdr'
    task=u.AssetImportTask();task.filename=str(hdr);task.destination_path=folder;task.destination_name='SkyHDR';task.automated=True;task.save=True;task.replace_existing=True
    u.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task]);env=u.load_asset(folder+'/SkyHDR')
    assert isinstance(env,u.TextureCube),type(env)
    mat=u.load_asset(folder+'/Environment')
    mat.set_editor_property('is_sky',True)
    u.MaterialEditingLibrary.delete_all_material_expressions(mat)
    sample=u.MaterialEditingLibrary.create_material_expression(mat,u.MaterialExpressionTextureSampleParameterCube,0,0);sample.set_editor_property('texture',env)
    vector=u.MaterialEditingLibrary.create_material_expression(mat,u.MaterialExpressionCameraVectorWS,-400,0)
    negate=u.MaterialEditingLibrary.create_material_expression(mat,u.MaterialExpressionMultiply,-200,0);negate.set_editor_property('const_b',-1.0)
    sample.set_editor_property('sampler_type',u.MaterialSamplerType.SAMPLERTYPE_LINEAR_COLOR)
    assert u.MaterialEditingLibrary.connect_material_expressions(vector,'',negate,'A')
    assert u.MaterialEditingLibrary.connect_material_expressions(negate,'',sample,'UVs')
    assert u.MaterialEditingLibrary.connect_material_property(sample,'RGB',u.MaterialProperty.MP_EMISSIVE_COLOR)
    u.MaterialEditingLibrary.recompile_material(mat)
    for a in actors.get_all_level_actors():
        for spec in m['lights']:
            if a.get_actor_label()==spec['name'] and spec['kind']!='directional':
                c=a.get_component_by_class(u.LocalLightComponent);c.set_editor_property('intensity_units',u.LightUnits.CANDELAS);c.set_intensity(spec['intensity']/(4*math.pi))
        if isinstance(a,u.SkyLight):
            c=a.get_component_by_class(u.SkyLightComponent);c.set_editor_property('source_type',u.SkyLightSourceType.SLS_SPECIFIED_CUBEMAP);c.set_editor_property('cubemap',env);c.set_editor_property('real_time_capture',False);c.set_editor_property('lower_hemisphere_is_black',False);c.set_editor_property('intensity',1.0);c.recapture_sky()
        if isinstance(a,u.StaticMeshActor) and a.static_mesh_component.get_material(0)==mat:
            a.static_mesh_component.set_editor_property('affect_distance_field_lighting',False)
            a.static_mesh_component.set_editor_property('affect_dynamic_indirect_lighting',False)
            a.static_mesh_component.set_editor_property('cast_shadow',False)
        if isinstance(a,u.StaticMeshActor):
            component=a.static_mesh_component
            if any(component.get_material(i) and component.get_material(i).get_base_material().get_editor_property('blend_mode') not in [u.BlendMode.BLEND_OPAQUE,u.BlendMode.BLEND_MASKED] for i in range(component.get_num_materials())):
                component.set_editor_property('disallow_nanite',True)
    u.SystemLibrary.execute_console_command(None,'r.EyeAdaptation.LensAttenuation 0.65')
    levels.save_current_level();u.EditorAssetLibrary.save_directory('/Game',False,True)
    (root/'logs'/('unreal-finalize-'+name+'.json')).write_text(json.dumps({'environment':env.get_path_name()}))
except Exception:(root/'logs'/('unreal-finalize-'+name+'-error.txt')).write_text(traceback.format_exc());raise
