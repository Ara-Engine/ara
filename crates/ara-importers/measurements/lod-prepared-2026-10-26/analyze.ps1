$ErrorActionPreference='Stop'
Set-Location 'C:/Users/snowy/.t3/worktrees/snengine/t3-602598ba'
$root="$PWD/target/ara-dev/import-speed/lod-prepared"
$culture=[Globalization.CultureInfo]::InvariantCulture
$cases=@(foreach($label in @('a1','b1','b2','a2')){
 if((Get-Content "$root/$label.exit").Trim()-ne'0'){throw "case failed $label"}
 $data=Get-Content "$root/$label.json" -Raw|ConvertFrom-Json
 if($data.models-ne101-or$data.threads-ne1){throw 'wrong corpus'}
 $lines=[IO.File]::ReadAllLines("$root/$label.log")
 $counts=@($lines|Where-Object {$_.StartsWith('ARA_PREPARED_COUNTS,')})
 if($counts.Count-ne1){throw 'missing counters'}
 $count=$counts[0].Split(',')
 if($count.Length-ne9-or$count[2]-ne'335'-or$count[5]-ne'1149'-or$count[6]-ne'897'){throw 'wrong native calls'}
 if($count[1]-eq'1'-and($count[3]-ne'270'-or$count[4]-ne'879')){throw 'wrong reuse count'}
 $times=[ordered]@{}
 foreach($line in $lines|Where-Object {$_.StartsWith('ARA_PREPARED_TIME,')}){
  $parts=$line.Split(',');if($parts.Length-ne4){throw 'bad timer record'}
  $times[$parts[1]]=[pscustomobject]@{cycles=[double]::Parse($parts[2],$culture);ms=[double]::Parse($parts[3],$culture)}
 }
 if($times.Count-ne5){throw 'missing timers'}
 function Stage([string]$name){($data.assets|ForEach-Object {[double]$_.stages.$name}|Measure-Object -Sum).Sum}
 [pscustomobject]@{label=$label;mode=[int]$count[1];models=$data.models;completeOutputBytesEqual=$true;scopeCount=[int]$count[2];contexts=[int]$count[3];resets=[int]$count[4];attributeCalls=[int]$count[5];positionCalls=[int]$count[6];resetBytes=[double]$count[7];peakSnapshotBytes=[double]$count[8];timers=$times;nativeLodCycles=$times.attributes.cycles+$times.positions.cycles+$times.destroy.cycles;nativeLodMs=$times.attributes.ms+$times.positions.ms+$times.destroy.ms;lodSimplificationMs=Stage 'lodSimplificationMs';lodsMs=Stage 'lodsMs';lodPreparationMs=Stage 'lodPreparationMs';virtualGeometryMs=Stage 'virtualGeometryMs';load=(Get-Content "$root/$label-load.json" -Raw|ConvertFrom-Json);assets=$data.assets}
})
$hashes=@($cases.load.binarySha256|Select-Object -Unique)
if($hashes.Count-ne1){throw 'binary differs'}
function Mean([object[]]$items,[string]$name){($items|ForEach-Object {[double]$_.$name}|Measure-Object -Average).Average}
$a=@($cases|Where-Object mode -EQ 0);$b=@($cases|Where-Object mode -EQ 1)
$means=[ordered]@{}
foreach($name in @('nativeLodCycles','nativeLodMs','lodSimplificationMs','lodsMs','virtualGeometryMs')){
 $ac=Mean $a $name;$bc=Mean $b $name
 $means[$name]=[pscustomobject]@{control=$ac;prepared=$bc;savingPercent=(1-$bc/$ac)*100;speedup=$ac/$bc}
}
$report=[ordered]@{date='2026-10-26';definition='Single-thread real Outpost geometry phase ABBA; same binary mode switch, fresh processes and production-cache full output comparisons; no end-to-end cold-import claim';provenance=(Get-Content "$root/provenance.json" -Raw|ConvertFrom-Json);cases=$cases;means=$means;resetAccounting='Native attribute timers already include initial setup/snapshot construction and every reset. Add context destruction once; do not add prepare/reset again. lodsMs also includes context destruction.';thresholdPercent=10;clearsThreshold=$means.nativeLodCycles.savingPercent-ge10-and$means.lodsMs.savingPercent-ge10;caveats=@('Integration/background load possible; per-case load retained','Only native attr mode97 inside explicit per-primitive lexical scope is prepared','All other simplify modes use unchanged original function','Snapshot peak is pristine bytes only, not total working memory','No shipping dependency, algorithm, importer version, key or sidecar changes')}
$report|ConvertTo-Json -Depth 30|Set-Content "$root/report-input.json"
$cases|Select-Object label,nativeLodMs,nativeLodCycles,lodSimplificationMs,lodsMs,virtualGeometryMs,peakSnapshotBytes|Format-Table
$means|ConvertTo-Json -Depth 8
"Clears 10% phase threshold: $($report.clearsThreshold)"
