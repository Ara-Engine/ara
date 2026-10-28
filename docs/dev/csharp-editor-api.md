# C# editor API v1

Place extensions in any exact `editor` folder below the shared authored root.
Implement `Ara.Editor.IEditorExtension.Register(EditorRegistry)`. The compiler
builds these sources as `<owner>.editor`, referencing their gameplay assembly;
player compilation and export exclude the editor domain. A nested editor
manifest remains editor-only. Gameplay cannot reference an editor assembly.

`RegisterCommand` contributes to the native editor's one command registry.
`AddMenu` places a registered command in a slash-separated menu. The palette,
menus, shortcuts and `editor.commands` consume the same definitions. IDs cannot
collide with native commands or another extension. A failed reload reports coded
Problems while keeping the last good callbacks and menu definitions. A callback
fault reports ARA0912, discards its queued operations and disables that command
until a successful changed generation reloads; other commands remain available.

Callbacks run on a hidden preparation worker, outside the authority thread.
`EditorContext.Scene` is an immutable resolved scene snapshot: find by id, exact
name or name path; query parents, children and siblings. Ambiguous names fail.
`Selection.Entities` is the initiating session's selection; agent invocations
use their private presence rather than moving the human's view. An explicit
`editor.command.target.entity` becomes the invocation selection.

`Commands.Create/Delete/Reparent/SetProperty/AddComponent/RemoveComponent` queue
the existing typed engine commands. They commit as one normal attributed
`cmd.apply` batch, producing ordinary ops and undo/redo history. Created entities
use batch symbols. The authority rejects stale scene revisions or extension
generations with ARA1501. `Selection.Set` requests ordinary editor navigation
after a successful content transaction, resolving created-entity symbols.
Typed values serialize vectors/quaternions as numeric arrays, enums as schema
names and `EntityId` as address text; anonymous component values use camel-case
field names. Explicit `JsonElement` values retain their supplied schema shape.

This is a command extension API, not a direct document or gameplay-state API.
Gameplay state stays in ECS components. See the compiled examples embedded in
`docs.json` and `ara docs --llms`: `topics/csharp-editor-api`,
`topics/csharp-scene-queries`, and `topics/csharp-destroy`.

Runtime `World.Find` uses stable ids, exact names or root-relative name paths.
`World.Parent`, `Children` and `Siblings` read the ECS; span queries refuse
insufficient buffers rather than truncate. `Commands.SetParent` queues a
keep-local hierarchy change at the normal deterministic sync point, with cycle
validation and gameplay-command undo support.

`Behaviour.OnDestroy` runs once when a previously ticked behaviour component or
its entity disappears, at the next fixed-tick membership sweep. Sweeps include
types with zero remaining instances. Order is module, behaviour type and
ascending generation-checked entity handle. Cleanup may access other live ECS
components/resources; the removed component itself is unavailable. A fault in
one cleanup callback is diagnosed without suppressing later callbacks. Reload
uses `OnReload` rather than treating a replaced wrapper as an entity destruction.

`ara fix` and MCP `ara_fix` prepare the shipped ARA0001, ARA0101 and ARA0506 code
fixes from the installed SDK. Preview returns `source.replace` commands and exact
base hashes. Apply submits those typed commands to the normal transaction
history; stale bytes refuse with ARA1501. CLI undo and redo restore source bytes
across separate invocations. No code-fix provider writes authored files directly.
Native script creation, rename and `script.write` use the same prepared
`source.replace` command; source jobs finish only after the file worker confirms
materialization. A missing base hash creates a checked-absent source, and `from`
relocates an existing source with its replacement text in one reversible batch.
New and relocated sources stay under the shared authored root; generated module
state stays in `.ara/scripts/`.
