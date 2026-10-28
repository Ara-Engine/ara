$ErrorActionPreference='Stop'
$workspace='C:/Users/snowy/.t3/worktrees/snengine/t3-9c4a1f92'
Set-Location $workspace
$handoff='D:/AraBuild/ux-editor-target/orchestration/build-handoff'
$utf8=[Text.UTF8Encoding]::new($false)
$paths=@('crates/ara-editor/src/shell/mod.rs','crates/ara-editor/src/app/mod.rs','crates/ara-editor/src/shell/commands/table.rs','crates/ara-editor/src/shell/commands/menus.rs','crates/ara-editor/src/shell/menu.rs','crates/ara-editor/src/shell/input.rs','crates/ara-editor/src/shell/keyboard.rs','crates/ara-editor/src/shell/confirm.rs','crates/ara-editor/src/settings.rs','crates/ara-editor/src/shell/quality_tests.rs')
if(Test-Path -LiteralPath $handoff){throw 'Build handoff already exists; inspect rather than overwrite'}
function Rewrite([string]$value,[string]$pattern,[string]$replacement,[int]$expected=1) {
  $count=([regex]::Matches($value,$pattern)).Count
  if($count -ne $expected){throw "Expected $expected matches, got $count : $pattern"}
  return [regex]::Replace($value,$pattern,$replacement)
}
function Span([string]$value,[string]$start,[string]$end) {
  $a=$value.IndexOf($start,[StringComparison]::Ordinal)
  if($a -lt 0){throw "Missing span start $start"}
  $b=$value.IndexOf($end,$a+$start.Length,[StringComparison]::Ordinal)
  if($b -lt 0){throw "Missing span end $end"}
  return $value.Substring($a,$b-$a)
}
$source=@{}
foreach($path in $paths){
  $text=[IO.File]::ReadAllText((Join-Path $workspace $path)).Replace("`r`n","`n")
  $source[$path]=$text
  $copy=Join-Path "$handoff/with-build" $path
  New-Item -ItemType Directory -Force -Path (Split-Path $copy) | Out-Null
  [IO.File]::WriteAllText($copy,$text,$utf8)
  if((Get-FileHash -LiteralPath $path).Hash -ne (Get-FileHash -LiteralPath $copy).Hash){throw "Backup mismatch $path"}
}
$path='crates/ara-editor/src/shell/mod.rs';$text=$source[$path]
$text=Rewrite $text '(?m)^mod build_options;\n' ''
$text=Rewrite $text '(?m)^    build_options: build_options::State,\n' ''
$text=Rewrite $text '(?m)^            build_options: Default::default\(\),\n' ''
$text=Rewrite $text '(?m)^        self\.build_options_markup\(editor, &mut markup\);\n' '' 2
$source[$path]=$text
$path='crates/ara-editor/src/app/mod.rs';$text=$source[$path]
$text=Rewrite $text '(?m)^                shell\.poll_build_options\(&mut editor\),\n' ''
$source[$path]=$text
$path='crates/ara-editor/src/shell/commands/table.rs';$text=$source[$path]
$base=((git show "HEAD:$path") -join "`n")+"`n"
$newBlock=Span $text '        cmd("build.profiles"' '        cmd("file.close_document"'
$oldBlock=Span $base '        later("build.profiles"' '        cmd("file.close_document"'
$source[$path]=$text.Replace($newBlock,$oldBlock)
$path='crates/ara-editor/src/shell/commands/menus.rs';$text=$source[$path]
$text=Rewrite $text '(?m)^    /// Cached canonical and legacy build profiles, selected through typed preferences\.\n    BuildProfiles,\n' ''
$text=Rewrite $text '(?m)^            Self::BuildProfiles => "build\.profile\.select",\n' ''
$text=Rewrite $text '(?m)^                    C\("build\.destination"\),\n' ''
$text=Rewrite $text '(?m)^                    Sub\("Selected Profile", &\[Item::Dynamic\(Dynamic::BuildProfiles\)\]\),\n' ''
$source[$path]=$text
$path='crates/ara-editor/src/shell/menu.rs';$text=$source[$path]
$text=Rewrite $text '(?ms)^            Dynamic::BuildProfiles => \{.*?(?=^            Dynamic::PrefabSources)' ''
$source[$path]=$text
$path='crates/ara-editor/src/shell/input.rs';$text=$source[$path]
$replacement=@'
        if let Some(event) = self.choice_event(event, clipboard)? {
            self.input.event(&mut self.ui, event, clipboard)?;
        } else {
            self.choice_input_dirty
'@
$text=Rewrite $text '(?ms)^        if self\.modal_root == Some\("build-options"\).*?^        \} else \{\n            self\.choice_input_dirty' $replacement
$source[$path]=$text
$path='crates/ara-editor/src/shell/keyboard.rs';$text=$source[$path]
$text=Rewrite $text '(?ms)^        if self\.modal_root == Some\("build-options"\) && self\.current\.is_some\(\) \{\n.*?^        \}\n' ''
$text=Rewrite $text '(?ms)^        if self\.modal_root == Some\("build-options"\) \{.*?(?=^        if scancode == 226)' ''
$text=Rewrite $text '(?ms)^    #\[test\]\n    fn build_options_traps_keys_stacks_select_and_dialog_then_restores_focus.*?(?=^    #\[test\])' ''
$source[$path]=$text
$path='crates/ara-editor/src/shell/confirm.rs';$text=$source[$path]
$text=Rewrite $text '(?m)^            \|\| self\.build_options_open\(\)\n' ''
$text=Rewrite $text '(?ms)^        \} else if self\.build_options_open\(\) \{\n.*?(?=^        \} else if editor\.state\.palette)' ''
$text=Rewrite $text '(?ms)^            if self\.modal_root == Some\("build-options"\) \{\n.*?^            \}\n' ''
$text=Rewrite $text '(?ms)^        if root == Some\("build-options"\) \{\n.*?^        \}\n' ''
$source[$path]=$text
$path='crates/ara-editor/src/settings.rs';$text=$source[$path]
$text=Rewrite $text '(?ms)^    def\(\n        "build\.profile",.*?(?=^    def\()' ''
$source[$path]=$text
$path='crates/ara-editor/src/shell/quality_tests.rs';$text=$source[$path]
$text=Rewrite $text '(?ms)^        shell\.open_build_options\(&mut editor\)\.unwrap\(\);\n.*?^        shell\.close_build_options\(&mut editor\)\.unwrap\(\);\n' ''
$source[$path]=$text
foreach($path in $paths){[IO.File]::WriteAllText((Join-Path $workspace $path),$source[$path],$utf8)}
& rustfmt --edition 2024 --config skip_children=true @paths
if($LASTEXITCODE -ne 0){throw 'Formatting asset-only files failed; original files are backed up'}
$patch=[Collections.Generic.List[string]]::new()
foreach($path in $paths){
  $current=Join-Path "$handoff/asset-only" $path
  New-Item -ItemType Directory -Force -Path (Split-Path $current) | Out-Null
  Copy-Item -LiteralPath $path -Destination $current
  $full=Join-Path "$handoff/with-build" $path
  $diff=git diff --no-index -- $current $full
  if($LASTEXITCODE -gt 1){throw "Diff failed $path"}
  foreach($line in $diff){
    if($line.StartsWith('diff --git ')){$patch.Add("diff --git a/$path b/$path")}
    elseif($line.StartsWith('--- ')){$patch.Add("--- a/$path")}
    elseif($line.StartsWith('+++ ')){$patch.Add("+++ b/$path")}
    else{$patch.Add($line)}
  }
}
[IO.File]::WriteAllText("$handoff/restore-build-dialog.patch",($patch -join "`n")+"`n",$utf8)
git apply --check "$handoff/restore-build-dialog.patch"
if($LASTEXITCODE -ne 0){throw 'Future Build dialog patch does not apply to the asset-only source'}
git diff --check
if($LASTEXITCODE -ne 0){throw 'Whitespace errors after separation'}
Write-Output "Build dialog preserved in $handoff/restore-build-dialog.patch and unlinked build_options.rs; asset source is independent."
