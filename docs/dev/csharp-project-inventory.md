# Project C# inventory and schema publication

`ara_script::project::{inventory, compile, publish}` are the shared D30 entry
points. Each takes the project root; the authored root is
`ara_doc::AUTHORING_ROOT`. The compile tool implements folder ownership once in
`dotnet/src/Ara.CompileServer/ProjectInventory.cs`; the warm compile sessions use
that same implementation. Migration and export consume the returned inventory.

Relative roots are canonicalized once at each API boundary before any child
working directory is selected. Generated module directories and tracked schema
files are absolute canonical paths, so reloads remain valid after a cwd change.
The project-root `packages/` directory stays outside the authored root; folders
named `packages` inside authored content are ordinary source folders.

Every source has an authored-relative `path` and project-relative `storage_path`.
Every module returns its sources, references, folder-derived domain, hot setting,
and explicit `state_path`, `assembly_path`, `schema_path`, schema JSON, result and
input fingerprint. `inventory` describes the last intact publication as
`published`; `publish` checks current inputs and returns `compiled` or `cached`.
Errors are coded diagnostics. Callers must reject errors before checking or
shipping a partial graph; an editor may keep the failed project open to display
and fix them.

Sources without a manifest belong to `game`. A lowercase `<name>.module.json`
creates a nested boundary; names compare case-insensitively. Only `name`,
`references` and `hot` are accepted. Exact `editor` folder ancestors assign a
separate editor assembly, including below a gameplay manifest. An editor assembly
references its gameplay owner. Gameplay cannot reference editor assemblies.
Symlinks and nested projects are excluded from discovery; the project-root
`packages/` directory is outside the authored root.

Initial `ara_play_host::open_project` calls `publish(Gameplay)` before loading
scenes. Thus open, `ara check` and export generate schemas on a fresh clone without
Play. The returned schema results populate the schema DB before validation.
Generated JSON is written canonically under `.ara/scripts/<module>/`; it is never
copied into authored content. Export source lists come from the same inventory.

The publication marker records input, schema and assembly hashes. A missing,
corrupt or interrupted publication is rebuilt. Source and SDK/dependency bytes
contribute to the input hash. The module's generated project, input descriptor,
assemblies, PDB and generator outputs all stay under `.ara/scripts/<module>/`.
Source membership changes rebuild the owning module; changes to module/domain
boundaries request an L4 Play restart from the current snapshot, which loads the
new inventory. Editor code is never loaded into a player.

Preparation requests reuse a hidden, parent-bound SDK process. Each request
rescans the requested absolute root and rechecks compilation inputs; folder
ownership and results are not cached by the host. SDK content changes replace
the transport, and worker failures/timeouts dispose its exact owned process.
Projects without source files or manifests return an empty inventory before
initializing private compiler state, keeping read-only CLI fixtures clean.

The Play integration fixtures stage legacy Outpost sources under `AUTHORING_ROOT`
with a JSON manifest preserving its name/hot setting. Generated schemas come from
`.ara/scripts/`, and the first-start regression places a C# component directly in
the scene with no checked-in schema or warm Play to hide missing publication.
