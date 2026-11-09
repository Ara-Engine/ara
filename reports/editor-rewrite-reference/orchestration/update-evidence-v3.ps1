$ErrorActionPreference='Stop'
$report='C:/Users/snowy/.t3/worktrees/snengine/t3-9c4a1f92/reports/ux-editor-preview4.html'
$html=[IO.File]::ReadAllText($report)
$legacy=[Text.Encoding]::GetEncoding(1252)
$strictUtf8=New-Object Text.UTF8Encoding($false,$true)
$pattern='(?:[\u00C2\u00C3\u00E2]|&#(?:194|195|226);)(?:[^\x00-\x7F]|&#\d+;){1,3}'
$html=[regex]::Replace($html,$pattern,[Text.RegularExpressions.MatchEvaluator]{param($match)
 try { [Net.WebUtility]::HtmlEncode($strictUtf8.GetString($legacy.GetBytes([Net.WebUtility]::HtmlDecode($match.Value)))) } catch { $match.Value }
})
$stamp=[DateTime]::UtcNow.ToString('yyyy-MM-dd HH:mm:ss')+' UTC'
$html=$html.Replace('2026-10-27 21:50:31 UTC',$stamp)
$section=@"
<section id='checkpoint-v3'><h2>Correctness runs and current gates</h2>
<p>Three 30-minute correctness seeds are scheduled serially on <code>d238e4d4</code>. Seed <code>104729</code> started at 03:16:46 local on 28 October. Their frame-time columns are <strong>informal (shared GPU)</strong>; no timing acceptance is claimed. A separate exclusive timing seed awaits the orchestrator's GPU slot. The full native behaviour route and asset Undo/Redo acceptance remain open.</p>
<table><thead><tr><th>Checkpoint</th><th>Verified evidence</th><th>Limit</th></tr></thead><tbody>
<tr><td>Kit <code>a5f0c4f3</code></td><td>Shared overlay sizing, searchable Select, modal/focus and emergency grapheme wrapping. Windows kit goldens plus 73 editor library tests; Linux focused gates green.</td><td>Existing embedded captures below show the product; narrow-field captures remain in the local evidence directory.</td></tr>
<tr><td>Editor <code>d238e4d4</code></td><td>Formatting, strict Clippy, touched-crate tests and UI-kit goldens; 277 editor library tests passed; editor integration, export and diagnostic coverage passed.</td><td>Linux excludes the declared Scene GPU case. This integration candidate includes export candidate <code>2519c026</code>; its owner's Windows replay remains a separate gate.</td></tr>
<tr><td>Native drag evidence</td><td>All 12 core drag stories passed: hierarchy, references, surface placement, material submeshes, components, floating windows and a real OS import.</td><td>Extended import cancellation and asset history were not accepted in that earlier run. Host canonical scene echoes interrupted Redo; the per-op receipts are preserved.</td></tr>
<tr><td>Recovery and live settings</td><td>Joined preview workers: 13 asset-tool and two resource recovery tests exited cleanly. Fourteen settings tests and three History tests passed, including real Host edits, Undo/Redo and focused drafts.</td><td>The previous resource test process passed its assertions but then crashed or hung; these process-exit checks distinguish the fix.</td></tr>
<tr><td>Monkey acceptance checker</td><td>28 runner tests, six panel-boundary tests and 29 synthetic driver checks passed. Final catalog drift, unfinished jobs/saves and independent crash evidence fail acceptance.</td><td>Synthetic checks are not native soak runs. Terminal Host jobs do not prove retirement of every detached backend child.</td></tr>
</tbody></table>
<h3>Recorded failures remain visible</h3>
<table><thead><tr><th>Source</th><th>Observed run</th><th>Outcome</th><th>Frame timing</th></tr></thead><tbody>
<tr><td><code>219902aa</code></td><td>1.62 seconds, 134 actions</td><td>Active Scene removal caused two panel failures and an early fatal exit. Recovery is fixed in source and focused tests.</td><td>Informal historical observation</td></tr>
<tr><td><code>ed243041</code></td><td>60.13 seconds, 2,998 actions</td><td>No panic or hang; two Animation/Inspector quarantines. Atomic resource initialization and empty-resource recovery address this failure.</td><td>Informal historical observation</td></tr>
<tr><td><code>18bfd589</code></td><td>44.55 seconds, 4,724 actions</td><td>Generated palette form closed as a panel; fatal markup error. Fixed in <code>103ca108</code> with two passing runtime regressions.</td><td>Informal (shared GPU)</td></tr>
<tr><td><code>d238e4d4</code></td><td>Three seeds requested: 104729, 130363, 155921; 1,800 seconds each</td><td>Running; no completed-soak acceptance claimed here.</td><td><strong>Informal (shared GPU)</strong></td></tr>
</tbody></table>
<p>Machine-local raw evidence: <a href='file:///D:/AraBuild/ux-editor-target/acceptance/18bfd589/gates/checkpoint.json'>independent CPU gate receipt</a>; <a href='file:///D:/AraBuild/ux-editor-target/acceptance/18bfd589/monkey-104729-20261028-025319/captures/monkey.json'>palette failure report</a>; <a href='file:///D:/AraBuild/ux-editor-target/acceptance/monkey-d238e4d4-20261028-031645-f70d9738/monkey-acceptance.json'>live correctness batch</a>. Key captures are embedded below; raw paths require access to the Windows machine.</p>
</section>
"@
if($html.Contains("id='checkpoint-v3'")){throw 'Version 3 section already exists'}
$anchor='<h2>Latest native editor result is not acceptance</h2>'
if(!$html.Contains($anchor)){throw 'Evidence section anchor changed'}
$html=$html.Replace($anchor,'<h2>Earlier native editor baseline</h2>')
$position=$html.IndexOf('<section', $html.IndexOf('<body'))
if($position -lt 0){throw 'No report section'}
$html=$html.Insert($position,$section)
[IO.File]::WriteAllText($report,$html,[Text.UTF8Encoding]::new($false))
$copy='D:/AraBuild/ux-editor-target/acceptance/evidence-index/ux-editor-preview4.html'
[IO.File]::Copy($report,$copy,$true)
[ordered]@{source=$report;copy=$copy;bytes=([IO.FileInfo]$report).Length;sha256=(Get-FileHash -LiteralPath $report -Algorithm SHA256).Hash;updatedUtc=$stamp;embeddedImages=[regex]::Matches($html,'data:image/').Count;nativeSoakAcceptance='running, no completed-soak acceptance claimed';formalTiming='pending GPU slot'}|ConvertTo-Json|Set-Content -Encoding UTF8 -LiteralPath 'D:/AraBuild/ux-editor-target/acceptance/evidence-index/v3-manifest.json'
Write-Output "Evidence HTML updated: $(([IO.FileInfo]$report).Length) bytes"