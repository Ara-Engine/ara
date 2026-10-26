param([string]$Label='sanity',[ValidateSet(0,1)][int]$Mode=1,[string]$Filter='')
$ErrorActionPreference='Stop'
Set-Location 'C:/Users/snowy/.t3/worktrees/snengine/t3-602598ba'
$root="$PWD/target/ara-dev/import-speed/lod-prepared"
if(Test-Path "$PWD/target/ara-dev/import-speed/HOLD"){exit 99}
$p=Get-Content "$root/provenance.json" -Raw|ConvertFrom-Json
if((Get-FileHash $p.binary).Hash-ne$p.binarySha256){throw 'binary changed'}
$native=Get-Content "$PWD/target/ara-dev/import-speed/native-paths.json" -Raw|ConvertFrom-Json
$env:PATH=(@($native)+@($env:PATH-split';')|Select-Object -Unique)-join';'
$env:ARA_PREPARED_LOD="$Mode"
Remove-Item Env:ARA_MESHOPT_CAPTURE_PATH -ErrorAction SilentlyContinue
$args=@('D:/AraBuild/ara-dev-infra-target/import-speed/projects/cold-mesh-profile-a0-outpost','1')
if($Filter){$args+=$Filter}
@{label=$Label;mode=$Mode;utc=[DateTime]::UtcNow.ToString('o');cpuPercent=(Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'").PercentProcessorTime;availableMemoryKiB=(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory;binarySha256=$p.binarySha256}|ConvertTo-Json|Set-Content "$root/$Label-load.json"
$process=Start-Process -FilePath $p.binary -ArgumentList $args -WindowStyle Hidden -PassThru -RedirectStandardOutput "$root/$Label.json" -RedirectStandardError "$root/$Label.log"
$handle=$process.Handle
if(-not$process.WaitForExit(180000)){$process.Kill();$process.WaitForExit();throw 'owned replay timed out'}
$code=$process.ExitCode
$code|Set-Content "$root/$Label.exit"
if($code){Get-Content "$root/$Label.log" -Tail 15;exit $code}
$data=Get-Content "$root/$Label.json" -Raw|ConvertFrom-Json
if($data.threads-ne1-or-not$data.models){throw 'wrong replay corpus'}
Get-Content "$root/$Label.log" -Tail 6
