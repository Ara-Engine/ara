import unreal as u,os,json,traceback
from unreal_common import high_overrides
from pathlib import Path
root=Path(r'D:\AraBench');name=os.environ.get('ARABENCH_SCENE','showcase');out=root/'report'/'videos'/name/'unreal';out.mkdir(parents=True,exist_ok=True)
u.EditorPythonScripting.set_keep_python_script_alive(True)
levels=u.get_editor_subsystem(u.LevelEditorSubsystem);levels.load_level('/Game/Maps/'+name)
subsystem=u.get_editor_subsystem(u.MoviePipelineQueueSubsystem);queue=subsystem.get_queue();queue.delete_all_jobs()
job=queue.allocate_new_job(u.MoviePipelineExecutorJob);job.job_name=name;job.map=u.SoftObjectPath('/Game/Maps/'+name);job.sequence=u.SoftObjectPath('/Game/Benchmark/'+name+'_flythrough')
config=job.get_configuration();output=config.find_or_add_setting_by_class(u.MoviePipelineOutputSetting);output.output_directory=u.DirectoryPath(str(out));output.file_name_format='{frame_number}';output.output_resolution=u.IntPoint(3840,2160);output.use_custom_frame_rate=True;output.output_frame_rate=u.FrameRate(60,1);output.zero_pad_frame_numbers=5
config.find_or_add_setting_by_class(u.MoviePipelineDeferredPassBase);config.find_or_add_setting_by_class(u.MoviePipelineImageSequenceOutput_PNG)
aa=config.find_or_add_setting_by_class(u.MoviePipelineAntiAliasingSetting);aa.spatial_sample_count=1;aa.temporal_sample_count=1;aa.engine_warm_up_count=120;aa.render_warm_up_count=32;aa.render_warm_up_frames=True
high_overrides(config)

# MRQ owns sequence time. Stop the map's runtime autoplay player from also driving
# the same possessable camera; the packaged map remains unchanged on disk.
actors=u.get_editor_subsystem(u.EditorActorSubsystem);restore={}
for actor in actors.get_all_level_actors():
    if isinstance(actor,u.LevelSequenceActor):
        settings=actor.get_editor_property('playback_settings');restore[actor.get_name()]=settings.get_editor_property('auto_play');settings.set_editor_property('auto_play',False);actor.set_editor_property('playback_settings',settings)
levels.save_current_level()
def finished(executor,success):
    for actor in actors.get_all_level_actors():
        if isinstance(actor,u.LevelSequenceActor)and actor.get_name()in restore:
            settings=actor.get_editor_property('playback_settings');settings.set_editor_property('auto_play',restore[actor.get_name()]);actor.set_editor_property('playback_settings',settings)
    levels.save_current_level()
    (out/'capture.json').write_text(json.dumps({'success':success,'fps':60,'frames':1200,'width':3840,'height':2160}));u.SystemLibrary.quit_editor()
executor=u.MoviePipelinePIEExecutor();executor.on_executor_finished_delegate.add_callable_unique(finished);subsystem.render_queue_with_executor_instance(executor)
