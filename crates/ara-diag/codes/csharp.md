# C# analyzers, generators and the C# host (ARA0001–ARA0999)

## ARA0001 cs::UNITY_ISM: Unity API or convention
owner: Ara.Analyzers

A Unity API or lifecycle convention has no equivalent meaning in Ara. This is a compilation error; the edit is not installed and the last good code keeps running.

Causes:
- Using `UnityEngine`, deriving from `MonoBehaviour`, or declaring Unity callbacks such as `Update` or `Start`.
- Calling `GetComponent<T>()`, `Debug.Log`, or `Time.deltaTime`, or using lowercase fields on `System.Numerics` vectors.

Fixes:
- Apply the offered code fix: remove `using UnityEngine`, use the `Ara` namespace, and derive a `partial` class from `Behaviour`.
- Override `OnTick(in Tick tick)` for `Update`/`FixedUpdate`, `OnFrame(in Frame frame)` for `LateUpdate`, and `OnSpawn()` for `Start`/`Awake`; combine duplicate callbacks when needed.
- Use `ref var value = ref Get<T>()` for ECS writes, `tick.Dt` for the fixed step, `Log.Info`/`Warn`/`Error` for output, and uppercase vector fields such as `.X`.

## ARA0100 cs::MUTABLE_STATIC: Mutable static state
owner: Ara.Analyzers

A gameplay module stores mutable state in a static field or auto-property. Statics are outside ECS save, replay and replication, and their lifetime does not follow gameplay state. The edit is rejected; the last good code keeps running.

Causes:
- A non-constant static field is not `readonly`.
- A static auto-property has a setter.

Fixes:
- Move authoritative shared state into a `[Resource]` struct, or per-entity state into a `[Component]` struct.
- Use `const` or `static readonly` only for genuinely immutable data; a readonly reference to mutable gameplay data does not make that state safe.

## ARA0101 cs::BEHAVIOUR_FIELD: Behaviour field without transient marker
owner: Ara.Analyzers

A `Behaviour` instance field is not marked `[Transient]`. Behaviour objects are recreated on reload and cannot own authoritative state. The edit is rejected; the last good code keeps running.

Causes:
- Gameplay values are stored in ordinary behaviour fields.
- A disposable presentation cache lacks `[Transient]`.

Fixes:
- Convert persistent values to `[Property]` partial instance properties backed by the behaviour's ECS column, or move them into a component.
- For a cache that can be lost and rebuilt on reload, apply the `Mark as [Transient] cache` code fix.

## ARA0200 cs::NON_DETERMINISM: Non-deterministic simulation code
owner: Ara.Analyzers

Code in a `[Simulation]` type or assembly depends on ambient state or scheduling. Identical inputs must produce identical fixed-tick results for prediction and replay. The edit is rejected; the last good code keeps running.

Causes:
- Reading wall-clock time, calling `Guid.NewGuid`, using `System.Random`, or accessing I/O.
- Using `async`, `await`, tasks, threads or timers in simulation code.
- A lambda in a simulation command captures state.

Fixes:
- Use the supplied `Tick` and its fixed `Dt`, and the context RNG (`ref Rng` in a query), with authoritative state stored in ECS components or resources.
- Move I/O and asynchronous work outside simulation and pass their results through explicit inputs or commands.
- Make command lambdas non-capturing and pass required values explicitly.

## ARA0300 cs::TICK_ALLOCATION: Allocation on the tick path
owner: Ara.Analyzers

A `[Query]` system, behaviour `OnTick`/`OnFrame`, or command `Execute` allocates on its tick path, including inside nested lambdas and local functions. This is a warning: compilation can succeed, but repeated allocations can cause GC pauses.

Causes:
- Creating classes or arrays, calling LINQ, boxing values, or declaring non-static lambdas.
- Building strings with interpolation or concatenation on the tick path.

Fixes:
- Use value types, spans, bounded `stackalloc`, or reusable pooled buffers instead of per-tick heap objects.
- Replace allocating LINQ with loops, avoid boxing, and use static lambdas where possible.
- Move string formatting and other avoidable allocations out of recurring tick work.

