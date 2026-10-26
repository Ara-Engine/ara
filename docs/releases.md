# Ara releases

Download Ara Hub from [GitHub Releases](https://github.com/Snowy7/ara/releases).
The source repository is public; releases live in **Snowy7/ara** itself.
Run the `ara-hub-<version>-windows-x64-setup.exe` per-user installer, then use
**Installs** to install an engine. Windows x64 is the initial packaged platform.
There is **no Authenticode code signing yet**: Windows SmartScreen warns.
Manifest signing authenticates updates; it does not remove SmartScreen warnings.

## Streams and channels

Hub tags are `hub-v<semver>`; engine tags are `engine-v<semver>`.
GitHub's prerelease flag must agree with the signed manifest's `preview` channel.
Stable sees stable releases. Preview sees stable and prerelease releases. Versions
are sorted by semver, independently of GitHub's release creation order.
Each release includes `release.json`, detached hexadecimal ed25519
`release.json.sig`, the Windows x64 portable ZIP, setup EXE and notices.
The signed manifest includes notes, platform, minimum Hub version and each
asset's exact byte count, SHA-256 and URL. Signatures are checked before metadata
is used; the complete streamed asset is hashed before extraction.
Hub packages also record `build {commit,generation}` in hub.json and the signed
manifest; generation is the publishing branch's Git ancestry count. Build and
publish from clean committed source. Setup and self-update compare semver, then
generation, so a newer same-version preview replaces a legacy Hub. Engine setup
keeps a newer standalone Hub and updates older copies under the shared update lock.
Running Hubs/locked files must be closed before replacement; editors and versioned
services remain separate. Packaging checks the Hub executable's embedded commit.

The default feed is `Snowy7/ara`. Settings → Release source persists an
owner/name in `<Ara config>/update.json`; `ARA_RELEASE_REPO` overrides that value.
Set optional `GITHUB_TOKEN` privately in the Hub service's environment for API
quota or private staging repositories. Restart the service after changing its
environment; project processes survive `ara hub shutdown`. Tokens never go in
settings, manifests, logs or URLs and are sent only to api.github.com. HTTPS is
required; redirects are restricted to GitHub asset hosts. Tests use an explicit,
compile-time-only loopback transport.

Checks run on Hub start and every four hours, on a worker. Per-page ETags avoid
redownloading an unchanged feed. Offline/rate-limit fallback visibly identifies
the cached feed and verifies its signatures again. Invalid signatures never
fall back silently. Interrupted downloads resume with HTTP Range, using the
signed hash as their staging identity. Cancel removes partial downloads.

## Signing keys

Generate a key **outside every repository**; never put a key or token in Git.
`--key` and `ARA_RELEASE_SIGNING_KEY` accept a file path, never raw secret text.

```powershell
$env:CARGO_BUILD_JOBS = '2'
$env:VULKAN_SDK = 'C:\VulkanSDK\1.4.363.0'
cargo run -p ara-release -- keygen --output "$env:APPDATA\Ara\release-signing.key"
```

The command refuses an existing key or a path inside this checkout, restricts
file permissions and prints **only the public key**. Back up the private key
securely. Add the printed public key to `crates/ara-update/public-keys.txt` and
commit that public metadata before packaging. Alternatively set build-time
`ARA_RELEASE_PUBLIC_KEYS` to comma-separated public keys before building the Hub,
service and release tool. An empty embedded trust list rejects every release.
For rotation, ship both old and new public keys first. Complete that rollout,
including restarting the Hub service from the new build, before changing the
signing key. A busy service keeps supervising its open projects until they close.
Keep the old public key while its engine releases remain installable.

Standalone Hub packaging is independent of an engine's NuGet restore: only an
engine carrying managed assemblies requires that restored NuGet notice inventory.
Both packages use the same release debug-info profile, avoiding a second dependency build.

## Release-freeze history cleanup

The owner approved the real rewrite **at release time**. Until then, run only the
scratch rehearsal. `ara-dev history-rewrite` is a small Rust tool using raw
`git cat-file` / `hash-object` objects, not Python or `filter-branch`. It creates a
new independent bare mirror with `--mirror --no-local`, disables its push URL,
and never opens the source repository for writes. It refuses an existing scratch
directory, shared object stores, shallow history or attached worktrees. No push
step exists in either history command.

It removes case-insensitive Claude/Anthropic co-author trailer lines, Claude Code
generated footers (with or without the robot), and standalone `claude.ai/code`
session links, including Markdown links. Other co-authors stay. Trailing blank
lines are removed only when exposed by removing an attribution footer; removing
an interior attribution line preserves pre-existing trailing blanks. All other bytes,
including CRLF, non-UTF-8 text, author/committer identities, dates, encoding and
extra headers, stay unchanged. A session URL mixed into substantive text fails
closed instead of deleting unrelated prose. Annotated tags receive the same
message filter and their targets are mapped recursively; lightweight tags map
directly. Merge parent **order and multiplicity** are preserved.

The output directory contains `mirror.git`, `report.json`, `commit-map.tsv`
(both tree hashes and ordered parent lists), `ref-map.tsv` and `fsck.log`.
Verification rereads every new object, checks exact retained header/message
bytes, verifies a bijective commit map and the reachable commit/ref counts,
and runs `git fsck --full`. Commit collisions fail before ref publication.
Signature headers/blocks are preserved, but changing a signed object's bytes
invalidates its signature. `report.json.signatures` lists every signed commit/tag
and whether its ID changed. Review that list; make final signed release tags
against the rewritten tips rather than presenting an old signature as valid.

### Rehearse without touching live refs

```powershell
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_TARGET_DIR = 'D:\AraBuild\ara-dev-infra-target'
cargo build -p ara-dev
$tool = (Resolve-Path "$env:CARGO_TARGET_DIR/debug/ara-dev.exe").Path
$source = (git rev-parse --path-format=absolute --git-common-dir).Trim()
$scratch = "D:\AraBuild\history-rehearsal-$([guid]::NewGuid().ToString('N'))"
& $tool history-rewrite --source $source --scratch $scratch
if ($LASTEXITCODE -ne 0) { throw 'History verification failed; do not publish' }
Get-Content "$scratch/report.json"
```

Use an explicit owned `CARGO_TARGET_DIR` when building the tool. Keep the full
scratch directory until release review. It includes every locally advertised
branch/tag and auxiliary ref, including offline agent branches. This is evidence,
not a reason to publish local checkpoint/recovery refs.

Measured rehearsal on 2026-10-26, while the batch gate/agents were active:
**2,723 commits before and after**, across 82 local branches and 1,640 total refs
(including three remote-tracking refs, one of which is `origin/HEAD`). No tags
existed; read-only `ls-remote` confirmed that both advertised branches (`main` and
`integration/0.0.1-alpha`) were included and no remote tags were missing. The filter
changed 497 messages; parent propagation rehashed 2,721 commits.
Every tree, ordered parent list, retained metadata byte and expected message byte
verified; all **367 merge commits** retained their topology. An independent pair
of Git log projections and a broader attribution regex also passed with zero
remaining matches. **No GPG-signed commits or signed tags were present**; the real
Git fixtures additionally cover nested/lightweight tags and preserved signatures.

Clone took **53.00 s**, rewriting **217.01 s**, verification **46.29 s**, total
**316.79 s**. An earlier 2,721-commit pass took 239.62 s under the same shared-machine
conditions; all 2,721 common mappings were identical after tightening the interior
whitespace guard. These are observed contended timings, not a quiet-machine bound.
The source `main` snapshot was `8a18f0685a03f02ce5e7d5a512afb84e18ccf657`;
no source ref or worktree was rewritten and nothing was pushed. The read-only
message matcher also found the mapped real `e63f8219` fork uniquely. Reports:
[rewrite proof](releases/history-rehearsal-20261026.json),
[independent audit](releases/history-independent-audit-20261026.json).
The complete mirror and per-commit maps remain at
`D:\AraBuild\ara-history-rehearsal-20261026-aefa6558` for release review.

### Release day: rewrite the remote branches together

1. After the final gate, publish the accepted release tips normally. Then freeze
   **all** writers: stop agents, packaging, fetch/push jobs and hooks that
   create refs. Checkpoint dirty work with plain WIP commits, never stash. Save
   `git worktree list --porcelain`, each branch/tip and the unmerged branch list.
   Create an offline `git bundle create <outside-repo>/pre-rewrite.bundle --all`.
   Keep the old checkouts and unmerged branches intact for recovery. Copy the
   verified `ara-dev.exe` outside the old checkouts before rebuilding anything.

```powershell
$oldRepo = 'D:\Coding\projects\desktop\snengine'
New-Item -ItemType Directory -Force 'D:\AraRelease\tools' -ErrorAction Stop | Out-Null
Copy-Item 'D:\AraBuild\ara-dev-infra-target\debug\ara-dev.exe' `
    'D:\AraRelease\tools\ara-dev.exe' -ErrorAction Stop # verified focused-green tool
$archive = "D:\AraRelease\pre-freeze-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $archive -ErrorAction Stop | Out-Null
git -C $oldRepo worktree list --porcelain | Set-Content "$archive/worktrees.txt"
git -C $oldRepo for-each-ref '--format=%(refname) %(objectname)' |
    Set-Content "$archive/refs.txt"
git -C $oldRepo branch --no-merged main | Set-Content "$archive/unmerged.txt"
git -C $oldRepo bundle create "$archive/pre-rewrite.bundle" --all
if ($LASTEXITCODE -ne 0) { throw 'Backup failed; do not rewrite' }
git -C $oldRepo bundle verify "$archive/pre-rewrite.bundle"
if ($LASTEXITCODE -ne 0) { throw 'Backup verification failed' }
```

2. Make a new rehearsal from the **remote URL**, not a local repo, so the release
   ref inventory is exactly what the server advertises. Rewrite `main` and
   `integration/0.0.1-alpha` in this **one** invocation, together with every other
   advertised branch/tag. Their shared commits must use the same mapping.

```powershell
$remote = 'https://github.com/Snowy7/ara.git'
$tool = 'D:\AraRelease\tools\ara-dev.exe' # copied from the focused-green build
$freeze = "D:\AraRelease\history-freeze-$([guid]::NewGuid().ToString('N'))"
& $tool history-rewrite --source $remote --scratch $freeze
if ($LASTEXITCODE -ne 0) { throw 'Do not push a failed rewrite' }
$mirror = "$freeze/mirror.git"
git -C $mirror show-ref --verify refs/heads/main
if ($LASTEXITCODE -ne 0) { throw 'main is missing' }
git -C $mirror show-ref --verify refs/heads/integration/0.0.1-alpha
if ($LASTEXITCODE -ne 0) { throw 'integration/0.0.1-alpha is missing' }
$rows = @(Import-Csv "$freeze/ref-map.tsv" -Delimiter "`t" |
    Where-Object { $_.ref -match '^refs/(heads|tags)/' })
$proof = Get-Content "$freeze/report.json" -Raw | ConvertFrom-Json
if ($proof.commits_before -ne $proof.commits_after -or $proof.remaining_matches -ne 0) {
    throw 'Invalid rewrite proof'
}
$proof.signatures | Format-Table
```

3. Review the proof, mappings and signatures. Immediately before publication,
   compare the **complete** remote head/tag inventory with `ref-map.tsv`; a new,
   deleted or advanced ref means abort and repeat from a fresh mirror. The
   explicit leases below also reject concurrent changes to listed refs. Publish
   all rewritten branches/tags atomically, with their frozen old OIDs as leases:

```powershell
$advertised = @(& git ls-remote --heads --tags $remote |
    Where-Object { $_ -notmatch '\^\{\}$' } | ForEach-Object {
        $fields = $_ -split "`t"; "$($fields[1])`t$($fields[0])"
    } | Sort-Object)
if ($LASTEXITCODE -ne 0) { throw 'Remote inventory failed' }
$expected = @($rows | ForEach-Object { "$($_.ref)`t$($_.old)" } | Sort-Object)
if (@(Compare-Object $expected $advertised).Count -ne 0) { throw 'Remote moved; abort' }
$leases = @($rows | ForEach-Object { "--force-with-lease=$($_.ref):$($_.old)" })
$refspecs = @($rows | ForEach-Object { "$($_.new):$($_.ref)" })
& git -C $mirror push --atomic @leases $remote @refspecs
if ($LASTEXITCODE -ne 0) { throw 'Lease/atomic push rejected; do not retry with --force' }
$published = @(& git ls-remote --heads --tags $remote |
    Where-Object { $_ -notmatch '\^\{\}$' } | ForEach-Object {
        $fields = $_ -split "`t"; "$($fields[1])`t$($fields[0])"
    } | Sort-Object)
if ($LASTEXITCODE -ne 0) { throw 'Publication verification failed' }
$wanted = @($rows | ForEach-Object { "$($_.ref)`t$($_.new)" } | Sort-Object)
if (@(Compare-Object $wanted $published).Count -ne 0) { throw 'Published refs differ' }
```

Never use `push --mirror`, an implicit lease based on a mutable tracking ref, or
plain `--force`. No checkpoint, stash, recovery, pull-request or remote-tracking
refs are published. If branch protection prevents the approved rewrite, the
owner must permit this release operation; do not bypass a rejected lease. The
saved old/new map is also the rollback inventory: rollback would require explicit
leases on the **new** OIDs. Retain the bundle and evidence offline.

### Recreate agent worktrees and recover unmerged work

Clone a fresh working repository after verifying the published refs. Preserve
the old repository offline; do not fetch/merge its old main into the new one.
For each agent that has no unmerged work, create a worktree **from new main**:

```powershell
$fresh = 'D:\Coding\ara-after-freeze' # nonexistent before clone
git clone --branch main $remote $fresh
if ($LASTEXITCODE -ne 0) { throw 'Fresh clone failed' }
$agentPath = 'D:\AraWorktrees\dev-after-freeze'
git -C $fresh worktree add --no-track -b t3/ara-dev-after-freeze $agentPath origin/main
if ($LASTEXITCODE -ne 0) { throw 'Worktree recreation failed' }
```

Repeat with unique branch/path names for every saved worktree. Use T3's worktree
handoff to bind each existing agent thread to the new path; `git worktree add`
alone does not change a thread's workspace. Keep each owned Cargo target separate.
Remove old worktrees only after every saved WIP is recovered and checked.

For an **unmerged** branch, first run `history-rewrite` on the saved old local
repository into another fresh scratch mirror. This cleans unsent commits too,
without reintroducing attribution during rebase. Match its old fork against
rewritten main using the **full cleaned message plus exact tree and retained
metadata**, never just the subject. The read-only matcher requires exactly one
match; no match/ambiguity stops recovery for inspection.

```powershell
$oldRepo = 'D:\Coding\projects\desktop\snengine' # saved old repo, still offline
$branch = 't3/example-unmerged-agent'
$localClean = "D:\AraRelease\local-clean-$([guid]::NewGuid().ToString('N'))"
& $tool history-rewrite --source $oldRepo --scratch $localClean
if ($LASTEXITCODE -ne 0) { throw 'Local branch cleanup failed' }
$oldFork = (git -C $oldRepo merge-base main $branch).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Old fork not found' }
$match = & $tool history-match --old-repo $oldRepo --old-base $oldFork `
    --new-repo $fresh --new-tip origin/main | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Fork message match failed; do not guess' }
$forkRow = @(Import-Csv "$localClean/commit-map.tsv" -Delimiter "`t" |
    Where-Object { $_.old -eq $oldFork })
