"""Run one explicitly owned process and retain its PID, start time and exit status."""
import argparse,subprocess,time,json,os
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--label',required=True);p.add_argument('--timeout',type=int,default=600);p.add_argument('--video-directory');p.add_argument('command',nargs=argparse.REMAINDER);a=p.parse_args()
cmd=a.command[1:] if a.command and a.command[0]=='--' else a.command
root=Path(r'D:\AraBench');dest=root/'logs'/a.label;dest.mkdir(parents=True,exist_ok=True)
started=time.time()
with (dest/'stdout.log').open('w') as output:
    monitor=None
    if os.environ.get('ARABENCH_PRESENTMON')=='1':
        monitor_log=(dest/'presentmon.log').open('w')
        monitor=subprocess.Popen([str(root/'tools'/'PresentMon.exe'),'--process_name',Path(cmd[0]).name,'--output_file',str(dest/'presents.csv'),'--qpc_time_ms','--no_console_stats','--terminate_on_proc_exit','--timed',str(a.timeout),'--terminate_after_timed','--session_name','AraBench-'+a.label],stdout=monitor_log,stderr=subprocess.STDOUT)
        time.sleep(.5)
    start_qpc_ms=time.perf_counter()*1000
    child=subprocess.Popen(cmd,stdout=output,stderr=subprocess.STDOUT)
    record={'pid':child.pid,'command':cmd,'started_epoch':started,'launch_qpc_ms':start_qpc_ms,'environment':{k:v for k,v in os.environ.items() if k.startswith('ARA_BENCH') or k in ['ARA_CONFIG_DIR','ARA_CRASH_DIR','CARGO_BUILD_JOBS']}}
    (dest/'session.json').write_text(json.dumps(record,indent=2))
    stream=None
    if a.video_directory:
        from png_stream import PngStream
        stream=PngStream(a.video_directory,child);stream.start()
    try:
        if stream is None:code=child.wait(timeout=a.timeout)
        else:
            deadline=time.monotonic()+a.timeout
            while child.poll()is None:
                if stream.error:
                    child.terminate()
                    try:child.wait(timeout=10)
                    except subprocess.TimeoutExpired:child.kill();child.wait()
                    raise RuntimeError(stream.error)
                if time.monotonic()>deadline:raise subprocess.TimeoutExpired(cmd,a.timeout)
                time.sleep(.1)
            code=child.returncode;stream.join();record['video_frames']=stream.frames
    except subprocess.TimeoutExpired:
        # Popen keeps an exact process handle. Do not kill descendants or remembered PIDs.
        child.kill();code=child.wait();record['timeout']=True
    except RuntimeError as error:
        record['video_error']=str(error);code=1
    record.update(exit_code=code,elapsed_seconds=time.time()-started)
    (dest/'session.json').write_text(json.dumps(record,indent=2))
    if monitor is not None:
        try:monitor.wait(timeout=10)
        except subprocess.TimeoutExpired:monitor.terminate();monitor.wait(timeout=10)
        monitor_log.close()
print(json.dumps(record));raise SystemExit(code if code>=0 else 1)
