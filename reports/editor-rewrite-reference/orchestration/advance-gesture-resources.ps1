$ErrorActionPreference='Stop'
$branch='refs/heads/t3/ux-editor-p4-gestures-ready'
$base=(git rev-parse $branch).Trim()
$oldIndex=$env:GIT_INDEX_FILE
$files=@(
 'crates/ara-editor/src/app/import_verify.rs',
 'crates/ara-editor/src/app/import_verify/fixture.rs',
 'crates/ara-editor/src/app/import_verify/recovery.rs',
 'crates/ara-editor/src/shell/asset_import/view.rs',
 'crates/ara-editor/src/shell/asset_import/view/layout_tests.rs',
 'crates/ara-editor/src/authoring/mod.rs',
 'crates/ara-editor/src/authoring/commands.rs',
 'crates/ara-editor/src/authoring/commands/resource.rs',
 'crates/ara-editor/src/editor/availability_tests.rs',
 'crates/ara-editor/src/shell/animation_panel.rs',
 'crates/ara-editor/src/shell/mod.rs'
)
try {
 $env:GIT_INDEX_FILE='D:/AraBuild/ux-editor-target/orchestration/gesture-resource-index'
 git read-tree $base
 if($LASTEXITCODE -ne 0){throw 'Could not load independent checkpoint'}
 git add -- $files
 if($LASTEXITCODE -ne 0){throw 'Could not stage independent source'}
 git apply --cached --reverse --include=crates/ara-editor/src/shell/mod.rs D:/AraBuild/ux-editor-target/orchestration/build-handoff/restore-build-dialog.patch
 if($LASTEXITCODE -ne 0){throw 'Could not remove own Build hooks'}
 git apply --cached D:/AraBuild/ux-editor-target/orchestration/modal-focus-regression.patch
 if($LASTEXITCODE -ne 0){throw 'Could not stage shared focus regression'}
 git grep --cached -n 'build_options' -- crates/ara-editor/src/shell/mod.rs
 if($LASTEXITCODE -eq 0){throw 'Independent source still references Build options'}
 git diff --cached --check $base
 if($LASTEXITCODE -ne 0){throw 'Independent source whitespace failure'}
 $tree=(git write-tree).Trim()
 $sha=('Recover resource lifecycle and expose native import job actions' | git commit-tree $tree -p $base).Trim()
 if($LASTEXITCODE -ne 0){throw 'Could not retain independent source'}
 git update-ref $branch $sha $base
 if($LASTEXITCODE -ne 0){throw 'Concurrent independent branch change'}
 Write-Output "Independent resource checkpoint $sha"
} finally {$env:GIT_INDEX_FILE=$oldIndex}