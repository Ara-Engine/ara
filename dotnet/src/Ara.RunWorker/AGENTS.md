# Ara.RunWorker

The `ara_run` worker process (R10 D5). Supervised by `ara_script::run::RunPool`, which
enforces wall-clock and memory limits and kills the worker on violation.

- Protocol (stdin/stdout, one JSON object per line): `run` → `call`* ↔ `reply` → `done`; `ready` once warm.
- `ScriptRunner`: wraps the script as the body of `async Task<object?> Run(ScriptContext ara)`
  (`#line` maps diagnostics to script lines), compiles against the framework and
  `Ara.Agent` only, checks the `Whitelist`, runs it in a throwaway collectible ALC, and
  serializes the return value.
- `Whitelist`: a first cut in the shape of s&box `Sandbox.Access`: every referenced type
  and member must be in the language runtime (primitives, collections, LINQ, text, JSON,
  tasks) or `Ara.Agent`; no files, network, processes, threads, reflection, interop or
  dynamic loading. P/Invoke and module refs are refused.

## Testing

`dotnet test tests/Ara.RunWorker.Tests` (facade calls, refusals, line mapping).
Limits: `cargo test -p ara-script --test run_worker`.