## ARA0400 cs::AOT_UNSAFE: NativeAOT unsafe API
owner: Ara.Analyzers

The module uses an API that needs runtime code generation, dynamic loading or reflection unavailable or unsafe under NativeAOT and trimming. This is a compilation error even when developing with the JIT; the last good code keeps running.

Causes:
- Calling `Assembly.Load`, using `AssemblyLoadContext` or `Reflection.Emit`, or compiling expression trees.
- Looking up types or members by name, invoking methods reflectively, constructing runtime generic types, or using `Activator.CreateInstance(Type)`.
- Using `dynamic` invocation, member access or construction.

Fixes:
- Replace runtime discovery and invocation with generated registration, serialization and direct typed calls.
- Use statically referenced types and explicit factories instead of loading or constructing code dynamically.

## ARA0500 cs::API_NOT_ALLOWED: API outside module allowlist
owner: Ara.Analyzers

A framework API is outside the module profile's allowlist. The diagnostic identifies the API and profile; compilation is rejected and the last good code keeps running. Agent `ara_run` scripts have a separate check reported as `ARA0961`.

Causes:
- Gameplay code calls a framework type or member outside the permitted core runtime and Ara APIs.
- Editor tooling is compiled under the default `gameplay` profile.

Fixes:
- Replace the reported API with an Ara API or an allowed collection, math or text API.
- Move genuinely editor-only functionality into a separate module with `profile = "editor"` in `ara.module.toml`; keep gameplay in its gameplay module.

## ARA0501 cs::API_FILE_SYSTEM: File system access
owner: Ara.Analyzers

The module profile forbids the reported file-system API. Gameplay data must enter through assets and gameplay state must live in ECS. The edit is rejected; the last good code keeps running.

Causes:
- Gameplay code uses file, directory or other disallowed `System.IO` APIs.
- Import or editor file handling was placed in a gameplay module.

Fixes:
- Load game data through assets and store state in components or resources.
- Move editor file handling into an editor module; for purely in-memory processing, use permitted types such as `MemoryStream` where appropriate.

## ARA0502 cs::API_NETWORK: Network access
owner: Ara.Analyzers

The module profile forbids direct network access. Gameplay networking belongs to the engine's replication and command paths. The edit is rejected; the last good code keeps running.

Causes:
- Gameplay code uses `System.Net`, sockets or HTTP APIs.

Fixes:
- Express gameplay communication through engine replication and typed commands rather than opening connections from gameplay code.
- Move editor-only network tooling into an editor module if it does not belong to the game simulation.

## ARA0503 cs::API_REFLECTION: Reflection or dynamic code
owner: Ara.Analyzers

The profile forbids a reflection or dynamic-code API that is not already reported by `ARA0400`. Such access can bypass generated registration and break reload or NativeAOT. The edit is rejected; the last good code keeps running.

Causes:
- Reflective member discovery or delegate invocation, such as `Type.GetFields` or `Delegate.DynamicInvoke`.
- Access to disallowed reflection, loader, expression-tree or dynamic-runtime APIs.

Fixes:
- Use generated registration and serializers, static references and direct typed calls.
- Replace reflective dispatch with an explicit typed interface or a generated dispatch table.

## ARA0504 cs::API_THREADING: Threads, locks or tasks
owner: Ara.Analyzers

Gameplay code attempts to control concurrency outside the engine schedule. The edit is rejected; the last good code keeps running. Systems already execute over chunks on the engine's worker threads.

Causes:
- Using `System.Threading` or task APIs from a gameplay module.
- Using `await` or `lock` in gameplay code.

Fixes:
- Express parallel gameplay work as systems with declared component and resource accesses.
- Keep asynchronous editor work in an editor module and pass resulting gameplay changes through typed commands.

## ARA0505 cs::API_INTEROP: Native interop
owner: Ara.Analyzers

A module tries to invoke native code directly or uses a disallowed interop API. Native interop belongs in the engine, including for editor modules. The edit is rejected; the last good code keeps running.

Causes:
- Declaring `extern` methods or applying `DllImport` or `LibraryImport`.
- Calling `Marshal`, native-library loading or other interop APIs outside the permitted layout and memory helpers.

