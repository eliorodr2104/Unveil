# Core Image RAW trial on Mac

Measured 2026-10-14 on an Apple M4 Mac with 16 GiB RAM, macOS 27.0.1 and Xcode 27.0.
Engine reference: `79be4e46f5880ce6bb83b2fb04043418043c3727`, before the E7 edited-status change.
The five RAW hashes match [raw-set.md](raw-set.md).

**Decision: retain the current engine for M6.** Core Image is faster for some workloads, but the default
appearance and lens treatment differ substantially. This trial does not establish a compatible replacement
or an iPad performance gain. No Core Image integration or golden regeneration was performed.

Both paths produced a populated CPU RGBA8 sRGB image at a 2048 px long edge. Each file and renderer ran in six
fresh processes: one warmup and five measured opens, followed by 30 measured exposure changes per process.
All 60 invocations completed, yielding 50 measured opens and 1,500 measured exposure updates. Both paths used
the real Apple M4 Metal device; all recorded measured thermal checks were nominal. Compilation finished before
the timing runs. These are fresh sessions, not cold OS file caches.

## Results

Median milliseconds. First image has five samples per cell; exposure has 150 correlated samples from five
processes. The detailed report includes p95, ranges, individual samples, defaults and artifact hashes.

| RAW | Unveil first image | Core Image first image | Unveil exposure | Core Image exposure |
|---|---:|---:|---:|---:|
| Canon R5 CR3 | 592.57 | 637.75 | 9.15 | 2.74 |
| Fujifilm X-T4 RAF | 415.28 | 397.03 | 8.18 | 2.85 |
| Sony A7 IV ARW | 328.76 | 501.50 | 8.30 | 2.69 |
| Nikon Z 7 NEF | 640.24 | 372.52 | 8.47 | 2.96 |
| iPhone 12 Pro DNG | 243.72 | 265.27 | 9.22 | 2.48 |

Unveil's first-image endpoint includes import, selection, rendering, histogram and callback delivery. Core
Image constructs and evaluates a `CIRAWFilter`; its context is primed outside that timer. The two paths do not
perform identical work. In particular, Unveil's exposure command alone costs about 3.7–3.9 ms; changing the
Core Image property costs less than 0.01 ms. Excluding those mutations reduces the observed exposure advantage
to 1.53–2.13 times. The remaining interval still includes different histogram/scheduling responsibilities and
is not isolated GPU time.

Core Image's median first-image time is about 42% lower on this NEF and 53% higher on this ARW. The smaller
differences on the other files and five open samples do not establish a universal improvement.

## Appearance and limits

All five outputs were visually checked. None was blank or misoriented. Defaults produce mean CIEDE2000
differences from 4.55 to 19.09; these include framing, lens correction, exposure, tone and color differences,
so they are not fidelity scores. Core Image opens the ProRAW scene much brighter. Three files have different
default lens treatment. The RAF differs by one output row and is resized for the descriptive comparison.

This is a preview trial with default looks. It does not test equivalent linear decoding, sensor-resolution
detail, a high-ISO suite, matched white balance, highlight recovery, battery use or iPad performance. Core
Image does not provide Unveil's complete catalog/history/histogram/mask workload in this harness.

A later comparison would need a defined common decoding or rendering contract, followed by matching output
and workload checks on the target iPad. The current evidence supports further investigation of the NEF path
and the ProRAW starting look, without changing either in this ticket.

## Local evidence

The full report and raw artifacts are retained in the ignored measurement directory:

- [Detailed method and decision](../../.superpowers/sdd/2026-10-12-m6/task-e8-report.md)
- [Harness, logs, samples and comparison images](../../.superpowers/sdd/2026-10-12-m6/e8-measurements/)

The harness uses the installed Xcode SDK's `CIRAWFilter` and `CIContext` API contracts. Existing cross-device
golden results remain separate evidence; this Mac trial neither replaces nor extends that validation.
