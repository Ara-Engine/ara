# Ara.Agent

The facade `ara_run` scripts compile against, and nothing else.

- `ScriptContext` (`ara` in scripts): `Call(command, params)` (anonymous objects are
  serialized camelCase), `Log`, `Why` (the transaction's reason).
- `ICommandSink`: the only write path. The worker's sink forwards to its supervisor (the
  Rust `CommandSink`); an Engine Protocol JSON-RPC client can implement it directly.
- `CommandReply`, `AgentCommandException` (a failed call throws into the script).

JIT-only (reflection-based JSON); scripts never ship.
