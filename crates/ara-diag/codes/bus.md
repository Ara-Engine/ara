# The diagnostics bus, `ara why` and runtime events (ARA2000–ARA2099)

Codes of the bus itself, of the `ara why` explainer, and of runtime events that
subsystems report through the bus so `ara why` can explain them (C# reloads,
frame budgets).

## ARA2001 DIAG_DROPPED: Diagnostics dropped
owner: ara-diag

Some diagnostics never reached the store: the thread that emitted them filled its bus ring (512 events) before the host drained it. The count in the message is exact; the dropped diagnostics themselves are gone.

Causes:
- A subsystem emitted hundreds of diagnostics in one host tick without sampling, usually inside a per-entity or per-frame loop.
- The host was blocked (a long call, a debugger pause) and did not drain for a while.
- A process emits diagnostics but has no consumer forwarding them to the host.

Fixes:
- Find the spamming code with `ara logs` (it has the highest count) and emit it with `ara_diag::emit!`, which checks admission before formatting.
- Lower the code's burst with `ara_diag::bus::set_burst(code, n)` so repeats are only counted.
- Give the diagnostic a stable dedup key (`Meta::dedup`) when its message varies per occurrence.

## ARA2002 DIAG_INVALID_REPORT: Unreadable reported diagnostic
owner: ara-host

Another process sent a diagnostic to `diag.report` that is not a valid diagnostic (unknown code, wrong field types). It was skipped; the others in the batch were recorded.

Causes:
- The reporting process was built against a different engine version.
- A named code that the host's catalog does not know.

Fixes:
- Rebuild the reporting process (player, import worker) from the same engine version as the host.
- Report numeric `ARA####` codes, or add the named code to `crates/ara-diag/codes/`.

## ARA2010 WHY_UNANSWERED: Question not understood
owner: ara-why

`ara why` found no explainer for the question. Explainers match a subject (an entity, an asset, a code) plus a few topic words.

Causes:
- The question names no entity, asset or code and no known topic (visibility, reload, frame time).
- The subject is spelled in a form the explainers do not parse.

Fixes:
- Name the subject: `ara why name:Crate not visible`, `ara why @meshes/crate.glb`, `ara why ARA1302`.
- Ask about a known topic: `ara why "did reload fall back to L3"`, `ara why "is the frame slow"`.
- List what exists with `ara why --list`.

## ARA2011 UNKNOWN_CODE: Unknown diagnostic code
owner: ara-diag

No diagnostic code has this number or name. Codes are `ARA` plus four digits (`ARA1302`), or a named code (`ARA-EXP-AND-007`, `ARA_GPU_STALE_HANDLE`).

Causes:
- A typo in the code.
- A code from a newer engine version than this one.

Fixes:
- Copy the code exactly from the diagnostic (`ara logs` lists them).
- Search the codes by words with `ara_docs` / `docs.search`.

## ARA2020 RELOAD_APPLIED: C# reload applied
owner: ara-script

A C# module's new code is running. The message gives the level (L1: method bodies patched in place; L2: additive edit; L3: a new module generation swapped in at a tick boundary) and how long it took from file event to the swap.

Causes:
- A C# file of the module was saved (human, file watcher) or an agent asked for a compile.

Fixes:
- Nothing to fix. If reloads are slow, `ara why "did reload fall back to L3"` explains escalations, and `ara why "is the frame slow"` shows where the time went.

## ARA2021 RELOAD_ESCALATED: Reload fell back to L3
owner: ara-script

An edit that could have been patched in place (L1/L2, tens of milliseconds) was applied as a full module swap (L3, a few hundred milliseconds) instead. The message names the reason the compile server or the runtime gave.

Causes:
- A changed method contains a lambda, local function or LINQ query (EnC deltas for closures crashed the runtime in S2, so they always take L3).
- A declaration changed: a signature, a struct field, an attribute or a virtual member.
- A constructor, field initializer, iterator or `async` method changed.
- The module was cold (built optimized); its first edit makes it hot.
- A debugger is attached, so the runtime refuses EnC deltas.
- A module this one references changed its public surface.

Fixes:
- Keep hot-path edits to plain method bodies; move lambdas into named methods when you iterate on them.
- Detach the debugger while iterating.
- Expect one L3 per module after a cold start; later body edits take L1.

## ARA2030 FRAME_OVER_BUDGET: Frame over budget
owner: ara-profile

A frame took longer than its budget (16.7 ms at 60 Hz unless configured). The message gives the frame time and the largest contributors the profiler measured.

Causes:
- A system or render pass is slower than usual (see the contributors).
- A hitch: a shader or pipeline compiled on the frame, a reload swapped at the tick boundary, a garbage collection, or IO on the main thread.
- The machine is busy with other work (compiles, other agents).

Fixes:
- Run `ara why "is the frame slow"` for the top self-time scopes and hitches.
- Start a capture with `ara_profile {action: "start"}` and compare it with a saved baseline (`ara profile compare`).
- Move IO and compiles off the frame (D20: background work runs below the editor's priority).