Fixes:
- Call the corresponding Ara API instead of invoking native functions from the module.
- Implement missing native integration in the engine and expose it through the generated, checked ABI.

## ARA0506 cs::API_PROCESS: Process or environment access
owner: Ara.Analyzers

The module profile forbids accessing or changing process-wide state. The process belongs to the editor or player. The edit is rejected; the last good code keeps running.

Causes:
- Using console or debug output, environment APIs, `GC`, process APIs or other disallowed process state.
- Attempting `Environment.Exit` or `Environment.FailFast`, which remain forbidden in editor modules.

Fixes:
- Apply the output code fix to use `Log.Info` or `Log.Error`; use the supplied `Tick` for simulation time.
- Apply the offered fixes to replace `Environment.NewLine` with `"\n"` or remove an explicit `GC.Collect()` statement.
- Remove process termination and configuration changes from module code; let the host manage its own lifetime.

## ARA0701 cs::NOT_PARTIAL: Type must be partial
owner: Ara.Generators

A type extended by the generator, or one of its enclosing types, is not `partial`. Registration code cannot be generated correctly; compilation fails and the last good module keeps running.

Causes:
- A component, resource, cue, behaviour, command or system container lacks `partial`.
- A nested generated type has an enclosing declaration without `partial`.

Fixes:
- Add `partial` to every declaration of the named type and to each enclosing type extended by the generator.

## ARA0702 cs::BAD_SYSTEM_PARAM: Unsupported system parameter
owner: Ara.Generators

The generator cannot bind a query parameter or filter to an ECS column, resource or supported execution context. Compilation fails; the last good module keeps running.

Causes:
- A parameter is not a component, resource, `Entity`, `in Tick`, `ref Rng` or `ref Commands` in a supported form.
- A fieldless tag component is requested as a column parameter.
- `[With<T>]` or `[Without<T>]` names a type that is not a component.

Fixes:
- Use supported query parameters, with `ref` for component writes and `in` for reads, and the correct context parameter forms.
- Replace a tag parameter with `[With<Tag>]`; use `[Without<Tag>]` to exclude it.
- Make filter types registered components and correct the type named in the diagnostic.

## ARA0703 cs::MISSING_DOC: Reflected declaration needs documentation
owner: Ara.Generators

A reflected declaration has no usable documentation. Its text becomes Schema DB documentation, inspector tooltips and agent tool descriptions. Compilation fails; the last good module keeps running.

Causes:
- A reflected type, field, property or enum variant lacks a nonempty XML summary.
- A command parameter lacks its XML parameter documentation, or the command has no summary on the class or `Execute` method.

Fixes:
- Add a concise `/// <summary>` explaining the named declaration's meaning, units and usage where relevant.
- Document command arguments with `/// <param name="argument">` on `Execute` and add a command summary.

## ARA0704 cs::NOT_BLITTABLE: Unsupported schema representation
owner: Ara.Generators

The generator cannot represent a declaration as a valid, uniquely named plain-data schema type. This code also covers invalid schema names and conflicting definitions. Compilation fails; the last good module keeps running.

Causes:
- A component contains reference types or a field has no supported blittable schema representation.
- An enum does not use an `int` or `uint` base with values `0, 1, 2, ...` in declaration order.
- A schema name is not PascalCase letters and digits, or two different definitions use the same schema name.
- Nested value structs exceed the generator's supported nesting depth.

Fixes:
- Use supported numeric types, `bool`, vectors, `Quaternion`, `Color`, `Entity`, enums or structs composed of supported fields; remove managed references from ECS data.
- Give enums contiguous zero-based values and an `int` or `uint` underlying type.
- Give each schema type a unique PascalCase name without namespace dots, and flatten excessively nested structures.

## ARA0705 cs::BAD_COMMAND: Invalid command shape
owner: Ara.Generators

A `[Command]` class has an `Execute` signature the generator cannot bind to a typed command. Compilation fails; the last good module keeps running and the new command is not registered.

Causes:
- `Execute` is missing, is not static, does not return `CommandResult`, or does not begin with `ref CommandContext`.
- A component parameter has no target `Entity` parameter.
- A non-component argument after the context is passed by `ref` or `in`.

