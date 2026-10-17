# E2 half storage

Status on 2026-10-15: implemented for diagnostics, disabled in the normal app by the user's explicit precision preference. Host and physical iPad storage/recovery checks pass, and photographic differences meet the project's numerical tolerances. They are not bit-exact and do not establish equal quality to Lightroom. The user prefers more RAM to accepting that uncertainty. Normal rendering therefore keeps f32 storage; future latency work must preserve that choice. The integrated half/reduced/mapped latency gate also failed independently in that artifact. Subsequent E4/E5 work uses f32 throughout; see the updated [draft](draft-raster.md) and [frame-output](frame-output.md) reports.

The GPU original, linear RGB stage and terminal scalar planes can use half storage. Shader arithmetic, geometry results, intermediate filters and masks remain f32; display output remains RGBA8. An accepted half original has no retained f32 CPU or GPU shadow. Its RGB payload is six bytes per pixel instead of twelve, with four-byte allocation alignment. Pool retention and conversion/decode peaks are separate costs.

Half support is optional. The device requests the feature only when available, and failure to compile the optional kernels preserves the f32 path. Packing validates range and finite values. A rejected derived result is discarded before frames, histograms or stage caches are published. Exact input is then rendered in f32, with a sticky preference for that content and source level. Valid negative samples are preserved.

The per-content precision preference uses a bounded 1,024-entry LRU. A live resident preserves its own preference through metadata eviction. Explicit forget and library replacement remove metadata; fully forgotten content may try half again on a later visit. No full-image fingerprint is computed in the render path. Original-file changes retain the existing catalog contract and require content reload.

Half samples cannot reconstruct the exact original. Pixel analysis, missing planner facts and high-depth export reload through the original decoder recipe instead. Concurrent callers share one live exact CPU lease, and its allocation is released after use. A healthy half GPU owner survives these temporary accesses. A real device failure or changed decoder interpretation instead replaces the owner. Loader errors propagate rather than returning quantized pixels as exact samples.

Recovery also pins the actual source selected at decode. An original that becomes unavailable cannot silently fall back to a same-size JPEG smart preview. A resident intentionally created from that proxy keeps its proxy recipe even if the original returns. Five added regressions cover both directions and host/device transitions; the final review passed 14 half-source Metal cases, the separate f32 proxy transition and ordinary offline loading. Details are in `task-e1-recovery-provenance-review.md` under the M6 evidence directory.

The final f32 device compiles direct f32 storage loads once, avoiding per-sample format checks. A device that successfully enables half storage keeps the dynamic mixed-format loads, including for exact-f32 retry buffers. There is no lazy typed-shader cache or first-use specialization in production. Compilation failure falls back to the compatible dynamic path. The discarded experiments and latency evidence are in [frame-output.md](frame-output.md).

## Verification

The host tests used the real Apple M4 Metal adapter. The engine's eleven half-source cases and separate CPU-disabled case cover exact recovery, concurrent leases, missing facts, high-depth export, source replacement, fallback and bounded policy metadata. They assert zero retained CPU/recovery pixels after temporary uses and no redundant source upload on the next normal frame. The GPU tests independently check source/stage byte counts, forced-f32 behavior, optional-feature/shader fallback and rejected-result publication.

Fresh photographic validation used the five RAW fixtures and five existing presets. A frozen E2 source tree rendered both modes into separate candidate directories. All 25 f32 outputs matched the existing Mac goldens exactly. All 25 half outputs passed the unchanged CIEDE2000 limits of mean ≤0.5 and p99 ≤2.0: the worst per-image mean was 0.0246, the worst p99 was 0.7202, and the largest individual pixel difference was 2.2677. These 25 are fit-preview comparisons. The final integrated E2/E4 renderer also passed 60 independent f32-control cases covering five RAW formats, six presets including spatial effects, fit and odd native windows. Half sources and planes were confirmed active. Worst mean was 0.030489, worst p99 was 0.817245, and the largest individual difference was 2.083215. The physical ownership/recovery and photographic subsets below have also passed; integrated latency acceptance remains separate.

Evidence is under `.superpowers/sdd/2026-10-12-m6/`: `e2-quality-checkpoint/source-sha256.json`, `e2-measurements/mac-{f32,half}.log`, `f32-vs-baseline.log`, `half-vs-f32.log`, `mac-quality-summary.json`, `task-e2-engine-report.md`, `task-e2-integrated-gpu-report.md` and `e4-post-color-final-quality-half-independent.log`. Reference golden directories were not changed.

For these checks, `UNVEIL_E2_HALF=source` selects source and selected-stage half storage; `off` selects f32. `lin` and `all` isolate stage storage. `UNVEIL_E2_NO_F16=1` and `UNVEIL_E2_FAIL_HALF=1` exercise fallback. The mode is read when the GPU device is created. These controls are diagnostic, not user-facing settings.

No whole-process memory-halving or speedup claim follows from the storage representation alone. E9 covers retained allocator memory and decode peaks; E10 covers the GPU pool.


## Physical iPad ownership and recovery

Three isolated lifecycle runs passed on the iPad Air 11-inch M2 with the final integrated engine: RAF f32 control, RAF half, and NEF half. The two half logs confirm source and selected-plane storage enabled. Each session had one manager, a fresh library and no competing editor work. The diagnostic memory budget was about 2.66 GiB; these are ownership checks, not a comparison with the ordinary UI budget or workload.

