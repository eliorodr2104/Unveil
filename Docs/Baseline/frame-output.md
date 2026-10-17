# E5 frame output

Updated 2026-10-17. The app requests retained GPU mappings through `run_for_display(true)`. Eligible display deliveries avoid the intermediate CPU `Vec`. The original M6 E5 contract requires copy removal and measured CPU cost; both have evidence. The user also accepted E4 with its documented Draft-vs-Full latency limit on 2026-10-17. Both remain enabled and normal precision stays f32. Measurements below are from 2026-10-16.

## Ownership and remaining copies

The GPU writes RGBA8 into a device buffer, copies it to a wgpu staging buffer, and completes mapping. `DisplayImage` retains that mapping through the worker-to-engine handoff. The engine rejects stale photo/generation results before the C callback. Swift FrameSink copies borrowed rows directly into its shared Metal buffer, which the canvas wraps as a texture. The mapped owner lives through the synchronous callback and unmaps on drop.

Two transfers remain: GPU output to staging, and staging to the final Metal buffer. E5 removes the intermediate owned CPU image and its copy. It concerns renderer delivery to FrameSink, not file export, shared wgpu/Metal storage or complete zero-copy rendering.

Native windows retain crop offset and working-image stride. Extra tail capacity keeps all `height * stride` bytes readable; only `width * 4` bytes per row are visible pixels. Dropped/stale results, failed sends, shutdown and callback failure release mappings safely.

Mapping depends on GPU/cache eligibility. CPU rendering, overlays, thumbnail caches, persistent Full view previews, whole-photo histogram jobs and export remain owned. `run()` remains owned. Production has no diagnostic ownership switch. ABI 7 physical/logical dimensions are equal between mapped and owned controls.

## Measured CPU and presentation

The clean f32 candidate completed 24 alternating physical iPad runs: three per ownership mode, RAW and fit/window workload, using the same 30-second sweep with nominal thermals. One suspended incomplete run is retained separately and was repeated. Values below are medians of three runs.

| Workload | Owned / mapped render CPU | Owned / mapped render wall | Owned / mapped presented median |
|---|---:|---:|---:|
| RAF fit | 2.777 / 1.942 ms | 10.862 / 10.153 ms | 34.016 / 33.896 ms |
| RAF window | 2.762 / 1.822 ms | 11.060 / 10.171 ms | 33.997 / 33.921 ms |
| NEF fit | 2.737 / 1.757 ms | 10.632 / 9.771 ms | 34.049 / 35.255 ms |
| NEF window | 2.741 / 1.846 ms | 11.394 / 10.488 ms | 34.135 / 34.032 ms |

Mapping removes about 0.8–1.0 ms of render-thread CPU in these reduced-output sweeps. The later FrameSink copy can cost more on an untouched mapping, so the saving is not equal to end-to-end CPU improvement. NEF fit's mapped median is 1.21 ms above owned; the first pair in both modes was around 50 ms. No universal latency improvement or unconditional 1-ms non-regression threshold is claimed. P95 medians remain around 50.6–51.3 ms in both modes. All individual runs and presented counts are retained.

The unmasked finish specialization and reduced GPU statistics improve the prior approximately 50-ms configuration while preserving f32 and the P1 scheduler. Unmap itself was measured in microseconds. Native-mapped controls also returned to roughly 34 ms, but the repeated table above is specifically for reduced output.

## Canonical Release confirmation

The final normal app, built from canonical source and launched without rendering overrides, completed eight 30-second runs with nominal thermals. All counted Draft outputs were mapped and physically reduced; normal storage remained f32. Three fit runs per RAW gave RAF presented medians of 34.155, 34.149 and 34.035 ms, and NEF medians of 33.946, 33.896 and 33.986 ms. One 100% window confirmation per RAW gave 33.968 and 34.019 ms. Fit runs rendered 1,799–1,801 Drafts; window runs rendered 1,796 and 1,791. P95 ranged from 50.572 to 51.646 ms. Presented/overtaken counts and costs are preserved in the raw logs.

This confirms the integrated artifact's ordinary sweep behavior; it does not replace the matched Full comparison or close E4's latency criterion. Evidence: `e45-resume/production-normal/summary.json`, `production-normal/app-sha256.json`, and `reviewed-framework-sha256.json`. Window repeat evidence belongs to the earlier 24-run paired diagnostic series; these two canonical window runs are confirmation only.

## Verification and limits

- Apple M4 Metal checks cover cross-thread mapped lifetime, exact rows and histograms, odd/cropped windows and padded tails, omitted work, failure recovery and release. The five-RAW gate passes all 50 owned/mapped comparisons.
- The canonical FFI suite reports 74 passed and two ignored. It covers source/cache residency, stale results, drafts, suspend/resume, shutdown and actual mapping counters. RAW fixture gates also ran explicitly.
- All 25 existing Mac Full golden PNGs remain byte-identical. The physical iPad passes 60 Draft/reference/restoration cases, with exact Full restoration and all 30 requested fit histograms exact.
- Canonical device and simulator ABI 7 libraries were rebuilt. Simulator evidence is compilation only. Rejected fusion, sharding, memory-touch and scheduling experiments are absent.

FrameSink retains three maximum-size buffers. One removed image allocation does not imply quartered app memory or a fixed footprint saving. Half precision remains disabled by user choice; no Lightroom parity comparison was made.

Evidence: `.superpowers/sdd/2026-10-12-m6/e45-resume/clean-acceptance/`, `production-gpu-raw.log`, `production-ffi.log`, `production-device-quality-summary.json` and `reviewed-framework-sha256.json`. Superseded failed experiments and proposed gates are preserved in `previous-reports/frame-output.md` and the original `e5-support/` / `e5-measurements/` directories. E4's unmet presentation criterion is recorded in [draft-raster.md](draft-raster.md).

The final physical app suite passes 311 tests in 44 suites (Debug, 76.936 s). The latest reviewed canonical library also produces 25 iPad Full PNGs byte-identical to the existing iPad references. Evidence: `e45-resume/production-app-tests-debug.log` and `production-ipad-golden-verification.json`. The Release performance build is separate.
