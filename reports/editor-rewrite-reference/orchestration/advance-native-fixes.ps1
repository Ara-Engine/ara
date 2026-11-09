$ErrorActionPreference='Stop'
$branch='refs/heads/t3/ux-editor-p4-gestures-ready'
$base=(git rev-parse $branch).Trim()
$previousIndex=$env:GIT_INDEX_FILE
try {
 $env:GIT_INDEX_FILE='D:/AraBuild/ux-editor-target/orchestration/gestures-advance-index'
 git read-tree $base
 if($LASTEXITCODE -ne 0){throw 'Could not load gesture checkpoint'}
 git add -- crates/ara-editor/src/app/asset_ops_verify/fixture.rs crates/ara-editor/tests/scrollbars.rs
 if($LASTEXITCODE -ne 0){throw 'Could not add owned native fixes'}
 $app=[IO.File]::ReadAllText((Join-Path $PWD 'crates/ara-editor/src/app/mod.rs')).Replace("`r`n","`n")
 $pattern='(?m)^\s*shell\.poll_build_options\(editor\),\n'
 if([regex]::Matches($app,$pattern).Count -ne 1){throw 'Expected exactly one Build-only poll in current source'}
 $app=[regex]::Replace($app,$pattern,'')
 $tempApp='D:/AraBuild/ux-editor-target/orchestration/gesture-app-mod.rs'
 [IO.File]::WriteAllText($tempApp,$app,[Text.UTF8Encoding]::new($false))
 $blob=(git hash-object -w $tempApp).Trim()
 git update-index --cacheinfo 100644 $blob crates/ara-editor/src/app/mod.rs
 if($LASTEXITCODE -ne 0){throw 'Could not stage independent async presentation fix'}
 git diff --cached --check $base
 if($LASTEXITCODE -ne 0){throw 'Independent fixes whitespace failure'}
 $tree=(git write-tree).Trim()
 $sha=('Reconcile asynchronous native controls and validate isolated capture paths' | git commit-tree $tree -p $base).Trim()
 if($LASTEXITCODE -ne 0){throw 'Could not commit independent native fixes'}
 git update-ref $branch $sha $base
 if($LASTEXITCODE -ne 0){throw 'Could not advance gesture checkpoint safely'}
 Write-Output "Independent native fixes $sha"
} finally {$env:GIT_INDEX_FILE=$previousIndex}