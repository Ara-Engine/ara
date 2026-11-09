param([Parameter(Mandatory=$true)][string]$CheckpointSHA,[ValidateSet('behaviours','behaviours-core','docking','monkey')][string]$Mode='behaviours',[string]$Project,[UInt64]$Seed=104729,[int]$Seconds=1800,[ValidateSet('correctness-shared','correctness-queued','timing-exclusive')][string]$EvidenceMode='timing-exclusive',[string]$MonkeyReplay)
$ErrorActionPreference='Stop'
if($EvidenceMode -eq 'correctness-shared' -and $Mode -ne 'monkey'){throw 'Shared correctness mode is restricted to the offscreen editor monkey'}
$source='D:/AraBuild/ux-editor-source'
$target='D:/AraBuild/ux-editor-target'
$exe="$target/debug/ara-editor.exe"
$dev='D:/AraBuild/ux-editor-target/debug/ara-dev.exe'
$replayJournal=$null
if($MonkeyReplay) {
 if($Mode -ne 'monkey'){throw 'Recorded event replay is restricted to the monkey route'}
 $MonkeyReplay=[IO.Path]::GetFullPath($MonkeyReplay)
 if(!$MonkeyReplay.StartsWith('D:\AraBuild\ux-editor-target\acceptance\',[StringComparison]::OrdinalIgnoreCase) -or !(Test-Path -LiteralPath $MonkeyReplay -PathType Leaf)){throw 'Replay journal must be a known acceptance file below the D: target'}
 $replayFile=Get-Item -LiteralPath $MonkeyReplay
 if($replayFile.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Replay journal cannot be a reparse point'}
 $replayJournal=[ordered]@{path=$MonkeyReplay;bytes=$replayFile.Length;sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $MonkeyReplay).Hash}
}
$actual=(git -C $source rev-parse --short=8 HEAD).Trim()
if ($actual -ne $CheckpointSHA) {throw "Source mismatch: $actual"}
$run="$target/acceptance/$CheckpointSHA/$Mode-$Seed-$(Get-Date -Format yyyyMMdd-HHmmss)"
if (Test-Path -LiteralPath $run) {throw 'Run directory already exists'}
New-Item -ItemType Directory -Path $run | Out-Null
foreach ($pair in @{ARA_CONFIG_DIR='config';ARA_CRASH_DIR='crashes';DOTNET_CLI_HOME='dotnet';APPDATA='appdata';NUGET_PACKAGES='nuget';XDG_CACHE_HOME='cache';XDG_CONFIG_HOME='config'}.GetEnumerator()) {
 $path=Join-Path $run $pair.Value
 New-Item -ItemType Directory -Force -Path $path | Out-Null
 [Environment]::SetEnvironmentVariable($pair.Key,$path,'Process')
}
# Native crash IPC uses a Unix-domain socket; keep its isolated TEMP path short.
$temporary="$target/tmp/n-$([Guid]::NewGuid().ToString('N').Substring(0,8))"
New-Item -ItemType Directory -Path $temporary | Out-Null
$env:TEMP=$temporary
$env:TMP=$temporary
$local="$run/localappdata"
New-Item -ItemType Directory -Force -Path $local | Out-Null
$env:LOCALAPPDATA=$local
$env:ARA_PLAY_OFFSCREEN='1'
$env:ARA_EDITOR_MEMO_CHECK='1'
$env:DOTNET_NOLOGO='1'
$env:DOTNET_CLI_TELEMETRY_OPTOUT='1'
$env:DOTNET_GENERATE_ASPNET_CERTIFICATE='false'
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE='1'
$env:ARA_DOTNET_ARTIFACTS="$target/sdk-tools/bin"
$env:ARA_DOTNET_SDK="$source/dotnet"
$env:CARGO_TARGET_DIR=$target
$env:CARGO_BUILD_BUILD_DIR=$target
$env:CARGO_BUILD_JOBS='3'
$env:CARGO_INCREMENTAL='0'
$env:RUSTC_WRAPPER=''
$env:ARA_SHARED_BUILD_DIR=''
[Diagnostics.Process]::GetCurrentProcess().PriorityClass='BelowNormal'
if ($Mode -in @('behaviours','behaviours-core')) {$Project="$run/unused-project"}
elseif (!(Test-Path -LiteralPath "$Project/ara.project.json")) {throw 'A fresh scratch project is required'}
if ($Mode -eq 'monkey') {
 $absolute=[IO.Path]::GetFullPath($Project)
 if (!$absolute.StartsWith('D:\AraBuild\ux-editor-target\acceptance\',[StringComparison]::OrdinalIgnoreCase)) {throw 'Monkey fixture must be a fresh acceptance project on D:'}
 [IO.File]::WriteAllText("$Project/.ara-monkey-fixture","Ara isolated acceptance fixture`n")
}
$editorArguments=@($Project,"--verify-$Mode",'--output',"$run/captures",'--size','1440x900','--dpi','1')
if($MonkeyReplay){$editorArguments+=@('--monkey-replay',$MonkeyReplay)}
if($EvidenceMode -eq 'correctness-shared') {$launch=$exe;$arguments=$editorArguments;$processRole='editor';$frameTimeStatus='informal (shared GPU)'}
else {$launch=$dev;$arguments=@('gpu','--wait-for-quiet','--timeout','300','--',$exe)+$editorArguments;$processRole='ara-dev gpu wrapper';$frameTimeStatus=if($EvidenceMode -eq 'correctness-queued'){'informal (shared GPU)'}else{'unverified (exclusive GPU requested)'}}
if ($EvidenceMode -eq 'correctness-queued') {$arguments=@('gpu','--wait-for-quiet','--foreign-cpu-percent','100','--timeout','300','--',$exe)+$editorArguments}
if ($Mode -eq 'monkey') {$arguments+=@('--monkey-seed',"$Seed",'--monkey-seconds',"$Seconds")}
if ($EvidenceMode -ne 'correctness-shared') {
 # The admission wrapper shares the machine queue; only its editor child receives
 # an isolated LOCALAPPDATA. Giving the wrapper that profile would fork its queue.
 $sharedLocal=[Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
 if([string]::IsNullOrWhiteSpace($sharedLocal)){throw 'The machine GPU queue directory is unavailable'}
 $launchConfig="$run/editor-launch.json"
 $launchArguments=@($editorArguments)
 if ($Mode -eq 'monkey') {$launchArguments+=@('--monkey-seed',"$Seed",'--monkey-seconds',"$Seconds")}
 [IO.File]::WriteAllText($launchConfig,([ordered]@{executable=$exe;arguments=$launchArguments;localAppData=$local}|ConvertTo-Json -Depth 10),[Text.UTF8Encoding]::new($false))
 $nativeShell=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
 $child="$target/orchestration/native-child.ps1"
 $delimiter=[Array]::IndexOf($arguments,'--')
 if($delimiter -lt 0){throw 'GPU admission has no child delimiter'}
 $arguments=@($arguments[0..$delimiter])+@($nativeShell,'-NoLogo','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',$child,'-LaunchConfig',$launchConfig)
 $env:LOCALAPPDATA=$sharedLocal
}
$manifest=[ordered]@{checkpoint=$CheckpointSHA;mode=$Mode;seed=$Seed;arguments=$arguments;source=$source;editorHash=(Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash;playerHash=(Get-FileHash -Algorithm SHA256 -LiteralPath "$target/debug/ara-player.exe").Hash;output=$run;temporary=$temporary;started=[DateTime]::UtcNow.ToString('o');completed=$false;accepted=$false;
 requestedEvidenceMode=$EvidenceMode;frameTimeStatus=$frameTimeStatus;timingAccepted=$false;gpuWrapperUsed=($EvidenceMode -ne 'correctness-shared');launchExecutable=$launch;processRole=$processRole;gpuToolHash=(Get-FileHash -Algorithm SHA256 -LiteralPath $dev).Hash;editorLocalAppData=$local;gpuQueueDirectory=if($EvidenceMode -ne 'correctness-shared'){Join-Path $sharedLocal 'ara/gpu-queue'}else{$null};childLauncher=if($EvidenceMode -ne 'correctness-shared'){Get-FileHash -Algorithm SHA256 -LiteralPath $child | Select-Object Path,Hash}else{$null}}
$process=Start-Process -FilePath $launch -ArgumentList $arguments -WorkingDirectory $source -WindowStyle Hidden -PassThru -RedirectStandardOutput "$run/stdout.log" -RedirectStandardError "$run/stderr.log"
# Windows PowerShell 5 must retain the native handle before waiting to preserve ExitCode.
$null=$process.Handle
$manifest.pid=$process.Id
$manifest.replayJournal=$replayJournal
$manifest.processStarted=$process.StartTime.ToUniversalTime().ToString('o')
$manifest | ConvertTo-Json -Depth 10 | Set-Content -Encoding UTF8 -LiteralPath "$run/run.json"
Write-Output "Native $Mode ($EvidenceMode) PID $($process.Id), evidence $run"
$timeout=if ($Mode -eq 'monkey') {($Seconds+600)*1000} else {1200000}
$completed=$process.WaitForExit($timeout)
$manifest.completed=$completed
if ($completed) {$process.WaitForExit();$process.Refresh();$manifest.exitCode=$process.ExitCode;$manifest.finished=[DateTime]::UtcNow.ToString('o');if($null -eq $manifest.exitCode){$manifest.exitCodeUnavailable=$true}}
else {$manifest.timeout=$true;$manifest.note='Owned process was not terminated automatically; inspect exact PID and birth time before stopping it.'}
if($EvidenceMode -eq 'timing-exclusive' -and $completed -and $null -ne $manifest.exitCode -and $manifest.exitCode -eq 0){$manifest.timingAccepted=$true;$manifest.frameTimeStatus='formal (exclusive GPU)'}
$manifest | ConvertTo-Json -Depth 10 | Set-Content -Encoding UTF8 -LiteralPath "$run/run.json"
Get-Content -LiteralPath "$run/stderr.log" | Where-Object {$_ -notlike 'ARA_DEV_GPU_READING*'} | Select-Object -Last 40
if (!$completed) {exit 124}
if ($null -eq $manifest.exitCode) {exit 1}
exit $manifest.exitCode
