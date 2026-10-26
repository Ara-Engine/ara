# Frame-window CPU metrics

`--perf-wait-ready` waits for `Presenter::readiness()` after presentation, requiring
native raster and GI enabled. Complete content/residency, GI delivery/convergence
and fades must settle before the configured warmup starts. The readiness frame
itself is excluded: ready at 430 plus 60 warmup frames samples from 491.
`--perf-ready-timeout-ms` defaults to 120000; expiry writes an atomic failed
`player.json` and exits unsuccessfully with the last readiness status. No renderer
wait or filesystem probe is added to the getter. Packed asset sources are immutable;
this is not a readiness contract for an external live import queue.

`run.cpu` has the same metrics described below for **all game frames in the
measurement request**, including readiness, arrival and warmup. D26 gates those
whole-run CPU/hitch values. `cpu` keeps the GPU window's CPU samples for analysis.
`arrival` reports excluded-frame peaks, per-pass quantiles and GPU coverage without
a budget gate. The ready snapshot is latched for the immutable pack; normal
in-flight rendering continues warmup. Later presentation errors fail the window.
Loading-view latency stays in `startup`. New keep-alive requests reset
the measurement clock; the first request uses the process launch clock.

`--perf-output` adds these fields under `player.json`'s `cpu` object:

- `work_ms`: quantiles of each frame's wall time minus **measured host waits**.
  Includes simulation, extraction, GPU recording/submission, audio and host
  scheduling. This is elapsed work time, not thread CPU cycles. It never subtracts
  GPU timestamp durations, which can overlap CPU work.
- `gpu_wait_ms`: host time in Vulkan timeline waits, upload queue-idle waits,
  swapchain acquisition/presentation and the presentation queue-idle call.
  Each leaf driver call is counted once on the rendering device.
- `work_coverage`: counts `measured` and `unavailable` frames. Work/wait quantiles
  are null without valid coverage (including the separate 2D presenter). Device
  counter resets or wait totals greater than frame wall time remain unavailable.
- `frame_samples_ms`: raw wall times for the measured window, in frame order.
- `hitches_over_16ms`: count of wall times strictly greater than 16 ms. All
  loading/warmup frames are outside the window; only the first game frame is
  excluded if it was measured. A new keep-alive window does not exclude its own
  first frame. `hitch_samples` gives the eligible measured frame count.

The cumulative counter is exposed by `Presenter::host_wait_stats()` and
`VulkanDevice::context().host_wait_stats()`. Capture it before and after the whole
frame, including window presentation. Other uninstrumented host waits remain in
work time; missing coverage must not be reported as zero work.

Windowed startup presents an animated loading view while the shipping runtime
prepares on a worker. `startup.first_frame_ms` is the first presented image,
`loading_frame_ms` identifies that loading presentation, and `full_scene_frame_ms`
identifies the first successful game presentation. All are cumulative launch
timestamps; device/world preparation overlaps. Headless captures stay synchronous.
`ARA_LOADING_CAPTURE=<target/path.png>` explicitly captures the loading view for
visual verification; leave it unset for timing runs.
