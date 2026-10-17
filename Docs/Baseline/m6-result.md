# M6 implementation and acceptance

Updated on 2026-10-17. The app retains P1, E1, E3 and E7, and normal storage remains f32 by the user's explicit precision choice. The app enables E4 reduced display drafts and E5 mapped delivery, with exact Full output verified on Mac and iPad. E5 removes the intermediate CPU image and has measured CPU savings. The user accepted E4 on 2026-10-17 with the documented presentation-latency limit. E4 is closed by that explicit acceptance; the original Draft-below-Full criterion still does not pass uniformly. No Lightroom parity is claimed.

## Ticket outcomes

| Item | Result | Evidence |
|---|---|---|
| P1 | Culprit `b6ca348d8444377de43f466cd2aabe3403accbc3`: CAMetalDisplayLink halved Draft input cadence from about 60 to 30 requests/s. CAD input advancement with bounded asynchronous drawable acquisition restored 1,800 renders/30 s and three 34.71–34.78 ms medians before E1 began. | M6 `task-p1-async-report.md`; final configuration measurements below |
| E1 | One immutable GPU original after successful promotion, without a retained CPU RGB duplicate. Exact temporary recovery, source retirement and offline-proxy provenance are implemented. Six physical ownership runs and three integrated lifecycle runs passed. | [Source residency](source-residency.md) |
| E2 | Half source/stage storage, f32 arithmetic, range/feature fallback and exact recovery are implemented for diagnostics. The measured differences pass the project's tolerance, but the user rejected this precision tradeoff. Normal app rendering stays f32. No Lightroom parity is claimed. | [Half storage](half-storage.md) |
| E3 | Prepared RAW samples are reused through the first full-resolution decode. Actual sample decodes fell from 2 to 1 for RAF and 3 to 1 for NEF in the measured sequence; cold NEF Full decode improved from 960.11 to 746.82 ms. Warm NEF did not improve. | [Decode transactions](decode-transactions.md) |
| E4 | Post-color reduced display output preserves native f32 effects, logical coverage and exact Full restoration. All 60 device quality cases pass. Matched render CPU/wall are lower, but four presentation cells are slower than Full. Accepted by the user on 2026-10-17 with this limit; the original latency gate remains unmet. | [Draft raster](draft-raster.md) |
| E5 | The app requests retained GPU mappings, removing the intermediate CPU image from eligible deliveries. Exact pixel, ownership and lifetime checks pass; 24 paired device sweeps measure about 0.8–1.0 ms less render-thread CPU. Final staging and Metal-buffer copies remain. | [Frame output](frame-output.md) |
| E7 | Only As Shot white balance is ignored for the non-RAW unedited state. Custom and preset changes count as edited. | [White balance](e7-white-balance.md) |
| E8 | Mac Core Image RAW trial completed: 60 invocations, 50 opens and 1,500 exposure updates on M4. Results do not justify replacing the existing engine; no Core Image integration was made. | [Core Image trial](core-image-trial.md) |
| E9 | Bounded AHD tiles, consuming fit, explicit returnable decoder storage and earlier cold CPU-source release. With the previous GPU policy, RAF/NEF median peaks fall from 2264.2/3740.1 to 1465.1/2006.9 MiB. | [Decode memory](decode-memory.md) |
| E10 | Active reusable reserve capped at min(budget/8, 320 MiB), with a three-second free-only idle trim. All six formal runs have zero reserve by +5 s; settled initial fit saves about 185 MiB per RAW. | [GPU pool budget](gpu-pool-budget.md) |

## Memory benefit and limits

E1 is active in the normal configuration. At settled full-source residency, it removes the second cached RGB original: about 297 MiB for RAF and 520 MiB for NEF. A temporary decoder or recovery lease still exists when required; this is not an equal reduction of lifetime peak or total process footprint.

The additional E2 comparison used one physical pair per RAW, with the same integrated post-color policy:

| Measurement | f32 | Half |
|---|---:|---:|
| RAF original GPU payload | 297 MiB | 149 MiB |
| NEF original GPU payload | 520 MiB | 260 MiB |
| RAF process footprint, fit +30 s | 320.8 MiB | 270.1 MiB |
| NEF process footprint, fit +30 s | 341.8 MiB | 288.7 MiB |

The roughly 51–53 MiB settled process difference is a single-pair observation, not a guaranteed saving. Earlier open/zoom points varied, including higher half-mode footprint. Before E9/E10, decode peaks remained about 2.2 GiB for RAF and 3.7 GiB for NEF. The 2360 × 1573 display raster occupies about 14.2 MiB per RGBA8 buffer; 1180 × 787 occupies about 3.5 MiB. Reusable allocations and other owners prevent multiplying this payload ratio into a process-memory claim.

