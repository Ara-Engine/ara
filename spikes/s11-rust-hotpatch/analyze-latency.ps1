# Read-only stage accounting for measure-latency.ps1 and opt-in trace files.
param([Parameter(Mandatory=$true)][string]$Output)
$ErrorActionPreference='Stop'
$events=@(Get-ChildItem -LiteralPath (Join-Path $Output 'trace') -Filter '*.json' | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json } | Sort-Object at_us)
$rows=Get-Content -LiteralPath (Join-Path $Output 'latency.json') -Raw | ConvertFrom-Json
$applies=@($events | Where-Object event -eq 'apply_end')
$dxPhases=[Collections.Generic.List[object]]::new()
$pending=[ordered]@{}
foreach($line in (Get-Content -LiteralPath (Join-Path $Output 'driver.out.log'))) {
    if($line -match '^\s*(\d+)ms\s+[\d.]+%\s+(Workspace hotpatch replay|Compiling|Patch: [A-Za-z ]+?)\s+\|') {
        $name=$Matches[2].Trim()
        $milliseconds=[int64]$Matches[1]
        if($name -eq 'Workspace hotpatch replay') { $pending=[ordered]@{} }
        $pending[$name]=$milliseconds
    } elseif($line -match '\bHot-patching:' -and $pending.Contains('Patch: Link')) {
        $dxPhases.Add([pscustomobject]@{generation=$dxPhases.Count+1;phases_ms=$pending})
        $pending=[ordered]@{}
    }
}
$dxPhases | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $Output 'dx-phases.json')
$results=[Collections.Generic.List[object]]::new()
function Last-Event([string]$Name,[double]$Before) { $events | Where-Object { $_.event -eq $Name -and $_.at_us -le $Before } | Select-Object -Last 1 }
function Overlap([double]$Start,[double]$End,[double]$From,[double]$Until) { [math]::Max(0,[math]::Min($End,$Until)-[math]::Max($Start,$From)) }
foreach($row in $rows) {
    $apply=if($null -ne $row.generation){$applies[[int]$row.generation-1]}else{Last-Event 'apply_end' $row.observed_us}
    if($null -eq $apply){throw 'Missing applied generation trace'}
    $start=Last-Event 'apply_start' $apply.at_us
    $received=Last-Event 'patch_received' $start.at_us
    $queued=Last-Event 'patch_queued' $start.at_us
    $watch=$events | Where-Object { $_.event -eq 'file_watch' -and $_.at_us -ge $row.save_us -and $_.at_us -le $received.at_us } | Select-Object -First 1
    if($null -eq $watch){throw 'Missing save/watch trace'}
    $changed=switch($row.kind){sim{'ara_hotpatch_sim_probe'}fidelity{'ara_render_fidelity'}editor{'ara_editor'}}
    $changedUs=0.0; $tipUs=0.0; $cpuUs=0.0; $guardUs=0.0; $hasCpu=$false
    $linkSpans=@($events | Where-Object { $_.event -eq 'linker_end' -and $_.at_us -ge $watch.at_us -and $_.at_us -le $received.at_us } | ForEach-Object {
        $linkEnd=$_
        $linkStart=$events | Where-Object { $_.event -eq 'linker_start' -and $_.pid -eq $linkEnd.pid -and $_.at_us -le $linkEnd.at_us } | Select-Object -Last 1
        [pscustomobject]@{start=$linkStart.at_us;end=$linkEnd.at_us}
    })
    $calls=[Collections.Generic.List[object]]::new()
    foreach($end in ($events | Where-Object { $_.event -eq 'rustc_end' -and $_.at_us -ge $watch.at_us -and $_.at_us -le $received.at_us -and $_.details.crate -ne 'probe' })) {
        $compileStart=$events | Where-Object { $_.event -eq 'rustc_start' -and $_.pid -eq $end.pid -and $_.at_us -le $end.at_us } | Select-Object -Last 1
        $wrapperStart=$events | Where-Object { $_.event -eq 'compiler_start' -and $_.pid -eq $end.pid -and $_.at_us -le $end.at_us } | Select-Object -Last 1
        $wrapperEnd=$events | Where-Object { $_.event -eq 'compiler_end' -and $_.pid -eq $end.pid -and $_.at_us -ge $end.at_us } | Select-Object -First 1
        $duration=Overlap $compileStart.at_us $end.at_us $watch.at_us $received.at_us
        $rawDuration=$duration
        foreach($link in $linkSpans) { $duration-=Overlap $link.start $link.end ([math]::Max($watch.at_us,$compileStart.at_us)) ([math]::Min($received.at_us,$end.at_us)) }
        $wrapper=Overlap $wrapperStart.at_us $wrapperEnd.at_us $watch.at_us $received.at_us
        $guardUs += [math]::Max(0,$wrapper-$rawDuration)
        if($end.details.crate -eq $changed){$changedUs+=$duration}else{$tipUs+=$duration}
        if($null -ne $end.details.cpu_us) { $cpuUs+=$end.details.cpu_us; $hasCpu=$true }
        $calls.Add([ordered]@{crate=$end.details.crate;wall_ms=[math]::Round($duration/1000,3);cpu_ms=$(if($null -ne $end.details.cpu_us){[math]::Round($end.details.cpu_us/1000,3)}else{$null})})
    }
    $links=@($events | Where-Object { $_.event -eq 'linker_end' -and $_.at_us -ge $watch.at_us -and $_.at_us -le $received.at_us })
    $linkUs=0.0
    foreach($link in $links) {
        $linkStart=$events | Where-Object { $_.event -eq 'linker_start' -and $_.pid -eq $link.pid -and $_.at_us -le $link.at_us } | Select-Object -Last 1
        $linkUs+=Overlap $linkStart.at_us $link.at_us $watch.at_us $received.at_us
    }
    $receiveUs=$queued.at_us-$received.at_us
    $boundaryUs=$start.at_us-$queued.at_us
    $otherUs=$received.at_us-$watch.at_us-$changedUs-$tipUs-$guardUs-$linkUs
    $result=[ordered]@{
        kind=$row.kind;mode=$row.mode;sample=$row.sample;configuration_first=$row.configuration_first;pid=$row.pid
        watch_ms=[math]::Round(($watch.at_us-$row.save_us)/1000,3)
        changed_rustc_ms=[math]::Round($changedUs/1000,3);tip_rustc_ms=[math]::Round($tipUs/1000,3)
        # Native macOS links have DX phase timings, but no MSVC facade span.
        # Leave unavailable counters null instead of presenting zero-cost work.
        compiler_guard_ms=[math]::Round($guardUs/1000,3);link_ms=$(if($links.Count -gt 0){[math]::Round($linkUs/1000,3)}else{$null})
        cargo_cache_patch_delivery_ms=[math]::Round($otherUs/1000,3)
        receiver_validation_ms=[math]::Round($receiveUs/1000,3);boundary_wait_ms=[math]::Round($boundaryUs/1000,3)
        apply_ms=[math]::Round(($apply.at_us-$start.at_us)/1000,3)
        observed_frame_ms=[math]::Round(($row.observed_us-$apply.at_us)/1000,3)
        total_ms=[math]::Round($row.observed_ms,3);rustc_cpu_ms=$(if($hasCpu){[math]::Round($cpuUs/1000,3)}else{$null});rustc_calls=$calls
    }
    $results.Add([pscustomobject]$result)
}
$results | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $Output 'stages.json')
$results | Select-Object kind,mode,sample,watch_ms,changed_rustc_ms,tip_rustc_ms,link_ms,cargo_cache_patch_delivery_ms,receiver_validation_ms,boundary_wait_ms,apply_ms,observed_frame_ms,total_ms | Format-Table -AutoSize
