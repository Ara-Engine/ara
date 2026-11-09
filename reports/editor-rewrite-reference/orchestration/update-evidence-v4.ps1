$ErrorActionPreference='Stop'
$repo='C:/Users/snowy/.t3/worktrees/snengine/t3-9c4a1f92'
$path=Join-Path $repo 'reports/ux-editor-preview4.html'
$page=[IO.File]::ReadAllText($path)
if($page.Contains("id='correctness-v4'")){throw 'v4 update already applied'}
$batch='D:/AraBuild/ux-editor-target/acceptance/monkey-d238e4d4-20261028-031645-f70d9738'
$summary=Get-Content -LiteralPath ($batch+'/correctness-interruptions.json') -Raw | ConvertFrom-Json
$rows=foreach($run in $summary.runs){
 $local=$run.run.Replace('\','/')
 "<tr><td><code>$($run.seed)</code></td><td>$($run.checkpointActions)</td><td>$($run.watchdogHangs)</td><td>$($run.panics) / $($run.panelFailures)</td><td>Interrupted after confirmed GPU wait hang</td><td><a href='file:///$local/interruption.json'>Interruption</a> · <a href='file:///$local/debug/stacks-symbols.log'>Matching-symbol stack</a> · <a href='file:///$local/captures/monkey-events.jsonl'>Replay journal</a></td></tr>"
}
$section=@"
<section id='correctness-v4'><h2>Three correctness attempts: acceptance failed</h2>
<p>All three seeds ran without GPU exclusivity on <code>d238e4d4</code>. Each stopped making progress after floating and redocking a panel. Matching-PDB, non-invasive stacks independently show <code>route_event → VulkanDevice::wait_idle</code> while handling a queued floating-window resize. Only the exact owned editor PIDs were stopped, after preserving those stacks and their process identities. These interrupted attempts do not meet the requested 30-minute acceptance or establish successful final drain and journal replay.</p>
<div class='table-wrap'><table><thead><tr><th>Seed</th><th>Checkpointed actions</th><th>Watchdog hangs</th><th>Action panics / panel quarantines</th><th>Correctness</th><th>Raw evidence</th></tr></thead><tbody>$($rows -join "`n")</tbody></table></div>
<p><strong>65,792 checkpointed actions; eight watchdog hangs; zero recorded action panics; zero panel quarantines.</strong> The last summary files contain only five of the eight hangs because they predate each final permanent stall. The watchdog counts supersede those stale counts; they must not be added together. Raw diagnostics include intentional malformed-command rejection counts, retained in the original reports. Native/managed crash artifact and stderr panic scans found zero crash artifacts or panic lines.</p>
<p>Every frame-time column is <strong>informal (shared GPU)</strong>. No timing gate was applied. The separate one-seed, approximately 30-minute exclusive timing run still awaits the orchestrator's exact GPU-slot start and will use <code>ara-dev gpu --</code>.</p>
<p>The old editor/player binaries and matching PDBs are archived with hashes under <a href='file:///D:/AraBuild/ux-editor-target/acceptance/d238e4d4/binaries/manifest.json'>the immutable run's binary manifest</a>. The combined <a href='file:///$batch/correctness-interruptions.json'>interruption report</a> retains each run's raw links and journal tail. A native Float → next-frame Move Right → queued Resize reproducer is committed as <code>50f6d3a2</code>; its newer-main Windows replay remains pending.</p>
<h3>Current source and gates</h3><p><code>b3067eb4</code> passed strict touched-crate Clippy and the editor suites on m6u: 278 library tests passed, nine were opt-in ignored and the declared Linux Scene GPU case was excluded. New command-capture, generated-form, shortcut-reload and callback-ownership work is saved through <code>5473651a</code>. At <code>6d8e4a88</code>, strict Clippy passed and runtime tests reported 296 passed / four failed; the failures exposed Gizmos/Orientation toggle polarity, missing typed Job palette entries and a scrollbar covering the numeric drag grip. Fixes are committed and await focused re-gates. No new acceptance is claimed for them.</p>
<p>The full native audit journeys, asset Undo/Redo replay and successful long monkey runs remain open. The existing kit proof and earlier captures below retain their own checkpoint identities.</p>
</section>
"@
$page=$page.Replace("<section id='checkpoint-v3'>",$section+"`n<section id='checkpoint-v3'>")
$page=$page.Replace('<h2>Correctness runs and current gates</h2>','<h2>Earlier pre-run checkpoint (v3)</h2>')
$page=[regex]::Replace($page,'Evidence snapshot: [^<]+ UTC','Evidence snapshot: '+[datetime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss')+' UTC')
[IO.File]::WriteAllText($path,$page,[Text.UTF8Encoding]::new($false))
$evidence='D:/AraBuild/ux-editor-target/acceptance/evidence-index'
Copy-Item -LiteralPath $path -Destination ($evidence+'/ux-editor-preview4.html')
[ordered]@{version=4;createdUtc=[datetime]::UtcNow.ToString('o');source=$path;bytes=(Get-Item -LiteralPath $path).Length;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash;embeddedImages=([regex]::Matches($page,'<img ')).Count;correctnessCheckpoint='d238e4d4';correctnessAccepted=$false;timingAccepted=$false;frameTimeStatus='informal (shared GPU)';watchdogHangs=8;checkpointActions=65792;actionPanics=0;panelQuarantines=0;rawReport=$batch+'/correctness-interruptions.json'} | ConvertTo-Json | Set-Content -LiteralPath ($evidence+'/v4-manifest.json') -Encoding UTF8
'v4 evidence index updated with interrupted correctness attempts.'