| Fixture | Preview half original, bytes | Full half original, bytes | Exact Preview analysis |
|---|---:|---:|---|
| RAF | 17,297,280 | 155,750,400 | Reloads 0 → 1; readbacks remain 0 |
| NEF | 26,219,520 | 272,646,144 | Reloads 0 → 1; readbacks remain 0 |

Both Full originals use six bytes per pixel, without a retained CPU RGB duplicate. The RAF f32 control uses 34,594,560 Preview bytes and 311,500,800 Full bytes and performs one exact GPU readback instead of reloading. Every settled snapshot in all three runs has zero cached CPU original bytes and zero live recovery-lease bytes.

`crop.autoStraighten` exercised the exact Preview pixel path and found no dominant line in these fixtures. After the half reload, the GPU original's bytes and recovery counters survived unchanged into the next ordinary render; the physical decoder counter also stayed unchanged for that render. This validates temporary exact recovery without retaining CPU pixels or replacing the healthy half owner. It does not measure source uploads directly, since that counter is not exposed through the current device diagnostics.

First zoom, explicit Full release, second zoom, and suspend/resume all completed. Every Full release removed its source and window stages; suspension removed Preview and Full owners and all live GPU allocations. Resumed fit and zoom recreated the expected half payloads with zero retained CPU/recovery bytes. Re-entering Full after release performed one new physical sample decode; the analysis performed one additional exact decode. Those costs are expected and have not yet been accepted as paired interaction-performance results.

Evidence: `.superpowers/sdd/2026-10-12-m6/e2-measurements/lifecycle/{raf-off,raf-source,nef-source}-isolated.log`, each ending in `E1-LIFECYCLE PASS`. The earlier non-isolated `raf-off.log` is excluded because the diagnostic UI exit path interrupted it. [Source residency](source-residency.md) records the complete sequence and limits. Physical-device half goldens, derived precision-overflow recovery, high-depth export, concurrent CPU analysis and forced device-failure behavior are not established by these three runs; the corresponding host checks remain separate evidence. Half stays disabled by the final user decision recorded above.

## Physical photographic check, 2026-10-15

The integrated diagnostic build rendered all 25 Full preset images on the iPad Air M2 with `requested=true enabled=true planes=true`. All 25 passed against the unchanged f32 iPad reference: worst mean 0.0246, worst p99 0.7202, largest individual pixel difference 2.2677. Against the Mac half outputs, all means and p99 values round to 0.0000 at the comparator's four-decimal precision, but the largest individual difference is 1.1421. This is not a bit-exact cross-device claim. The two comparisons use the same unchanged mean ≤0.5/p99 ≤2 limits.

Evidence: `e2-measurements/ipad-half.log`, `ipad-half/`, `ipad-half-vs-f32.log`, `ipad-half-vs-mac-half.log` and `ipad-quality-summary.json`. The outputs were written under a separate diagnostic device directory; reference goldens were not overwritten. These tolerance-based results do not override the final decision to preserve f32 precision.

## Paired process memory observation

Four fresh-process runs of the same integrated diagnostic binary (one f32/half pair per RAW, all nominal) measured the whole app independently of source accounting. This is an observation, not a repeated performance distribution.

| RAW | State | f32 footprint MiB | Half footprint MiB |
|---|---|---:|---:|
| RAF | fit after 15 seconds | 516.0 | 625.1 |
| RAF | 100% after 10 seconds | 1347.7 | 1580.2 |
| RAF | fit after 30 seconds | 320.8 | 270.1 |
| NEF | fit after 15 seconds | 569.0 | 600.8 |
| NEF | 100% after 10 seconds | 1767.6 | 1393.8 |
| NEF | fit after 30 seconds | 341.8 | 288.7 |

Lifetime peaks were 2263.7/2233.0 MiB for RAF f32/half and 3747.4/3745.9 MiB for NEF. Half storage halves the measured original payload, but does not halve the process or remove decode peaks; early pool/allocator retention can increase footprint. The wide earlier E1 repeated ranges also caution against attributing the single 100% differences entirely to precision. The full 20 snapshots and logs are in `e2-measurements/memory-paired/`. E9 and E10 remain separate planned work.

## Typed finish load experiment

The isolated typed variant binds actual half inputs as scalar `array<f16>` and immediately converts each load to f32. It preserves dynamic-load fallback when optional compilation fails and bounds the half finish cache to eight attempted format masks, including failures. This experiment has not been enabled in production.

On the physical M2, all 25 Full PNGs are byte-identical to the preceding dynamic-load half outputs. This comparison is within the same precision policy; it does not make half output identical to the original f32 reference. Evidence: `e2-measurements/ipad-typed-half.log`, `ipad-typed-half/` and `ipad-typed-exact-summary.json`, with build identity in `integrated-fast-typed-sha256.json`.

In the same diagnostic binary, typed finish reduced the half reduced-Draft render wall from 14.193 to 13.455 ms in one RAF run per mode. Presented medians remained about 51 ms, so this is not latency acceptance. First-use compilation took 118.920 ms for mask 23 in the ordinary RAW sweep and 134.730 ms for mask 7 in the golden run. Mask 23 includes the texture plane required by the RAW import's capture sharpening. These costs occur on first observed formats and are absent from steady sweep averages. The experiment cannot be accepted solely from its warm-frame improvement.
