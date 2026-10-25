# Frame-window CPU metrics

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
- `hitches_over_16ms`: count of wall times strictly greater than 16 ms. Warmup
  frames are outside the window; only the process's first presented frame is
  excluded if it was measured. A new keep-alive window does not exclude its own
  first frame. `hitch_samples` gives the eligible measured frame count.

The cumulative counter is exposed by `Presenter::host_wait_stats()` and
`VulkanDevice::context().host_wait_stats()`. Capture it before and after the whole
frame, including window presentation. Other uninstrumented host waits remain in
work time; missing coverage must not be reported as zero work.
