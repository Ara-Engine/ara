# arabench

The agent-ergonomics benchmark (R03 §4.13; M1 "arabench v0"). It drives headless
agent CLIs (Claude Code, Codex) against fresh `ara init` projects over the generated
MCP config, verifies the results deterministically and records how the agents got
there. Agent success on this suite is a product metric: a regression is a bug.
Depends on ara-host, ara-doc, ara-ops, ara-schema, ara-merge, ara-rpc, ara-proto,
ara-diag, ara-json.

## Run it

```sh
cargo run -p arabench -- run --agent claude:sonnet-5.5 --suite v0     # one agent
cargo run -p arabench -- run --agent codex:gpt-6-luna --label baseline --jobs 3
cargo run -p arabench -- selftest [--binary]   # reference solutions, no agents
cargo run -p arabench -- compare results/a/results.json results/b/results.json
cargo run -p arabench -- list
```

`run` builds `ara` first (same profile) and puts it first on the agent's `PATH`
(`--ara <dir>/ara.exe` pins a frozen binary; it must be named `ara`). `--merge`
re-runs some `--task`s into an existing `results.json` of the same label.
Recorded runs and the before/after analysis: [`results/README.md`](results/README.md).
Output: `results/<label>-<agent>/results.json`, `report.md` and
`transcripts/<task>.jsonl` (trimmed, failures only). `--keep` keeps each attempt's
temp project (`<temp>/arabench/<agent>-<task>-…/proj`, transcript beside it).
Agents are `<cli>:<model>[@effort]`; Claude model names map `sonnet-5.5` →
`claude-sonnet-5-5`; Codex defaults to `medium` reasoning effort.

## Code map

- `task.rs`: `Task` (id, area, prompt, fixture, setup, budget, verify, solve),
  `Check`/`Checks`.
- `suite/`: suite v0, 22 tasks by area (`assembly`, `components`, `prefabs`,
  `hierarchy`, `repair`, `workflow`) plus shared verifier helpers in `mod.rs`
  (`one`, `at`, `vec_is`, `num_is`, `parent_is`, `with_why`, `clean`,
  `prefabs_untouched`).
- `fixture.rs`: `ara init` (or `Init::Minimal` for tests), copy
  `fixtures/outpost/`, run the task's setup; `patch`, `scratch`.
- `world.rs`: the finished project, reopened in-process (journal replayed; a
  daemon the agent left running is shut down): resolved entities (`Ent`),
  history, `ara check` errors, files, git, the agent's tool calls.
- `solve.rs`: `Solver`, the in-process agent session reference solutions and setup
  hooks use; records calls like a transcript would.
- `agent.rs`: agent specs and their isolated headless command lines.
- `transcript.rs`: Claude `stream-json` / Codex `--json` → normalized `ToolCall`s,
  turns, tokens, cost, ARA codes of failed calls, raw document edits.
- `runner.rs`: attempts (temp project, live turn/time budgets, kill the process
  tree, verify, save failing transcripts), `self_test`.
- `record.rs`, `report.rs`: JSON results and markdown (one run; compare runs).

## Writing a task

- The prompt is a user request ("make the gate door open by itself"), never a tool
  tutorial: no command names, no IDs unless a user would know them.
- The verifier reads only state: documents (effective values, prefab-resolved),
  history (`with_why`: changed by commands carrying a `why`, not file edits), files,
  git, and calls (`ToolCall::is_dry_run`, `applies`). Never the agent's prose.
- Give a reference solution that does what an ideal agent would through the
  Engine Protocol. `cargo test -p arabench` runs every solution and asserts its
  verifier rejects the untouched fixture and accepts the solution.
- Fixture IDs are fixed (`e_gen001`, `pf_crate001`); verifiers may use them, prompts
  use names. Setup hooks patch the fixture (`fixture::patch`) or create history
  (`Solver::open(root, "agent:helper")`).

## Isolation

- Claude: `--strict-mcp-config --mcp-config <proj>/.mcp.json --setting-sources
  project,local --disable-slash-commands --no-session-persistence
  --permission-mode bypassPermissions`.
- Codex: `--ignore-user-config --ignore-rules --ephemeral --skip-git-repo-check
  --dangerously-bypass-approvals-and-sandbox` plus `-c mcp_servers.ara.*` (what the
  project `AGENTS.md` tells Codex users to add).
- The user's own memory file (`~/.claude*/CLAUDE.md`, `~/.codex/AGENTS.md`) still
  loads; neither CLI can skip it without dropping OAuth. Keep that in mind when
  comparing machines.

## Gotchas

- Each attempt is a separate temp project, so attempts can run in parallel
  (`--jobs`); about 11 agents share this machine, so keep `--jobs` small.
- Turns are tool-using steps (Claude: assistant messages with tool calls; Codex:
  tool items). Exceeding the budget kills the agent and fails the task.
- Raw edits come from two sources: transcript heuristics (`edit` on a document,
  shell writes naming one) and the engine's own `external edit` transactions.
- `World::open` waits up to 20 s for the agent's `ara mcp` to release the lock.
- The merge task's setup needs `git`; it uses a stand-in merge driver, then writes
  exactly what `ara merge` produces.
- Agent markers inherited from the shell that launched arabench (`CLAUDECODE`,
  `CLAUDE_CODE_*`, `CODEX_*`, `ARA_AUTHOR`) are removed from the agent's
  environment, so `ara` attributes CLI edits to the agent under test.
- Verify colors as linear (`Light.color`): sRGB orange is about (1, 0.21, 0).

## Testing

`cargo test -p arabench`: transcript parsing (Claude and Codex fixtures), agent
spec parsing, timestamps, and the suite self-test (every task, `Init::Minimal`).
