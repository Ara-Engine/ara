# ara (CLI)

The `ara` binary: the single entry point for humans and agents. The kernel commands
work offline on project files; the Engine Protocol daemon comes later in M1.

## Commands

| Command | Does | Exit 1 when |
|---|---|---|
| `ara fmt [--check] <files>` | rewrites documents canonically, repairing `@path#id` references | `--check` and a file would change; a file has errors (left untouched) |
| `ara check [--json] [files]` | schema, reference, structure and prefab checks (all project documents by default) | any error |
| `ara schema [--json] [--out dir]` | Schema DB summary; `--json` prints `reflection.json`; `--out` writes `reflection.json`, `schema/<kind>.schema.json`, `schema/types/<Type>.schema.json` | project types are invalid |
| `ara merge [--json] [-o out] <base> <ours> <theirs>` | semantic 3-way merge, result over `ours` (git `%O %A %B`) | conflicts (recorded in `$conflicts`) |

Exit 2: the command could not run (bad root, unreadable file, unmergeable input).
`--root <dir>` is global.

## Project layout

- Root: `--root`, else the nearest ancestor of the current directory containing
  `ara.project.json` (contents reserved), else the current directory.
- `**/*.types.json`: data-only types (`{"$schema": "ara://schema/types/1", "types": {...}}`,
  the same shape as `reflection.json` types).
- Documents: files whose extension matches a registered kind (`.scene`, `.prefab`).
- `**/<file>.import.json`: sidecar giving an imported source file (`meshes/a.glb`) its
  asset `id`.
- Hidden directories, `target/` and `node_modules/` are skipped.
- Reference paths and diagnostic `file`s are root-relative with `/`.

## Code map

- `main.rs`: clap definitions and exit codes.
- `project.rs`: root discovery, type loading, scanning, asset index, `load()`.
- `commands/{fmt,check,schema,merge}.rs`: one file per subcommand.

## Gotchas

- `merge` reads without the asset index (git passes temp files), so it never
  rewrites references; run `ara fmt` afterwards to repair paths.
- `check` loads every project document (needed for prefab checks), even when given
  a single file. Fine for now; make it lazy before projects get large.
- JSON output goes through `ara_json::to_canonical_string`, so it is stable.

## Testing

`cargo test -p ara` runs the binary on `tests/fixtures/`:
- `project/`: a clean sample (types, sidecars, nested prefabs, a scene);
- `broken/`: one scene with located errors (JSON snapshot);
- `merge/`: base/ours/theirs with one conflict;
- `messy.scene`: non-canonical input for `fmt` (snapshot).

The `reflection` snapshot is the full Schema DB of the sample project; review its
diff whenever types or commands change.
