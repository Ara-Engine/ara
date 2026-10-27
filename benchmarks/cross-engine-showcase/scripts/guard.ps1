param([Parameter(Mandatory=$true)][string]$Label)
$ErrorActionPreference='Stop'
$stamp=Get-Date -Format 'yyyyMMdd-HHmmss'
$dest="D:\AraBench\logs\guard-$Label-$stamp.txt"
$pmon=& nvidia-smi pmon -c 1 2>&1
$pmon | Out-File -LiteralPath $dest -Encoding utf8
& nvidia-smi --query-gpu=name,driver_version,temperature.gpu,clocks.current.graphics,clocks.current.memory,power.draw,utilization.gpu,memory.used --format=csv | Add-Content -LiteralPath $dest
$all=Get-CimInstance Win32_Process
$all | Select-Object ProcessId,Name,ExecutablePath | ConvertTo-Json -Depth 3 | Add-Content -LiteralPath $dest
$games=$all | Where-Object {
    $_.Name -match '^(VALORANT.*|obs64|obs32|cs2|dota2|FortniteClient.*|GTA5.*|Overwatch|RocketLeague|r5apex|Minecraft.*|javaw)\.exe$' -or
    ($_.ExecutablePath -match '\\steamapps\\common\\' -and $_.Name -notmatch 'crash|helper|service') -or
    ($_.Name -match 'Shipping\.exe$' -and $_.ExecutablePath -notlike 'D:\AraBench\*')
}
if($games) { $games | Format-List ProcessId,Name,ExecutablePath; throw "Aborted: foreign game or recorder present. Evidence: $dest" }
Write-Output "Preflight clean: $dest. ara-dev gpu will also measure foreign GPU load."