## E9/E10 verification on 2026-10-17

The fresh control and candidate run on iPadOS 27.0.1 (24A446), while E0 used 27.0. Three
processes per RAW meet the isolated E9 peak and return-to-fit limits. With E10 enabled,
maximum lifetime peaks are 1366.5 MiB RAF and 1908.5 MiB NEF; settled initial fit is about
332/385 MiB. Isolated trims confirm about 188 MiB physically returned, with three valid
observations per RAW and one noisy NEF observation retained but excluded from attribution.

All five full-resolution native RAW fingerprints match the frozen control. All 25 Mac PNGs
and 25 iPad PNGs are byte-identical to their respective existing references. The precise
cross-platform comparison has maximum mean delta-E 0.000014451 and p99 zero. These sparse
differences already exist in the references; earlier four-decimal summaries rounded the
mean to zero. No reference image was regenerated and no half storage was enabled.

The physical app suite passes 312 tests in 44 suites, including exact pixels after suspend
and resume. Engine/GPU/FFI report 515/64/79 passes respectively, with their existing ignored
cases retained. RAW library, raster and explicit allocator tests pass; the separate RAW
corpus target still lacks its pre-existing manifest and is not claimed as passing.

Release and Debug builds complete without compiler warnings. The cleanup addresses the unused
capture, redundant unsafe nonisolation, missing optional fonts and unused App Intents metadata
extraction. Explicitly required missing fonts still fail; warnings are not globally suppressed.

The 36 paired slider sweeps and twelve targeted repetitions retain all outcomes and show no
repeated allocation/free cycle during dragging. Their initial small tail differences do not
persist consistently in the reverse-order repetitions. Mixed presentation remains sensitive
to display phase; exact run values and limits are retained in the E10 report. The additional
six settled mixed sweeps have about 33 ms run medians for both policies, with the candidate's
median/p95 inside the control ranges at the median of three runs. Individual slower phases
remain documented; no universal latency improvement is claimed.

The corrected three-cycle runs pass on both RAWs: lifetime peaks remain below 1433/1926 MiB,
all returned-fit states remain below initial fit +100 MiB, and every idle reserve is zero.
Source uploads are exactly one at fit then two/three/four cumulatively at the three windows.
The final Release build is installed and launched normally; its 25 PNGs also match the existing
iPad references byte for byte. The earlier cycle trial that could accept stale window metadata
is preserved as invalid for timing. Accepted evidence and final artifact identities are under
`.superpowers/sdd/2026-10-12-m6/e910-implementation/` and the E9/E10 measurement folders.

## E4/E5 verification on 2026-10-16

The reviewed GPU implementation adds an eagerly compiled unmasked finish specialization, checked 64-pixel reduction tiles and exact reduced GPU statistics. The generic kernel remains available if optional compilation fails. Native f32 processing and the P1 scheduler remain unchanged. Failed fusion, histogram sharding and CPU-touch experiments are excluded.

- Canonical GPU package on Apple M4 Metal: 62 reported passes, two ignored. Canonical FFI suite: 74 reported passes, two ignored. Separate gates execute 60 RAW quality/restoration cases and 50 exact owned/mapped comparisons.
- All 25 current Mac and all 25 current iPad Full PNGs are byte-identical to their existing references. No reference images were rewritten.
- The physical iPad passes 60 independent Draft/reference/restoration cases, including all 30 requested fit histograms exactly. Every final Full is byte-exact.
- Physical app tests: 311 tests in 44 suites pass in 76.936 seconds. This suite requires Debug instrumentation. Release test invocations failed to compile because testability and then DEBUG-only counters were absent; no source or settings change was needed. Device performance uses a separate Release build.
- Canonical device and simulator ABI 7 archives were rebuilt; simulator evidence is compilation only. Source, library and app hashes are retained in `e45-resume/`.

The 24 paired reduced-output sweeps show mapped median-of-run presentation around 34–35 ms; the first NEF fit pair remained around 50 ms in both modes. NEF fit's mapped median is 1.21 ms above owned, so no universal latency improvement is claimed. The matched 72-trial Draft/Full workload lowers render CPU by 37–72% and render wall time by 9–32%, but four of twelve median presentation cells are slower. The serial 30-Hz harness is not phase-locked to display ticks. The universal 20% wall-time proposal and under-25-ms goal remain unmet. E4 was accepted by the user on 2026-10-17 with these limits.

