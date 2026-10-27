"""Still MRQ jobs preserve temporal AA and the same High settings as the movie."""
import unreal as u,json,os
from unreal_common import high_overrides
from pathlib import Path
root=Path(r'D:\AraBench');name=os.environ.get('ARABENCH_SCENE','showcase');out=root/'report'/'images'/name/'unreal';out.mkdir(parents=True,exist_ok=True)
u.EditorPythonScripting.set_keep_python_script_alive(True)
u.get_editor_subsystem(u.LevelEditorSubsystem).load_level('/Game/Maps/'+name)
actors=u.get_editor_subsystem(u.EditorActorSubsystem).get_all_level_actors();cameras={a.get_actor_label():a for a in actors if isinstance(a,u.CameraActor)}
m=json.loads((root/'source'/name/'manifest.json').read_text());tools=u.AssetToolsHelpers.get_asset_tools()
subsystem=u.get_editor_subsystem(u.MoviePipelineQueueSubsystem);queue=subsystem.get_queue();queue.delete_all_jobs()
for shot in m['shots']:
    asset_name=name+'_still_'+shot['name'];path='/Game/Benchmark/'+asset_name
    if u.EditorAssetLibrary.does_asset_exist(path):u.EditorAssetLibrary.delete_asset(path)
    sequence=tools.create_asset(asset_name,'/Game/Benchmark',u.LevelSequence,u.LevelSequenceFactoryNew())
    sequence.set_display_rate(u.FrameRate(60,1));sequence.set_playback_start(0);sequence.set_playback_end(1)
    binding=sequence.add_possessable(cameras['Shot_'+shot['name']]);cuts=sequence.add_track(u.MovieSceneCameraCutTrack);cut=cuts.add_section();cut.set_range(0,1)
    bid=u.MovieSceneObjectBindingID();bid.set_editor_property('guid',binding.get_id());cut.set_camera_binding_id(bid);u.EditorAssetLibrary.save_asset(path)
    job=queue.allocate_new_job(u.MoviePipelineExecutorJob);job.job_name=shot['name'];job.map=u.SoftObjectPath('/Game/Maps/'+name);job.sequence=u.SoftObjectPath(path)
    config=job.get_configuration();output=config.find_or_add_setting_by_class(u.MoviePipelineOutputSetting);output.output_directory=u.DirectoryPath(str(out));output.file_name_format=shot['name'];output.output_resolution=u.IntPoint(3840,2160);output.use_custom_frame_rate=True;output.output_frame_rate=u.FrameRate(60,1)
    config.find_or_add_setting_by_class(u.MoviePipelineDeferredPassBase);config.find_or_add_setting_by_class(u.MoviePipelineImageSequenceOutput_PNG)
    aa=config.find_or_add_setting_by_class(u.MoviePipelineAntiAliasingSetting);aa.spatial_sample_count=1;aa.temporal_sample_count=1;aa.engine_warm_up_count=120;aa.render_warm_up_count=32;aa.render_warm_up_frames=True
    high_overrides(config)
def finished(executor,success):
    (out/'capture.json').write_text(json.dumps({'success':success,'method':'MRQ','width':3840,'height':2160}));u.SystemLibrary.quit_editor()
executor=u.MoviePipelinePIEExecutor();executor.on_executor_finished_delegate.add_callable_unique(finished);subsystem.render_queue_with_executor_instance(executor)