Fixes:
- Use `public static CommandResult Execute(ref CommandContext ctx, Entity target, ...)` and place the target before its component parameters.
- Pass component arguments by `ref` or `in` as needed and ordinary serialized arguments by value.

## ARA0706 cs::BAD_BEHAVIOUR: Invalid behaviour declaration
owner: Ara.Generators

The generator cannot construct the behaviour or implement its ECS-backed properties. Compilation fails; the last good module keeps running.

Causes:
- The behaviour has no public or internal parameterless constructor.
- A `[Property]` member is static or is not a partial property definition without a body.

Fixes:
- Provide a public parameterless constructor so the runtime can create one behaviour object per entity.
- Declare ECS-backed values as instance properties such as `[Property] public partial float Speed { get; set; }` and let the generator implement their storage.

## ARA0707 cs::BAD_SYSTEM: Invalid system method
owner: Ara.Generators

A `[Query]` method cannot be emitted as a chunk-dispatched system. Compilation fails; the last good module keeps running.

Causes:
- The query method is an instance method, returns a value or has generic parameters.
- The query has no component column parameter; resources, context parameters and tag filters alone do not supply one.

Fixes:
- Declare a non-generic `static void` query method.
- Include at least one supported component with fields as a query parameter, using `ref` for writes or `in` for reads.

## ARA0900 script::COMPILE_ERROR: C# compilation failed
owner: ara-script, Ara.CompileServer

A C# compiler error prevented producing the replacement module. The message retains the Roslyn `CS` identifier and source location. The last good code keeps running; a module without a successful prior compilation is unavailable.

Causes:
- Syntax, type or name-resolution errors in edited C# source.
- Missing or incompatible referenced types after a dependency change.

Fixes:
- Fix the `CS` error at the reported file, line and column, then compile or reload again.
- Correct module references and update callers to match changed dependency APIs.

## ARA0901 script::COMPILE_WARNING: C# compiler warning
owner: ara-script, Ara.CompileServer

A C# compiler warning is mapped to the host diagnostic format with its Roslyn `CS` identifier in the message. A warning alone does not reject a module. The compile server's normal report filter currently omits ordinary non-Ara warnings.

Causes:
- Roslyn reports a non-error issue and that diagnostic is passed through the compiler diagnostic mapper.

Fixes:
- Read the retained `CS` identifier and correct the reported source issue at its location.
- Check accompanying errors before assuming the module was installed; this warning alone does not establish reload success.

## ARA0902 script::MODULE_LARGE: Module exceeds soft size cap
owner: ara-script, Ara.CompileServer

The module has more than 5,000 source lines. This informational diagnostic recommends keeping modules small for fast L3 generation swaps; it does not block compilation or reload.

Causes:
- Gameplay and tooling responsibilities have accumulated in a single module.

Fixes:
- Split the module along dependency boundaries and declare the resulting dependencies in `ara.module.toml`.
- Keep frequently edited gameplay in small modules to reduce full-generation compile and reload work.

## ARA0903 script::MODULE_TOO_LARGE: Module exceeds reload size target
owner: ara-script, Ara.CompileServer

The module has more than 8,000 source lines, beyond the size target for L3 reload latency. Despite the constant's name, this is a warning: it does not reject compilation or reload.

Causes:
- A module has grown beyond the hard size threshold used for reload-performance warnings.

Fixes:
- Split the module into smaller, acyclic modules with explicit `ara.module.toml` references.
- Separate stable shared declarations from frequently edited implementations where the dependency structure permits it.

## ARA0904 script::COMPILE_SERVER: Compile server unavailable
owner: ara-script, Ara.Host

The compile-server connection failed, the process exited, or a requested module image did not arrive. Existing code is not replaced. Hot reload cannot progress while the connection is unavailable; an exited server requires the host to restart.

Causes:
- The compile server crashed, could not start, or lost its pipe connection.
- No initial module image arrived within the loader's three-minute timeout.

Fixes:
- Correct the reported process or connection failure, then restart the script host and its compile server.
- Verify the configured compile-server executable or DLL exists and uses the expected .NET installation; inspect server output for a stalled compilation.

