# E1 source residency

Status on 2026-10-15: host validation, six frozen-E1 physical ownership/memory runs and three integrated physical lifecycle/recovery runs passed. Paired latency, pressure eviction and device-failure recovery remain open. This document does not claim the M6 footprint targets or attribute E3/E5 performance to E1.

The 2026-10-15 combined app control kept E1 active with f32 storage, native Draft and owned output. Subsequent f32 E4/E5 candidate results are recorded in [M6 results](m6-result.md). Three RAF sweeps and one NEF sweep verify 34.58–34.73 ms presented medians and 1,799–1,800 Drafts per 30 seconds, all nominal. This confirms the combined app preserves the restored P1 cadence; it does not isolate E1's cost or close the remaining device pressure/failure cases. The user's decision to avoid half-precision loss does not disable E1, which removes duplicate ownership without quantizing the original. See [M6 results](m6-result.md).

The preview and full caches now hold either decoded host pixels or an immutable GPU original. A source is promoted only after upload and rendering succeed. Acceptance checks both the photo's content key and the source-cache epoch; replacement, full release, pressure eviction and preview-capacity eviction invalidate older work. The source ID is independent of buffer addresses and frame generations.

| Operation | Ownership afterward |
|---|---|
| Initial decode or unsuccessful promotion | Host RGB remains cached |
| Validated ordinary GPU render | GPU original and bounded planner facts; no cached CPU RGB |
| Pixel analysis or a high-depth export | One temporary exact CPU recovery lease; GPU ownership remains |
| Missing facts for a pixel-dependent edit | One exact recovery for that request, then cached facts |
| Persistent CPU mode or failed ordinary GPU render | Recovered/reloaded host RGB cached for subsequent CPU frames |
| Idle window release | Full source and window stages released; preview remains |
| Tight-crop fit preview using Full | Full remains available when windows are released |
| Suspend | Device source owners and full cache released after in-flight work drains |

Exact f32 recovery reads the immutable original, not edited stages. A half original instead reloads exact pixels through its decoder recipe; a healthy device owner survives that temporary access when the decoder interpretation matches. Concurrent consumers share a weakly retained temporary allocation while it is live. The recovery mutex is released before device work. Device failure or changed interpretation replaces the source with a fresh host identity; loader and offline errors propagate. Ordinary source caches do not retain a completed temporary lease. Denoised/twin sources and caller-supplied loaded images keep their existing host/legacy behavior.

The final review fixed a provenance bug in that recipe: ordinary File loading could substitute a same-size JPEG proxy and pass the metadata comparison. Recovery now pins either the decoded original without fallback or the explicitly selected smart preview. It preserves this distinction through exact f32 readback and later promotion. An unavailable original errors without replacing its healthy GPU owner or publishing proxy-derived analysis. Five regression cases and actual-Metal checks are recorded in `task-e1-recovery-provenance-review.md`; the full host engine suite reported 514 passed, zero failed and six ignored.

`library.memory.previewResidency` and `fullResidency` expose `cpuPixels`, `gpuPixels`, `recoveryPixels`, `planningFacts`, `readbacks` and `reloads`. The GPU payload is already included in `gpu.allocated` and is not added to `engineBytes`. Cache eviction cost includes the GPU payload. `previewSources.bytes` and `fullSource.bytes` now count retained host data and metadata, so a device entry still has nonzero metadata bytes with `cpuPixels = 0`.

The residency counters describe entries currently owned by the source cache. A detached job can retain its valid owner until it completes, and completed/discarded source counters are not accumulated as process totals. `recoveryPixels` reports a live exact lease without assigning its ownership to the cache. These counters are not physical-footprint measurements.

Planning facts retain at most 8 MiB per source. They cover lateral CA, Upright analysis, automatic spot selection, red-eye detection and window airlight. Geometry and source identity determine the pixel-dependent keys; exposure changes reuse them. Window airlight reuses the exact small frame sample when white balance changes. The planner's own regression suite covers the extracted algorithms and keys.

Host evidence is archived under `.superpowers/sdd/2026-10-12-m6/`: `task-e1-engine-report.md`, `task-e1-gpu-report.md`, `e1-engine-*.log` and `e1-ffi-suite.log`. Required-GPU assertions prevent adapter-free runs from passing the ownership tests silently. The original sandbox run failed those assertions because it had no Metal adapter; subsequent ownership validation ran on the Mac GPU.

The integrated device runner below covers zoom after explicit Full release, one exact Preview analysis and suspend/resume. Pressure-driven eviction, activation of edits requiring new planner facts, high-depth export, injected device failure and paired latency remain separate device checks. Intentional Full release requires one cold decode on re-entry; source upload counts are covered by host tests, not the public device diagnostics.

## Physical iPad ownership check, 2026-10-15

The frozen E1 checkpoint includes P1, E3 and E5, with f32 storage and native draft rasters. It does not include E2 or E4. The iPad Air 11-inch M2 ran Release on iPadOS 27.0 (24A437): three RAF and three NEF cold opens, a 15-second fit hold, first 100% zoom, and fit after 10/30 seconds. All six completed with nominal thermal state. Sources, library hash and full logs are under `e1-checkpoint/`, `e1-ios-library-sha256.txt` and `e1-measurements/` in the M6 evidence directory.

