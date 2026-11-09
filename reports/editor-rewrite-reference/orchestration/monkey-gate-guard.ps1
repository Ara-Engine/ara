# Dot-source only. Parsing/planning helpers are pure; only Stop-GateOwnedTree controls
# processes, and it accepts identities previously observed below our launched wrapper.
function Gate-Property($Object,[string]$Name) {
    if($null -eq $Object){return $null}
    if($Object -is [Collections.IDictionary]){return $Object[$Name]}
    $property=$Object.PSObject.Properties[$Name]
    if($null -ne $property){return $property.Value}
    return $null
}
function Gate-Nonnegative($Value) {
    if($Value -isnot [ValueType] -or $Value -is [bool]){return $false}
    try {$number=[double]$Value;return ![double]::IsNaN($number) -and ![double]::IsInfinity($number) -and $number -ge 0} catch {return $false}
}
function Test-GateTerminal($Value) {
    # ara-dev/check.rs Report, not its Selection/header plan or GPU telemetry JSON.
    foreach($key in @('passed','full','green_checkpoint_updated','compiler_cache_bypassed')) {
        if((Gate-Property $Value $key) -isnot [bool]){return $false}
    }
    if(!(Gate-Nonnegative (Gate-Property $Value 'elapsed_seconds'))){return $false}
    foreach($key in @('head','base')){if((Gate-Property $Value $key) -notmatch '^[0-9a-fA-F]{40}$'){return $false}}
    if((Gate-Property $Value 'base_reason') -isnot [string] -or $null -eq (Gate-Property $Value 'compilation')){return $false}
    foreach($key in @('affected','test_packages','changed_libraries','test_only','reasons','known_failures','checks')) {
        $property=$Value.PSObject.Properties[$key]
        if($null -eq $property -or $property.Value -isnot [array]){return $false}
    }
    $checks=@(Gate-Property $Value 'checks')
    if($checks.Count -eq 0){return $false}
    foreach($check in $checks) {
        if((Gate-Property $check 'check') -isnot [string] -or (Gate-Property $check 'detail') -isnot [string]){return $false}
        foreach($key in @('passed','known_failure','timed_out')){if((Gate-Property $check $key) -isnot [bool]){return $false}}
        if(!(Gate-Nonnegative (Gate-Property $check 'seconds'))){return $false}
    }
    # These final hygiene rows are always appended before the terminal Report.
    $names=@($checks|ForEach-Object{Gate-Property $_ 'check'})
    return ('metadata' -in $names -and 'unchanged tree' -in $names -and 'pending snapshots' -in $names)
}
function Parse-GateLog([string]$Text) {
    $result=[ordered]@{allowed=$false;phase='unknown';reason='No recognized ara-dev gate invocation';prepared=$false;terminal=$false;gatePassed=$null;head=$null}
    $starts=[regex]::Matches($Text,'(?m)^Base: [0-9a-fA-F]{40}\b[^\r\n]*')
    if($starts.Count -eq 0){return [pscustomobject]$result}
    $body=$Text.Substring($starts[$starts.Count-1].Index)
    # A --plan invocation cannot authorize a workload, even with result-like data.
    if($Text -match '(?m)^\s*(?:Running .*|[^\r\n]*ara-dev[^\r\n]*)check[^\r\n]*--plan\b'){$result.reason='Plan-only gate log';return [pscustomobject]$result}
    $prepared=[regex]::Matches($body,'(?m)^tests:\s*\d+\s+prepared harnesses\b[^\r\n]*')
    $result.prepared=$prepared.Count -gt 0
    $lastPrepared=if($result.prepared){$prepared[$prepared.Count-1].Index}else{-1}
    $terminalEnd=-1;$terminalStart=-1;$coveredEnd=-1;$terminal=$null;$partial=$false;$malformed=$false
    # Only JSON objects starting on their own line can be an ara-dev Report.
    # Balanced string-aware scanning ignores braces inside telemetry strings.
    $candidates=[regex]::Matches($body,'(?m)^\{')
    foreach($candidate in $candidates) {
        if($candidate.Index -lt $coveredEnd){continue}
        $depth=0;$quoted=$false;$escaped=$false;$end=-1
        for($index=$candidate.Index;$index -lt $body.Length;$index++) {
            $character=$body[$index]
            if($quoted){if($escaped){$escaped=$false}elseif($character -eq '\'){$escaped=$true}elseif($character -eq '"'){$quoted=$false};continue}
            if($character -eq '"'){$quoted=$true;continue}
            if($character -eq '{' -or $character -eq '['){$depth++}
            elseif($character -eq '}' -or $character -eq ']'){$depth--;if($depth -eq 0){$end=$index+1;break}}
        }
        if($end -lt 0){$partial=$true;continue}
        $coveredEnd=$end
        try {$value=$body.Substring($candidate.Index,$end-$candidate.Index)|ConvertFrom-Json -ErrorAction Stop} catch {$malformed=$true;continue}
        if(Test-GateTerminal $value){$terminal=$value;$terminalEnd=$end;$terminalStart=$candidate.Index;$partial=$false;$malformed=$false}
        elseif($null -ne $value.PSObject.Properties['passed']){$malformed=$true}
    }
    if($partial){$result.reason='Partial JSON after gate start; completion unknown';return [pscustomobject]$result}
    if($malformed){$result.reason='Malformed JSON after gate start; completion unknown';return [pscustomobject]$result}
    if($null -ne $terminal -and $terminalStart -gt $lastPrepared) {
        $tail=$body.Substring($terminalEnd)
        if($tail -match '(?m)^(?:Compilation:|Compiler cache bypass:|metadata:|test-build:|clippy:|tests:)'){$result.reason='New gate work follows the terminal report';return [pscustomobject]$result}
        $result.allowed=$true;$result.phase='completed';$result.reason='Validated terminal ara-dev check Report (pass or fail)';$result.terminal=$true;$result.gatePassed=$terminal.passed;$result.head=$terminal.head
        return [pscustomobject]$result
    }
    if($result.prepared){$result.phase='prepared-tests';$result.reason='Prepared test harnesses have no validated terminal Report';return [pscustomobject]$result}
    if($body -match '(?m)^(?:Compilation:|Compiler cache bypass:|test-build:|clippy:)\s*\S') {
        $result.allowed=$true;$result.phase='compiling';$result.reason='Recognized compilation; no prepared test harnesses'
    } else {$result.reason='New/unknown gate phase; completion cannot be established'}
    return [pscustomobject]$result
}
function Select-NewestGate([object[]]$Files) {
    $rows=@(foreach($file in $Files) {
        if($file.Name -match '^gate-b(?<number>\d+)(?<suffix>[^.]*)\.log$') {
            $sequence=[UInt64]0
            if(![UInt64]::TryParse($Matches.number,[ref]$sequence)){return $null}
            [pscustomobject]@{file=$file;sequence=$sequence;suffix=$Matches.suffix}
        }
    })
    if($rows.Count -eq 0){return $null}
    # .utf8 copies are derivatives, not live gate reports. A lower numbered late
    # flush never supersedes the highest bN; same-number retries use newest write.
    return ($rows|Sort-Object @{Expression='sequence';Descending=$true},@{Expression={$_.file.LastWriteTimeUtc};Descending=$true},@{Expression='suffix';Descending=$true}|Select-Object -First 1)
}
function New-GateGuard {
    return @{highWater=[UInt64]0;path=$null;length=-1L;write=$null;read=$null;observations=0L}
}
function Read-GateGuard($State,[string]$Directory='D:\AraBuild') {
    $State.observations++
    $selected=$null
    try {
        $files=@(Get-ChildItem -LiteralPath $Directory -File -Filter 'gate-b*.log' -ErrorAction Stop)
        $selected=Select-NewestGate $files
        if($null -eq $selected){throw 'No recognizable numbered gate log'}
        if($selected.sequence -lt $State.highWater){throw 'Newest gate regressed below the previously observed bN'}
        $State.highWater=$selected.sequence
        $file=$selected.file
        # The running gate owns a write handle. Readers must share that write
        # access (and rotation/delete) without locking or modifying its log.
        $stream=$null;$reader=$null
        try {
            $sharing=[IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
            $stream=[IO.File]::Open($file.FullName,[IO.FileMode]::Open,[IO.FileAccess]::Read,$sharing)
            # FindFirstFile/Get-ChildItem may carry stale directory-entry length
            # or write time while another handle is writing. Opening the reader
            # can refresh that entry; establish both snapshots AFTER that open.
            $file.Refresh()
            $beforeLength=$stream.Length;$beforeWrite=$file.LastWriteTimeUtc;$beforeCreation=$file.CreationTimeUtc
            if($beforeWrite -gt [DateTime]::UtcNow.AddSeconds(5)){throw 'Newest gate timestamp is in the future'}
            if($file.Length -ne $beforeLength){throw 'Gate changed while opening; stream/path length disagree'}
            if($State.path -eq $file.FullName -and $beforeLength -lt $State.length){throw 'Newest gate was truncated; previous parse is stale'}
            if($State.path -eq $file.FullName -and $State.length -eq $beforeLength -and $State.write -eq $beforeWrite -and $null -ne $State.read){return $State.read}
            if($beforeLength -gt 16777216){throw 'Gate log exceeds bounded 16 MiB read; status unknown'}
            $reader=New-Object IO.StreamReader($stream,[Text.Encoding]::UTF8,$true)
            $text=$reader.ReadToEnd()
            $afterLength=$stream.Length
            $file.Refresh()
            if($afterLength -ne $beforeLength -or $file.Length -ne $beforeLength -or $file.LastWriteTimeUtc -ne $beforeWrite -or $file.CreationTimeUtc -ne $beforeCreation){
                throw "Gate changed while reading; status unknown (bytes $beforeLength->$afterLength/path $($file.Length); write $($beforeWrite.ToString('o'))->$($file.LastWriteTimeUtc.ToString('o')); creation $($beforeCreation.ToString('o'))->$($file.CreationTimeUtc.ToString('o')))"
            }
        } finally {
            if($null -ne $reader){$reader.Dispose()}
            elseif($null -ne $stream){$stream.Dispose()}
        }
        $parsed=Parse-GateLog $text
        $parsed|Add-Member path $file.FullName
        $parsed|Add-Member sequence $selected.sequence
        $parsed|Add-Member bytes $beforeLength
        $parsed|Add-Member modifiedUtc $beforeWrite.ToString('o')
        $parsed|Add-Member observedUtc ([DateTime]::UtcNow.ToString('o'))
        $State.path=$file.FullName;$State.length=$beforeLength;$State.write=$beforeWrite;$State.read=$parsed
        return $parsed
    } catch {
        # Never reuse a previous clear state after a failed/newest-log read.
        return [pscustomobject]@{allowed=$false;phase='unknown';reason=$_.Exception.Message;terminal=$false;prepared=$null;path=if($null -ne $selected){$selected.file.FullName}else{$null};sequence=$State.highWater;observedUtc=[DateTime]::UtcNow.ToString('o')}
    }
}
function Same-OwnedIdentity($Expected,$Actual) {
    if($null -eq $Actual){return $false}
    try {
        # Win32_Process CreationDate exposes microseconds; Process.StartTime can
        # expose an extra tick. Compare the exact shared microsecond precision.
        $expectedBirth=[decimal]([DateTimeOffset]::Parse($Expected.startedUtc)).UtcDateTime.Ticks
        $actualBirth=[decimal]([DateTimeOffset]::Parse($Actual.startedUtc)).UtcDateTime.Ticks
        return [long]$Expected.pid -eq [long]$Actual.pid -and
            [Math]::Truncate($expectedBirth/10) -eq [Math]::Truncate($actualBirth/10) -and
            [string]::Equals([IO.Path]::GetFullPath($Expected.executable),[IO.Path]::GetFullPath($Actual.executable),[StringComparison]::OrdinalIgnoreCase)
    } catch {return $false}
}
function Get-GateProcessSnapshot {
    return @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop|ForEach-Object {
        [pscustomobject]@{pid=[long]$_.ProcessId;parentPid=[long]$_.ParentProcessId;startedUtc=if($null -ne $_.CreationDate){$_.CreationDate.ToUniversalTime().ToString('o')}else{$null};executable=$_.ExecutablePath}
    })
}
function Plan-GateOwnedTree($Root,[object[]]$Known,[object[]]$Snapshot) {
    $owned=@{};$live=@{};$issues=New-Object 'Collections.Generic.List[string]'
    foreach($row in $Snapshot){$live[[long]$row.pid]=$row}
    foreach($row in $Known) {
        if(Same-OwnedIdentity $row $live[[long]$row.pid]){$owned[[long]$row.pid]=$row}
        elseif($live.ContainsKey([long]$row.pid)){$issues.Add("Owned PID $($row.pid) was reused or cannot be revalidated")}
    }
    if(Same-OwnedIdentity $Root $live[[long]$Root.pid]) {
        $owned[[long]$Root.pid]=[pscustomobject]@{pid=[long]$Root.pid;startedUtc=$Root.startedUtc;executable=$Root.executable;depth=0;parentPid=$null;parentStartedUtc=$null}
    } elseif($live.ContainsKey([long]$Root.pid)) {$issues.Add('Wrapper PID identity mismatch; never terminate the replacement process')}
    do {
        $added=$false
        foreach($row in $Snapshot) {
            if($owned.ContainsKey([long]$row.pid) -or !$owned.ContainsKey([long]$row.parentPid)){continue}
            $parent=$owned[[long]$row.parentPid]
            try {
                if([string]::IsNullOrWhiteSpace($row.executable) -or ![IO.Path]::IsPathRooted($row.executable)){throw 'Missing executable identity'}
                if(([DateTimeOffset]::Parse($row.startedUtc)) -lt ([DateTimeOffset]::Parse($parent.startedUtc))){throw 'Child predates its supposed parent'}
                $owned[[long]$row.pid]=[pscustomobject]@{pid=[long]$row.pid;startedUtc=$row.startedUtc;executable=$row.executable;depth=$parent.depth+1;parentPid=$parent.pid;parentStartedUtc=$parent.startedUtc}
                $added=$true
            } catch {$issues.Add("Cannot establish owned ancestry for PID $($row.pid): $($_.Exception.Message)")}
        }
    } while($added)
    foreach($row in $Snapshot) {
        if($owned.ContainsKey([long]$row.pid)){continue}
        $oldParent=@($Known|Where-Object{$_.pid -eq $row.parentPid}|Select-Object -First 1)
        if($oldParent.Count -eq 0){continue}
        try {
            if(([DateTimeOffset]::Parse($row.startedUtc)) -ge ([DateTimeOffset]::Parse($oldParent[0].startedUtc))){$issues.Add("Possible descendant PID $($row.pid) outlived an unobserved/reused parent; ancestry unknown, termination refused")}
        } catch {$issues.Add("Possible descendant PID $($row.pid) has unknown birth; termination refused")}
    }
    return [pscustomobject]@{known=@($Known)+@($owned.Values)|Group-Object pid|ForEach-Object{$_.Group|Select-Object -Last 1};processes=@($owned.Values|Sort-Object @{Expression='depth';Descending=$true},pid);issues=$issues.ToArray();rootAlive=$owned.ContainsKey([long]$Root.pid)}
}
function Stop-GateVerifiedProcess($Identity) {
    $handle=$null
    try {
        try {$handle=[Diagnostics.Process]::GetProcessById([int]$Identity.pid)} catch [ArgumentException] {return [pscustomobject]@{pid=$Identity.pid;state='already-exited';verified=$true}}
        # Retain this exact process handle before checking identity; Kill uses that
        # handle, not a subsequent lookup of a potentially reused PID.
        $null=$handle.Handle
        $actual=[pscustomobject]@{pid=$handle.Id;startedUtc=$handle.StartTime.ToUniversalTime().ToString('o');executable=$handle.MainModule.FileName}
        if(!(Same-OwnedIdentity $Identity $actual)){throw 'PID birth/executable changed; termination refused'}
        $handle.Kill()
        $exited=$handle.WaitForExit(500)
        return [pscustomobject]@{pid=$Identity.pid;startedUtc=$Identity.startedUtc;executable=$Identity.executable;depth=$Identity.depth;parentPid=$Identity.parentPid;parentStartedUtc=$Identity.parentStartedUtc;state=if($exited){'terminated'}else{'termination-pending'};verified=$true}
    } catch {return [pscustomobject]@{pid=$Identity.pid;state='refused-or-failed';verified=$false;reason=$_.Exception.Message}}
    finally {if($null -ne $handle){$handle.Dispose()}}
}
function Stop-GateOwnedTree($Root,[object[]]$Known) {
    $stops=New-Object 'Collections.Generic.List[object]';$errors=New-Object 'Collections.Generic.List[string]'
    $deadline=[DateTime]::UtcNow.AddSeconds(15);$remaining=@();$wrapperStopped=$false
    do {
        try {$snapshot=@(Get-GateProcessSnapshot);$plan=Plan-GateOwnedTree $Root $Known $snapshot;$Known=@($plan.known)} catch {$errors.Add($_.Exception.Message);break}
        foreach($issue in $plan.issues){$errors.Add($issue)}
        $children=@($plan.processes|Where-Object{$_.pid -ne $Root.pid})
        if($children.Count -eq 0) {
            if($errors.Count -gt 0){break}
            if($plan.rootAlive){$stop=Stop-GateVerifiedProcess ($plan.processes|Where-Object{$_.pid -eq $Root.pid}|Select-Object -First 1);$stops.Add($stop);$wrapperStopped=$stop.state -in @('terminated','already-exited')}
            else {$wrapperStopped=$true}
            break
        }
        foreach($child in $children) {
            if([DateTime]::UtcNow -ge $deadline){break}
            # Only leaves: if a child has not exited, its editor/launcher ancestor
            # stays alive for the next pass. All GPU descendants precede wrapper.
            if(@($children|Where-Object{$_.parentPid -eq $child.pid}).Count -gt 0){continue}
            $stop=Stop-GateVerifiedProcess $child;$stops.Add($stop)
            if(!$stop.verified){$errors.Add("PID $($child.pid): $($stop.reason)")}
        }
        Start-Sleep -Milliseconds 100
    } while([DateTime]::UtcNow -lt $deadline)
    try {$last=Plan-GateOwnedTree $Root $Known @(Get-GateProcessSnapshot);$remaining=@($last.processes);foreach($issue in $last.issues){$errors.Add($issue)}} catch {$errors.Add($_.Exception.Message)}
    return [pscustomobject]@{complete=($wrapperStopped -and $remaining.Count -eq 0 -and $errors.Count -eq 0);wrapperStopped=$wrapperStopped;stops=$stops.ToArray();remaining=$remaining;errors=$errors.ToArray();known=$Known;finishedUtc=[DateTime]::UtcNow.ToString('o')}
}