## ARA0905 script::DELTA_REJECTED: Runtime rejected reload delta
owner: ara-script, Ara.Runtime

The runtime threw while applying an Edit and Continue delta for L1 body edits or L2 additive edits. The host escalates the edit to a full L3 module rebuild instead of treating the delta as successfully installed.

Causes:
- The emitted delta is unsupported by the running runtime or incompatible with its current module baseline.

Fixes:
- Let the automatic L3 rebuild complete and address any subsequent compile or load diagnostics.
- If this repeats, check that the runtime and compiler use matching artifacts and restart the host to establish a fresh baseline.

## ARA0910 script::SYSTEM_FAULT: System threw an exception
owner: ara-script, Ara.Runtime

A C# system threw during chunk execution. The exception is contained and that system is disabled until the next successful reload. Other systems remain available; this is not a transactional rollback of component writes already made by the failing system.

Causes:
- An unhandled exception in the system, such as invalid indexing, missing data or an invalid operation.

Fixes:
- Fix the exception identified by the diagnostic and stack trace, then reload the module to re-enable the system.
- Validate inputs before writing ECS data and repair any partial state left by the failed tick when needed.

## ARA0911 script::BEHAVIOUR_FAULT: Behaviour callback threw
owner: ara-script, Ara.Runtime

Behaviour construction or a lifecycle callback threw. The runtime disables the entire behaviour type until the next successful reload, not just the affected entity. Other types continue; writes already made by the callback are not transactionally rolled back.

Causes:
- An exception in construction, `OnSpawn`, `OnReload`, `OnTick` or `OnFrame`.
- A presentation cache was assumed to survive recreation during reload.

Fixes:
- Fix the reported exception and reload the module to re-enable the behaviour type.
- Rebuild transient caches after recreation and keep persistent values in ECS-backed properties or components.

## ARA0912 script::COMMAND_FAULT: Command threw an exception
owner: ara-script, Ara.Runtime

A C# command threw, or a caller invoked a command already disabled by an earlier fault. The host restores journaled state and discards buffered operations for the failed call. The command remains disabled until the next successful reload.

Causes:
- An unhandled exception in `Execute` or generated command invocation.
- Retrying a command before reloading after its first fault.

Fixes:
- Fix the original exception and reload the module before retrying the command.
- Use `ctx.Reject` for expected invalid gameplay requests instead of throwing.

## ARA0920 script::LOAD_FAILED: Module load or registration failed
owner: ara-script, Ara.Runtime, Ara.Host

A module image could not be loaded or prepared, or its registration failed. The replacement is rejected and the last good code keeps running. A module with no prior good generation remains unavailable.

Causes:
- Missing or incompatible assembly artifacts or a failure in module preparation.
- A shipping manifest/image is missing, its image path escapes the export, or its linked module inventory differs from the image.
- The generated registration entry point throws, or a module descriptor cannot be registered.

Fixes:
- Fix the load or registration error named in the message and regenerate or rebuild the affected module artifacts.
- Ensure the module and its dependencies match the running engine and inspect accompanying schema or layout diagnostics.
- Restore the export's complete `game/` directory or export again; shipping images do not load source folders or the development compiler.

## ARA0921 script::STALE_BINDINGS: Generated bindings are stale
owner: ara-script, Ara.Runtime

The C# runtime's ABI version, layout hash or generated structure sizes do not match the Rust host. Script-runtime initialization is rejected before incompatible bindings can be used.

Causes:
- Generated C# bindings were not updated after an ABI or schema change.
- The host and C# runtime were built from different engine revisions or stale artifacts.
- Exported `scripts.json` has a different ABI version or engine layout hash than the player.

Fixes:
- Regenerate with `ARA_UPDATE_GENERATED=1 cargo test -p ara-script --test generated`, then rebuild the .NET artifacts and matching Rust host.
- Restart the host with artifacts from the same revision; a default-context runtime assembly cannot be replaced by a module reload.
- For a shipping export, rebuild gameplay scripts and the player with the same Ara SDK, then export again.

