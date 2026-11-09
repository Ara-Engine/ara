$ErrorActionPreference='Stop'
Set-Location 'C:/Users/snowy/.t3/worktrees/snengine/t3-9c4a1f92'
$sliceDir='D:/AraBuild/ux-editor-target/orchestration/pointer-slice'
New-Item -ItemType Directory -Force -Path $sliceDir | Out-Null
$utf8=[Text.UTF8Encoding]::new($false)
function Replace-Once([string]$text,[string]$old,[string]$new) {
    $count=([regex]::Matches($text,[regex]::Escape($old))).Count
    if($count -ne 1){throw "Expected exactly one anchor, got $count : $old"}
    return $text.Replace($old,$new)
}
function Stage-Text([string]$path,[string]$text) {
    $tmp=Join-Path $sliceDir ($path.Replace('/','-'))
    [IO.File]::WriteAllText($tmp,$text,$utf8)
    & rustfmt --edition 2024 --config skip_children=true $tmp
    if($LASTEXITCODE -ne 0){throw "rustfmt $path failed"}
    $blob=git hash-object -w -- $tmp
    if($LASTEXITCODE -ne 0){throw "hash $path failed"}
    git update-index --cacheinfo "100644,$blob,$path"
    if($LASTEXITCODE -ne 0){throw "index $path failed"}
}
$path='crates/ara-editor/src/shell/mod.rs'
$text=(git show "HEAD:$path") -join "`n"
$text+="`n"
$old='    fn stylesheet(&self, editor: &Editor) -> String {'
$new=@'
    /// Pointer use makes Alt part of a chord rather than a menubar tap.
    pub(crate) fn note_pointer_chord(&mut self) {
        self.alt_tap = false;
    }
    /// End pointer capture on every native surface while retaining modal scopes.
    pub(crate) fn cancel_pointer_capture(&mut self) {
        self.input.cancel_interaction(&mut self.ui);
        for surface in &mut self.floats {
            surface.input.cancel_interaction(&mut surface.ui);
        }
    }
    fn stylesheet(&self, editor: &Editor) -> String {
'@
$text=Replace-Once $text $old $new
Stage-Text $path $text
$path='crates/ara-editor/src/shell/input.rs'
$text=((git show "HEAD:$path") -join "`n")+"`n"
$old="        if let Some(host) = &mut self.toolkit_verify {`n            host.event"
$new=@'
        if matches!(event, InputEvent::PointerDown(_) | InputEvent::Wheel { .. })
            || (matches!(event, InputEvent::PointerMove(_))
                && (self.input.captured().is_some()
                    || self.input.scrolling()
                    || self.input.scrubbing()))
        {
            self.note_pointer_chord();
        }
        if let Some(host) = &mut self.toolkit_verify {
            host.event
'@
$text=Replace-Once $text $old $new
Stage-Text $path $text
$path='crates/ara-editor/src/app/mod.rs'
$text=((git show "HEAD:$path") -join "`n")+"`n"
$old=@'
                    &scene_view,
                    &mut events,
                )?;
            }
            if selection_check
'@
$new=@'
                    &scene_view,
                    &mut events,
                    &controller,
                )?;
            }
            if selection_check
'@
$text=Replace-Once $text $old $new
$old=@'
            if let Some(check) = &mut selection_check {
                check.end(&editor, &mut shell, &mut gpu)?;
'@
$new=@'
            if let Some(check) = &mut selection_check {
                check.end(&editor, &mut shell, &mut gpu, &controller)?;
'@
$text=Replace-Once $text $old $new
$old=@'
    if shell.game_captured && (shell.modal_active() || shell.choice_open()) {
'@
$new=@'
    if matches!(event, Event::MouseButton { pressed: true, .. } | Event::Wheel(_)) {
        shell.note_pointer_chord();
    }
    if shell.game_captured && (shell.modal_active() || shell.choice_open()) {
'@
$text=Replace-Once $text $old $new
$old=@'
                shell.drag = None;
                shell.release_tab_capture(clipboard)?;
                controller.release(1);
                return Ok((true, false, false));
'@
$new=@'
                controller.cancel_pointer_gestures(editor, shell)?;
                return Ok((true, true, false));
'@
$text=Replace-Once $text $old $new
$old=@'
                shell.drag = None;
                shell.drop_preview = None;
                shell.release_tab_capture(clipboard)?;
                controller.release(1);
                for float in &mut floats.windows {
                    float.dirty = true;
                }
                return Ok((true, false, false));
'@
$new=@'
                controller.cancel_pointer_gestures(editor, shell)?;
                for float in &mut floats.windows {
                    float.dirty = true;
                }
                return Ok((true, true, false));
'@
$text=Replace-Once $text $old $new
Stage-Text $path $text
$path='crates/ara-editor/src/shell/quality_tests.rs'
$text=((git show "HEAD:$path") -join "`n")+"`n"
$text=Replace-Once $text 'mod measure;' "mod measure;`nmod row_sizing;"
Stage-Text $path $text
git add -- crates/ara-editor/src/app/controller.rs crates/ara-editor/src/app/controller/selection_gesture_tests.rs crates/ara-editor/src/app/selection_verify.rs crates/ara-editor/src/app/selection_verify/fixture.rs crates/ara-editor/src/app/selection_verify/input.rs crates/ara-editor/src/app/selection_verify/observe.rs crates/ara-editor/src/app/snap_verify.rs crates/ara-editor/src/app/snap_verify/fixture.rs crates/ara-editor/src/app/snap_verify/input.rs crates/ara-editor/src/app/snap_verify/proof.rs crates/ara-editor/src/shell/dock_drag.rs crates/ara-editor/src/shell/editor.css crates/ara-editor/src/shell/editor-comfortable.css crates/ara-editor/src/shell/quality_tests/row_sizing.rs
if($LASTEXITCODE -ne 0){throw 'git add failed'}
git diff --cached --check
if($LASTEXITCODE -ne 0){throw 'staged whitespace check failed'}
git diff --cached --stat
