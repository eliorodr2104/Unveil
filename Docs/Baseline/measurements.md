# Baseline measurements on the iPad (T15, spec §6.2)

All numbers below were measured on the device, from **Release** builds, on 2026-10-09 (CEST, 19:07 to 19:58). CSV
file names use UTC (CEST minus 2 h). Raw material: `Baseline/traces/` (1.4 GB, git-ignored) and
`Baseline/measurements/` (CSVs, `engine.jsonl`, soak console log, git-ignored).

## Setup

| Item | Value |
|---|---|
| Device | iPad Air 11-inch (M2), `iPad14,8`, 8 GB RAM, iPadOS 27.0 (24A437). UDID `00008112-0001189C3A46601E`, devicectl id `86B6DAFF-CBC5-5272-9C2A-F9C07FC92F0B` |
| Device state | cabled and charging, unlocked, Auto-Lock Never, not touched during runs (reported by the user) |
| Repo | branch `elio/v0-baseline`, HEAD `e7db415` (working tree as dispatched) |
| Toolchain | Xcode 27.0 (27A266a), rustc 1.99.0 (2026-09-28) |
| Build | `xcodebuild -project Unveil.xcodeproj -scheme Unveil -configuration Release -destination "platform=iOS,id=00008112-0001189C3A46601E" -derivedDataPath DerivedData-device -allowProvisioningUpdates build`, installed with `xcrun devicectl device install app --device 86B6DAFF-… DerivedData-device/Build/Products/Release-iphoneos/Unveil.app`. Mach-O UUID `369AA564-59AF-3DA3-8565-493522C96D86` |
| Engine | default XCFramework (`scripts/build-xcframework.sh`, Rust `-C target-cpu` default for `aarch64-apple-ios`, apple-a7) |
| RAWs | 24 MP class: `DSCF0267.RAF` (X-T4, 26.7 MB). 48 MP class: `Z7-14bit-lossless-L.NEF` (Z 7, 56 MB). Both in the app's `Documents/raw` |
| Bundle | `com.eliorodr2104.unveil` |

### How each number was recorded

- Every trace is `xcrun xctrace record --device 00008112-0001189C3A46601E --template "<T>" --instrument os_signpost --all-processes --time-limit <N>s --output <file>`
  started **before** `xcrun devicectl device process launch --device 86B6DAFF-… --terminate-existing com.eliorodr2104.unveil -- <args>`
  (8 s later; `xctrace --attach` is flaky, see the M3.4 report). Launch arguments are `-UnveilOpen <raw> -UnveilDelay 3`,
  plus `-UnveilEngineDir fresh` for cold and `-UnveilSweep 60` for sweeps.
- Intervals come from `xcrun xctrace export --input <trace> --xpath '//table[@schema="OSSignpostIntervals"]'` (this
  Xcode has no `os-signpost-interval` schema; `OSSignpostIntervals` is the one that holds the intervals), filtered to
  subsystem `com.unveil`. The parsing and statistics scripts are in the session scratchpad, not in the repo.
- Median and p95 use linear interpolation between ranks. With 5 opens the p95 is close to the maximum, so the min and max
  are shown as well.
- **"Cold"** means a fresh engine library (`-UnveilEngineDir fresh`), not a cold disk cache. **"Warm"** means the default
  library, which already knows the photo (the second import goes through `photo.relink`).
- **Time budget.** The brief says 60 s traces. The sweep starts after the launch, the delay, and the open (about 12 s in), so
  the traces are 85 s long to hold the whole 60 s sweep. Open traces are 24 s.
- **Observer cost.** The Time Profiler and Metal System Trace sweeps run with the profiler attached, so they cost something.
  Metal System Trace visibly does: 3495 and 3537 of the 3600 sweep ticks produced a request, and p95 is 80 to 90 ms.
  Time Profiler runs are the cleaner numbers.

## A. Open latency (`OpenToFirstFrame`)

Interval: `EditorViewModel.open` to the presented first frame. In all 20 runs it ended with outcome `dropped`: the canvas'
presented handler reports `presentedTime == 0` for the first frame after an open (M3.4 concern 1, unproven whether it is
a Metal quirk or a real missed frame; the photo does appear in use). The ends are counted as the first frame.

