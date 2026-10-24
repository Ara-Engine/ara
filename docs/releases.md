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
$env:CARGO_BUILD_JOBS = '4'
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
signing key. GUI self-update deliberately preserves the running service.
Keep the old public key while its engine releases remain installable.

## Package, verify, publish

`workspace.package.version` must match `--version`. Bump it deliberately for each
new binary version. These are explicit release builds, with no upload until the
last two commands. Packaging refuses existing output directories.

```powershell
$env:ARA_RELEASE_SIGNING_KEY = "$env:APPDATA\Ara\release-signing.key"
cargo run -p ara-release -- package --kind engine --version 0.0.1-alpha
cargo run -p ara-release -- smoke --version 0.0.1-alpha
cargo run -p ara-release -- package --kind hub --version 0.0.1-alpha
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

## Installation and update boundaries

Hub-managed engines live in `%LOCALAPPDATA%\Ara\Engines\<version>`. Extraction
uses sibling staging and atomic rename, then the service registers the build.
The engine ZIP uses the same complete layout as the installed-build smoke test:
CLI/editor/player/workers, runtime resources, shader tools, C# host, templates
and notices. Open projects prevent uninstall of their pinned version. Externally
registered builds have **Remove** (unregister); Hub-owned builds have **Uninstall**.
Importing a project with a missing semantic-version pin offers **Install and open**.

The standalone Hub lives in `%LOCALAPPDATA%\Programs\Ara Hub`. Its bootstrap
`ara` service is copied once into `%LOCALAPPDATA%\Ara\Services\<version>`,
separate from both the GUI and installed engines. A bootstrap service is not an
engine build. The installer adds this external CLI directory to the user PATH;
restart the terminal before using `ara hub` or configuring the global MCP.
The existing engine installer stays compatible under
`%LOCALAPPDATA%\Programs\Ara\<version>` and installs/reuses the separate Hub for
its Start menu shortcut. Directly launching a legacy Hub inside an engine folder
requires installing the standalone Hub for GUI self-update. Smoke mode with
`--no-system-integration` never creates that shared installation.

Standalone Hub updates download in the background. **Restart to update** starts
an updater copied outside the Hub directory. It waits only for that GUI PID,
rechecks the signature/hash, extracts to a sibling, swaps current → `.old` and
new → current, and launches the new Hub with the existing service path. Only
after the GUI renders a frame does it acknowledge health and permit `.old`
cleanup. Startup failure/timeout restores and relaunches the previous Hub.
Inspect `<Ara config>/hub-update.log` for failed restarts. Open editors and the
running Hub service are never terminated by GUI updates.

## CLI and RPC

Development tests, captures and manual checks always use an isolated profile:

```powershell
$env:ARA_CONFIG_DIR = (New-Item -ItemType Directory -Force "target/hub-checks/$([guid]::NewGuid())/config").FullName
```

Checkout binaries refuse the real user profile before accessing discovery or
registry files. Worktree engine/editor paths cannot be registered in that real
profile by an installed service either. Test harnesses fail if ARA_CONFIG_DIR is
missing or resolves outside target/. Stop every service/updater started by a check;
GUI checks use offscreen captures only. Smoke uses its own isolated config and
cleans up the service on failure as well as success.

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