## ARA0922 script::RUNTIME_START: Script runtime could not start
owner: ara-script, Ara.Runtime

The host could not load or initialize the .NET or NativeAOT script runtime, or a runtime entry point reported a startup failure. C# execution is unavailable for the failed initialization.

Causes:
- Missing runtime libraries, host configuration, `Ara.Host` artifacts or the NativeAOT entry library.
- Runtime initialization throws or the script API handshake fails.
- The process already hosts .NET with a different runtime directory or Edit and Continue setting.

Fixes:
- Correct the missing path, library or initialization error in the message and provide the matching .NET 10 runtime or NativeAOT artifacts.
- Restart the process when changing runtime configuration; only one .NET runtime configuration can be active per process.

## ARA0923 script::MODULE_CYCLE: Module dependency cycle
owner: ara-script

Module references form a cycle, so a valid dependency-first load order cannot be produced. Modules in the cycle are not loaded; dependents can also be blocked with `ARA0925`.

Causes:
- Two modules reference each other in `ara.module.toml`.
- A longer chain of module references leads back to its starting module.

Fixes:
- Remove the circular reference and move shared types into a lower-level module that both sides can reference.
- Follow the cycle named in the diagnostic and keep the resulting dependency graph acyclic.

## ARA0924 script::UNKNOWN_MODULE: Referenced module is unavailable
owner: ara-script, Ara.CompileServer

A module names a dependency that is not present in the module set available to the host or compile server. The dependent cannot load or compile against that reference; an existing good generation is not replaced by the failed attempt.

Causes:
- A misspelled or removed module name in `references` in `ara.module.toml`.
- The dependency exists on disk but was not included in the modules supplied to the host or compile server.

Fixes:
- Correct the reference to the dependency's declared module name.
- Include and load the dependency before its dependent, or remove a reference that is no longer needed.

## ARA0925 script::DEPENDENCY_FAILED: Dependency cascade failed
owner: ara-script, Ara.CompileServer

A dependency prevents loading a module, or a reload cascade cannot rebuild every affected dependent against the proposed change. A failed reload cascade applies none of its replacements: every affected module keeps its last good version.

Causes:
- A dependency is missing, cyclic or otherwise unable to load.
- A changed dependency API breaks a dependent's compilation or analyzer checks.

Fixes:
- Fix the originating dependency or dependent diagnostics, then retry the reload.
- Update affected callers together with the dependency API so the whole cascade can compile successfully.

## ARA0926 script::RESTART_REQUIRED: Play process restart required
owner: ara-script, Ara.Host

The change requires L4: restarting the play process from a snapshot. The compile server detects changed engine bindings loaded in the host's non-unloadable default context. The last good code keeps running until restart; L1, L2 or an L3 module swap cannot replace that assembly.

Causes:
- A watched engine reference such as `Ara.Runtime` changed on disk while the process was running.

Fixes:
- Restart the play process from a snapshot using matching engine and script artifacts.
- Keep routine gameplay edits in reloadable modules rather than changing the runtime assembly.

## ARA0927 script::GENERATION_LEAKED: Retired generation remains alive
owner: ara-script, Ara.Host

A retired module's collectible assembly load context is still alive after two natural full garbage collections. The active generation can keep running, but the retired generation's memory remains retained. The loader reports an L4 restart request to reclaim it.

Causes:
- A static event subscription or long-lived cache outside the retired generation still references its objects, delegates or types.

Fixes:
- Remove long-lived references to module generations and release subscriptions or caches when retiring them.
- Restart the play process from a snapshot to reclaim leaked generations; do not add gameplay `GC.Collect()` calls as a workaround.

## ARA0906 script::COMPILE_RESULT: C# compilation lifecycle result
owner: ara-script

Informational lifecycle metadata for a module-owned compiler attempt. The private
compile context identifies its producer lifetime, binding, attempt, profile,
coverage and trusted Play source. Compilation and runtime application are separate.
Only a completed success validated against current module inputs supersedes older
compiler errors in the same active scope; Console retains the historical records.

Causes:
- A producer was bound, an input snapshot began evaluation, or an attempt completed.
- Errors, cancellation, truncated output and partial warning coverage are explicit.

