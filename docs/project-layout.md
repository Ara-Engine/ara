# Project layout

`ara new <directory> --name <name>` (also `ara init`) and Hub Create produce the
same project. All authored files share the freely organized `content/` root (D30).
That name has one engine definition, `ara_doc::AUTHORING_ROOT`.

```text
my-project/
  ara.project.json              identity, main scene and engine pin
  content/                     all authored files, freely organized
    scenes/main.scene
    scripts/combat/combat.module.json
    editor/                    editor-only C# may also live in nested editor folders
  settings/
    project.json               input, rendering/look, physics, layers and editor defaults
    ara.imports                texture import defaults
    builds/windows.export.json store-agnostic export presets
  packages/                    reserved; excluded until plugins exist
  .ara/                        generated projects, assemblies, schema, caches and logs
  builds/                      export output
  AGENTS.md
  .mcp.json
  .gitattributes
  .gitignore
```

`content/` imposes no folder taxonomy. Scenes, prefabs, C#, materials, source models,
textures, audio, UI, animations, terrain and looks may be grouped by feature.
Asset identity is carried by IDs. **References are relative to the authored root:**
`@props/crate.prefab#pf_crate001` addresses `content/props/crate.prefab`. The marker's
`main` is likewise `scenes/main.scene`, without a `content/` prefix. Host document
storage paths remain project-relative so project settings and authored files
with similar names remain distinct. The shared semantic file inventory exposes
both `path` (reference-relative) and `storagePath`, plus Authored or Project scope.

C# uses lowercase `<name>.module.json` manifests, canonical JSON such as
`{"name":"Combat","references":["Core"],"hot":false}`. References default to an
empty list and hot defaults to false. Each folder has at most one manifest; a
nested manifest starts a separate module. No manifest means the implicit game
module. Every exact lowercase `editor` ancestor forces editor-only compilation,
including within a gameplay module's folder. Generated editor modules depend on
their gameplay owner. Gameplay modules cannot reference editor modules, and no
profile override can make editor code ship. Module names are case-insensitively
unique; generated csproj, obj, bin, reflected types and assemblies live under
`.ara/scripts/<module>/`.

Settings are versioned project policy. Build presets hold platform, configuration,
scripting lane, icon/version metadata and signed/hash-pinned runtime templates.
The preset's `content` is the physical project-relative authored-root directory;
`icon` and conformance `scriptProject` are authored-root-relative. Build artifact
pins and native plugin manifests retain their project-relative build paths.
New presets have no runtime pins: choose an installed release's runtime template
before exporting. Executable names default to the project name.

`settings/project.json` keeps input, rendering/look, physics, layers and editor policy;
its initial `{}` inherits engine defaults. The host exposes it as `pj_preferences`,
with the existing `p_settings` record and `ProjectSettings` schema. For example:

```sh
ara do property.set --doc settings/project.json -p entity=p_settings -p path=settings.rendering.quality -p value=high --why 'Use High by default'
```

Native game UI defaults to `ui/game.ui`, `ui/game.css`, `ui/bindings.json` and
optional `ui/resources.json` beneath the authored root. To organize it elsewhere,
set the `ui` policy in `settings/project.json` with `main`, `stylesheet`, `bindings`
and `resources` authored-relative paths. Entries also accept `@path#id` references;
known IDs resolve through the asset index. Explicit missing entries report errors.
All source and resource files must resolve inside the authored root, including
through links. Missing optional resource bytes retain their visible diagnostics.
Migration writes existing bootstrap paths into this policy so asset moves can
repair them, and pins legacy UI identities before moving native markup.

These edits use the ordinary op stream and history. Migration reads legacy marker
policies; identity, main scene, engine pin and description stay in the root marker.
Import defaults use `import.project.configure`. Ordinary open, compilation and export
require layout version 4; older layouts receive a migration action before work starts.

