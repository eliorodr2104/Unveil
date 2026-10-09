# Resident stages: before and after on the iPad

Measures the `elio/resident-stages` build (HEAD `ce4214b`: the FFI attaches a `StageCache` to every preview job, so the
uploaded source and the sampled, white-balanced and log-luminance stages stay on the GPU between renders) against the v0
baseline, with the baseline's own methods. "Before" numbers are cited from
[`measurements.md`](measurements.md) (M) and [`profiling.md`](profiling.md) (P); every "after" number has its trace or
CSV named below. Raw material is under `Baseline/` in the worktree (git-ignored): `Baseline/traces/rs-*.trace`,
`Baseline/measurements/rs-*`, `Baseline/measurements/csv/rs-*.csv`.

## Setup and starting state

| Item | Value |
|---|---|
| Device | iPad Air 11-inch (M2), `iPad14,8`, 8 GB, iPadOS 27.0 (24A437); UDID `00008112-0001189C3A46601E`, devicectl id `86B6DAFF-CBC5-5272-9C2A-F9C07FC92F0B`. Same device as the baseline |
| Device state | cabled, charging, unlocked, Auto-Lock Never, untouched for about 20 min before the first launch (reported by the user) |
| **Thermal at start** | **`nominal`**. A 4 s probe launch at 19:42:04Z (before any trace) wrote `nominal` in all 3 rows (`csv/rs-2026-10-09T194204Z.csv`), so no wait was needed. It was `nominal` for both 60 s sweeps and for the whole soak |
| Build | `scripts/build-xcframework.sh` (default CPU, rewrites the shared `Frameworks/`), then `xcodebuild -project Unveil.xcodeproj -scheme Unveil -configuration Release -destination "platform=iOS,id=00008112-0001189C3A46601E" -derivedDataPath DerivedData-device-rs -allowProvisioningUpdates build` (log `Baseline/measurements/rs-build.log`), installed with `xcrun devicectl device install app --device 86B6DAFF-... DerivedData-device-rs/Build/Products/Release-iphoneos/Unveil.app`. Mach-O UUID `F88C886F-9CCC-3285-BC8B-224E3696D1E9` (baseline build: `369AA564-...`) |
| Toolchain | same as the baseline (Xcode 27.0, rustc 1.99.0) |
| Methods | identical to M "How each number was recorded": `xcrun xctrace record --device 00008112-... --template "Time Profiler" --instrument os_signpost --all-processes --time-limit 85s` started 8 s **before** `xcrun devicectl device process launch --device 86B6DAFF-... --terminate-existing com.eliorodr2104.unveil -- -UnveilOpen <raw> -UnveilDelay 3 -UnveilSweep 60`; intervals from `OSSignpostIntervals` filtered to `com.unveil`; only intervals that start after the first `draft` (sweep start) are kept; median and p95 with linear interpolation; counts presented / overtaken / dropped. The soak is the baseline's command (see C) |
| Run order | NEF sweep 19:42:18Z, RAF sweep 19:44:15Z, NEF soak 19:48:49Z, all on 2026-10-09 (CSV names are UTC) |

## Summary (before to after)

| Metric | Before (source) | After | Change |
|---|---|---|---|
| NEF 48 MP `SliderToFrame` draft, all-ended median / p95 | 67.0 / 68.1 ms (M B, `time-48mp`) | **34.4 / 35.5 ms** | -49 % / -48 % |
| RAF 24 MP draft, all-ended median / p95 | 51.2 / 68.1 ms (M B, `time-24mp`) | **34.4 / 35.3 ms** | -33 % / -48 % |
| NEF draft overtaken | 740 of 3600 (20.6 %) | 87 of 3599 (2.4 %) | -88 % |
| RAF draft overtaken | 420 of 3599 (11.7 %) | 81 of 3601 (2.2 %) | -81 % |
| NEF process CPU per presented frame | 16.3 ms (P section 2, 47469 ms / 2906) | **8.78 ms** (31348 ms / 3572) | -46 % |
| RAF process CPU per presented frame | 13.0 ms (P section 2, 41931 ms / 3233) | **8.55 ms** (30580 ms / 3578) | -34 % |
| NEF `unveil-render` CPU per frame | 9.8 ms (P section 4) | **4.77 ms** | -51 % |
| RAF `unveil-render` CPU per frame | 7.4 ms (P section 4) | **4.37 ms** | -41 % |
| 600 s NEF soak thermal | nominal, `fair` at 54.1 s, `serious` at 284.1 s (M C) | **`nominal` for all 600 s** | no throttling |
| 600 s NEF soak steady footprint (median 300 to 600 s) | 656.6 MiB (M C) | **902.0 MiB** | +245 MiB (+37 %) |
| 600 s NEF soak peak footprint | 806.8 MiB, open transient at 2 s (M C) | **916.5 MiB**, on the plateau at 448 s | +110 MiB |
| Soak exit | code 0 (M C) | code 0 | same |

