$ErrorActionPreference='Stop'
Set-Location 'C:/Users/snowy/.t3/worktrees/snengine/t3-602598ba'
$repo="$PWD".Replace('\','/')
$root="$repo/target/ara-dev/import-speed/lod-prepared"
$copy='D:/AraBuild/ara-dev-infra-target/lod-prepared/importers'
if(Test-Path "$repo/target/ara-dev/import-speed/HOLD"){throw 'hold active'}
New-Item -ItemType Directory -Force "$copy/src","$root/src" | Out-Null
Copy-Item -Path "$repo/crates/ara-importers/src/*" -Destination "$copy/src" -Recurse -Force
$workspace=[IO.File]::ReadAllText("$repo/Cargo.toml")
$manifest=[IO.File]::ReadAllText("$repo/crates/ara-importers/Cargo.toml")
$manifest=$manifest.Replace('rust-version.workspace = true','rust-version = "1.99"').Replace('version.workspace = true','version = "0.0.1-alpha"').Replace('edition.workspace = true','edition = "2024"').Replace('publish.workspace = true','publish = false')
$manifest=$manifest.Replace("[lints]`nworkspace = true",'').Replace("[lints]`r`nworkspace = true",'')
$output=@()
foreach($line in $manifest.Replace("`r`n","`n").Split("`n")){
 if($line-match'^([\w-]+)\.workspace = true$'){
  $name=$Matches[1];$base=[regex]::Match($workspace,'(?m)^'+[regex]::Escape($name)+' = (.+)$').Groups[1].Value
  if(-not$base){throw "missing root dependency $name"}
  $line="$name = $base"
 }elseif($line-match'^([\w-]+) = \{ workspace = true(.*)\}$'){
  $name=$Matches[1];$tail=$Matches[2];$base=[regex]::Match($workspace,'(?m)^'+[regex]::Escape($name)+' = (.+)$').Groups[1].Value.Trim()
  if(-not$base){throw "missing root dependency $name"}
  $inner=if($base.StartsWith('{')){$base.TrimStart('{').TrimEnd('}').Trim()}else{"version = $base"}
  $line="$name = { $inner$tail}"
 }
 $line=$line.Replace('path = "../','path = "'+$repo+'/crates/').Replace('path = "crates/','path = "'+$repo+'/crates/')
 $output+=$line
}
[IO.File]::WriteAllText("$copy/Cargo.toml",($output-join"`n"))
$lod=[IO.File]::ReadAllText("$copy/src/mesh/lods.rs")
$scope=@'

unsafe extern "C" { fn ara_prepared_begin(); fn ara_prepared_end(); }
struct PreparedScope;
impl PreparedScope {
    fn enter() -> Self {
        // SAFETY: Isolated single-thread replay; each append owns one native context.
        unsafe { ara_prepared_begin(); }
        Self
    }
}
impl Drop for PreparedScope {
    fn drop(&mut self) {
        // SAFETY: The same lexical append releases its native context on every exit.
        unsafe { ara_prepared_end(); }
    }
}
'@
$lod=$lod.Replace('fn useful(',"$scope`nfn useful(")
$lod=$lod.Replace('let preparation = metrics.map(',"let _prepared = PreparedScope::enter();`n    let preparation = metrics.map(")
[IO.File]::WriteAllText("$copy/src/mesh/lods.rs",$lod)
$cargo=[IO.File]::ReadAllText("$repo/target/ara-dev/import-speed/lod-profile/Cargo.toml")
$cargo=$cargo.Replace('name = "ara-lod-profile"','name = "ara-lod-prepared"').Replace("path = `"$repo/crates/ara-importers`"","path = `"$copy`"")
[IO.File]::WriteAllText("$root/Cargo.toml",$cargo)
Copy-Item "$repo/target/ara-dev/import-speed/lod-profile/Cargo.lock" "$root/Cargo.lock" -Force
$main=[IO.File]::ReadAllText("$repo/crates/ara-importers/examples/geometry_replay.rs").Replace('fn main() -> std::process::ExitCode {','fn main_inner() -> std::process::ExitCode {')
$main+=@'

unsafe extern "C" { fn ara_prepared_dump(); }
fn main() -> std::process::ExitCode {
    let result=main_inner();
    // SAFETY: All single-thread jobs and their contexts have finished.
    unsafe { ara_prepared_dump(); }
    result
}
'@
[IO.File]::WriteAllText("$root/src/main.rs",$main)
& "$root/generate.ps1"
