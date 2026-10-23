# Ara.Host

Dev-only hosting (JIT): hostfxr loads `Ara.Host.dll` with its runtimeconfig into the
default ALC and calls `Ara.Hosting.DevExports.Init`. Never part of a NativeAOT image.

## Pieces

- `DevExports.Init`: `ScriptRuntime.Init(host, api, new DevModuleLoader())`.
- `DevModuleLoader`: loads each module generation into a collectible `ModuleLoadContext`
  (which resolves `Ara.Runtime` to the default copy), runs its generated `Register` and
  pre-JITs its entry points while staging, then queues a `SwapUpdate`; EnC deltas become
  `DeltaUpdate`s (`MetadataUpdater.ApplyUpdate` at the tick boundary). Prebuilt
  assemblies load without a compile server (no reload).
- `CompileClient`: starts `Ara.CompileServer` (app host or `dotnet exec`), serves it one
  duplex named pipe (`Protocol.cs`, shared with the server), forwards compile state for
  tick-thread priority.

## Runtime config (S2 findings, keep them)

`TieredCompilationQuickJitForLoops=false` (else optimized loop methods pin ALCs),
`MetadataUpdaterSupport=true` (Release would disable EnC). `DOTNET_MODIFIABLE_ASSEMBLIES=debug`
is set by the Rust host.

## Gotchas

- With a debugger attached, deltas are refused: the loader asks for an L3 image instead.
- Retired ALCs must unload: nothing may keep module types (tables are dropped first at
  shutdown). `AlcsAlive` counts leaks; the Rust tests assert zero.