Current reports and raw evidence are in [Draft raster](draft-raster.md), [Frame output](frame-output.md), and `.superpowers/sdd/2026-10-12-m6/e45-resume/`. The final normal Release app completes eight nominal 30-second sweeps without rendering overrides: six RAF/NEF fit runs and two window confirmations. Presented medians range from 33.896 to 34.155 ms, with p95 50.572–51.646 ms and 1,791–1,801 Draft renders. All counted Drafts use reduced mapped output. Logs, counts and artifact identity are in `e45-resume/production-normal/`. These ordinary sweeps do not close the separately measured Draft-below-Full criterion.

## Earlier verification on 2026-10-15

The following results describe the preceding native-Draft/owned-output artifact. They are historical controls, not the E4/E5 candidate above.

The final f32 implementation compiles direct f32 loads once. Half-enabled devices keep dynamic mixed-format loads; production contains no lazy typed-kernel cache or first-use specialization. Failed optional compilation retains the compatible path.

- Full engine host suite: 514 reported passed, zero failed, six ignored. Separate real-Metal tests exercise half recovery and f32 smart-preview transitions. Sandbox-only loopback failures passed on the host rerun without code changes.
- Final GPU suites on Apple M4 Metal: 55 passed, zero failed, one pre-existing ignored test in each of f32 and source-half modes. Gated cases have separate capability/precision coverage.
- Final FFI suite: 73 reported passed, zero failed, two ignored. Fixture tests that initially skipped without their inputs were then run explicitly: all 25 Full PNGs are byte-identical to the existing Mac references, and all 50 RAW mapped/owned comparisons pass. Reference files were not changed.
- Final physical app suite: 311 tests in 44 suites passed in 72.842 seconds on iPad Air M2.
- Device and simulator ABI7 libraries rebuilt cleanly, with matching headers. Simulator evidence is compilation only. Final library and executable hashes are retained under the M6 evidence directory.

The final normal Release app ran three RAF sweeps and one NEF sweep without precision or output overrides. All were nominal and used native, owned Draft output. RAF presented medians were 34.7290, 34.6387 and 34.5776 ms; NEF was 34.6213 ms. The runs produced 1,799–1,800 Drafts per 30 seconds, with p95 50.9151–51.0851 ms. Logs and the executable hash are in `final-device/` and `final-app-sha256.json`. These results verify the combined final configuration, not an isolated E1 speedup or a zero-hitch claim.

An interim change selected half storage by default while discussing its quality tradeoff. After the user's decision, the production source was restored exactly to the f32 source used for those measurements. Clean iPad/simulator archives reproduce the prior hashes byte-for-byte; the app executable also matches the measured artifact. Fresh real-Metal tests pass for the default f32 storage policy and exact proxy readback/re-promotion. Identity and logs are recorded in `final-quality-restore-verification.json`, `final-quality-default-gpu.log` and `final-quality-default-engine.log`. The diagnostic paths and their evidence remain available; they are not selected in the normal app.

## Manual and operational work

The user reported regular spring return with no blocks during finger interactions, and stable RAF fit/100% exposure changes with sharp Full restoration. The trackpad-specific stale-pinch guard remains unverified because only fingers were available.

The requested main push was already up to date. A final remote read confirms both heads at `79be4e46f5880ce6bb83b2fb04043418043c3727`; new work has no commit and is not published. Seven obsolete worktrees and the empty `Unveil-worktrees` directory were removed after preserving archives and manifests. Only the canonical checkout remains. `probe-size-check.NEF` was deleted from the iPad and its absence verified.

Detailed commands, failed experiments, frozen sources and device logs remain in `.superpowers/sdd/2026-10-12-m6/`. Known limits remain explicit: no trackpad acceptance, no whole-process halving claim, no Lightroom quality comparison, no general 20% Draft speedup, and no closure of E4's original presentation-latency criterion. E2 is deliberately deferred by the user's precision requirement.

## E4 presentation follow-up on 2026-10-17

A fresh investigation reproduces the old serial 30-Hz comparison problem and identifies display-phase imbalance plus warmup contamination. A supplementary phase-balanced 10-Hz comparison on RAF/NEF, three repetitions each, measures Draft below Full in all 12 median and p95 comparisons. This supports the benefit within that controlled workload; it does not prove a continuous-input latency improvement or the under-25-ms goal. Three canvas variants regress ordinary 60-Hz latency or presentation throughput and were rejected. No new production optimization was integrated. See [E4 latency follow-up](e4-latency-followup.md) for methods, limits and retained evidence.