Commands: `Baseline/traces/opens/<raf|nef>-<cold|warm>-<1..5>.trace`, template `Logging`, `--time-limit 24s`, launch
arguments as above (cold: `-UnveilEngineDir fresh -UnveilOpen <raw> -UnveilDelay 3`; warm: the same without `fresh`).
One pilot run (RAF, cold) was done first to check the pipeline; its trace was deleted and the run repeated as `raf-cold-1`.

| RAW | Open | n | Median ms | p95 ms | Min ms | Max ms | Outcomes |
|---|---|---|---|---|---|---|---|
| RAF 24 MP | cold | 5 | 562.5 | 579.3 | 556.0 | 581.9 | 5 dropped |
| RAF 24 MP | warm | 5 | 517.0 | 534.7 | 509.4 | 538.0 | 5 dropped |
| NEF 48 MP | cold | 5 | 865.5 | 874.1 | 855.3 | 875.7 | 5 dropped |
| NEF 48 MP | warm | 5 | 1089.9 | 1098.3 | 1080.7 | 1099.7 | 5 dropped |

Per-run values (ms): RAF cold 581.9, 562.5, 556.0, 560.0, 568.8. RAF warm 521.3, 514.3, 517.0, 509.4, 538.0.
NEF cold 875.7, 867.9, 862.5, 855.3, 865.5. NEF warm 1099.7, 1092.8, 1089.9, 1080.7, 1088.7.

### Where an open spends its time (medians over the 5 runs, same traces)

The open interval is the sum of the copy and import commands, which run in series on the engine queue, and then the first
full render (the `SliderToFrame full` that the open itself requests).

| RAW | Open | `ImportCopy` | `Command library.import` | `Command photo.relink` | `Command library.select` + `develop.controls` | First full preview (`SliderToFrame full`) | Open total |
|---|---|---|---|---|---|---|---|
| RAF 24 MP | cold | 5.2 | 53.9 | n/a | 0.6 | 496.0 | 562.5 |
| RAF 24 MP | warm | 5.2 | 38.6 | 24.5 | 0.5 | 443.6 | 517.0 |
| NEF 48 MP | cold | 5.3 | 344.1 | n/a | 0.4 | 512.1 | 865.5 |
| NEF 48 MP | warm | 4.0 | 335.5 | 286.4 | 0.4 | 457.0 | 1089.9 |

