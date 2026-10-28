# Ara Hub 0.0.2-alpha.4

Preview 4 adds explicit project migration and expands engine installation,
repair and update controls.

**Draft:** the final editor follow-up and release qualification are still pending.

## Highlights

- **Review before migrating.** Opening an older project with the new engine shows
  the move into `content/`, with **Migrate** and **Cancel**. Cancel leaves the
  project closed and unchanged. A migrated project cannot be launched with an
  engine that does not support its layout.
- **Control each update step.** An available update offers **Update now**,
  **Later** and **Skip this version**. Download and verification follow your
  choice; applying a verified Hub update requires **Restart and update**.
- **See installation progress and repair a build.** Engine installs show download,
  verification and extraction separately, with disk-space checks and resumable
  work. Repair restores the signed release. Removal shows the location, space and
  affected project pins before you confirm, and does not close open projects.
- **A dedicated Create page.** New-project creation has Back navigation, refreshed
  branding and shared searchable selections and dialogs. Templates use the new
  content layout and the selected engine's files.

## Updating projects

Use **Installs → Preview** to install Engine **0.0.2-alpha.4** alongside this Hub.
A preview-3 layout-v3 project can keep using its compatible old engine until migrated.
Migration pins the engine performing it; keep a backup including `.ara/` history.

**ARA1914** requests migration of an old layout. **ARA1915** means the pinned
engine cannot open the `content/` layout: choose **Use current engine** or
**Install current engine**, or **Cancel** to leave the project and pin unchanged.
See the [migration guide](https://github.com/Ara-Engine/ara/blob/main/docs/project-layout.md).

## Install

Download `ara-hub-0.0.2-alpha.4-windows-x64-setup.exe` from
[GitHub Releases](https://github.com/Ara-Engine/ara/releases). Installation is per
user. Hub and engine releases are separate; select the Preview channel for this
prerelease. Windows x64 is the packaged Hub platform.

**No Authenticode signing yet:** Windows SmartScreen may warn when running the
installer. Signed manifests and SHA-256 verification protect downloads and updates;
they do not remove the SmartScreen warning.

## TODO — final editor and release qualification

Add any further Hub-facing changes from `t3/ux-editor-p4` after they land. Confirm
the final installed-package/update checks before removing this draft marker.
