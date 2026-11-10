"""Optional actual-CLI allocation-counter chart; matplotlib is not an engine dependency."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--data', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--title', required=True)
parser.add_argument('--candidate-label', required=True)
args = parser.parse_args()
data = json.loads(args.data.read_text())
plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 11, 'axes.spines.top': False, 'axes.spines.right': False})
fig, axes = plt.subplots(1, 2, figsize=(11, 6))
for ax, field, title in zip(axes, ['median_allocation_bytes', 'median_final_heap_bytes'], ['Allocation per edited request', 'Final live heap: two processes per arm']):
    values = [data['summary'][arm][field] / 1e6 for arm in ['baseline', 'candidate']]
    bars = ax.bar(['Previous', args.candidate_label], values, color=['#62738e', '#007d77'], width=.6)
    ax.bar_label(bars, labels=[f'{value:.2f} MB' for value in values], padding=6)
    ax.set_ylim(0, max(values) * 1.2)
    ax.set_title(title, fontsize=11, pad=16)
    ax.set_ylabel('Decimal MB')
    ax.grid(axis='y', alpha=.15)
    ax.set_axisbelow(True)
for i, arm in enumerate(['baseline', 'candidate']):
    heaps = data['summary'][arm]['final_heap_bytes']
    axes[1].scatter([i] * len(heaps), [value / 1e6 for value in heaps], s=30, c='#b36b28', zorder=3)
fig.suptitle(args.title, fontsize=15, fontweight='bold', y=.96)
fig.text(.08, .86, 'Actual instrumented compiler CLI · 1,024 scripts / 8 modules · ABBA · 20 measured requests per arm', color='#44546a', fontsize=10)
fig.text(.08, .09, '360 exact PE/PDB/schema module comparisons. Final heap is forced-GC state; two observations per arm.', fontsize=10, color='#44546a')
fig.text(.08, .05, 'Allocation counters only. Hook overhead and shared-host activity invalidate latency and p95 conclusions.', fontsize=10, color='#44546a')
fig.subplots_adjust(left=.08, right=.97, top=.76, bottom=.23, wspace=.35)
args.output.parent.mkdir(parents=True, exist_ok=True)
fig.savefig(args.output, dpi=180)
