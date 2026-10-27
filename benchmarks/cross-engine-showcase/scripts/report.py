"""Generate the self-contained report from accepted raw-derived summaries."""
from pathlib import Path
import json,html,base64,io,statistics,datetime
from PIL import Image
root=Path(r'D:\AraBench');out=root/'report'
def load(name,fallback):
    path=out/name
    return json.loads(path.read_text()) if path.exists() else fallback
runtime=load('runtime-summary.json',[]);quality=load('quality-metrics.json',[]);editors=load('editor-summary.json',[]);startup=load('startup-summary.json',[]);media=load('media-links.json',{});notes=load('findings.json',{});call=load('call-policy.json',{});scalability=load('unreal-scalability.json',{})
notes.setdefault('limits',[]).append('Snist media has a 25 MB upload limit. Published 4K delivery videos use 8 Mbit/s HEVC; the higher-quality CQ18 4K originals and H.264 1080p copies remain under D:\\AraBench\\report\\videos. Both versions retain 1,200 frames at 60 Hz.')
image_links=load('image-links.json',{})
scenes=['showcase'];visuals=load('visual-findings.json',[])
def esc(value):return html.escape(str(value))
def number(value,digits=2):return '—' if value is None else f'{value:.{digits}f}'
def table(headers,rows):
    return '<div class="table"><table><thead><tr>'+''.join('<th>'+esc(x)+'</th>' for x in headers)+'</tr></thead><tbody>'+''.join('<tr>'+''.join('<td>'+str(c)+'</td>' for c in row)+'</tr>' for row in rows)+'</tbody></table></div>'
def image(path):
    # Cache only the report's compressed display copy. Metrics always use original PNGs.
    cache=path.with_suffix('.preview.webp')
    if not cache.exists() or cache.stat().st_mtime<path.stat().st_mtime:Image.open(path).convert('RGB').resize((960,540),Image.Resampling.LANCZOS).save(cache,format='WEBP',quality=65,method=6)
    return 'data:image/webp;base64,'+base64.b64encode(cache.read_bytes()).decode()
