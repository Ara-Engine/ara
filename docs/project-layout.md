# Project layout

`ara new <directory> --name <name>` (also `ara init`) and Hub Create produce the
same project. All authored files share one freely organized root, provisionally
named `game/`. That name has one engine definition, `ara_doc::AUTHORING_ROOT`.

```text
my-project/
  ara.project.json              identity, main scene and engine pin
  game/                        all authored files, freely organized
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

`game/` imposes no folder taxonomy. Scenes, prefabs, C#, materials, source models,
textures, audio, UI, animations, terrain and looks may be grouped by feature.
Asset identity is carried by IDs. **References are relative to the authored root:**
`@props/crate.prefab#pf_crate001` addresses `game/props/crate.prefab`. The marker's
`main` is likewise `scenes/main.scene`, without a `game/` prefix. Host document
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

These edits use the ordinary op stream and history. Legacy marker policies remain
readable until migration; identity, main scene, engine pin and description stay
in the root marker. Import defaults use `import.project.configure`.

`.ara/` and root `builds/` are gitignored and excluded from authored discovery.
Root `packages/` is present for future plugins and excluded too. A freely authored
folder called `builds` or `target` inside `game/` stays visible. Project
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
legacy type folders, `content/`, intermediate `assets/` and root `scripts/` into
the authored root. Overlapping file destinations, including case collisions,
are rejected before anything changes. Empty authored folders are preserved.

References in native documents, graph payloads, UI markup, C#, configuration and
documentation become authored-root-relative while retaining IDs. Relative source
media companion URIs retain their meaning. Legacy TOML modules become lowercase
JSON manifests with names, references and hot behavior preserved; an old editor
profile outside an editor folder moves beneath an editor subtree. Generated
legacy module type output moves to `.ara/scripts/<module>/`.

Migration moves import defaults into `settings/`, old presets into
`settings/builds/`, and marker policy into `settings/project.json`. Existing export
fields and pins are preserved; old sample helper literal presets are translated.
Evidence, tests and obsolete machine-specific export helpers move to the sibling
`<project>-support/`; `--support-dir` selects another directory outside the project.

Apply takes the authority lock, rechecks planned source files, and rolls back
completed moves and writes on failure. Journal reference paths, document storage
paths and matching flush hashes are updated. The marker records `layoutVersion: 3`.
A subsequent dry run reports no changes. Run `ara check` to validate the result.
