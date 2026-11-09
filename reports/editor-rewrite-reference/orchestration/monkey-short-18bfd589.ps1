$ErrorActionPreference='Stop'
$fixture='D:/AraBuild/ux-editor-target/acceptance/219902aa/behaviours-104729-20261028-011212/captures/behaviour-project-14164'
$project='D:/AraBuild/ux-editor-target/acceptance/18bfd589/short-project-104729'
if(Test-Path -LiteralPath $project){throw 'Short fixture already exists'}
New-Item -ItemType Directory -Path $project | Out-Null
foreach($entry in Get-ChildItem -LiteralPath $fixture -Force){
 if($entry.Name -in @('.ara','.git','builds','packages','target','.ara-profile')){continue}
 if(($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Short fixture refuses a linked source'}
 Copy-Item -LiteralPath $entry.FullName -Destination (Join-Path $project $entry.Name) -Recurse
}
& powershell -NoProfile -ExecutionPolicy Bypass -File D:/AraBuild/ux-editor-target/orchestration/run-native.ps1 -CheckpointSHA 18bfd589 -Mode monkey -Project $project -Seed 104729 -Seconds 60
exit $LASTEXITCODE