def paragraphs(items):return ''.join('<p>'+esc(p)+'</p>' for p in items)
parts=['''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Ara vs Unity URP vs Unreal — same-scene benchmark</title><style>
:root{color-scheme:dark;--bg:#0e1012;--panel:#15181c;--text:#f1f3f5;--muted:#a7b0ba;--line:#30363d;--accent:#88c7b4}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:16px/1.6 system-ui,Segoe UI,sans-serif}main{max-width:1480px;margin:auto;padding:54px 28px 100px}h1{font-size:clamp(2rem,4vw,3.6rem);line-height:1.12;max-width:1100px;letter-spacing:-.035em}h2{margin-top:64px;font-size:1.8rem}h3{margin-top:32px}p{max-width:1050px}.muted,figcaption,small{color:var(--muted)}a{color:var(--accent)}nav{display:flex;gap:22px;flex-wrap:wrap;border-block:1px solid var(--line);padding:16px 0;margin:30px 0}.eyebrow{color:var(--accent);font-size:.8rem;text-transform:uppercase;letter-spacing:.12em}.table{overflow-x:auto;margin:24px 0}table{border-collapse:collapse;width:100%;font-size:.9rem;font-variant-numeric:tabular-nums}th,td{text-align:left;padding:12px 14px;border-bottom:1px solid var(--line);vertical-align:top}th{color:var(--muted);font-weight:500;white-space:nowrap}td:first-child{font-weight:600}code{font-size:.85em;overflow-wrap:anywhere}.images{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}figure{margin:0}img{width:100%;height:auto;display:block;background:var(--panel)}figcaption{padding:8px 0}.note{border-left:3px solid var(--accent);padding:8px 20px;margin:30px 0;background:var(--panel)}details{margin:18px 0;border-top:1px solid var(--line);padding-top:14px}summary{cursor:pointer;color:var(--accent)}.bar{display:grid;grid-template-columns:110px 1fr 100px;gap:12px;align-items:center;margin:12px 0}.track{background:#252b31;height:22px}.fill{height:100%;background:var(--accent)}video{width:100%;max-width:900px;background:#000}footer{margin-top:70px;border-top:1px solid var(--line);padding-top:24px;color:var(--muted)}@media(max-width:700px){main{padding:30px 16px}.images{grid-template-columns:1fr}th,td{padding:10px 8px}.bar{grid-template-columns:75px 1fr 75px}}
</style><main><div class="eyebrow">Islam’s cross-engine benchmark · RTX 4070 SUPER · 27 October 2026</div><h1>Ara vs Unity URP vs Unreal</h1>''']
parts+=paragraphs(notes.get('lead',['Measured on this PC with one engine running at a time. This is an interim Showcase report; Outpost and Hangar follow after Showcase is verified end to end.']))
parts+=['<div class="note"><strong>Final scope: Showcase only; Outpost and Hangar not run.</strong></div>']
parts+=['<nav><a href="#standings">Standings</a><a href="#settings">Exact settings</a>',''.join(f'<a href="#{s}">{s.title()}</a>' for s in scenes),'<a href="#method">Method & limits</a></nav>']
parts+=['<section id="standings"><h2>Where Ara stands</h2>',paragraphs(notes.get('standings',['Interpret the runtime table together with the image comparisons. Equal effect names do not imply equal visual quality or cost.'])), '</section>']
if call:
    parts+=[f'<div class="note"><strong>Primary condition: with an active Discord call (DWM ~{call["dwm_percent"]["mean"]:.1f}%, Discord ~{call["discord_percent"]["mean"]:.1f}%).</strong><p>The engine-free baseline lasted {call["baseline_seconds"]:.1f} s. DWM p50/p95: {call["dwm_percent"]["p50"]:.1f}/{call["dwm_percent"]["p95"]:.1f}%; Discord p50/p95: {call["discord_percent"]["p50"]:.1f}/{call["discord_percent"]["p95"]:.1f}%. These are the guard’s PDH GPU-engine counter sums across adapters, not an exclusive RTX utilization percentage.</p><p>DWM allowance: {call["dwm_allowance_percent"]:.1f}% (idle p95 + five percentage points). The foreign sum still rejects &gt;10% sustained for five samples, Discord stays charged, games invalidate on presence, and new GPU process identities above .5% for two samples invalidate the additional check. Chrome receives no GPU allowance.</p><p>Runs interleave Ara → Unity → Unreal for each repeat. Three repeats expand to five if any engine’s GPU p50/p95 range exceeds 5% of its median, or wall p95 / 1% low range exceeds 10%. All earlier quiet-admission data is retained separately.</p></div>']