## B. Slider latency (Time Profiler, 60 s sweeps, 85 s traces)

Sources: `Baseline/traces/rs-time-48mp.trace` (NEF), `Baseline/traces/rs-time-24mp.trace` (RAF); statistics in
`Baseline/measurements/rs-sweep-stats.txt` (script output, same code as the baseline's `stats.py`). Sweep starts at 10.97 s
and 11.47 s; one `SliderToFrame full` that the open itself requests is excluded in each (as in the baseline).
Counts are presented / overtaken / dropped. Before = M section B (Time Profiler rows).

| RAW | Quality | n (presented / overtaken / dropped) | All ended: median / p95 ms | Presented only: median / p95 ms | Before: n (p / o / d), all-ended median / p95 (M B) |
|---|---|---|---|---|---|
| RAF 24 MP | draft | 3601 (3520 / 81 / 0) | **34.4 / 35.3** | 34.4 / 34.9 | 3599 (3179 / 420 / 0), 51.2 / 68.1 |
| RAF 24 MP | full | 60 (58 / 2 / 0) | **50.9 / 51.4** | 50.9 / 51.4 | 60 (54 / 6 / 0), 51.4 / 68.0 |
| NEF 48 MP | draft | 3599 (3512 / 87 / 0) | **34.4 / 35.5** | 34.4 / 35.2 | 3600 (2860 / 740 / 0), 67.0 / 68.1 |
| NEF 48 MP | full | 60 (60 / 0 / 0) | **51.1 / 51.6** | 51.1 / 51.6 | 60 (46 / 14 / 0), 67.0 / 68.2 |

`Command develop.set` (the exposure drag command): 2.9 ms median, 3.4 ms p95 on both RAWs (before 3.0 / 3.6, M B).
`dropped` is 0 everywhere, as before. Presented frames per second over the sweep: 59.5 (NEF) and 59.6 (RAF), against 48.4
and 53.9 before (P section 2): now almost every display tick shows a new frame.

### CPU per frame (same Time Profiler traces, 1 kHz samples, running threads only)

Method of P section 2 and 4: samples of the `Unveil` process inside the sweep window (first draft to last frame, 60.03 s
and 60.05 s), split by thread, divided by the presented frames of the window (3572 NEF = 3512 + 60; 3578 RAF = 3520 + 58).
Before, the number of renders was counted as upload bursts (2914 NEF, 3238 RAF, P section 2) and matched the presented
frames to 0.3 %, so presented frames are the divisor on both sides (the "before" per-frame figures are P's totals divided by
the presented counts of M B: 2906 and 3233). Symbolication with the new dSYM through `atos` at the load address from the
xctrace overlap warning, as in P section 1 (`0x100f6c000` NEF, `0x104118000` RAF). Output:
`Baseline/measurements/rs-cpu-48mp.txt`, `rs-cpu-24mp.txt`, and the full phase report `rs-an-48mp.txt`, `rs-an-24mp.txt`.

| NEF (48 MP) | Before ms (P section 2) | Before ms/frame | After ms | After ms/frame |
|---|---|---|---|---|
| Process CPU in the 60 s window | 47469 (0.79 cores) | 16.34 | 31348 (0.52 cores) | **8.78** |
| `unveil-render` | 28564 | 9.83 | 17052 | **4.77** |
| Main thread | 7092 | 2.44 | 8298 | 2.32 |
| Metal / dispatch threads | 10364 | 3.57 | 4462 | 1.25 |
| `unveil-engine` | 1441 | 0.50 | 1536 | 0.43 |

| RAF (24 MP) | Before ms (P section 2) | Before ms/frame | After ms | After ms/frame |
|---|---|---|---|---|
| Process CPU in the 60 s window | 41931 (0.70 cores) | 12.97 | 30580 (0.51 cores) | **8.55** |
| `unveil-render` | 23987 | 7.42 | 15620 | **4.37** |
| Main thread | 7051 | 2.18 | 8649 | 2.42 |
| Metal / dispatch threads | 9360 | 2.90 | 4749 | 1.33 |
| `unveil-engine` | 1526 | 0.47 | 1562 | 0.44 |

What is left on the render thread (after, per presented frame; before, P section 4, per render):

| Render-thread work | NEF after | NEF before | RAF after | RAF before |
|---|---|---|---|---|
| Source upload (zero-fill + copy + other) | **0.00** | 6.49 | **0.01** | 4.26 |
| CPU histogram of the output (`Histogram::of_srgb8`) | 2.54 | 1.42 | 1.89 | 1.25 |
| Readback copy (`finish_and_read` memmove) | 1.87 | 1.12 | 1.29 | 1.15 |
| Scan for unwritten pixels | 0.27 | 0.35 | 0.84 | 0.30 |
| Other (plan, finish params, submit, ...) | 0.09 | 0.42 | 0.34 | 0.45 |
| Total | 4.77 | 9.80 | 4.37 | 7.41 |

The 4.8 to 6.5 ms of `create_buffer_init` zero-fill and copy is gone, which is the whole saving on the render thread.
The three byte-moving items that remain cost **more** per frame than before (histogram 1.42 to 2.54 ms and readback 1.12
to 1.87 ms on the NEF). I did not find out why. They process the same 14.85 MB output. The verifier ruled out one hypothesis: the render thread did not move from performance to efficiency cores (100 % on the P cores before and after: 28563 of 28564 ms, 17052 of 17052 ms). The cause remains open; a plausible cause is that the
CPU now reads freshly written GPU-shared memory with no upload work in between, or a different clock or cache state, but
no trace here separates those. The RAF scan (0.30 to 0.84 ms) has the same ambiguity: the sampler splits time between
inlined histogram and scan code, so single categories are good to a few tenths of a millisecond.
Main-thread CPU did not fall: it is driven by the SwiftUI slider (60 updates a second) and now more frames reach it
(59.5 against 48.4 a second).

Not measured: GPU time per render (no Metal System Trace was requested; the baseline GPU figures, 9.4 ms per NEF render
of which about 3.4 ms of sampling, white balance and log-luminance and 1.55 ms of upload blit should have gone, are
unverified after the change).

## C. Soak (10 minutes, NEF)

Command, the baseline's (M C), no xctrace attached:
`xcrun devicectl device process launch --device 86B6DAFF-CBC5-5272-9C2A-F9C07FC92F0B --terminate-existing --console com.eliorodr2104.unveil -- -UnveilOpen Z7-14bit-lossless-L.NEF -UnveilSweep 600 -UnveilExitAfterSweep`.
Log: `Baseline/measurements/rs-soak-console.log` (`sweep completed`, `The app terminated with the exit code 0.`, `EXIT 0`).
CSV: `Baseline/measurements/csv/rs-2026-10-09T194850Z.csv` (19:48:50Z; 302 rows, 0.0 to 600.0 s). Same sampler, same
windows (half-open, start included) as M C.

| Quantity | Before (M C, `T173407Z.csv`) | After |
|---|---|---|
| Exit code | 0 | **0**, no jetsam |
| Peak `phys_footprint` | 806.8 MiB at 2.0 s (open transient) | **916.5 MiB** at 448.1 s (plateau) |
| Peak after the first 10 s | 742.4 MiB | 916.5 MiB |
| Footprint at 2.0 s (open transient) | 806.8 MiB | 514.8 MiB (572.8 MiB in the first row at 0.0 s; the CSV starts at sweep start, after the open's first frame, so neither side is strictly the open transient) |
| Steady median [10, 60) / [60, 300) / [300, 600) s | 662.2 / 656.8 / 656.6 MiB | 491.3 / 901.9 / **902.0** MiB |
| Minimum available memory (`os_proc_available_memory`) | 7385.2 MiB (7449.6 after 10 s) | **7275.5 MiB** (all after 10 s) |
| Thermal transitions | `nominal` to `fair` at 54.1 s (17:35:02Z), to `serious` at 284.1 s (17:38:52Z), `serious` until 600 s | **none: `nominal` in all 302 rows** |

Footprint shape after the change: 392 MiB at 4 s, then a straight ramp of about 3.1 MiB per second up to about 902 MiB at
about 174 s, then a plateau until 600 s. On the plateau the footprint is not a single value: it sits at 902.0 MiB
(901.9 to 902.3) with excursions up to 916.1 to 916.5 MiB (at 206, 212, 354 and 448 s) and down to 887.9 MiB (at 488
and 528 s); the 916.5 MiB peak is one of these. The baseline did not ramp. The ramp stops by itself, so within 10
minutes it is bounded, but its cause is **unexplained and under investigation**. It is not the stages: they are built
by the open's first full render, before the CSV's first row (the console log prints `measure: first full frame` before
`sweep started`), and the footprint is 392 MiB at 4 s, already below the baseline's 628 MiB at 4 s. The whole ramp
(about +510 MiB, from 392 to 902 MiB) is a uniform growth of about 52 KiB per frame that does not depend on the RAW
(3.09 MiB/s in both the NEF and the RAF 60 s sweeps), so it cannot come from the stages (171 and 153 MB), and no
budget or pool limit in the code is equal to 510 MiB. Consequence for the 60 s sweeps: their CSVs end in the middle of
the ramp, so their peaks are not steady state.

60 s sweep CSVs (peak footprint, minimum available, thermal), for completeness (before: M "Peak memory and thermal"):

| RAW | CSV | Peak MiB | Min available MiB | Thermal | Before peak (CSV) |
|---|---|---|---|---|---|
| NEF 48 MP | `csv/rs-2026-10-09T194231Z.csv` | 626.8 | 7565.2 | nominal | 879.6 (`T172439Z`) |
| RAF 24 MP | `csv/rs-2026-10-09T194427Z.csv` | 559.3 | 7632.7 | nominal | 661.0 (`T172917Z`) |

The 2 s sampler misses short spikes, so all peaks are lower bounds (as in M).

## Open (side observation, one warm open per trace, not repeated)

`Baseline/measurements/rs-open-stats.txt`: NEF `OpenToFirstFrame` 1154.8 ms (`library.import` 342.7, `photo.relink` 283.4,
first full render 456.2); RAF 490.1 ms (first full render 452.9). The baseline warm medians are 1089.9 and 517.0 ms with a
first render of 457.0 and 443.6 ms (M A). Resident stages do not help an open: the first render must upload and derive
everything once. The after build (`ce4214b`) also contains open-path-only Swift changes made after the baseline build (commit `497b6ad`: hashed import copies, rollback and duplicate handling), so an open difference could not be attributed to resident stages even if there were one, and the opens were not re-measured. The single runs are within the baseline's run
spread for the render and a little above it for the NEF total; no conclusion is drawn.

## Conclusion

**What improved.**

- Latency in the sense the user feels: the NEF draft latency halves (67.0 to 34.4 ms median, 68.1 to 35.5 p95) and the
  RAF's goes from 51.2 to 34.4 ms; the RAF and NEF now behave the same. Overtaken requests fall from 20.6 % and 11.7 % to
  about 2 %: the render now finishes inside one 16.7 ms display tick, so a new frame is shown on almost every tick
  (59.5 of 60 a second).
- CPU: 46 % (NEF) and 34 % (RAF) less process CPU per frame, 51 % and 41 % less on the render thread, from removing the
  per-render source upload (6.5 and 4.3 ms). The Metal and dispatch threads also fall by 54 to 65 % per frame.
- Heat: the 600 s NEF soak never left `nominal`, where the baseline reached `fair` in 54 s and `serious` in 284 s. This is
  the clearest win for the "sustained editing" case and the main reason to keep the change. One soak per build, so no
  run-to-run spread is known; the starting state was `nominal` in both.

**What did not improve, and why.**

- Latency is quantized by the display (60 Hz, 16.7 ms), as the baseline said. 34.4 ms is 2 display frames, the
  baseline's 51 and 67 ms were 3 and 4. Below 2 frames the path is the hop to the main thread, the next display-link
  tick and the compositor, none of which this change touches. Render time is now smaller than the metric can resolve, so
  CPU time per frame, not `SliderToFrame`, is the number that shows the gain from here on.
- `full` quality is now about one display frame slower than `draft` (51 against 34 ms) on both RAWs; before they were
  equal. It is still no worse than the old full (51.4 and 67.0 ms). I did not look into why the release frame takes the
  extra tick.
- The CPU histogram, the readback copy and the unwritten-pixel scan now make up essentially all of the render thread
  (4.7 of 4.8 ms on the NEF) and, per frame, cost more than before. They are the next CPU target; they are
  unchanged by resident stages.
- The Main thread (SwiftUI slider updates) did not shrink; it is about 2.3 to 2.4 ms per frame and 0.14 cores.
- Open latency was not re-measured (see above); it is not expected to change with resident stages.

**What it costs.** Memory: the steady footprint rises from 657 to 902 MiB on the NEF (+245 MiB), and the peak from 806.8
to 916.5 MiB. Available memory stays above 7.2 GiB on the 8 GB device, and nothing was killed, but the footprint ramps for
about 170 s before it plateaus, and the cause of the whole ramp (+510 MiB) is not understood and is under investigation. A long session on a smaller
device, or with a second large photo loaded, is not covered by these runs. The open transient is lower than before
(514.8 against 806.8 MiB at 2 s).

## Limits

- One 60 s sweep per RAW and one soak per build; no run-to-run spread for the new build. The baseline's spread (A/B runs,
  M D) was under 0.5 ms on the draft all-ended median and 0.2 ms CPU per frame.
- Time Profiler only: GPU time per render is not remeasured.
- The CPU split is symbolicated with `atos` after the overlap workaround of P section 1; categories within the render
  thread are good to a few tenths of a millisecond.
- The 24 MP soak was not run (as in the baseline).
- The optimized Release build (`DerivedData-device-rs`, UUID `F88C886F-...`) is left installed on the iPad; the shared
  `Frameworks/UnveilEngine.xcframework` now holds the optimized engine.
