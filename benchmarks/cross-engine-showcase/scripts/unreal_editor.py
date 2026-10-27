import unreal as u,json,math,time,os,traceback
from pathlib import Path
root=Path(r'D:\AraBench');name=os.environ.get('ARABENCH_SCENE','showcase');m=json.loads((root/'source'/name/'manifest.json').read_text());out=Path(os.environ.get('ARABENCH_EDITOR_OUTPUT',str(root/'measurements'/name/'unreal'/'editor')));out.mkdir(parents=True,exist_ok=True)
u.EditorPythonScripting.set_keep_python_script_alive(True)
level=u.get_editor_subsystem(u.LevelEditorSubsystem);level.load_level('/Game/Maps/'+name);level.editor_set_viewport_realtime(True)
editor=u.get_editor_subsystem(u.UnrealEditorSubsystem)
from unreal_common import HIGH_CVARS
for cvar,value in HIGH_CVARS.items():u.SystemLibrary.execute_console_command(editor.get_editor_world(),cvar+' '+str(value))
start=time.perf_counter();last=None;frames=[];plays=[];stage='orbit';requested=0;finished=0
def tick(dt):
    global last,stage,requested,finished
    try:
        now=time.perf_counter()
        if stage=='orbit':
            t=now-start;v=m['path'][min(1199,int(t*60))];p=u.Vector(v['position'][0]*100,v['position'][2]*100,v['position'][1]*100);target=u.Vector(v['target'][0]*100,v['target'][2]*100,v['target'][1]*100)
            editor.set_level_viewport_camera_info(p,u.MathLibrary.find_look_at_rotation(p,target))
            if t>2 and last is not None:frames.append((now-last)*1000)
            last=now
            if t>12:
                (out/'scene-view-ms.json').write_text(json.dumps(frames));stage='enter';requested=now;level.editor_request_begin_play()
        elif stage=='enter':
            world=editor.get_game_world()
            if world:
                # First observed PIE world tick, with a separate marker after one rendered editor tick.
                plays.append({'request_to_observed_world_ms':(now-requested)*1000});stage='frame';finished=now
            elif now-requested>120:raise RuntimeError('PIE timed out')
        elif stage=='frame':
            plays[-1]['request_to_next_editor_frame_ms']=(now-requested)*1000;level.editor_request_end_play();stage='exit';finished=now
        elif stage=='exit' and editor.get_game_world() is None and now-finished>1:
            if len(plays)<3:stage='enter';requested=now;level.editor_request_begin_play()
            else:
                (out/'play-entry.json').write_text(json.dumps(plays,indent=2));u.unregister_slate_post_tick_callback(handle);u.SystemLibrary.quit_editor()
    except Exception:
        (out/'error.txt').write_text(traceback.format_exc());u.unregister_slate_post_tick_callback(handle);u.SystemLibrary.quit_editor()
handle=u.register_slate_post_tick_callback(tick)
