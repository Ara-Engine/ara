"""Validate completed media and publish each encode once."""
from pathlib import Path
import subprocess,json,sys
from call_policy import validate
root=Path(r'D:\AraBench');scene=sys.argv[1] if len(sys.argv)>1 else 'showcase'
links_path=root/'report/media-links.json';links=json.loads(links_path.read_text())if links_path.exists()else{}
for engine in ['ara','unity','unreal']:
    folder=root/'report/videos'/scene/engine;verification=[]
    for kind in ['4k60','1080p60']:
        video=folder/(kind+'.mp4')
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-count_frames','-select_streams','v:0','-show_entries','stream=codec_name,width,height,r_frame_rate,nb_read_frames:format=duration','-of','json',str(video)],text=True))
        stream=probe['streams'][0];assert int(stream['nb_read_frames'])==1200,probe
        assert stream['r_frame_rate']=='60/1'and stream['width']==(3840 if kind=='4k60' else 1920),probe
        assert abs(float(probe['format']['duration'])-20)<.02,probe
        verification.append({'file':str(video),**probe})
        if video.stat().st_size>26_214_400:
            web=folder/(kind+'-web.mp4')
            if not web.exists():subprocess.run(['ffmpeg','-y','-v','warning','-i',str(video),'-c:v','hevc_nvenc','-preset','p6','-rc','cbr','-b:v','8M','-maxrate','8M','-bufsize','16M','-pix_fmt','yuv420p','-tag:v','hvc1','-movflags','+faststart',str(web)],check=True)
            assert web.stat().st_size<26_214_400;video=web
        receipt=folder/(kind+'-upload.json')
        if not receipt.exists():
            result=subprocess.check_output(['pnpm.cmd','dlx','@snowy7/snist','media','upload',str(video),'--alt',scene.title()+' '+engine.title()+' '+kind+' flythrough','--json'],text=True)
            data=json.loads(result);receipt.write_text(json.dumps(data,indent=2))
        raw=receipt.read_bytes();data=json.loads(raw.decode('utf-16'if raw.startswith((b'\xff\xfe',b'\xfe\xff'))else'utf-8-sig'));links.setdefault(scene+'/'+engine,{})[kind]=data['asset']['rawUrl']
        links_path.write_text(json.dumps(links,indent=2));print('PUBLISHED',engine,kind,flush=True)
    (folder/'verification.json').write_text(json.dumps(verification,indent=2))
    if engine=='unreal':
        capture=json.loads((folder/'capture.json').read_text());result=json.loads((folder/'stream-result.json').read_text());assert capture['success']and result['frames']==1200 and result['error']is None
        policy=validate(root/'logs'/(scene+'-unreal-video-guarded.log'));session=json.loads((root/'logs'/(scene+'-unreal-video')/'session.json').read_text());policy['producer_exit_code']=session['exit_code'];policy['note']='All frames and completion callback succeeded; editor exited abnormally during shutdown. Encodes independently decoded and verified; capture is not a timing run.'
        (folder/'capture-policy.json').write_text(json.dumps(policy,indent=2))