Fixes:
- Inspect the matching module's current compiler diagnostics when the result is errors.
- Wait for a newer validated success after repairing source; cancellation does not resolve errors.
- Check runtime/load diagnostics separately: compilation success does not prove application.

## ARA0930 script::NOT_BLITTABLE: Component has no blittable layout
owner: ara-script

The Schema DB cannot provide the plain-data layout required for C# to view a component or related data in place. Binding generation, engine binding setup or module registration fails. Rejected module registration leaves existing generations unchanged.

Causes:
- A requested schema type contains a field that cannot be laid out as fixed-size plain data.
- A requested component schema is missing or cannot be resolved into a canonical struct layout.

Fixes:
- Replace unsupported fields with supported plain-data values or handles and register every required schema type.
- Regenerate the bindings after correcting the schema and retry the failed setup or registration.

## ARA0931 script::LAYOUT_MISMATCH: Component layout mismatch
owner: ara-script

A C# declaration or Rust engine binding disagrees with the Schema DB's canonical size, alignment or field offsets. The binding or module registration is rejected rather than exposing incompatible memory. A failed module replacement leaves the last good code running.

Causes:
- C# fields differ from schema order or use incompatible types or packing.
- A Rust component lacks `#[repr(C)]`, uses the wrong field offsets, or no longer matches its schema.
- Generated bindings are stale after a schema change.

Fixes:
- Match fields and their order to the schema, use supported blittable types, and regenerate stale bindings.
- Declare Rust engine components with `#[repr(C)]` and correct the registered size, alignment and field offsets shown in the message.

## ARA0932 script::UNKNOWN_COMPONENT: Component or resource is unknown
owner: ara-script

A binding, query, behaviour, command or host operation names a component or resource the host does not know. The requested operation fails. If found while validating a module batch, registration is rejected and the last good generations remain active.

Causes:
- A generated descriptor names an unregistered component, resource, query filter or behaviour data component.
- A module dependency or engine binding that supplies the type is missing.
- A caller uses a stale or misspelled schema name.

Fixes:
- Correct the schema name and register the supplying engine binding or module before using it.
- Declare the required module reference and regenerate descriptors after renaming types.

## ARA0933 script::BAD_DESCRIPTOR: Invalid module descriptor
owner: ara-script

Host descriptor validation found malformed data, duplicate declarations or invalid access sets. Module registration is all-or-nothing, so rejected replacements leave the world and active module generations unchanged. This code also reports invalid structural command-buffer operations; those operations are skipped while other operations in the buffer may apply.

Causes:
- A descriptor is malformed or declares a component or resource already owned by another module.
- A system takes the same component or resource more than once.
- A command buffer names a missing entity, unknown component, wrong data size or unsupported operation.

Fixes:
- Fix the named declaration or access set in source and regenerate the module descriptor; give shared types a single owning module.
- Remove duplicate query parameters and use one binding for each component or resource.
- For a command-buffer error, correct the target entity, component index or payload layout and inspect which other operations already applied.

## ARA0934 script::SPARSE_COMPONENT: Component needs table storage
owner: ara-script

An engine component bound for C# uses sparse-set storage. C# systems require contiguous table columns, so binding resolution fails before the component can be viewed by C#.

Causes:
- A Rust component exposed through engine bindings was registered with `StorageType::SparseSet`.

Fixes:
- Use table storage for the component exposed to C# and recreate the host/world with the corrected registration.
- If sparse storage is essential, keep that component out of direct C# bindings and expose suitable table-backed data instead.

## ARA0950 script::UNKNOWN_COMMAND: C# command is not registered
owner: ara-script

No loaded C# module provides the requested command, or its dispatch entry is no longer available. The call fails without keeping command writes.

Causes:
- The command name is misspelled or changed.
- The defining module did not load, or its command registration is missing.

Fixes:
- Use the registered command name; the diagnostic suggests a close match when one exists.
- Fix compilation or registration errors in the defining module and load it before invoking the command.

## ARA0951 script::INVALID_PARAMS: Invalid command parameters
owner: ara-script

Command parameters could not be encoded as the registered params schema. The diagnostic retains the failing field path and validation detail. Validation happens before invocation, so the command has made no changes.

