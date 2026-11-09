$ErrorActionPreference='Stop'
$fixture='D:/AraBuild/ux-editor-target/acceptance/219902aa/behaviours-104729-20261028-011212/captures/behaviour-project-14164'
$baseline='D:/AraBuild/ux-editor-target/acceptance/219902aa/baseline-project-104729'
if(Test-Path -LiteralPath $baseline){throw 'Baseline fixture already exists'}
New-Item -ItemType Directory -Path $baseline | Out-Null
foreach($entry in Get-ChildItem -LiteralPath $fixture -Force){
 if($entry.Name -in @('.ara','.git','builds','packages','target')){continue}
 if(($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Baseline refuses a linked fixture entry'}
 Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $baseline $entry.Name) -Recurse
}
& powershell -NoProfile -ExecutionPolicy Bypass -File D:/AraBuild/ux-editor-target/orchestration/run-native.ps1 -CheckpointSHA 219902aa -Mode monkey -Project $baseline -Seed 104729 -Seconds 60
exit $LASTEXITCODE