Reading: the first preview render takes about 0.45 to 0.5 s for both sizes. For the 48 MP NEF the import command
(about 340 ms) is the second big piece, and the warm open is slower than the cold one by the `photo.relink` command
(about 286 ms), which cold does not run. The user's "open feels slow" is therefore about 0.9 s cold and 1.1 s warm on the
NEF, of which more than 0.7 s is engine import, relink and the preview render.
`Command` and `ShaderCompile` intervals are in the same traces (`ShaderCompile` 0.2 ms; the launch `app.gpu` read
15.7 to 17.1 ms happens before the open's timer starts, per M3.4).

## B. Slider latency (`SliderToFrame`, 60 s sweep)

Sweep: `-UnveilOpen <raw> -UnveilDelay 3 -UnveilSweep 60` (60 Hz CADisplayLink drag on `light.exposure`, a release
with a full render every second). Only intervals that begin **after the sweep starts** are kept (the start is the first
`draft` interval; the open's own full request is excluded: 1 interval in 8 of the 10 sweep traces, 0 in `nef-m1-time-2` and
`nef-default-time-1`). Two statistics are given:

- **All ended**: every interval, whatever its end, so an `overtaken` request counts up to the time a newer frame was
  shown. This is the latency the user sees from a request to the next frame on screen.
- **Presented only**: only the requests whose own frame was shown.

Counts are `presented / overtaken / dropped`. `dropped` is 0 everywhere in sweeps.

Source traces: `Baseline/traces/time-24mp.trace`, `time-48mp.trace` (template `Time Profiler`, `--instrument os_signpost`)
and `metal-24mp.trace`, `metal-48mp.trace` (template `Metal System Trace`, `--instrument os_signpost`). All with
`--all-processes --time-limit 85s`. All four open and export (TOC checked, `Time limit reached`, 85.8 s).

| RAW | Trace | Quality | n | presented / overtaken / dropped | All ended: median / p95 ms | Presented only: median / p95 ms |
|---|---|---|---|---|---|---|
| RAF 24 MP | time-24mp | draft | 3599 | 3179 / 420 / 0 | 51.2 / 68.1 | 51.1 / 67.9 |
| RAF 24 MP | time-24mp | full | 60 | 54 / 6 / 0 | 51.4 / 68.0 | 51.2 / 67.9 |
| RAF 24 MP | metal-24mp | draft | 3495 | 2843 / 652 / 0 | 65.9 / 84.3 | 51.6 / 79.9 |
| RAF 24 MP | metal-24mp | full | 60 | 45 / 15 / 0 | 51.7 / 87.9 | 51.0 / 78.8 |
| NEF 48 MP | time-48mp | draft | 3600 | 2860 / 740 / 0 | 67.0 / 68.1 | 51.3 / 68.0 |
| NEF 48 MP | time-48mp | full | 60 | 46 / 14 / 0 | 67.0 / 68.2 | 51.2 / 67.9 |
| NEF 48 MP | metal-48mp | draft | 3537 | 2516 / 1021 / 0 | 67.2 / 83.4 | 66.4 / 68.9 |
| NEF 48 MP | metal-48mp | full | 60 | 43 / 17 / 0 | 67.4 / 90.1 | 66.8 / 68.1 |

Command cost during the same sweeps (`Command develop.set`, the exposure drag command on the engine queue): 3.0 ms median,
3.6 ms p95 in `time-24mp` and `time-48mp`; 3.3 / 11.8 ms and 3.1 / 10.2 ms with Metal System Trace attached.

Observation (not a diagnosis, that is T16). For the Time Profiler all-ended rows only (`time-24mp`, `time-48mp`), the draft and
the full latency are the same to within a millisecond, they sit at about 51 ms (3 display frames at 60 Hz) or 67 ms
(4 frames), and the 48 MP NEF is one display frame slower than the 24 MP RAF at the median. The Metal System Trace rows
do not follow this (see the table). The latency looks pipeline and display bound, not render bound, so it does not yet say how much
GPU time a 48 MP draft needs.

## C. Soak (10 minutes, NEF)

Command (from `Baseline/measurements/soak-console.log`):
`xcrun devicectl device process launch --device 86B6DAFF-CBC5-5272-9C2A-F9C07FC92F0B --terminate-existing --console com.eliorodr2104.unveil -- -UnveilOpen Z7-14bit-lossless-L.NEF -UnveilSweep 600 -UnveilExitAfterSweep`.
No xctrace was attached (the Thermal State recording is optional in the plan; the thermal state comes from the sampler).
Result: `sweep completed`, 302 CSV rows from 0.0 to 600.0 s, **`The app terminated with the exit code 0.`** (last line of the
log is `EXIT 0`). No jetsam: the process lived the full 600 s.

CSV: `Baseline/measurements/csv/2026-10-09T173407Z.csv` (UTC; started 19:34:07 CEST).

| Quantity | Value | Where |
|---|---|---|
| Peak `phys_footprint` | **806.8 MiB** (845,956,320 B) | at 2.0 s, the open's transient |
| Peak after the first 10 s | 742.4 MiB | at 264 s and 364 s |
| Steady footprint, median | 662.2 MiB (10 to 60 s), 656.8 MiB (60 to 300 s), 656.6 MiB (300 to 600 s); windows are half-open, start included and end excluded: [10, 60), [60, 300), [300, 600) | no upward trend |
| Minimum available memory (`os_proc_available_memory`) | **7385.2 MiB** | at 2.0 s |
| Minimum available after the first 10 s | 7449.6 MiB | at 264 s |
| Thermal transitions | `nominal` (0 s) to `fair` at 54.1 s (17:35:02Z) to `serious` at 284.1 s (17:38:52Z); stays `serious` until 600 s | CSV column `thermal_state` |
| Jetsam | none | exit code 0 |

Draft/full latency of the soak was not traced; the 60 s sweeps in section B carry the latency numbers.

## Peak memory and thermal, per size (60 s sweeps)

Every peak footprint and minimum available memory in this document comes from a sampler that reads every 2 s, so a
short spike between two samples is missed: the peaks are **lower bounds**, and the minimum available values are upper
bounds on the true minimum.

From the CSVs the sweeps wrote (`Baseline/measurements/csv/`, one per sweep, 31 or 32 rows each; the sampler is the app's
own `FootprintSampler`). Thermal was `nominal` for the whole of each of these four sweeps.

| RAW | Sweep (trace) | CSV | Peak `phys_footprint` MiB | Min available MiB | Thermal |
|---|---|---|---|---|---|
| RAF 24 MP | time-24mp | `2026-10-09T172917Z.csv` | 661.0 | 7531.0 | nominal |
| RAF 24 MP | metal-24mp | `2026-10-09T173115Z.csv` | 723.3 | 7468.7 | nominal |
| NEF 48 MP | time-48mp | `2026-10-09T172439Z.csv` | 879.6 | 7312.3 | nominal |
| NEF 48 MP | metal-48mp | `2026-10-09T172637Z.csv` | 877.4 | 7314.6 | nominal |
| NEF 48 MP | soak 600 s | `2026-10-09T173407Z.csv` | 806.8 | 7385.2 | nominal, fair, serious |

The peaks occur in the first 2 to 4 seconds of each sweep (right after the open). The five CSVs whose names start
`2026-10-09T16…` are the M3.4 smoke runs and are not used anywhere.

## D. A/B: `-C target-cpu=apple-m1` against the default (NEF, 60 s sweeps)

Procedure: `scripts/build-xcframework.sh --cpu apple-m1`; `xcodebuild ... -configuration Release -derivedDataPath DerivedData-device-m1 build`
(a different Mach-O: 34,905,136 B against 34,730,672 B for the default); installed from `DerivedData-device-m1`;
3 sweeps with `xcrun xctrace record ... --template "Time Profiler" --instrument os_signpost --all-processes --time-limit 85s`
and `-UnveilOpen Z7-14bit-lossless-L.NEF -UnveilDelay 3 -UnveilSweep 60`
(`Baseline/traces/ab/nef-m1-time-{1,2,3}.trace`). Then `scripts/build-xcframework.sh` (default), the Release build rebuilt into
`DerivedData-device`, installed, and the **same 3 sweeps again on the default build**
(`Baseline/traces/ab/nef-default-time-{1,2,3}.trace`), so both arms ran under the same conditions. The default arm is
separate from the single `time-48mp` run of section B (taken at `nominal` thermal state).

**Caveat on conditions.** The A/B ran right after the soak, and the iPad was at thermal state `serious` for all six runs
(`default-1` had a short `fair` interval from 20.2 s to 46.2 s). The two arms are matched, but throttled; the section B
`time-48mp` sweep at `nominal` gave the same latency (67.0 / 68.1 ms), so the metric did not move with thermal state.

| Run | Trace | CSV (UTC) | Draft n (pres / over / drop) | Draft all-ended median / p95 ms | Draft presented median / p95 ms | Full n (pres / over / drop) | Full all-ended median / p95 ms | Peak footprint MiB | Min avail MiB |
|---|---|---|---|---|---|---|---|---|---|
| m1-1 | `ab/nef-m1-time-1` | `T174503Z` | 3596 (2802 / 794 / 0) | 67.1 / 68.2 | 52.0 / 68.0 | 60 (46 / 14 / 0) | 67.1 / 69.3 | 819.2 | 7372.8 |
| m1-2 | `ab/nef-m1-time-2` | `T174706Z` | 3558 (2752 / 806 / 0) | 67.0 / 68.4 | 52.5 / 68.0 | 60 (46 / 14 / 0) | 67.0 / 68.4 | 880.9 | 7311.1 |
| m1-3 | `ab/nef-m1-time-3` | `T174946Z` | 3598 (2838 / 760 / 0) | 67.0 / 68.2 | 51.7 / 68.0 | 60 (42 / 18 / 0) | 67.2 / 68.2 | 877.6 | 7314.4 |
| default-1 | `ab/nef-default-time-1` | `T175305Z` | 3480 (2743 / 737 / 0) | 67.0 / 68.5 | 51.7 / 68.2 | 59 (47 / 12 / 0) | 67.2 / 68.9 | 861.6 | 7330.4 |
| default-2 | `ab/nef-default-time-2` | `T175532Z` | 3600 (2816 / 784 / 0) | 67.1 / 68.1 | 51.5 / 68.0 | 60 (55 / 5 / 0) | 51.2 / 68.1 | 803.5 | 7388.5 |
| default-3 | `ab/nef-default-time-3` | `T175731Z` | 3601 (2834 / 767 / 0) | 67.1 / 68.2 | 51.5 / 68.0 | 60 (46 / 14 / 0) | 60.9 / 68.3 | 805.0 | 7387.0 |

Summary (median of the three per-run values):

| Arm | Draft all-ended median ms | Draft all-ended p95 ms | Draft presented median ms | Full all-ended median ms | Full all-ended p95 ms |
|---|---|---|---|---|---|
| `apple-m1` | 67.0 | 68.2 | 52.0 | 67.1 | 68.4 |
| default (`apple-a7`) | 67.1 | 68.2 | 51.5 | 60.9 | 68.3 |

Verdict: **no measurable difference** in `SliderToFrame` (differences are 0.1 ms on the draft all-ended median and p95,
within 0.5 ms on the presented median; the full all-ended median varies between 51 and 67 ms inside the default arm alone).
The latency is quantized by the display, so a CPU-codegen effect smaller than a frame cannot show in this metric. A
finer comparison would need the CPU time per frame from the Time Profiler samples in the same traces, which T16 can do.
Two runs lost sweep ticks (`m1-2` 3558, `default-1` 3480 of 3600 drafts); both have a normal latency distribution, and
these runs are reported as taken, not repeated.

### Default build restored

After the A/B, `scripts/build-xcframework.sh` (no options) and the same Release build command as above were run into
`DerivedData-device`, and the app was installed from there. Checks: `Config/UnveilEngine.xcconfig` is byte-identical to the
copy taken before the A/B (`diff`, no output; `git diff Config/` was not run because this subtask forbids git). The
rebuilt `libunveil_ffi.a` has a different checksum from the earlier one (static-archive build is not byte-reproducible), but
the app's Mach-O is the same size (34,730,672 B) with the same UUID `369AA564-59AF-3DA3-8565-493522C96D86` as the build all of
sections A to C were traced with, so the dSYM in `DerivedData-device/Build/Products/Release-iphoneos/Unveil.app.dSYM` still
matches those traces. A copy of the pre-A/B app and dSYM is in `Baseline/traced-build/`.

## E. Backend (GPU or CPU)

Source: `Baseline/measurements/engine.jsonl`, copied with
`xcrun devicectl device copy from --device 86B6DAFF-… --domain-type appDataContainer --domain-identifier com.eliorodr2104.unveil --source Documents/diagnostics/engine.jsonl --destination Baseline/measurements/engine.jsonl`.
Each launch writes one `launch` line (the engine's `app.gpu` query). All 32 lines from this measurement session
(2026-10-09T17:07:53Z to 17:57:26Z, one per launch: 20 opens, 4 profiled sweeps, 1 pilot, 1 soak, 6 A/B) are identical
apart from the time, and there is no `suspend`, fallback or CPU line in that span:

```
{"event":"launch","gpu":{"adapter":"Apple M2 GPU (Metal)","available":true,"enabled":true,"lastFallback":null,"reason":null},"time":"2026-10-09T17:34:06Z"}
```

(the soak's line; the others differ only in `time`). So the GPU (Metal, Apple M2 GPU) was enabled and available at the start
of every run, with `lastFallback` and `reason` null. The app was never backgrounded in these runs, so no suspend or resume
state line was written; the engine does not write a line when it falls back mid-run, so this file is the launch evidence.

Independent evidence that the GPU did the work during the runs: in `Baseline/traces/metal-48mp.trace`, table
`metal-gpu-intervals` (`xcrun xctrace export --input Baseline/traces/metal-48mp.trace --xpath '//table[@schema="metal-gpu-intervals"]'`)
has 30,696 GPU intervals for `Unveil` (summed channel time 26,964 ms; channels overlap, so this is not a busy fraction),
against 51,600 for `backboardd`.

## Limits of this baseline

- The soak ran only on the NEF (48 MP), as briefed. The 24 MP memory numbers come from the 60 s sweeps.
- No memory sampler ran during the open-only traces, so section A has no memory column.
- `presentedTime == 0` on the first frame (see section A) is not understood; the open numbers rely on it as the end of
  the first frame.
- Sweeps with Metal System Trace attached lose some ticks and show a worse p95; use the Time Profiler rows for latency.
- The A/B ran under `serious` thermal state (see section D).
- Total device time was about 50 minutes, longer than the 30 minutes announced.