Causes:
- Missing required fields, unknown fields, incorrect types, invalid enum values or values outside the schema's range.
- A caller uses a parameter shape from an older command version.

Fixes:
- Correct the field identified by the diagnostic to match the current command schema, including required values and ranges.
- Refresh the caller's parameter shape after a command schema change.

## ARA0952 script::COMMAND_REJECTED: Command rejected the request
owner: ara-script

The command returned a rejection through `ctx.Reject`. The host restores journaled component and resource writes and discards buffered operations for that call. This is an expected refusal, not a fault: the command remains enabled.

Causes:
- The request violates a gameplay precondition reported in the rejection reason.
- State changed between preparing the request and executing it.

Fixes:
- Read the rejection reason, inspect current ECS state and satisfy the command's preconditions before retrying.
- Change the request parameters when the requested action is invalid for the target.

## ARA0953 script::UNDO_FAILED: Undo record no longer applies
owner: ara-script

The host cannot apply an undo record to the current world. Undo validates all steps before applying any, so a validation failure leaves the world unchanged. This code can also identify a host operation targeting an entity that no longer exists.

Causes:
- A recorded entity is gone or no longer has the component state required by the undo.
- A component or resource schema/layout changed since the record was created.

Fixes:
- Inspect the named entity and schema mismatch; restore a compatible snapshot when the old undo must be replayed.
- Otherwise issue a new typed command against current entities and schemas to make the desired correction.

## ARA0960 script::RUN_LIMIT: Agent script exceeded limits
owner: ara-script

An `ara_run` script exceeded its wall-clock or memory limit, or reported an out-of-memory failure. The supervisor kills the worker and the pool replaces it. Killing the worker does not itself undo commands already sent to the command sink; inspect their transaction outcome before retrying.

Causes:
- An unbounded loop, too much work, or a wait exceeds the run's time budget.
- Large allocations or retained results exceed the worker memory budget.

Fixes:
- Bound loops, reduce allocations and split large jobs into smaller runs with explicit progress.
- Check which commands already succeeded before resuming to avoid repeating mutations.
- Raise the configured limit only when the workload is intentional and still bounded.

## ARA0961 script::RUN_ACCESS: Agent API outside allowlist
owner: ara-script, Ara.RunWorker

The compiled `ara_run` script references a type or member outside the agent API allowlist, or contains native interop metadata. The worker rejects it before execution, so the rejected script issues no engine commands.

Causes:
- Direct file, network, process, reflection, native interop or dynamic-loading access.
- A referenced framework member is denied even though other members of its type or namespace are allowed.

Fixes:
- Use the supplied `ara` facade for engine operations and typed commands for mutations.
- Rewrite other work using allowed collections, math, text, JSON and task APIs; remove every forbidden reference reported by the IL check.

## ARA0962 script::RUN_FAILED: Agent script failed
owner: ara-script, Ara.RunWorker

An `ara_run` script failed compilation or threw during execution or result serialization. Compile failures execute no script code; runtime failures may occur after commands have already reached the sink. The diagnostic carries the compiler ID or exception and a script location when available.

Causes:
- Invalid C# syntax, unresolved names or type errors in the submitted script body.
- An unhandled exception, failed awaited engine command or unserializable result.

Fixes:
- Correct the compiler error or exception at the reported script line and return a JSON-serializable value.
- Inspect the underlying command error and any already-applied changes before retrying a partially executed script.

## ARA0963 script::RUN_WORKER: Agent worker failed
owner: ara-script, Ara.RunWorker

The `ara_run` worker could not start, become ready, stay connected or exchange valid protocol messages with its supervisor. The run fails; completed sink calls are not automatically undone by a worker transport failure.

Causes:
- The worker exits unexpectedly, never reports readiness or sends malformed protocol JSON.
- Worker artifacts or runtime dependencies are unavailable, or the worker has no supervisor connection.

Fixes:
- Restore matching worker artifacts and the required .NET runtime, then restart or replace the worker through its supervisor.
- Fix the protocol or connection error named in the message and inspect prior command outcomes before retrying.
