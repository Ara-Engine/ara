"""Optional documentation-guard allocation chart; matplotlib is not an engine dependency."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--data', type=Path, default=Path(__file__).with_name('doc_ownership_quality_mac_2026-11-10.json'))
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
rows = json.loads(args.data.read_text())['summary']
plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 11, 'axes.spines.top': False, 'axes.spines.right': False})
fig, ax = plt.subplots(figsize=(10, 6))
for offset, (key, label, color) in enumerate([('baseline', 'Previous text-only check', '#62738e'), ('candidate', 'Correct attachment check', '#007d77')]):
    bars = ax.bar([i + (offset - .5) * .35 for i in range(len(rows))], [r[key] for r in rows], width=.32, color=color, label=label)
    ax.bar_label(bars, labels=[f'{r[key]:,.0f}' for r in rows], padding=5, fontsize=10)
ax.set_yscale('log')
ax.set_ylim(100, 2_000_000)
ax.set_xticks(range(len(rows)), [str(r['fields']) for r in rows])
ax.set_xlabel('Documented fields in a file with an ordinary method-body edit')
ax.set_ylabel('Allocated bytes per guard call (log scale)')
ax.legend(frameon=False, fontsize=10, loc='upper left')
ax.grid(axis='y', alpha=.15)
ax.set_axisbelow(True)
fig.suptitle('Validate documentation attachment without a node dictionary', fontsize=15, fontweight='bold', y=.96)
fig.text(.10, .87, 'Actual method · four fresh processes ABBA · forty allocation samples per arm and case', color='#44546a', fontsize=10)
fig.text(.10, .09, 'One-field guard adds 320 B/call. Successful ordinary L1 edits now calculate the guard once instead of twice.', fontsize=10, color='#44546a')
fig.text(.10, .05, 'Guard phase only; reflection boxing included. No compile latency, p95 or whole-process memory claim.', fontsize=10, color='#44546a')
fig.subplots_adjust(left=.11, right=.97, top=.80, bottom=.23)
args.output.parent.mkdir(parents=True, exist_ok=True)
fig.savefig(args.output, dpi=180)
