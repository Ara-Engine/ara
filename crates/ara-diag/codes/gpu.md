# GPU (named codes, slots 6000–6099)

`ara-gpu` reports problems with named codes. They keep their names on the bus;
the numbers are internal slots.

## ARA6090 GPU_FOREIGN_LOAD: Invalid GPU timing reading
owner: ara-dev
shown as: ARA_GPU_FOREIGN_LOAD

A D26 scoreboard or GPU-leased reading is invalid because foreign GPU activity was observed, an actual game executable was present, or monitoring was unavailable. This is not an engine budget failure. Evidence retains process names, PIDs, peak utilization and UTC epoch-millisecond time ranges.

Causes:
- Another process used more than 5% summed GPU-engine utilization for at least three consecutive samples.
- An actual game executable in the data-driven presence list was active. Idle services, launchers, tray apps and overlays never invalidate by presence alone; they count through measured GPU utilization.
- Windows per-process GPU performance counters could not be sampled reliably.

Fixes:
- Close or pause foreign GPU work yourself, then rerun; Ara never terminates other applications.
- Use `ara-dev gpu --wait-for-quiet -- COMMAND` or `ara-dev d26 --wait-for-quiet` to poll for a quiet three-second baseline before starting.
- Inspect `gpu-load.json` or the leased reading's reported evidence path. Restore unavailable Windows GPU counters before relying on timing results. INVALID exits with code 3.

## ARA6001 GPU_STALE_HANDLE: Stale GPU handle
owner: ara-gpu
shown as: ARA_GPU_STALE_HANDLE

A buffer, image, pipeline or acceleration-structure handle was used after its resource was destroyed, or with a device it does not belong to. Handles are generation-checked, so the call failed instead of touching freed memory.

Causes:
- A system kept a handle across a frame in which the resource was destroyed (a reload, a resize, an asset swap).
- A handle from one `ara-gpu` device was used with another (several viewports or a recreated device).
- A double destroy: the second call sees the bumped generation.

Fixes:
- Re-fetch handles from their owner after asset reloads and swapchain resizes instead of caching them.
- Destroy resources through their owner only, once, and drop the handle with them.

## ARA6002 GPU_BACKEND: GPU backend failure
owner: ara-gpu
shown as: ARA_GPU_BACKEND

The graphics backend (Vulkan 1.3 or Metal 4) failed: a missing capability, an allocation, command recording or a queue submission. The message carries the driver's error.

Causes:
- The GPU or driver lacks a required feature (descriptor indexing, timeline semaphores, ray tracing for the RT tier).
- Video memory ran out.
- The device was lost (driver reset, TDR after a long GPU hang).

Fixes:
- Update the GPU driver; check the device with `vulkaninfo` (Vulkan SDK) for the missing feature.
- Lower the quality tier or texture budget when allocations fail.
- After a device loss, restart the editor or player; recreating the device is the only recovery.

## ARA6003 GPU_CONTRACT: GPU API contract broken
owner: ara-gpu
shown as: ARA_GPU_CONTRACT

A caller broke a precondition of the typed GPU API: a size, format, usage, barrier or descriptor that cannot be right. Nothing was submitted.

Causes:
- A buffer or image created or written with sizes or formats that do not match.
- A barrier or queue-family transfer that does not match the resource's last use.
- A heap index or root constant outside the bound range.

Fixes:
- Read the message: it names the violated precondition and the values passed.
- Run the failing case with validation on (`ara-gpu` tests do) to see the exact Vulkan rule.

## ARA6004 GPU_VALIDATION: Vulkan validation message
owner: ara-gpu
shown as: ARA_GPU_VALIDATION

The Vulkan validation or synchronization-validation layer reported a problem (error or warning). Validated runs must be clean; `assert_clean` fails on any error.

Causes:
- A missing or wrong pipeline barrier (synchronization hazard).
- A descriptor, image layout or resource state that does not match its use.
- An API call outside the spec that some drivers tolerate.

Fixes:
- Read the message ID (`VUID-…`) and look it up in the Vulkan spec.
- Add the barrier the render graph should have emitted; fix the pass's declared reads and writes.

## ARA6005 GPU_ENVIRONMENT: GPU environment message
owner: ara-gpu
shown as: ARA_GPU_ENVIRONMENT

The Vulkan loader, a layer or the driver reported something about the environment rather than about Ara's API use (for example an implicit layer from another program). It is informational, never counted as a validation warning/error, and emitted once per exact message in a process. A stable diagnostic key groups the same message from editor/player/worker processes in the session's Console.

Causes:
- Third-party implicit layers (overlays, capture tools) loaded into the process.
- An outdated loader or layer manifest on the machine.

Fixes:
- Usually nothing; disable the third-party overlay if the message repeats or a crash follows.
- Update the Vulkan SDK and GPU driver.

## ARA6006 GPU_EXTERNAL: Shared GPU surface unavailable
owner: ara-gpu
shown as: ARA_GPU_EXTERNAL

A shared surface (GPU memory one process renders and another samples, such as an embedded Play client shown in the editor's Game view) could not be exported or imported. Opaque memory only moves between devices of the same physical GPU and driver, through Windows NT handles.

Causes:
- The platform or backend has no shared surfaces yet (macOS/Metal, Linux).
- The driver does not offer `VK_KHR_external_memory_win32`, or cannot export or import RGBA8 images.
- The player and the editor picked different GPUs or driver versions (device or driver UUID differ), for example after a driver update while the editor kept running.

Fixes:
- Update the GPU driver, then restart the editor so both processes use the same driver.
- On machines with several GPUs, make the editor and players use the same adapter.
- Play in a window (`play.start` with `window: true`) until shared surfaces are available on this platform.