`.ara/` and root `builds/` are gitignored and excluded from authored discovery.
Root `packages/` is present for future plugins and excluded too. A freely authored
folder called `builds` or `target` inside `content/` stays visible. Project
`settings/builds/` presets remain versioned and discoverable.

Repository tests and evidence belong outside sample projects, in
`tests/samples/<sample>/` and `evidence/samples/<sample>/`. Generated captures go
under `target/ara-samples/`; `ARA_CAPTURE_OUTPUT_DIR` explicitly selects another
location. Templates exclude machine state, output, evidence and test tooling.

## Migrating an existing project

Close its editor and daemon, then inspect and apply:

```sh
ara --root path/to/project project migrate --dry-run --json
ara --root path/to/project project migrate
```

The dry run reads without creating a lock, directory or cache. Migration merges
legacy type folders, earlier `assets/` and `game/` drafts, and root `scripts/` into
`content/`. Existing authored files beneath `content/` stay there. Overlapping
file destinations, including case collisions,
are rejected before anything changes. Empty authored folders are preserved.

References in native documents, graph payloads, UI markup, C#, configuration and
documentation become authored-root-relative while retaining IDs. Relative source
media companion URIs retain their meaning. Legacy TOML modules become lowercase
JSON manifests with names, references and hot behavior preserved; an old editor
profile outside an editor folder moves beneath an editor subtree. Generated
legacy module output is retained under `migration-support/generated-scripts/<module>/`;
the compiler regenerates current schemas and assemblies under `.ara/scripts/<module>/`.

Migration moves import defaults into `settings/`, old presets into
`settings/builds/`, and marker policy into `settings/project.json`. Existing export
fields and pins are preserved; old sample helper literal presets are translated.
Evidence, tests and obsolete machine-specific export helpers are retained under
project-local `migration-support/`, excluded from authored discovery. Keeping these
files inside the authority lets the same transaction restore them on Undo.

Apply takes the authority lock, rechecks the complete prepared inventory, and
submits one typed `project.migrate` batch. The command emits reversible file states
and a marker edit; one attributed transaction is appended to the journal. Existing
history is retained unchanged. Success waits for physical writes and journal
acknowledgement. The marker records `layoutVersion: 4`. A subsequent dry run reports
no changes. Run `ara check` to validate the result.

If the old journal contains committed native edits not yet saved to their files,
migration stops with ARA1905 and leaves everything unchanged. Recover and save that
project with its originating Ara version before retrying; retain the journal and
file-history snapshots throughout recovery.

Hub and editor opening an older project show the move preview and Migrate / Cancel.
Cancel keeps it closed and unchanged. Other opens report ARA1914 with the migration
action, before compilation or import. Undo restores the old layout and exact bytes;
ordinary commands then require migration again. Redo remains available in the same
open authority. Older history cannot be applied across the migration boundary; Undo
the migration first to restore its original path semantics.

## Exporting the worked project

For an Outpost Windows build, `settings/builds/windows.export.json` selects
`content` as its authored input and supplies the runtime template pins. Export
produces an envelope whose `build/` folder is the player distribution:

```text
builds/windows/<build-id>/
  build/
    Outpost.exe                game name, icon and version metadata
    data/
      ara-launch.json
      ara-crash.json
      engine.json
      game.arapack              cooked project content, no author checkout paths
      game/                     compiled gameplay image and scripts.json
      resources/                cooked engine runtime resources
    THIRD_PARTY_NOTICES.txt
  distribution.json            store-agnostic delivery manifest
  report/                      verification captures and reports
  logs/
  symbols/<build-id>/
```

Only the contents of `build/` ship to players. Linux follows the same structure
with an `Outpost` executable. macOS ships `Outpost.app`, placing its executable in
`Contents/MacOS` and its one data root in `Contents/Resources`. Distribution output
stays compatible with any store's upload tooling (D22).

The runtime's `data/game/` directory holds compiled scripts; its name is independent
of the authored `content/` root. Export reports, captures, logs and symbols stay
outside the distribution. Relocated launch resolves data relative to the executable.
