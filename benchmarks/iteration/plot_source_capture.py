"""Optional allocation graph; requires matplotlib, never part of an engine build."""
from pathlib import Path
import json,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
import argparse
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--data',type=Path,default=Path(__file__).with_name('source_capture_allocations_mac_2026-11-10.json'))
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
d=json.loads(args.data.read_text())
args.output.parent.mkdir(parents=True,exist_ok=True)
values={}
for i,row in enumerate(d['processes'][0]['rows']):
 values[(row['utf16UnitsPerFile'],row['scenario'],row['edit'])]={arm:statistics.median([v for p in d['processes'] if p['arm']==arm for v in p['rows'][i]['allocatedBytesPerSet']])/row['files'] for arm in ['baseline','candidate']}
plt.rcParams.update({'font.family':'DejaVu Sans','font.size':10,'axes.spines.top':False,'axes.spines.right':False,'figure.facecolor':'#f5f7fb','axes.facecolor':'#f5f7fb'})
fig,(ax,table)=plt.subplots(1,2,figsize=(13,6.2),gridspec_kw={'width_ratios':[1,1.1]})
fig.suptitle('Verified source capture: fewer temporary allocations',fontsize=18,fontweight='bold',x=.06,ha='left',y=.97)
fig.text(.06,.90,'Actual production read/hash method · four fresh processes ABBA · 20 counter samples per case/process',color='#44546a')
sizes=[384,16384,524288];labels=['384 UTF-16 units','16,384 units','524,288 units'];ys=np.arange(3)
base=[values[s,'unchanged','none']['baseline'] for s in sizes];now=[values[s,'unchanged','none']['candidate'] for s in sizes]
ax.barh(ys+.17,base,height=.28,color='#62738e',label='Previous capture')
ax.barh(ys-.17,now,height=.28,color='#007d77',label='Reuse after full EOF match')
ax.set_yticks(ys,labels);ax.invert_yaxis();ax.set_xscale('log');ax.set_xlim(1000,9_000_000)
ax.set_xlabel('Allocated bytes per unchanged file (log scale)');ax.set_title('Unchanged files',loc='left',fontweight='bold',pad=16)
for i,(a,b) in enumerate(zip(base,now)):
 ax.text(a*1.12,i+.17,f'{a:,.0f}',va='center',fontsize=9)
 ax.text(b*1.12,i-.17,f'{b:,.0f}',va='center',fontsize=9,color='#00655e')
fig.legend(*ax.get_legend_handles_labels(),loc='upper left',bbox_to_anchor=(.13,.885),ncol=2,frameon=False,fontsize=9);ax.grid(axis='x',alpha=.15);ax.set_axisbelow(True)
table.axis('off');table.set_title('All files changed: allocation change',loc='left',fontweight='bold',pad=16)
edits=['early','middle','late','replacement']
rows=[];colors=[]
for size in sizes:
 row=[];cs=[]
 for edit in edits:
  v=values[size,'all_changed',edit];delta=100*(v['candidate']/v['baseline']-1)
  row.append(f'{delta:+.2f}%');cs.append('#f9e4df' if delta>0 else '#dceee8')
 rows.append(row);colors.append(cs)
t=table.table(cellText=rows,rowLabels=['384','16K','512K'],colLabels=['Early edit','Middle','Near EOF','Replace text'],cellColours=colors,loc='center',cellLoc='center',bbox=[.14,.35,.86,.40])
t.auto_set_font_size(False);t.set_fontsize(10)
for (r,c),cell in t.get_celld().items():
 cell.set_edgecolor('#f5f7fb');cell.set_linewidth(3)
 if r==0 or c==-1:cell.set_facecolor('#e7ebf2');cell.set_text_props(weight='bold',fontsize=9)
table.text(.14,.21,'Negative = fewer allocated bytes. Some early/full edits\nallocate slightly more; no universal changed-file win.',transform=table.transAxes,fontsize=10,color='#44546a')
fig.text(.06,.10,'No compile-time or p95 claim. Inputs are synthetic decoded text; every character is still read and every hash verified.',fontsize=10,color='#44546a')
fig.text(.06,.055,'Pools/JIT warmed. “Cold” means no prior text; its existing read path remains, with 8 extra closure bytes per capture set.',fontsize=9,color='#44546a')
fig.subplots_adjust(left=.13,right=.98,bottom=.24,top=.78,wspace=.45)
fig.savefig(args.output,dpi=180)