if ($forkRow.Count -ne 1 -or $forkRow[0].new -ne $match.new) { throw 'Fork mappings disagree' }
$recovered = 'recover/example-agent'
git -C $fresh fetch "$localClean/mirror.git" "refs/heads/${branch}:refs/heads/$recovered"
if ($LASTEXITCODE -ne 0) { throw 'Clean branch transfer failed' }
$recoverPath = 'D:\AraWorktrees\example-recovered'
git -C $fresh worktree add $recoverPath $recovered
if ($LASTEXITCODE -ne 0) { throw 'Recovery worktree failed' }
git -C $recoverPath rebase --rebase-merges --onto origin/main $match.new
if ($LASTEXITCODE -ne 0) { throw 'Resolve the rebase here; do not merge old main' }
```

Review conflicts, compare the recovered work with its saved branch, then run its
focused checks before handing over the new SHA. A recovery rebase normally gives
new committer dates; it is separate from the byte-preserving release rewrite.
Finally rebuild release packages from clean rewritten source and run the Hub
checklist below: embedded commit IDs and build hashes changed even though every
source tree is identical. Do not publish old binaries under a new commit identity.

## Package, verify, publish

`workspace.package.version` must match `--version`. Bump it deliberately for each
new binary version. These are explicit release builds, with no upload until the
last two commands. Packaging refuses existing output directories.

```powershell
$env:ARA_RELEASE_SIGNING_KEY = "$env:APPDATA\Ara\release-signing.key"
$env:CARGO_PROFILE_RELEASE_DEBUG = 'line-tables-only'
cargo run -p ara-release -- package --kind engine --version 0.0.1-alpha
cargo run -p ara-release -- package --kind hub --version 0.0.1-alpha
cargo run -p ara-release -- smoke --version 0.0.1-alpha
cargo run -p ara-release -- publish --repo Snowy7/ara --tag engine-v0.0.1-alpha --prerelease --dry-run
cargo run -p ara-release -- publish --repo Snowy7/ara --tag hub-v0.0.1-alpha --prerelease --dry-run
```

Review the signed folders under `target/release-dist/published/<tag>` and the
installed-engine smoke report. Dry-run never invokes `gh` or accesses GitHub.
The real commands reuse those folders only if their manifests, signatures,
notes and artifact hashes are identical; extra or changed files are rejected.
Publish the Hub first so engines' minimum Hub version is already downloadable.
The developer must have
`gh` installed and authenticated with write access to Snowy7/ara:

```powershell
cargo run -p ara-release -- publish --repo Snowy7/ara --tag hub-v0.0.1-alpha --prerelease
cargo run -p ara-release -- publish --repo Snowy7/ara --tag engine-v0.0.1-alpha --prerelease
```

Publication first creates a draft, uploads the complete signed artifact set, then
publishes the draft. Failed uploads leave the draft for inspection/retry with
`gh release upload`; never publish a draft missing either manifest file. A
missing `gh` is diagnosed as a developer publishing prerequisite, never a build
prerequisite. `ara-release sign <release.json> --key <path>` signs an existing
manifest; prefer `publish`, which hashes the actual packaged files.

## Release-day checklist: 0.0.1-alpha

1. Select the green, clean main commit to ship. The workspace version must be
   `0.0.1-alpha`. Push that source commit before real publication; packaging and
   dry runs need no GitHub credentials. Use full Git history for build generations.
   Install Rust/MSVC, Vulkan SDK 1.4.363.0 and .NET 10 SDK on the packaging machine;
   run with two Cargo jobs. Leave the `rehearsal` feature off for shipping builds.
   Prepare the git-ignored real sample art first: with
   CARGO_PROFILE_RELEASE_DEBUG=line-tables-only, run
   `cargo run --release -p ara-samples -- fetch`, then the same command with `verify`.
   Downloads are pinned by samples/assets.lock.json (no asset-site secret is needed).
   An existing verified samples/.cache/blobs cache can be reused. Packaging generates
   kits in staging, but does not download missing source art. See [sample setup](../samples/README.md).
2. Generate the release signing key once, outside all checkouts, using the keygen
   command above; otherwise reuse the backed-up key. Commit only its reviewed
   public key in public-keys.txt before packaging, or supply that public key through
   build-time ARA_RELEASE_PUBLIC_KEYS consistently to every binary/tool. The only
   signing secret is the external seed file named by ARA_RELEASE_SIGNING_KEY.
   Keep it backed up and private. An older Hub must already trust this public key
   to self-update; adding a new key only to the new archive cannot establish trust.
3. Finalize root `RELEASE_NOTES.md`, including the formal D26 reading; its first
   line must match `# Ara <version>`. Engine packaging/`notes` copy it verbatim.
   Run both `package` commands and the installed `smoke` above. Preserve logs,
   `SMOKE_RESULTS.json`, the source SHA and the private symbols archive. Packaging
   writes under target/release-dist and refuses existing output directories; move
   a previous run into a named archive before rebuilding. Do not regenerate notes
   after signing a reviewed release folder.
