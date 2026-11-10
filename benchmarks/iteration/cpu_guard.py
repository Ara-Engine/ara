"""Observe compiler presence and cumulative VBCS CPU consumption without stopping tools."""
from decimal import Decimal, InvalidOperation
import os
from pathlib import Path
import subprocess
import time

BUILD_TOOLS = {'cargo', 'rustc', 'clippy-driver', 'clang', 'clang++', 'cc', 'c++',
               'gcc', 'g++', 'ld', 'ld64', 'lld', 'dsymutil', 'swiftc', 'dotnet', 'msbuild'}


def cpu_nanoseconds(raw):
    """Parse ps cumulative [[days-]hours:]minutes:seconds.fraction exactly."""
    days = 0
    clock = raw
    if '-' in clock:
        day, clock = clock.split('-', 1)
        days = int(day)
    parts = clock.split(':')
    if len(parts) not in (2, 3):
        raise ValueError('unsupported cumulative ps CPU clock: ' + raw)
    try:
        seconds = Decimal(parts[-1]) + Decimal(int(parts[-2]) * 60 + days * 86400)
        if len(parts) == 3:
            seconds += Decimal(int(parts[0]) * 3600)
    except (ValueError, InvalidOperation) as error:
        raise ValueError('invalid cumulative ps CPU clock: ' + raw) from error
    if seconds < 0:
        raise ValueError('negative cumulative CPU clock')
    return int(seconds * 1_000_000_000)


def resources():
    blockers = []
    daemons = []
    errors = []
    rows = subprocess.check_output(['ps', '-axo', 'pid=,pcpu=,time=,comm='], text=True)
    for line in rows.splitlines():
        parts = line.split(None, 3)
        if len(parts) != 4:
            continue
        pid, cpu, cumulative, command = parts
        leaf = Path(command).name.lower()
        compiler = leaf in BUILD_TOOLS or 'compileserver' in leaf or leaf.startswith('vbcscompiler')
        test = '/target/' in command and any(name in command for name in ('/deps/', '/examples/'))
        if not compiler and not (test and float(cpu) > 1):
            continue
        argv = subprocess.run(['ps', '-p', pid, '-o', 'command='], capture_output=True, text=True).stdout.strip()
        row = {'pid': int(pid), 'pcpu_diagnostic_only': float(cpu), 'comm': command, 'argv': argv,
               'ps_cumulative_cpu': cumulative}
        vbcs = leaf == 'dotnet' and 'vbcscompiler.dll' in argv.lower()
        if vbcs:
            identity = subprocess.run(['ps', '-p', pid, '-o', 'lstart='], capture_output=True, text=True).stdout.strip()
            if not identity:
                errors.append({'kind': 'daemon_disappeared_during_sample', **row})
                continue
            try:
                elapsed = cpu_nanoseconds(cumulative)
            except ValueError as error:
                errors.append({'kind': 'unreadable_daemon_cpu_time', 'error': str(error), **row})
                continue
            daemons.append({**row, 'start_identity': identity, 'cpu_nanoseconds': elapsed})
        else:
            blockers.append(row)
    return {'loadavg': list(os.getloadavg()), 'busy_tools': blockers, 'vbcs_daemons': daemons,
            'sample_errors': errors, 'power': subprocess.check_output(['pmset', '-g', 'batt'], text=True).strip(),
            'sample_monotonic': time.monotonic(),
            'cpu_time_scope': 'ps time is cumulative user+system CPU, with reported fractional precision; pcpu never permits a daemon'}


class CumulativeGuard:
    """Only a fixed pre-existing daemon identity with zero cumulative CPU delta is allowed."""
    def __init__(self):
        self.initial = None
        self.rejected = False

    def inspect(self, snapshot):
        errors = list(snapshot['sample_errors'])
        errors.extend({'kind': 'competing_tool', 'tool': row} for row in snapshot['busy_tools'])
        current = {row['pid']: row for row in snapshot['vbcs_daemons']}
        if self.initial is None:
            self.initial = current
        else:
            for pid, row in current.items():
                old = self.initial.get(pid)
                if old is None:
                    errors.append({'kind': 'new_compiler_daemon', 'daemon': row})
                elif old['start_identity'] != row['start_identity']:
                    errors.append({'kind': 'compiler_daemon_identity_changed', 'before': old, 'after': row})
                elif row['cpu_nanoseconds'] != old['cpu_nanoseconds']:
                    errors.append({'kind': 'compiler_daemon_cpu_time_changed', 'pid': pid,
                                   'before_cpu': old['ps_cumulative_cpu'], 'after_cpu': row['ps_cumulative_cpu'],
                                   'delta_nanoseconds': row['cpu_nanoseconds'] - old['cpu_nanoseconds'],
                                   'pcpu_diagnostic_only': row['pcpu_diagnostic_only']})
            for pid in self.initial.keys() - current.keys():
                errors.append({'kind': 'compiler_daemon_disappeared', 'pid': pid})
        self.rejected = self.rejected or bool(errors)
        return errors
