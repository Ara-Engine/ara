"""Diagnostic live-edit allocation ABBA with initial image/IL parity; never a latency claim."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def closure(path):
    return {str(p.relative_to(path)): sha(p) for p in sorted(path.rglob('*')) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ['baseline', 'candidate', 'helper', 'dotnet-host', 'output']:
        parser.add_argument('--' + arg, type=Path, required=True)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    dirs = dict(baseline=args.baseline.resolve(), candidate=args.candidate.resolve(), helper=args.helper.resolve().parent)
    frozen = {name: closure(path) for name, path in dirs.items()}
    report = dict(passed=False, timings_accepted=False, frozen_sha256=frozen, cases=[], script_sha256=sha(Path(__file__)))
    try:
        for name, files, mode in [('one-script', 0, 'single'), ('129-scripts-one-edit', 128, 'single'), ('129-parts-all-edited', 128, 'all')]:
            case = dict(name=name, cells=[])
            report['cases'].append(case)
            reference = None
            for slot, arm in enumerate(['baseline', 'candidate', 'candidate', 'baseline']):
                key = name + '-' + str(slot) + '-' + arm
                bundle = out / key
                shutil.copytree(dirs['helper'], bundle)
                shutil.copytree(dirs[arm], bundle, dirs_exist_ok=True)
                fixture = out / 'PROJECT'
                command = [str(args.dotnet_host.resolve()), str(bundle / args.helper.name), str(fixture), str(files), mode]
                process = subprocess.run(command, capture_output=True, timeout=300,
                                         env={**os.environ, 'DOTNET_PROCESSOR_COUNT': '2'})
                (out / (key + '.stdout')).write_bytes(process.stdout)
                (out / (key + '.stderr')).write_bytes(process.stderr)
                if process.returncode or process.stderr:
                    raise RuntimeError(f'{key}: exit {process.returncode}')
                data = json.loads(process.stdout)
                if len(data['rows']) != 25 or [row['revision'] for row in data['rows']] != list(range(1, 26)):
                    raise RuntimeError('missing/repeated request counters')
                fingerprint = (data['initialPe'], data['initialPdb'], [row['ilSha256'] for row in data['rows']])
                if reference is None:
                    reference = fingerprint
                elif fingerprint != reference:
                    raise RuntimeError('initial image or candidate IL bytes differ')
                case['cells'].append(dict(slot=slot, arm=arm, command=command, data=data,
                                          median_allocation_bytes=statistics.median(row['allocatedBytes'] for row in data['rows'] if not row['warmup'])))
                shutil.rmtree(fixture)
            case['summary'] = {arm: statistics.median(row['allocatedBytes'] for cell in case['cells'] if cell['arm'] == arm
                                                     for row in cell['data']['rows'] if not row['warmup']) for arm in ['baseline', 'candidate']}
            print(json.dumps(dict(case=name, **case['summary'])), flush=True)
        if any(closure(path) != frozen[name] for name, path in dirs.items()):
            raise RuntimeError('frozen SDK/helper bytes changed')
        report['passed'] = True
    except BaseException as error:
        report['failure'] = repr(error)
        raise
    finally:
        (out / 'result.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
