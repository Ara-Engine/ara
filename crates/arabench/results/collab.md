# arabench `collab` results (2026-10-23)

Two tasks where a human teammate, Mia, holds claims in the outpost level while
the agent works (D21). Claims are enforced for agents, so the agent must find out
what is claimed and work around it.

- `collab-avoid-claimed`: "double every light"; Mia claims the DoorLamp. Pass:
  the three other lights doubled live, the DoorLamp untouched, no hand edits.
- `collab-branch-claimed`: "make the gate door twice as fast, set it up so it
  goes in once Mia is done"; Mia claims the GateDoor. Pass: the live door
  untouched and a submitted agent branch session holding the doubled speed.

One round per configuration, `--jobs 2`, Windows 11, Claude Code
`claude:sonnet-5.5` and Codex `codex:gpt-6-luna` (medium) over the generated MCP
config.

| Run | Engine | Claude | Codex |
|---|---|---|---|
| `collab` | `62b8ce4` | 2/2 · 11 turns · 53 s · $0.18 | 1/2 · 26 turns · 80 s |
| `collab2` | `73903ab` | 2/2 · 15 turns · 43 s · $0.19 | 2/2 · 21 turns · 60 s |

| Task | Claude run 1 / 2 (turns) | Codex run 1 / 2 (turns) |
|---|---|---|
| `collab-avoid-claimed` | pass 4 / pass 7 | **FAIL** 14 / pass 13 |
| `collab-branch-claimed` | pass 7 / pass 8 | pass 12 / pass 8 |

## What happened

- Claude had no failed calls in either run (no ARA2500): it left the claimed
  DoorLamp out and, for the door, submitted a branch session. Transcripts are only
  kept for failures, so whether it read `ara_session who` first is not recorded.
- Codex in run 1 batched all four lights, got ARA2500 naming Mia, then put the
  **whole** change in a branch session. The branch correctly waited on Mia's
  claim, so none of the unclaimed lights landed: a fail. ARA2500 said "make the
  change in a branch session", which reads as the whole change.
- Fix (`73903ab`): ARA2500, its catalog entry and the MCP texts now say the batch
  was not applied, to apply the rest without the claimed subject, and to queue
  only the claimed part in a branch. Run 2: Codex 2/2. With one round each, the
  pass is consistent with the fix but not proof of it.

## Remaining friction

- Codex sends fields one `ara_session` action has to another (`doc` to `branch`;
  ARA1101 ×3 in run 2): the merged `action` schema lists `doc` "for: history,
  claim". Accepting and ignoring a few common fields per action, or per-action
  schemas, would remove it.
- The `ara` CLI has no `who`, `claim` or `--branch`, and each CLI call is its own
  session (claims end with it). CLI-driven agents only learn about claims from
  ARA2500. Both tasks were solved through MCP here.
