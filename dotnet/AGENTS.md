# dotnet/: the C# side of Ara

C# 14 / .NET 10 (D11). The Rust host is `crates/ara-script`; read its `AGENTS.md` for
the ABI and the host API. Design: `docs/research/10-csharp-scripting.md` (R10 D1–D5),
`docs/decisions.md` A13 and `spikes/s2-csharp-host/RESULT.md`.

## Projects

| Project | Role | Loaded by |
|---|---|---|
| `src/Ara.Runtime` | Gameplay API (`Entity`, `[Component]`, `Behaviour`, `[Query]`, `[Command]`, `World`, `Rng`, `Commands`) and the C# half of the ABI. AOT-safe. `Generated/` is written by Rust (`csgen`). | every build |
| `src/Ara.Generators` | Source generator: registration, chunk-batched system entry points, Behaviour data columns, command entry points, the NativeAOT export | modules (analyzer) |
| `src/Ara.Analyzers` (+ `.CodeFixes`) | ARA0001–ARA0506 (incl. the API allowlist profiles) and their code fixes | modules (analyzer), compile server |
| `src/Ara.Host` | Dev-only loader: collectible ALCs, EnC deltas, cascades and relinks, the compile-server client. hostfxr loads it | JIT hosts |
| `src/Ara.CompileServer` | Out-of-process warm Roslyn: module graph and cascades, human/agent scheduling, generator, analyzers, L1/L2/L3 classifier, heat, watcher | its own process |
| `src/Ara.Agent` | The facade `ara_run` scripts compile against (`ara.Call`, `ICommandSink`) | worker |
| `src/Ara.RunWorker` | The `ara_run` worker: compile, IL whitelist, run | its own process |
| `samples/Outpost.Sample` | R10 §D3 sample: Door behaviour, UseMedkit command, Gravity/Movement systems | host |
| `samples/Outpost.Sample.Aot` | The sample as one NativeAOT library exporting `ara_script_init` | host (AOT) |
| `tests/*` | xunit tests, plus `Ara.TestModule` (a module the Rust tests load) | `dotnet test` |

## Commands

```sh
dotnet build Ara.slnx -c Release      # what the Rust tests use (artifacts/bin/<Project>/release)
dotnet test Ara.slnx -c Release
dotnet publish samples/Outpost.Sample.Aot -c Release -r win-x64   # NativeAOT (needs vswhere.exe on PATH here)
```

Outputs go to `dotnet/artifacts/` (`ArtifactsPath`); never commit them. Package versions
are central (`Directory.Packages.props`). Warnings are errors.

## Writing gameplay modules

- A module is a folder with `ara.module.toml` (`[module] name = "..."`, optional `hot`,
  `references = ["Core"]`, `profile = "editor"`), compiled as one assembly; keep it under ~5k
  lines (L3 cost, including the Windows AMSI scan of the new image, scales with module size).
- A module may use public types of the modules it references (no cycles). An L3 of a
  referenced module relinks or rebuilds this one in the same swap.
- The compile server adds these usings: `System`, `System.Collections.Generic`,
  `System.Linq`, `System.Numerics`, `Ara`. Mirror them in a module `.csproj` for IDEs.
- Every reflected type and field needs a `/// <summary>` (ARA0703): it becomes the
  inspector tooltip, the schema doc and the agent tool description.
- Types that the generator extends must be `partial` (ARA0701).

## Rules

- `Ara.Runtime` stays AOT-safe (`IsAotCompatible`): no reflection, no dynamic code. Anything
  that loads assemblies or applies deltas goes in `Ara.Host`.
- Never edit `src/Ara.Runtime/Generated/*.g.cs`; change the Rust ABI or schema and run
  `ARA_UPDATE_GENERATED=1 cargo test -p ara-script --test generated`.
- Every `[UnmanagedCallersOnly]` entry point catches everything.
- Analyzer/generator IDs: analyzers 0001–0699, generator 0700–0799, host 0900–0999.
  Never renumber or reuse one.
- The L1/L2 classifier (`Ara.CompileServer/EditClassifier.cs`) is conservative on purpose: a
  missed rude edit crashes the game process (S2 finding 1). New L1/L2 allowances are reviewed changes.