parts+=['<section id="settings"><h2>Exact settings</h2><p>Each scene is authored once in Blender 5.1, then exported as one shared glTF 2.0 scene. All engines consume that geometry and the same 1,200 camera samples over 20 seconds. Lighting is recreated from one manifest. Characters are static rest poses; this measures rendering and editor use, without gameplay or animation load.</p>']
parts+=[table(['Setting','Ara','Unity URP','Unreal'],[
['Version / build','0.0.2-alpha.3 · 13e8b9da · release','6000.6.6f1 · URP 17.6.0 · Mono standalone, non-development','5.7.4 · Development packaged'],
['Graphics API','Vulkan','D3D12','D3D12'],
['Tier','High / Epic','High / Ultra, explicitly configured URP assets','High / Epic scalability (all sg.* = 2 / 3)'],
['Output / internal scale','2560×1440 or 3840×2160 · ⅔ / 0.77','Same output and scale','Same output and scale'],
['AA / scaling','TAA with temporal upsampling','High TAA + linear upscaling','TSR'],
['Dynamic resolution, VSync, HW RT, DLSS/FSR','All off','All off','All off'],
['GI','Dynamic probe GI; update budget 2048 / 4096','Baked Adaptive Probe Volumes; mixed indirect-only lighting; no dynamic GI','Software Lumen; hardware RT disabled'],
['Directional shadows','4×2048 cascades, 150 / 250 m; 2048 far map, 1500 / 2000 m; virtual page budget 1024/2048','4 cascades; 2048 / 4096 atlas, 100 / 150 m; soft shadows globally enabled','Virtual shadow maps; scalability defaults for each tier'],
['Local shadows','4 / 8 lights, 512 resolution, 12 / 24 redraw budget','Soft shadows, 2048 / 4096 atlas','Virtual shadow maps; tier defaults'],
['AO','GTAO: radius 1 m; 2×6 / 3×8 samples','SSAO: intensity 1, radius .3, medium samples, direct-light strength .25','Default Lumen AO for the tier'],
['Reflections','SSR: roughness ≤ .5/.6; 30/50 m; 24/32 steps + sky IBL','Sky cubemap IBL; URP has no SSR','SSR quality 75 + sky reflection; reflection method explicitly screen-space'],
['Bloom / tone / exposure','Bloom .04 · filmic · manual EV100','Bloom .04, threshold 1 · ACES · postExposure = −EV − log₂1.2','Bloom .04 · default filmic · manual bias −EV, lens attenuation .65'],
['Motion blur / DOF','No volume requesting either','Off','Off'],
['Geometry policy','Imported glTF with renderer’s normal LOD/culling','Imported glTFast meshes; no manual engine-specific LODs','Interchange import; opaque Nanite default, translucent components force fallback'],
])]
parts+=['<p class="muted">High is a matched-intent tier, not a claim that the three algorithms perform equal work. APV is baked, while Ara and Lumen can update dynamically. Unreal’s Development build also differs from the other release players. These are explicit limits on attributing a speed difference solely to renderer efficiency.</p></section>']
if scalability:
    parts+=['<details><summary>Installed Unreal High/Epic CVar definitions (before explicit overrides above)</summary>']
    for category in ['AntiAliasingQuality','ShadowQuality','GlobalIlluminationQuality','ReflectionQuality','PostProcessQuality','TextureQuality','EffectsQuality','ShadingQuality']:
        high=scalability['sections'].get(category+'@2',{});epic=scalability['sections'].get(category+'@3',{});keys=sorted(high.keys()|epic.keys());parts+=[f'<h3>{category}</h3>',table(['CVar','High','Epic'],[[esc(k),esc(high.get(k,'inherited')),esc(epic.get(k,'inherited'))]for k in keys])]
    parts+=['</details>']
