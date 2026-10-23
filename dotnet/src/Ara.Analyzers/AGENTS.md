# Ara.Analyzers (+ Ara.Analyzers.CodeFixes)

Roslyn analyzers for gameplay modules (any compilation referencing Ara.Runtime). Code
fixes live in `Ara.Analyzers.CodeFixes` (they need Workspaces; analyzers must not).

| ID | Rule | Fix |
|---|---|---|
| ARA0001 | Unity-isms: `using UnityEngine`, `MonoBehaviour`, `Update`/`Start`/..., `GetComponent<T>()`, `.x/.y/.z/.w`, `Time.deltaTime`, `Debug.Log` | yes (also CS1061 on `.x`) |
| ARA0100 | Mutable static field or static auto-property | - |
| ARA0101 | Behaviour instance field without `[Transient]` | adds `[Transient]` |
| ARA0200 | In `[Simulation]` code: async/await, tasks, threads, `System.Random`, wall clock, I/O, capturing lambdas in commands | - |
| ARA0300 | Allocation on the tick path (`[Query]`, `OnTick`/`OnFrame`, command `Execute`): class/array `new`, LINQ, non-static lambdas, boxing, string building. Warning | - |
| ARA0400 | NativeAOT/trim-unsafe: `Assembly.Load*`, `Reflection.Emit`, `Activator.CreateInstance(Type)`, string reflection, `MakeGeneric*`, `Invoke`, `Expression.Compile`, `dynamic` | - |

## Rules

- IDs are stable; add rules at the end of a range. Messages say what to use instead.
- Analyzers skip generated code and attribute arguments.
- The compile server runs them on every edit (changed files only); errors block the reload.

## Testing

`dotnet test tests/Ara.Analyzers.Tests`: `Verify.Diagnostics` checks `[|marked|]` spans;
`Verify.Fix` applies fixes until none remain. The Unity `MonoBehaviour` door must come
out as an Ara `Behaviour` that compiles with no errors.
