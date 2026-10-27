"""Keep timing executables immutable while adding a temporal-history-preserving readback."""
from pathlib import Path
import os,subprocess,shutil,hashlib,json,sys
root=Path(r'D:\AraBench');scene=sys.argv[1]if len(sys.argv)>1 else'showcase'
def digest(path):
    h=hashlib.sha256()
    with path.open('rb')as file:
        for chunk in iter(lambda:file.read(1024*1024),b''):h.update(chunk)
    return h.hexdigest()
timing=root/'builds/ara'/scene/'build/ara-player.exe';before=digest(timing);binary=root/'capture-bin/ara-player.exe'
if not binary.exists()or'--rebuild'in sys.argv:
    source=root/'ara-source/crates/ara-player/src/standalone/benchmark.rs';archive=root/'report/harness';archive.mkdir(exist_ok=True)
    if not(archive/'timing-ara-benchmark.rs').exists():shutil.copyfile(source,archive/'timing-ara-benchmark.rs')
    shutil.copyfile(root/'scripts/ara_benchmark.rs',source);subprocess.run(['rustfmt','--edition','2024',str(source)],check=True)
    os.environ.update(CARGO_BUILD_JOBS='8',CARGO_TARGET_DIR=str(root/'ara-target'),ARA_CONFIG_DIR=str(root/'profiles/capture-build'),ARA_CRASH_DIR=str(root/'profiles/crashes'))
    with(root/'logs/capture-player-build.log').open('w')as log:subprocess.run([str(root/'infra-target/debug/ara-dev.exe'),'cargo','--','build','--release','-p','ara-editor'],cwd=root/'ara-source',stdout=log,stderr=subprocess.STDOUT,check=True)
    binary.parent.mkdir(exist_ok=True);shutil.copyfile(root/'ara-target/release/ara-player.exe',binary)
destination=root/'capture-builds/ara'/scene;assert destination.resolve().is_relative_to(root.resolve())
if not destination.exists():shutil.copytree(timing.parent,destination,copy_function=os.link)
target=destination/'ara-player.exe'
if target.exists():target.unlink() # Remove the owned hardlink before replacing the capture executable.
shutil.copyfile(binary,target);assert digest(timing)==before,'Timing executable changed'
(destination/'capture-build.json').write_text(json.dumps({'source_commit':'13e8b9da83c0fdd36d0d12ea3a7fb8ce7a819c23','timing_executable_sha256':before,'capture_executable_sha256':digest(target),'difference':'Benchmark-only video readback preserves temporal history. Renderer algorithms and packaged content are unchanged. Resources are shared with immutable hardlinks.'},indent=2))
print('CAPTURE_PLAYER_READY',target,flush=True)