for scene in scenes:
    m=json.loads((root/'source'/scene/'manifest.json').read_text());parts+=[f'<section id="{scene}"><h2>{scene.title()}</h2>']
    parts+=paragraphs(notes.get(scene,[]))
    if visuals:
        parts+=['<details open><summary>Specific visual differences: Ara versus Unreal and Cycles</summary>']
        for finding in visuals:
            shot=finding['shot'];links=' · '.join(f'<a href="{esc(image_links.get(scene+"/"+engine+"/"+shot,""))}">{engine.title()} 4K</a>'for engine in ['ara','unreal','cycles'])
            parts+=[f'<h3>{esc(finding["title"])}</h3><p>{esc(finding["text"])} <span class="muted">{esc(shot.title())}:</span> {links}.</p>']
        parts+=['</details>']
    parts+=[f'<p class="muted">Source: {m.get("renderedTriangles",m["triangles"]):,} rendered Blender triangles. Manual exposure EV100 {m["ev100"]:g}. Duration 20 s at 60 camera samples/s, continuously interpolated for the primary runtime matrix.</p>']
    parts+=[table(['Light','Type / units','Intensity','Linear RGB','Position m / rotation °','Range m','Inner / outer half-angle °'],[[esc(l['name']),esc(l['kind']+' / '+l['unit']),number(l['intensity'],1),esc(l['color']),esc(str(l['position'])+' / '+str(l['rotation'])),number(l['range'],1),str(l.get('innerAngle','—'))+' / '+str(l.get('outerAngle','—'))] for l in m['lights']])]
    environment=m['environment'];parts+=[table(['Common sky setting','Value'],[['Zenith linear RGB',esc(environment['sky'][:3])],['Horizon linear RGB',esc(environment['horizon'][:3])],['Ground linear RGB',esc(environment['ground'][:3])],['Gradient intensity',str(environment.get('intensity',900))+' cd/m²'],['Fog / motion blur / DOF','Off in all three engines']])]
    parts+=['<p class="muted">Sun RGB is authoritative; no engine-specific color-temperature control is enabled. Local “lumens” use Ara’s point-equivalent convention: center intensity = value/(4π) candela, including spots. Unity uses relative URP intensity with the corresponding π conversion, then the common EV exposure. Cycles uses the documented 683 lm/W conversion and compensating exposure. Spot inner/outer half angles and transforms are shared in manifest.json.</p>']
    parts+=['<h3>Packaged runtime</h3><p>Medians of the stated number of per-run measurements. GPU and CPU columns are p50 / p95 milliseconds. Spread is the minimum–maximum of each run’s GPU p95, with sample coefficient of variation. “1% low” is 1000 divided by the mean of the slowest 1% wall-frame intervals; hitches count intervals strictly above 16 ms in the scored 20-second window.</p>']
    rows=[r for r in runtime if r['scene']==scene]
    for condition,label in [('active-discord-call','With an active Discord call · continuously interpolated camera'),('quiet-admission-continuous','Earlier quiet-admission runs · continuous camera'),('quiet-admission-original','Earlier quiet-admission baseline · camera modes differ')]:
        selected=[r for r in rows if r.get('condition')==condition]
        if not selected:continue
        if condition!='active-discord-call':parts+=[f'<details><summary>{label}</summary>']
        else:parts+=[f'<h3>{label}</h3>']
        if condition=='quiet-admission-original':parts+=['<p class="muted">Ara and Unity used 60 Hz camera-pose steps; Unreal interpolated continuously. These measurements remain valid for those recorded configurations, but their p95 values must not be treated as a controlled cross-engine speed comparison. The primary call-condition matrix corrects the camera sampling.</p>']
        parts+=[table(['Engine','Tier','Output','Runs','GPU ms','GPU p95 spread / CV','CPU ms¹','Wall ms','1% low FPS','Hitches median / total'],[[r['engine'].title(),r['tier'].title(),str(r['width'])+'×'+('1440' if r['width']==2560 else '2160'),r['runs'],number(r['gpu_p50'])+' / '+number(r['gpu_p95']),number(r['spread']['gpu_p95']['min'])+'–'+number(r['spread']['gpu_p95']['max'])+' / '+number(r['spread']['gpu_p95']['sample_cv_percent'],1)+'%',number(r['cpu_p50'])+' / '+number(r['cpu_p95']),number(r['wall_p50'])+' / '+number(r['wall_p95']),number(r['one_percent_low_fps'],1),number(r['hitches_over_16ms'],0)+' / '+number(r['hitches_total'],0)] for r in selected])]
        if condition!='active-discord-call':parts+=['</details>']
    bars=[r for r in rows if r['tier']=='high' and r['width']==3840 and r.get('condition')=='active-discord-call']
    if bars:
        maximum=max(r['gpu_p95'] for r in bars);parts+=['<h3>4K High GPU p95 · lower is better</h3>']
        for r in bars:parts+=[f'<div class="bar"><span>{r["engine"].title()}</span><div class="track"><div class="fill" style="width:{r["gpu_p95"]/maximum*100:.2f}%"></div></div><span>{r["gpu_p95"]:.2f} ms</span></div>']
    parts+=['<p class="muted">¹ CPU definitions differ: Ara main-loop work excludes explicit device/UI fence waits; Unity is FrameTimingManager’s CPU main-thread time; Unreal is CSV GameThreadTime. They are not interchangeable CPU utilization measurements. Wall intervals are reported separately. Ara GPU span, Unity GPU frame time and Unreal GPUTime are the engines’ own GPU timestamp measurements.</p><h3>Startup markers</h3>']
    sr=[r for r in startup if r['scene']==scene]
    parts+=[table(['Engine','Condition','Output width','Runs','Loading present ms','Full scene present ms','First scene CPU callback ms','Engine initialization ms'],[[r['engine'].title(),esc(r.get('condition','quiet-admission')),r['width'],r['runs'],number(r.get('loading_ms'),0),number(r.get('scene_ms'),0),number(r.get('scene_callback_ms'),0),number(r.get('engine_initialized_ms'),0)] for r in sr])]
    parts+=['<p class="muted">Ara’s present markers come from its player. Unity’s EndOfFrame marker is CPU submission, and Unreal’s initialization log is only engine readiness. PresentMon ETW access was denied in this Windows session. Actual first display presentation and a comparable first-full-scene display timestamp are unmeasured for Unity/Unreal; these columns must not be used to rank startup.</p><h3>Editor orbit and Play entry</h3>']
    er=[r for r in editors if r['scene']==scene]
    def median(v):return number(statistics.median(v),1) if v else '—'
    parts+=[table(['Editor','Condition','Orbit interval p50 / p95 ms','Samples','Play marker median ms','Reload off median ms','Individual Play samples ms'],[[r['engine'].title(),esc(r.get('condition','quiet-admission')),number(r['orbit'].get('p50_ms'))+' / '+number(r['orbit'].get('p95_ms')),r['orbit'].get('samples',0),median(r.get('play_reload_on_ms',r.get('play_ms',[]))),median(r.get('play_reload_off_ms',[])),esc(str([round(x,1) for x in r.get('play_reload_on_ms',r.get('play_ms',[]))]))] for r in er])]
    for r in er:parts+=[f'<p class="muted">{r["engine"].title()}: {esc(r.get("play_marker",""))}. View/window: {esc(r.get("window","not logged"))}.</p>']
    parts+=['<p class="muted">Editor measurements use each native UI’s frame/repaint callbacks. Window layouts, viewport pixel counts and editor caps are not fully matched; this is a native editor workload comparison, not a controlled editor renderer speed ranking. Unreal’s ~8.33 ms cadence is consistent with a 120 Hz editor cap. PIE’s world/Slate marker does not prove that its first scene frame has presented. The scene contains no gameplay code beyond the benchmark harness, so domain-reload costs are small.</p>']
    parts+=paragraphs(notes.get(scene+'-editor',[]))
    parts+=['<h3>High quality · three fixed cameras</h3><p>Click each embedded preview to open its original lossless 3840×2160 PNG. The page embeds 960×540 previews to fit Snist’s 2 MB HTML limit; original 4K and 1080p images are also retained locally. Cycles uses CUDA software tracing, 128 samples and denoising with AgX Medium High Contrast. Image metrics use original 4K PNGs; tone mapping, material import, screen-space effects and probe approximations all affect the result.</p>']
    for shot in m['shots']:
        paths=[(e,out/'images'/scene/e/(shot['name']+'.png')) for e in ['ara','unity','unreal','cycles']]
        if not all(p.exists() for e,p in paths):continue
        parts+=[f'<details open><summary>{esc(shot["name"].title())} · Ara / Unity / Unreal / Cycles</summary><div class="images">']
        for engine,path in paths:
            full=image_links.get('/'.join([scene,engine,shot['name']]),'')
            parts+=[f'<figure><a href="{esc(full)}"><img loading="lazy" width="960" height="540" alt="{scene} {shot["name"]}, {engine}; open original 4K" src="{image(path)}"></a><figcaption>{engine.title()} · {esc(shot["name"])} · <a href="{esc(full)}">original 4K PNG</a></figcaption></figure>']
        parts+=['</div></details>']
    qr=[r for r in quality if r['scene']==scene]
    parts+=[table(['Camera','Engine','FLIP ↓','SSIM ↑'],[[esc(r['shot']),r['engine'].title(),number(r['flip'],4),number(r['ssim'],4)] for r in qr])]
    parts+=['<p class="muted">FLIP: LDR, sRGB inputs, 67 pixels/degree. SSIM: RGB, data_range 1. These are deviations from this Cycles rendering, not an absolute engine quality score. No exposure fitting or image alignment is applied after capture.</p><h3>Fixed-step flythroughs</h3>']
    for engine in ['ara','unity','unreal']:
        key=scene+'/'+engine;links=media.get(key,{})
        if links:parts+=[f'<p>{engine.title()}: <a href="{esc(links.get("4k60",""))}">4K60 H.265</a> · <a href="{esc(links.get("1080p60",""))}">1080p60 H.264</a></p>']
        else:parts+=[f'<p class="muted">{engine.title()}: capture or publication pending.</p>']
    parts+=['</section>']
