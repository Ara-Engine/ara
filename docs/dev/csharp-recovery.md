# Private C# recovery and exception diagnostics

Native editor clients take `PlayService::script_recovery()` before registering the
service. `ScriptRecoveryHandle::request` and `poll` use bounded owned mailboxes;
neither waits for a player, compiler or CLR. A host tick publishes the current J10
world/player/source/binding roster. Requests bind to that incarnation, coalesce
while pending, and refuse if it stops or changes. Unknown tickets return `None`.
At most 32 requests are pending and 64 completed statuses are retained per handle.
The backend publishes every returned diagnostic once before exposing a finished
status. The UI displays these diagnostics without logging them again. Local
refusals also wait for Host ingress; scoped script rows are never replayed by the
control mailbox. If the owning Host disappears, new shutdown refusals use the
diagnostic bus, without re-emitting previously acknowledged rows.
The player also pumps queued managed diagnostics while paused, using a lock-free
pending hint. This never steps simulation or activates an ordinary reload, and
keeps earlier fault rows ahead of the authoritative recovery marker.

Recovery rebuilds the loaded gameplay module graph from current inputs on the warm
compile server. It validates those inputs again before publishing the candidate.
Every module gets a fresh collectible load context; managed statics do not transfer.
Only the player boundary activates the complete graph and reports `Reloaded`.
Boundary polling also works while paused and never advances simulation. ECS data,
world identity, authored files and transaction history remain intact. Compile or
activation failures retain last-good code and its disabled fault state. Changed
engine bindings or module boundaries require an explicit stop/reopen; recovery
never falls back to restarting Play.

Runtime fault rows carry `data.module` from their actual loaded module slot.
Only an authoritative `reloaded` marker may retire older fault contributions for
the same trusted Play world/player/source/binding and a listed module. Other
outcomes do not clear faults, and Console history remains intact. Later faults
must remain active even if an earlier recovery marker is processed afterward.

The private notification travels through the existing player link. `ARA0935` on
the ordinary diagnostic ingress carries `data.recovery` with `schema: 1`, `ticket`,
`outcome` and `modules`. Compiler rows retain their J10 compile metadata; host
ingress adds the trusted Play binding. No registry, MCP, public C# editor API or
native ABI slot is added.

Gameplay systems, behaviours, commands and editor callbacks attach structured
managed fault details to their original diagnostic row as `data.exception`:

```text
{schema:1, type, message, frames:[{method,file,line,column}], inners:[], truncated}
```

The actual caught exception is the root. Aggregate inners preserve their order;
frames run from throw site outward. Source paths come from portable PDBs and absent
coordinates are null. Formatting retains at most 64 frames across the whole tree,
16 nodes, eight inner-exception edges, and 8192 UTF-8 bytes per string. Omission
marks the affected node and root as truncated. Existing diagnostic code, message,
severity, subjects, location and compile/Play metadata are preserved. Rust command
failures return this same diagnostic instead of rebuilding it from message text.

Focused coverage: `.NET ExceptionDiagnosticsTests`, `EditorContractTests`,
`RecoveryTests`, `CompileLifecycleTests`; Rust `ara-script --test recovery`,
`ara-play-host` recovery/ingress units and `ara-player --test csharp`.