4. Review the packaged `RELEASE_NOTES.md` (an exact copy of the reviewed root file) and
   `HUB_RELEASE_NOTES.md` (Hub's packaged release summary). Both must explain the
   unsigned installer/SmartScreen warning. The local publish folder carries the
   same notes in RELEASE_NOTES.md and inside the signed manifest.
5. Run both `publish --dry-run` commands. Review the six assets in each folder:

   | Asset | Hub release | Engine release |
   |---|---|---|
   | Portable archive | ara-hub-0.0.1-alpha-windows-x64.zip | ara-0.0.1-alpha-windows-x64.zip |
   | Per-user setup | ara-hub-0.0.1-alpha-windows-x64-setup.exe | ara-0.0.1-alpha-windows-x64-setup.exe |
   | Notices | THIRD_PARTY_NOTICES.txt | THIRD_PARTY_NOTICES.txt |
   | Signed metadata | release.json, release.json.sig | release.json, release.json.sig |
   | Notes | RELEASE_NOTES.md | RELEASE_NOTES.md |

   Keep symbols ZIPs private; the player PDB required by exports is already in the
   engine payload. Each uploaded file must be **under 2 GiB**; publishing checks
   this before hashing. GitHub permits up to 1,000 assets per release. Ara bounds
   ZIP downloads to 16 GiB, expanded installs to 64 GiB, API pages to 8 MiB,
   release.json to 1 MiB and its signature to 1 KiB.
   [GitHub release limits](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).
6. Install/authenticate `gh` on the publisher; set GH_HOST=github.com if the shell
   normally targets an enterprise host. Recommended: a fine-grained token
   restricted to Snowy7/ara with **Contents: read and write**, supplied privately as
   GH_TOKEN. The account also needs repository write access and permission under
   any tag ruleset to create `hub-v*` and `engine-v*`. A classic token can use
   `public_repo` for this public repository; `repo` is broader. `gh auth login`'s
   browser/classic login normally asks for `repo`, `read:org`, `gist`; fine-grained
   tokens belong in GH_TOKEN rather than `--with-token`. Publishing a target with
   workflow-file changes relative to the default branch can additionally require
   Workflows: write / the classic `workflow` scope. No Packages or admin scope is
   needed. Do not print tokens or put them in command arguments.
   [GitHub release permissions](https://docs.github.com/en/rest/releases/releases),
   [gh authentication](https://cli.github.com/manual/gh_auth_login),
   [classic scopes](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps).
7. Publish **hub-v0.0.1-alpha first**, then **engine-v0.0.1-alpha**, using the two
   non-dry commands above with `--prerelease`. `publish` uses the packaged source
   SHA as `gh release create --target`, verifies existing tags resolve to that SHA,
   creates drafts, uploads all six files and only then publishes. It creates missing
   tags; no manual tag push is necessary. Existing tags/releases are immutable
   version identities: use a new version rather than moving them or replacing
   their assets. If an upload fails, inspect the retained draft and finish its
   uploads from the reviewed folder before `gh release edit TAG --draft=false`.
   [gh tag behavior](https://cli.github.com/manual/gh_release_create).
8. From the Releases page, download the **Hub** setup, accept SmartScreen's unsigned
   warning, and select **Preview** in Hub Settings. A clean profile defaults to
   Stable, which deliberately hides this alpha. Install the engine through Installs,
   create Showcase, open it, Build Current Profile, then run the exported game.
   Test anonymously too: testers need no GitHub token for the public feed. .NET 10
   SDK/runtime, VC++ x64 runtime and a Vulkan-capable driver remain tester prerequisites.
   Confirm both tags, both manifests/signatures and the six complete asset sets
   in GitHub; a prerelease is not the repository's stable `/releases/latest`.

Earlier preview builds can leave a service whose working directory locks the Hub
installation (Windows error 32). For that one-time migration, close its projects,
stop the idle service with `ara hub shutdown`, and run the current Hub setup before
testing automatic updates. Current builds start services in their profile and
project processes in their project root, outside the Hub being swapped.

## Local rehearsal, with no publication

Generate a throwaway key in an OS temporary directory outside the checkout; keep
its output private and embed only its public key at build time. Build an older
Hub rehearsal setup and retain it before committing/building the newer candidate;
both must trust this same throwaway key. No fake build identity is substituted.
Run the two `package` commands with `--features rehearsal` on the release tool,
then the two dry-run publish commands with that key. Rehearsal packaging propagates
the feature to Hub/service; markers reject any attempt at real publication.

```powershell
cargo run -p ara-release --features rehearsal -- rehearse `
  --artifacts target/release-dist `
  --older-setup target/older-rehearsal/ara-hub-0.0.1-alpha-windows-x64-setup.exe `
  --directory target/rehearsal-profile
```

The directory must be fresh and below target. The command starts a local mock of
the Releases API, serving the exact dry-run signed bytes and GitHub asset paths.
Only explicit test-support builds map these URLs to that loopback origin. The
older installed Hub uses its real model to discover/download the update, launches
the external updater and exits. The new Hub renders offscreen for startup health.
Then the actual service installs/registers the engine ZIP, creates and opens
the real-art Showcase sample, opens its editor offscreen, builds it and runs the export for eight
verified ticks. REHEARSAL_RESULTS.json, mock request logs, the offscreen Hub frame
and ARA_RESULT remain under that directory; owned processes and the server stop.
Delete the throwaway private key afterward. Production packages have neither this
host nor a runtime HTTP override.

## Installation and update boundaries

Hub-managed engines live in `%LOCALAPPDATA%\Ara\Engines\<version>`. Extraction
uses sibling staging and atomic rename, then the service registers the build.
The engine ZIP uses the same complete layout as the installed-build smoke test:
CLI/editor/player/workers, runtime resources, shader tools, C# host, templates
and notices. Open projects prevent uninstall of their pinned version. Externally
registered builds have **Remove** (unregister); Hub-owned builds have **Uninstall**.
Importing a project with a missing semantic-version pin offers **Install and open**.

The standalone Hub lives in `%LOCALAPPDATA%\Programs\Ara Hub`. Its bootstrap
`ara` service is staged into `%LOCALAPPDATA%\Ara\Services\<version>\<build-hash>`,
separate from both the GUI and installed engines. A bootstrap service is not an
engine build. Same-version previews get distinct immutable copies; `service.json`
records the executable build and payload hashes. Templates come from registered
engines, newest semantic version first, then service/checkout fallbacks. Creation
copies the selected engine's files, checks its asset lock and pins that engine.
The installer adds its external CLI directory to the user PATH;
restart the terminal before using `ara hub` or configuring the global MCP.
The existing engine installer stays compatible under
`%LOCALAPPDATA%\Programs\Ara\<version>` and installs/reuses the separate Hub for
its Start menu shortcut. Directly launching a legacy Hub inside an engine folder
requires installing the standalone Hub for GUI self-update. Smoke mode with
`--no-system-integration` suppresses shell/registry/PATH writes; smoke isolates
LOCALAPPDATA so its managed launcher installation stays inside target.

Standalone Hub updates download in the background. **Restart to update** starts
an updater copied outside the Hub directory. It waits only for that GUI PID,
with its working directory outside the install too (Windows otherwise locks the swap),
rechecks the signature/hash, extracts to a sibling, swaps current → `.old` and
new → current, and launches the new Hub with its own bootstrap. Only
after the GUI renders a frame does it acknowledge health and permit `.old`
cleanup. Startup failure/timeout restores and relaunches the previous Hub.
Inspect `<Ara config>/hub-update.log` for failed restarts. The build handshake
gracefully replaces an idle older service. A service supervising editors, daemons
or downloads stays connected, with a non-fatal restart notice; open projects survive.

## CLI and RPC

Development tests, captures and manual checks always use an isolated profile.
`cargo test` and `cargo run` get `ARA_CONFIG_DIR=target/ara-config` from the
tracked `.cargo/config.toml`; set it explicitly for a per-check scratch profile:

```powershell
$env:ARA_CONFIG_DIR = (New-Item -ItemType Directory -Force "target/hub-checks/$([guid]::NewGuid())/config").FullName
```

Checkout binaries refuse the real user profile before accessing discovery or
registry files. Worktree engine/editor paths cannot be registered in that real
profile by an installed service either. Test harnesses fail if ARA_CONFIG_DIR
resolves outside target/. Stop every service/updater started by a check;
GUI checks use offscreen captures only. Smoke isolates config, LOCALAPPDATA and
APPDATA and cleans up on failure as well as success. It installs both setups,
seeds a stale same-version service, launches the standalone Hub without a window,
checks outpost/showcase/starter discovery and creates Starter from the engine.

```powershell
ara hub engines --available --channel preview
ara hub install 0.0.1-alpha
ara hub uninstall 0.0.1-alpha
ara hub cancel engine-0.0.1-alpha
```

The UI and CLI use the service's `hub.engines.available`, `hub.engine.install`,
`hub.engine.uninstall`, `hub.downloads` and `hub.download.cancel`. Installs return
a job immediately; `hub.changed` with `kind: "progress"` invalidates job snapshots.
`hub.releases.check`, `hub.releases.configure`, `hub.updates` and
`hub.update.prepare` drive the Hub update/settings controls. The full wire
contract is in `crates/ara-hub-service/AGENTS.md`.

Global MCP uses the same jobs through `ara_projects`:
`{action:"checkReleases"}`, `{action:"available",channel:"preview"}`,
`{action:"installEngine",version:"0.0.1-alpha"}`, `{action:"downloads"}`,
`{action:"cancelDownload",job:"engine-0.0.1-alpha"}` and
`{action:"uninstallEngine",version:"0.0.1-alpha"}`. These need no project/session.
