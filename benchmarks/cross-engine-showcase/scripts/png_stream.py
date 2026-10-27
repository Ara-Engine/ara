"""Encode completed PNGs in order, keeping captured frame storage bounded."""
from pathlib import Path
import threading,subprocess,time,json,shutil

class PngStream:
    def __init__(self,directory,producer,count=1200):
        self.directory=Path(directory);self.producer=producer;self.expected=count;self.frames=0;self.error=None;self.process=None
        self.thread=threading.Thread(target=self.run,name='png-video-stream')
    def start(self):self.thread.start()
    def run(self):
        log=None
        try:
            self.directory.mkdir(parents=True,exist_ok=True);partial=self.directory/'4k60.partial.mp4'
            for index in range(self.expected):
                path=self.directory/(str(index).zfill(5)+'.png');missing_since=None
                while True:
                    if shutil.disk_usage(self.directory).free<1_500_000_000:raise RuntimeError('Video scratch disk fell below 1.5 GB; stopping owned capture')
                    if path.exists():
                        try:data=path.read_bytes()
                        except(PermissionError,FileNotFoundError):data=b''
                        if data.endswith(b'IEND\xaeB`\x82'):break
                    if self.producer.poll()is not None:
                        if missing_since is None:missing_since=time.monotonic()
                        if time.monotonic()-missing_since>3:raise RuntimeError('Producer exited before complete PNG '+str(index))
                    time.sleep(.02)
                if self.process is None:
                    log=(self.directory/'encode.log').open('w')
                    self.process=subprocess.Popen(['ffmpeg','-y','-v','warning','-f','image2pipe','-framerate','60','-vcodec','png','-threads','8','-i','pipe:0','-an','-c:v','hevc_nvenc','-preset','p6','-cq','18','-pix_fmt','yuv420p','-tag:v','hvc1','-movflags','+faststart',str(partial)],stdin=subprocess.PIPE,stdout=log,stderr=subprocess.STDOUT)
                self.process.stdin.write(data);self.frames+=1
                for attempt in range(100):
                    try:path.unlink();break
                    except PermissionError:time.sleep(.02)
                (self.directory/'stream-progress.txt').write_text(str(self.frames))
            self.process.stdin.close();code=self.process.wait(timeout=120)
            if code:raise RuntimeError('FFmpeg exited '+str(code))
            partial.replace(self.directory/'4k60.mp4')
        except Exception as error:
            self.error=str(error)
            if self.process is not None and self.process.poll()is None:
                self.process.terminate()
                try:self.process.wait(timeout=10)
                except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        finally:
            if log is not None:log.close()
            (self.directory/'stream-result.json').write_text(json.dumps({'frames':self.frames,'expected':self.expected,'error':self.error,'encoder_pid':self.process.pid if self.process else None,'method':'PNG image2pipe, NVENC HEVC p6 CQ18, yuv420p, 60 Hz'},indent=2))
    def join(self):
        self.thread.join(timeout=130)
        if self.thread.is_alive():raise RuntimeError('Video encoder thread did not finish')
        if self.error:raise RuntimeError(self.error)
