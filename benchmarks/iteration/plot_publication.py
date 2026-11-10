"""Optional publication-discovery allocation chart; needs matplotlib, not an engine dependency."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--data', type=Path, default=Path(__file__).with_name('publication_discovery_mac_2026-11-10.json'))
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
data = json.loads(args.data.read_text())
rows = data['summary']
plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 11, 'axes.spines.top': False, 'axes.spines.right': False})
fig, ax = plt.subplots(figsize=(10, 6))
colors = ['#62738e', '#007d77', '#c19247']
for offset, (key, label, color) in enumerate(zip(
    ['baseline_public', 'candidate_gameplay-discovery', 'candidate_public'],
    ['Previous discovery', 'New build discovery', 'Public inventory after change'], colors)):
    positions = [i + (offset - 1) * .24 for i in range(len(rows))]
    values = [row[key] for row in rows]
    bars = ax.bar(positions, values, width=.22, label=label, color=color)
    ax.bar_label(bars, labels=[f'{value:,.0f}' for value in values], padding=5, fontsize=10)
ax.set_xticks(range(len(rows)), ['One source / one module', 'Outpost: four sources / two modules'])
ax.set_ylim(0, 70000)
ax.set_ylabel('Allocated bytes per discovery call')
ax.grid(axis='y', alpha=.15)
ax.set_axisbelow(True)
ax.legend(frameon=False, fontsize=9, loc='upper left')
fig.suptitle('Skip duplicate publication hydration during preparation', fontsize=16, fontweight='bold', y=.96)
fig.text(.10, .87, 'Actual method · real Rust-published fixtures · four fresh processes ABBA', color='#44546a')
fig.text(.10, .09, 'Discovery phase only. Compilation still verifies current inputs and schema/PE/PDB/snapshot hashes.', fontsize=10, color='#44546a')
fig.text(.10, .05, 'Public/ownership responses match exactly. No compile-time, time-to-Play or p95 claim.', fontsize=10, color='#44546a')
fig.subplots_adjust(left=.11, right=.97, top=.80, bottom=.20)
args.output.parent.mkdir(parents=True, exist_ok=True)
fig.savefig(args.output, dpi=180)
