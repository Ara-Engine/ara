"""Build the frozen library with today's identical timing harness, inside perf-local.

Run from crates/ara-anim. Requires the baseline commit in the local repository.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--commit",default="4e01c27501da1f25f28f2d26db81371511068dad")
    args=parser.parse_args()
    crate=Path(__file__).resolve().parent.parent
    root=crate.parent.parent
    workspace=crate/"perf-local/baseline-workspace"
    workspace.mkdir(parents=True,exist_ok=True)
    manifest=(root/"Cargo.toml").read_text().replace('members = ["crates/*"]','members = ["ara-anim"]')
    (workspace/"Cargo.toml").write_text(manifest,encoding="utf-8",newline="\n")
    files=subprocess.check_output(["git","ls-tree","-r","--name-only",args.commit,"crates/ara-anim"],cwd=root,text=True).splitlines()
    for name in files:
        relative=Path(name).relative_to("crates/ara-anim")
        destination=workspace/"ara-anim"/relative
        destination.parent.mkdir(parents=True,exist_ok=True)
        destination.write_bytes(subprocess.check_output(["git","show",args.commit+":"+name],cwd=root))
    path=workspace/"ara-anim/Cargo.toml"
    manifest=path.read_text()
    for name in ["ara-schema","ara-json","ara-diag","ara-doc","ara-ops"]:
        manifest=manifest.replace("../"+name,(root/"crates"/name).as_posix())
    path.write_text(manifest,encoding="utf-8",newline="\n")
    for name in ["stages.rs","crowd.rs"]:
        source=(crate/"examples"/name).read_text()
        if name=="crowd.rs":
            source=source.replace('    println!("runtime clip bytes {}", clip.runtime_bytes());\n',"").replace("let (clip, report) =","let (_, report) =")
        (workspace/"ara-anim/examples"/name).write_text(source,encoding="utf-8",newline="\n")
    shutil.copyfile(root/"Cargo.lock",workspace/"Cargo.lock")
    environment=dict(os.environ,CARGO_BUILD_JOBS="2")
    subprocess.run(["cargo","build","--manifest-path",str(workspace/"Cargo.toml"),"-p","ara-anim","--release","--example","stages","--example","crowd"],cwd=root,env=environment,check=True)
    for name in ["stages","crowd"]:
        shutil.copyfile(workspace/"target/release/examples"/(name+".exe"),crate/"perf-local"/("baseline-"+name+".exe"))

if __name__=="__main__":
    main()
