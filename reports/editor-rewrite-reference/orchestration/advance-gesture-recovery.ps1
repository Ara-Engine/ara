$ErrorActionPreference='Stop'
$branch='refs/heads/t3/ux-editor-p4-gestures-ready'
$base=(git rev-parse $branch).Trim()
$oldIndex=$env:GIT_INDEX_FILE
$files=@(
 'crates/ara-editor/src/app/import_verify.rs',
 'crates/ara-editor/src/app/import_verify/fixture.rs',
 'crates/ara-editor/src/app/import_verify/stream.rs',
 'crates/ara-editor/src/app/import_verify/recovery.rs',
 'crates/ara-editor/tests/scrollbars.rs',
 'crates/ara-editor/src/editor.rs',
 'crates/ara-editor/src/editor/availability_tests.rs',
 'crates/ara-editor/src/world/mod.rs',
 'crates/ara-editor/src/world/resolver.rs',
 'crates/ara-editor/src/shell/hierarchy.rs',
 'crates/ara-editor/src/shell/inspector_panel.rs',
 'crates/ara-editor/src/shell/mod.rs'
)
try {
 $env:GIT_INDEX_FILE='D:/AraBuild/ux-editor-target/orchestration/gesture-recovery-index'
 git read-tree $base
 if($LASTEXITCODE -ne 0){throw 'Could not load independent checkpoint'}
 git add -- $files
 if($LASTEXITCODE -ne 0){throw 'Could not stage independent recovery fixes'}
 git apply --cached --reverse --include=crates/ara-editor/src/shell/mod.rs D:/AraBuild/ux-editor-target/orchestration/build-handoff/restore-build-dialog.patch
 if($LASTEXITCODE -ne 0){throw 'Could not remove own Build hooks from independent recovery'}
 git grep --cached -n 'build_options' -- crates/ara-editor/src/shell/mod.rs
 if($LASTEXITCODE -eq 0){throw 'Independent source still references Build UI'}
 git diff --cached --check $base
 if($LASTEXITCODE -ne 0){throw 'Independent recovery whitespace failure'}
 $tree=(git write-tree).Trim()
 $sha=('Recover removed active scenes and native import transitions' | git commit-tree $tree -p $base).Trim()
 if($LASTEXITCODE -ne 0){throw 'Could not retain independent recovery'}
 git update-ref $branch $sha $base
 if($LASTEXITCODE -ne 0){throw 'Concurrent independent branch change'}
 Write-Output "Independent recovery $sha"
} finally {$env:GIT_INDEX_FILE=$oldIndex}