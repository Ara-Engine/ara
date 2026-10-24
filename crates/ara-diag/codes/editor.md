# The editor: actions, previews, preferences, transport and rendering (ARA4500–ARA4599)

## ARA4500 editor::ACTION: Editor action failed
owner: ara-editor

A menu item, shortcut, panel button or palette command could not run. The message names the action and why; nothing was half-applied, because content edits run as one transaction.

Causes:
- The command needs something that is missing (a selection, an open document, a running Play world).
- A file or document it reads changed or disappeared.

Fixes:
- Read the message: disabled commands in menus say what to do first.
- Reopen the document or reselect the target and run the command again.

## ARA4501 editor::THUMBNAIL: Thumbnail or preview failed
owner: ara-editor

The editor could not produce a thumbnail or preview image for an asset. The asset itself is unaffected; the browser shows a placeholder and keeps the last good image.

Causes:
- The source file is corrupt or uses a format variant the importer does not read.
- The asset's import failed (see its import diagnostics).

Fixes:
- Reimport the asset (Assets > Reimport) and check the Problems panel for its import errors.
- Replace or re-export the source file.

## ARA4502 editor::SETTINGS: Preferences not saved
owner: ara-editor

Changed preferences or project settings could not be written to disk. They stay active for this session but are lost when the editor closes.

Causes:
- `%APPDATA%\Ara\editor-settings.json` or `ara.project.json` is read-only or locked by another program.
- The disk is full.

Fixes:
- Make the file writable or close the program holding it, then change the setting again.

## ARA4503 editor::TRANSPORT: Engine Protocol transport stopped
owner: ara-editor

The editor's localhost Engine Protocol server stopped accepting connections, so `ara` commands and MCP agents cannot reach this editor.

Causes:
- The port was taken or the listener failed.

Fixes:
- Restart the editor; agents reconnect through the project's `.ara/daemon.json`.

## ARA4504 editor::RENDER: Editor frame not rendered
owner: ara-editor

An editor window, the Scene view or a preview failed to render a frame. The editor keeps the last good image and retries.

Causes:
- The GPU device was lost or a shader pipeline failed.
- The scene uses more GPU resources than the view supports.

Fixes:
- Switch the Scene view to Lit · Raster (View > View Mode) and check `ara doctor` for GPU driver problems.

## ARA4506 editor::MISSING_SOURCE: Scene asset file missing
owner: ara-editor

A scene references a model file that is not in the project. The editor opens anyway and the entity draws a placeholder box; Play shows the same placeholder (`ARA7310`).

Causes:
- The file was deleted, moved or renamed outside the editor.
- The project was copied or installed without its asset folders.

Fixes:
- Restore the file at the named path (or reimport it), then the model appears.
- Point the entity's MeshRenderer.model at an existing asset, or remove the entity.