Every settled measurement reports zero cached CPU preview/full RGB, zero temporary recovery pixels and zero readbacks. At 100%, the one full GPU original is 311,500,800 bytes (RAF, 297.07 MiB) or 545,292,288 bytes (NEF, 520.03 MiB). Both return to zero after window release; the preview remains 34,594,560 / 52,439,040 bytes. This verifies the ownership removal directly, independently of allocator/compressor behavior.

| RAW | State | Physical footprint median (range), MiB |
|---|---|---|
| RAF | fit | 515.3 (514.8–515.5) |
| RAF | zoom-100 | 1644.6 (1053.8–1937.2) |
| RAF | fit-after-10s | 323.3 (320.3–331.2) |
| RAF | fit-after-30s | 324.9 (320.3–329.2) |
| NEF | fit | 568.8 (568.3–572.0) |
| NEF | zoom-100 | 1333.4 (1291.8–2297.4) |
| NEF | fit-after-10s | 344.8 (341.5–359.5) |
| NEF | fit-after-30s | 343.6 (342.1–359.6) |

The lifetime peaks remain 2253.6–2268.8 MiB RAF and 3743.3–3749.4 MiB NEF. They are decode/development peaks, not evidence of a retained second original. The wide 100% footprint spread remains visible despite identical live source owners; E9 covers allocator/decode work and E10 covers the pool. These combined E1/E3/E5 results are not an isolated attribution of every footprint change to E1.

E3 retains 53,397,637 / 91,501,347 sensor bytes before first zoom. Each run records one physical sample decode total, one prepared hit at first RAF zoom (two for NEF including import-to-preview), and zero retained sensor bytes afterward. That bounded sensor allocation is separate from the removed CPU RGB original.

Fresh Mac fixture validation on the frozen sources passed 50 mapped/owned RAW comparisons and all 12 window tests including real RAW windows. The 25 candidate Full PNGs match the existing Mac goldens exactly, mean/p99/max delta-E zero; the reference directories were not changed. The same frozen E1 renderer also produced 25 Full PNGs on the physical iPad, all exactly equal to the existing iPad goldens (mean/p99/max delta-E zero). Outputs were copied from the separate `m6-candidate-control` device directory into `e1-measurements/ipad-candidate/`; `e1-ipad-vs-baseline.log` records the comparison. The integrated lifecycle subset is recorded below. Injected failure recovery, integrated E2/E4 photographic output and paired interaction acceptance remain separate gates.


## Integrated physical lifecycle and exact recovery, 2026-10-15

The final integrated engine ran three isolated sessions on the same iPad Air M2: RAF with `UNVEIL_E2_HALF=off`, RAF with `source`, and NEF with `source`. Each used one manager and a fresh library, without canvas/editor observers or competing render requests. The diagnostic budget was available process memory divided by three, approximately 2.66 GiB (about 1.33 GiB for source/result caches). This differs from the earlier UI workload and is not a paired performance or footprint comparison.

Each run completed fit → first native zoom → Full release → second zoom → exact Preview analysis → ordinary fit → Full release → suspend/resume → recreated fit and zoom → release. Fit used a 1280-pixel long edge; the centered native window was 1025 × 769. All three ended with the runner's explicit PASS marker.

| Fixture and storage | Preview original, bytes | Full original at each zoom, bytes | Preview analysis recovery |
|---|---:|---:|---|
| RAF, f32 | 34,594,560 | 311,500,800 | One readback, no reload |
| RAF, half | 17,297,280 | 155,750,400 | One exact reload, no readback |
| NEF, half | 26,219,520 | 272,646,144 | One exact reload, no readback |

The half originals occupy six bytes per pixel, exactly half the corresponding f32 payload. Every one of the 36 settled memory snapshots reports zero Preview/Full `cpuPixels` and `recoveryPixels`. The analysis command was `crop.autoStraighten`, which obtains exact Preview pixels through `source_now`; all three fixtures returned no dominant line, so no crop edit was applied. The test proves that exact pixel access completed and its lease was released, not that an automatic correction was accepted.

In the two half runs, `previewResidency.reloads` changed from 0 to 1 while `readbacks` stayed 0. In the f32 control, `readbacks` changed from 0 to 1 while `reloads` stayed 0. Preview GPU bytes and these per-owner counters stayed unchanged through the next ordinary render. The source was not replaced or recovered again. Public diagnostics do not expose upload counters; this is not a direct device upload-count measurement.

Each release reports `fullSource.count = 0`, `fullResidency.gpuPixels = 0` and zero window stage bytes while preserving Preview. Each suspension reports zero Preview/Full source counts and GPU payloads, zero stage bytes, and `gpu.allocated = 0`. Fit and zoom then recreate the expected GPU owners after resume, again without a retained CPU original. This is suspend/resume in one session; process destruction and deterministic reopening were not exercised.

E3's physical sample-decode counter advances from 1 at first zoom to 2 at second zoom after explicit release. Half analysis adds one decode (2 → 3); f32 readback leaves it at 2. The following ordinary render adds none. Thus the test records the expected cost of exact half recovery and of revisiting a released Full source rather than hiding either behind a resident CPU duplicate.

The only accepted lifecycle logs are `.superpowers/sdd/2026-10-12-m6/e2-measurements/lifecycle/{raf-off,raf-source,nef-source}-isolated.log`. The earlier `raf-off.log` was interrupted by the diagnostic UI exit path and is not acceptance evidence. The scratch runner and its API limits are documented in `e1-support/README.md`. These runs do not establish half photographic equivalence on iPad, high-depth export behavior on iPad, presentation latency or a whole-process memory reduction.