parts+=[r'''<section id="method"><h2>Method, provenance and limits</h2><p>Windows 11, RTX 4070 SUPER, Ryzen 7 7800X3D, approximately 31 GB RAM. One engine runs at a time. Every timing batch has an nvidia-smi process/load check plus a game/OBS process scan, then runs under <code>cargo run -p ara-dev -- gpu --</code> from the required Ara repository. GPU-guard records, per-run process ownership and raw measurements are retained under <code>D:\AraBench\logs</code> and <code>measurements</code>. Only owned processes are allowed to be stopped.</p><p>The monitors are physically 2560×1440. 4K uses oversized borderless windows with verified 3840×2160 render/capture surfaces, clipped by the desktop. It is a 4K rendering workload, not 4K physical scanout. Capture runs are separate from timing. Encoded videos contain fixed-step frames; their playback rate is not a claim of measured realtime 60 FPS.</p><p>Ara uses a git-archive snapshot of 13e8b9da and isolated scratch configuration/cache directories. The benchmark adapter changes camera input, capture/timing instrumentation and window presentation only; it does not change rendering algorithms. The original source checkout is retained unchanged. Engine builds and repeated calibration errors are recorded; rejected captures are kept outside the accepted measurement paths.</p><p>Warm steady-state results exclude startup. Ara waits for scene readiness, then 180 frames; Unity also warms 180 frames. Unreal CSV excludes its first five seconds and scores one complete 20-second period of the looping shared path. Three or five per-run statistics are reduced by median, using the variance rule above. GPU clocks and temperature are logged, with no manual overclock changes. Filesystem and driver caches are not reset or cold-boot controlled.</p><p>This preview is known to have cold pipeline and first-lit-frame regressions. The table reports this binary’s measured behavior. Low/Medium presets and historical alpha comparisons were not run, so no result here establishes that those regressions are fixed.</p><p>Raw summaries, source manifests, adapters, original images and encoded media are retained in <code>D:\AraBench</code>. This page is generated from those summaries; it uses no JavaScript or external image dependencies. Video links point to separately published media.</p></section>''']
parts+=paragraphs(notes.get('limits',[]));parts+=['<footer>Generated '+datetime.datetime.now().isoformat(timespec='seconds')+' · Local report: D:\\AraBench\\report\\cross-engine-benchmark.html</footer></main></html>']
dest=out/'cross-engine-benchmark.html';dest.write_text(''.join(parts),encoding='utf-8');print(dest,round(dest.stat().st_size/1024/1024,2),'MiB')
