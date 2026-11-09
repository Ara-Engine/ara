$ErrorActionPreference='Stop'
$base='2b902a47'
$branch='refs/heads/t3/ux-editor-p4-gestures-ready'
$splitIndex='D:/AraBuild/ux-editor-target/orchestration/gestures-index'
$previousIndex=$env:GIT_INDEX_FILE
$paths=@(
 'crates/ara-editor/src/shell/choices.rs',
 'crates/ara-editor/src/shell/collection_adapter.rs',
 'crates/ara-editor/src/shell/collection_adapter/append_tests.rs',
 'crates/ara-editor/src/shell/compound.rs',
 'crates/ara-editor/src/shell/input.rs',
 'crates/ara-editor/src/shell/mod.rs',
 'crates/ara-editor/tests/inspector.rs',
 'crates/ara-editor/tests/scrollbars.rs',
 'crates/ara-editor/tests/ux.rs'
)
try {
 $env:GIT_INDEX_FILE=$splitIndex
 git read-tree $base
 if($LASTEXITCODE -ne 0){throw 'Could not initialize isolated split index'}
 git add -- $paths
 if($LASTEXITCODE -ne 0){throw 'Could not stage owned gesture files'}
 foreach($path in @('crates/ara-editor/src/shell/input.rs','crates/ara-editor/src/shell/mod.rs')) {
  git apply --cached --reverse "--include=$path" D:/AraBuild/ux-editor-target/orchestration/build-handoff/restore-build-dialog.patch
  if($LASTEXITCODE -ne 0){throw "Could not remove own Build-only hooks from split index: $path"}
 }
 git diff --cached --check $base
 if($LASTEXITCODE -ne 0){throw 'Split whitespace check failed'}
 $changed=@(git diff --cached --name-only $base)
 foreach($path in $changed){if($paths -notcontains $path){throw "Unexpected split path $path"}}
 git grep --cached -n 'build_options' -- crates/ara-editor/src/shell/input.rs crates/ara-editor/src/shell/mod.rs
 if($LASTEXITCODE -eq 0){throw 'Build dialog coupling leaked into gesture split'}
 $tree=(git write-tree).Trim()
 $splitSha=('Preserve asset gestures and collection fixes independently of export integration' | git commit-tree $tree -p $base).Trim()
 if($LASTEXITCODE -ne 0){throw 'Could not create owned independent checkpoint'}
 git show-ref --verify --quiet $branch
 if($LASTEXITCODE -eq 0){throw 'Independent gesture ref already exists; no ref was overwritten'}
 git update-ref $branch $splitSha
 if($LASTEXITCODE -ne 0){throw 'Could not retain independent gesture checkpoint'}
 Write-Output "Gesture checkpoint $splitSha"
 git diff --stat $base $splitSha -- $paths
} finally {
 $env:GIT_INDEX_FILE=$previousIndex
}