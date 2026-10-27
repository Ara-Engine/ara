# Ara.Host

Dev-only hosting (JIT): hostfxr loads `Ara.Host.dll` with its runtimeconfig into the
default ALC and calls `Ara.Hosting.DevExports.Init`. Never part of a NativeAOT image.

## Pieces

- `DevExports.Init`: `ScriptRuntime.Init(host, api, new DevModuleLoader())`.
  Each call creates its own runtime/loader; the CLR and Ara.Runtime assembly are shared.
  `ModuleLoadContext`: one collectible ALC per module generation; `Ara.Runtime` resolves to
  the default copy, referenced modules to the generations staged when the context was made
  (only the module's reference closure, so unrelated old generations are not pinned).
- `DevModuleLoader`: stages everything off the tick thread, then queues updates for the next
  tick boundary.
  - `Reload` (initial, L3, cascade): loads each image into a fresh ALC (a relink entry reloads
    the module's last image and re-applies its EnC delta chain), resolves `Register`, pre-JITs
    the entry points, runs `Register`, and queues one `SwapUpdate` for the whole cascade. If
    any module fails to stage, none is queued.
  - `Delta` (L1/L2): appended to the module's chain (relinks replay it) and queued as a
    `DeltaUpdate`; `ApplyUpdate` runs at the boundary. A rejected delta asks the server for L3
    (`EscRuntimeRejected`); with a debugger attached it asks for L3 up front.
  - `Diagnostics` with errors → an `ErrorUpdate`; `Restart` → `ARA0926` and an L4 report per module.
  - Leak check after each swap: a retired generation alive after two natural gen2
    collections is reported once (`ARA0927`, L4). Nothing is collected on the game's behalf.
  - Fills `ReloadReport` phases: server timings, transfer (cross-process `Stopwatch`), load
    (includes the Windows AMSI scan), pre-JIT, register.
- `CompileClient`: starts `Ara.CompileServer` (app host or `dotnet exec`), serves one duplex
  named pipe, sends `Hello` with the runtime's EnC capabilities (`MetadataUpdater.GetCapabilities`),
  `Watch` (with module references and profile), `Change` (`request_compile`), `SetHeat`,
  `Rebuild`; forwards compile state for tick-thread priority.
  Its reader explicitly binds the owning runtime, so logs, diagnostics and staged
  updates cannot route to another world. Shutdown joins only this loader's reader.

## Runtime config (S2 findings, keep them)

`TieredCompilationQuickJitForLoops=false` (else optimized loop methods pin ALCs),
`MetadataUpdaterSupport=true` (Release would disable EnC). `DOTNET_MODIFIABLE_ASSEMBLIES=debug`
is set by the Rust host.

## Gotchas

- Retired ALCs must unload: nothing may keep module types. `AlcsAlive` counts leaks; the
  Rust tests assert zero at shutdown.
- A swap the host refuses at registration rolls the module's staged view back; the server
  still thinks it applied, so its next delta fails the generation check and asks for L3.
- Prebuilt modules (`assembly`, no watch) wait for the modules they reference to stage first.
