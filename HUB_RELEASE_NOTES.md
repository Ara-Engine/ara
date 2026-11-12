# Ara Hub 0.0.2-alpha.5

Preview 5 gives Hub a new interface. Migration, update, repair and removal work as in
preview 4.

## Highlights

- **A rebuilt interface.** Hub now uses the same component layer as the new editor.
  Projects are cover-art cards or a dense list, each with one primary action and
  the rest in its menu (the ⋯ button, a right click, or the Menu key). Double-click
  or Enter opens a project, and F2 renames it in place.
- **Keyboard first.** A command palette (Ctrl K, ⌘K on macOS) runs Hub's actions
  from the keyboard. Ctrl N starts a new project, Ctrl O adds an existing one,
  Ctrl 1 to 4 switch pages, `/` searches, and Esc leaves Create. Folders dropped on the
  window are added.
- **Quiet when idle.** Hub redraws only what changed and uses no timer while idle.
- **Same flows, new look.** Opening an older project still shows the migration
  preview with **Migrate** and **Cancel**; an available update still offers
  **Update now**, **Later** and **Skip this version** and restarts only on
  **Restart and update**; Repair and removal behave as before.

## Updating projects

Use **Installs → Preview** to install Engine **0.0.2-alpha.5** alongside this Hub.
The project layout is unchanged (layout version 4), so preview-4 projects need no
migration. Projects that predate the `content/` layout still migrate through the
review preview, as before. Keep a backup including `.ara/` history.

**ARA1914** requests migration of an old layout. **ARA1915** means the pinned
engine cannot open the `content/` layout: choose **Use current engine** or
**Install current engine**, or **Cancel** to leave the project and pin unchanged.
See the [migration guide](https://github.com/Ara-Engine/ara/blob/main/docs/project-layout.md).

## Known issues

- The engine this Hub installs draws with Renderlab only: it takes much longer per
  frame than the previous release's renderer, and the engine's release notes list what
  it does not yet draw. There is no way to switch back.

## Install

Download `ara-hub-0.0.2-alpha.5-windows-x64-setup.exe` from
[GitHub Releases](https://github.com/Ara-Engine/ara/releases). Installation is per
user. Hub and engine releases are separate; select the Preview channel for this
prerelease. Windows x64 is the packaged Hub platform.

**No Authenticode signing yet:** Windows SmartScreen may warn when running the
installer. Signed manifests and SHA-256 verification protect downloads and updates;
they do not remove the SmartScreen warning.
