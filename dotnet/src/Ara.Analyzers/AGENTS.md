# Ara.Analyzers (+ Ara.Analyzers.CodeFixes)

Roslyn analyzers for gameplay and editor modules (any compilation referencing Ara.Runtime). Code
fixes live in `Ara.Analyzers.CodeFixes` (they need Workspaces; analyzers must not).

| ID | Rule | Fix |
|---|---|---|
| ARA0001 | Unity-isms: `using UnityEngine`, `MonoBehaviour`, `Update`/`Start`/..., `GetComponent<T>()`, `.x/.y/.z/.w`, `Time.deltaTime`, `Debug.Log` | yes (also CS1061 on `.x`) |
| ARA0100 | Mutable static field or static auto-property | - |
| ARA0101 | Behaviour instance field without `[Transient]` | adds `[Transient]` |
| ARA0200 | In `[Simulation]` code: async/await, tasks, threads, `System.Random`, wall clock, I/O, capturing lambdas in commands | - |
| ARA0300 | Allocation on the tick path (`[Query]`, `OnTick`/`OnFrame`, command `Execute`): class/array `new`, LINQ, non-static lambdas, boxing, string building. Warning | - |
| ARA0400 | NativeAOT/trim-unsafe: `Assembly.Load*`, `Reflection.Emit`, `Activator.CreateInstance(Type)`, string reflection, `MakeGeneric*`, `Invoke`, `Expression.Compile`, `dynamic` | - |
| ARA0500 | API not on the module profile's allowlist (`ApiProfiles`: gameplay by default, editor) | - |
| ARA0501-0505 | Files, network, reflection/dynamic code, threads/locks/tasks/`await`, native interop (`extern`, `Marshal`) | - |
| ARA0506 | Process state: `Console`, `Environment`, `GC`, `Debug.Write*`, `Process`, debugger | `Log.Info`/`Log.Error`, `"\n"`, drops `GC.Collect()` |

## API profiles (`ApiProfiles.cs`)

Allowlists by profile, applied to `System.*`/`Microsoft.*` namespaces (Ara, module and user
types are always allowed). **gameplay**: primitives, strings, math, spans, collections, LINQ,
numerics and SIMD intrinsics, text and regex, globalization, buffers, `Unsafe`/`MemoryMarshal`,
in-memory streams, `Debug.Assert`, `Stopwatch`. **editor** (`profile = "editor"` in
`ara.module.toml`, MSBuild `AraModuleProfile`): everything except dynamic code and assembly
loading, native interop and ending the process. **agent**: the `ara_run` worker's IL check
(the file is linked into `Ara.RunWorker`). ARA0400 and, in `[Simulation]` code, ARA0200 report
their own APIs; ARA05xx stays quiet there instead of reporting twice.

## Rules

- IDs are stable; add rules at the end of a range. Messages say what to use instead.
- Analyzers skip generated code and attribute arguments.
- The compile server runs them on every edit (changed files only); errors block the reload.

## Testing

`dotnet test tests/Ara.Analyzers.Tests`: `Verify.Diagnostics` checks `[|marked|]` spans;
`Verify.Fix` applies fixes until none remain. The Unity `MonoBehaviour` door must come
out as an Ara `Behaviour` that compiles with no errors. `ApiAccessTests`: refusals per area,
the allowed core runtime, the editor profile, `[Simulation]` deduplication, the output fixes.
