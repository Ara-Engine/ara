"""Windows WASAPI loopback correlation probe. Install soundcard==0.4.6 and numpy.

Run after `cargo run -p ara-audio --release --example verify -- offline`:
  python crates/ara-audio/tools/loopback.py target/release/examples/verify.exe
Uses WASAPI's QPC packet timestamps, not Python record-call timing. Captures output only.
"""
import json
import os
import pathlib
import subprocess
import sys
import time
import wave
import numpy as np
import soundcard as sc
from soundcard import mediafoundation as mf


def main():
    executable = pathlib.Path(sys.argv[1]).resolve()
    artifact = pathlib.Path(__file__).resolve().parents[1] / "artifacts"
    artifact.mkdir(exist_ok=True)
    speaker = sc.default_speaker()
    loopback = sc.get_microphone(id=speaker.id, include_loopback=True)
    assert loopback.isloopback, "Probe must never record a microphone"
    chunks, times = [], []
    with loopback.recorder(samplerate=48000, channels=2, blocksize=480) as recorder:
        process = subprocess.Popen([str(executable), "latency"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        deadline = time.monotonic() + int(os.environ.get("ARA_AUDIO_LATENCY_TRIALS", "8")) * 0.25 + 5
        while time.monotonic() < deadline:
            if recorder._capture_available_frames() == 0:
                if process.poll() is not None:
                    break
                time.sleep(0.001)
                continue
            data = mf._ffi.new("BYTE**")
            frames = mf._ffi.new("UINT32*")
            flags = mf._ffi.new("DWORD*")
            device_position = mf._ffi.new("UINT64*")
            qpc = mf._ffi.new("UINT64*")
            client = recorder._ppCaptureClient[0]
            result = client[0].lpVtbl.GetBuffer(client, data, frames, flags, device_position, qpc)
            mf._com.check_error(result)
            try:
                if flags[0] & 4:
                    raise RuntimeError("WASAPI timestamp error; latency measurement invalid")
                if chunks and flags[0] & 1:
                    raise RuntimeError("WASAPI packet discontinuity; repeat the measurement")
                if flags[0] & 2:
                    chunk = np.zeros((frames[0], 2), dtype=np.float32)
                else:
                    chunk = np.frombuffer(mf._ffi.buffer(data[0], frames[0] * 8), dtype=np.float32).copy().reshape(-1, 2)
                times.append(int(qpc[0]))
                chunks.append(chunk)
            finally:
                recorder._capture_release(frames[0])
        stdout, stderr = process.communicate(timeout=10)
    if process.returncode:
        raise RuntimeError(stderr.decode())
    report = json.loads(stdout)
    data = np.concatenate(chunks)
    mono = np.mean(data, axis=1, dtype=np.float64)
    random, reference = 123, []
    for _ in range(768):
        random = (random * 1664525 + 1013904223) & 0xffffffff
        reference.append(0.2 if random & 0x80000000 == 0 else -0.2)
    reference = np.array(reference)
    numerator = np.correlate(mono, reference, mode="valid")
    power = np.convolve(mono * mono, np.ones(len(reference)), mode="valid")
    correlation = numerator / np.sqrt(np.maximum(power, 1e-30) * np.sum(reference * reference))
    sample_qpc = np.concatenate([t + np.arange(len(c)) * 1e7 / 48000 for c, t in zip(chunks, times)])
    latencies, quality = [], []
    for command in report["commandQpc100ns"]:
        candidates = np.flatnonzero((sample_qpc[:len(correlation)] >= command) & (sample_qpc[:len(correlation)] < command + 1500000))
        if len(candidates) == 0:
            raise RuntimeError("Missing captured output for command")
        index = candidates[np.argmax(correlation[candidates])]
        if correlation[index] < 0.6:
            raise RuntimeError(f"Probe correlation too low: {correlation[index]}")
        latencies.append(float((sample_qpc[index] - command) / 10000))
        quality.append(float(correlation[index]))
    report.update({"method": "WASAPI loopback packet QPC timestamp + 768-frame MLS correlation",
                   "loopbackDevice": speaker.name,
                   "latencyMs": latencies, "p50Ms": float(np.median(latencies)),
                   "p95Ms": float(np.percentile(latencies, 95)), "correlations": quality,
                   "capturedPeak": float(np.max(np.abs(data))),
                   "limitation": "Measures command to Windows output mix; excludes DAC, wireless and acoustic propagation."})
    canonical = subprocess.run([str(executable), "canonical"], input=json.dumps(report).encode(), capture_output=True, check=True).stdout
    (artifact / "loopback.json").write_bytes(canonical)
    with wave.open(str(artifact / "latency-loopback.wav"), "wb") as output:
        output.setparams((2, 2, 48000, 0, "NONE", "not compressed"))
        output.writeframes((np.clip(data, -1, 1) * 32767).astype("<i2").tobytes())
    print(canonical.decode())


if __name__ == "__main__":
    main()
