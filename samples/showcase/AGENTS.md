# AGENTS.md: Material and character showcase

An Ara game project. These are the engine's conventions; follow them in every
edit.

## Conventions

- **Axes and units:** Y-up, right-handed. Lengths in meters, time in seconds,
  mass in kilograms, angles in **degrees** (`Transform.rotation` is XYZ Euler
  degrees). Each field's unit is in its schema (`x-unit`; `ara schema`).
- **Documents** (`*.scene`, `*.prefab`) are canonical JSON: a flat `records` map
  keyed by short IDs, defaults omitted. Never hand-order keys; `ara fmt` owns the
  layout.
- **IDs:** `<prefix>_<base32>`: entities `e_8b21`, scenes `sc_…`, prefabs `pf_…`.
  An entity inside a prefab instance is addressed through it: `e_4f9a/e_m4tz`.
- **Naming entities in commands:** an ID or address, a batch symbol (`$crate`),
  or a name: `name:Crate`, or `name:Floor/Crate` when the name alone matches
  several entities.
- **References:** `@path#id` (`@prefabs/crate.prefab#pf_crate001`). Either half
  repairs the other.
- **Use commands, not raw file surgery, for graphs**, hierarchy (parents, order)
  and prefab overrides: `ara_apply` over MCP or `ara do <command> -p key=value`.
  `doc` is optional: the engine picks the document holding the entities you name,
  else the main scene (`levels/main.scene`). Batches are atomic; mint IDs with
  `"as": "$crate"` and use them in later calls. Preview with `dryRun`. Small
  value edits straight in the JSON are fine: the engine validates them.
- **Review:** every change is a transaction with a `why` (agents must give one).
  Agent changes apply live but stay unreviewed until a human runs `ara accept` or
  `ara revert`.
- **When something is wrong, ask the engine first:** `ara why name:Crate not
  visible`, `ara why ARA1302`, `ara why "is the frame slow"` (MCP: `ara_why`). It
  answers with ranked causes, their evidence and fixes you can run as is.

## Tools

- MCP: `.mcp.json` starts `ara mcp` (13 tools). Over `ara mcp` the session is
  optional (one begins on first use); `ara_session {"action": "begin", "client":
  "<you>"}` names yours. Codex: add `[mcp_servers.ara]` with `command = "ara"`,
  `args = ["mcp"]` to its config.
- CLI: `ara check` (validate), `ara query "Light"` / `ara query Crate`,
  `ara inspect` (main scene) or `ara inspect --doc outpost name:Crate`,
  `ara do property.set -p entity=name:Crate -p path=Light.intensity -p value=500
  --why "<reason>"`, `ara history`, `ara undo`, `ara logs`, `ara why`, `ara schema`. Add
  `--json` to any. `--args` takes JSON with or without quotes (`{entity: e_8b21}`),
  `@file.json` or `-` (stdin). The CLI attributes changes to the agent running it
  (Codex, Claude Code) or to `ARA_AUTHOR`.
- Reference for this engine version: `ara docs --llms` writes
  `.ara/docs/llms.txt` and per-type pages.
- `ara serve` runs the project daemon; the CLI and `ara mcp` use it when it runs.
