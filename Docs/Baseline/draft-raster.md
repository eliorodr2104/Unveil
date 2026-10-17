# E4 reduced display drafts

Accepted by the user on 2026-10-17 with the documented presentation-latency limit. Reduced display drafts remain enabled together with E5 mapped delivery. Implementation and image checks pass in f32; render CPU and wall time are lower than matched Full, while presented latency is not consistently lower. This closes E4 by explicit acceptance of the limitation, not by claiming that the original latency criterion passed. Half storage remains disabled. The measurements below were collected on 2026-10-16.

## Rendering and quality

`reduced_draft()` keeps the Full source level, native f32 color and spatial processing, current edits and logical geometry. Only the finished RGBA8 display region is area-averaged into `ceil(width / 2) × ceil(height / 2)` pixels. Early source reduction was rejected because it altered spatial effects. Final Full processing is unchanged.

Draft and Full share the native stage cache and resident source, with no second native stage-cache owner or source upload. The separate 512-pixel whole-photo histogram keeps its source, meaning and cadence. Fit histograms describe the delivered pixels.

ABI 7 separates physical width, height and stride from logical display dimensions. Swift uses logical dimensions for placement and zoom anchors, and physical dimensions for the texture. Reduced drafts use bilinear sampling; Full restores its native raster and placement. Tests cover odd extents, windows, crop halos and one-pixel edges.

The GPU reducer preserves row-major f32 area accumulation. Each 16×16 workgroup processes a 64×64 output tile and produces the exact four-channel histogram plus native-alpha and visited-pixel checks. CPU validation rejects malformed statistics, missing work, invalid alpha or precision failure before publication. Native output keeps its existing validation.

An optional unmasked finish kernel removes mask iteration only when the actual mask list is empty. It is eagerly compiled from the same shader source; compilation failure retains the generic kernel. No lazy compilation, approximate color math or P1 scheduler change was introduced.

## Image verification

- Current Apple M4 Metal checks pass the GPU suite, 60 RAW Draft/reference/restoration cases and 50 exact owned/mapped RAW comparisons. All 25 existing Mac Full PNGs remain byte-identical; references were not rewritten.
- The physical iPad Air M2 passes all 60 Full → Draft → Full cases across five RAW formats, six presets, fit and odd 901×703 windows. Draft matches an independent integer-area reference within one RGB byte. Alpha and logical geometry agree exactly; every restored Full is byte-exact. All 30 fit histograms and clipping counts are exact. Window histograms are deliberately not requested by this pixel harness; their cadence is covered separately.
- The canonical FFI suite reports 74 passed and two ignored cases. Fixture-specific RAW gates also ran explicitly. Coverage includes source residency, reduced dimensions, odd-window geometry, stale results and lifecycle.
- The user previously accepted stable placement and sharp Full restoration while changing exposure with fingers at fit and 100%. This observation does not verify trackpad input.

These checks preserve the app's f32 Full result on tested fixtures. They do not establish Lightroom parity or native-resolution equality of every temporary Draft pixel.

## Matched performance

Three runs each of RAF and NEF produced 72 trials: Draft/Full, exposure/white balance/texture, fit/window. Each delivered 60 changing requests with nominal thermals. Values are medians of three per-run metrics, not pooled frames. Positive percentages mean lower render-thread cost.

| RAW | View / edit | Render CPU saving | Render wall saving | Draft / Full presented median |
|---|---|---:|---:|---:|
| RAF | fit / exposure | 68.7% | 28.6% | 42.96 / 37.91 ms |
| RAF | fit / temperature | 46.3% | 14.3% | 62.62 / 65.65 ms |
| RAF | fit / texture | 55.7% | 28.1% | 40.08 / 45.76 ms |
| RAF | window / exposure | 65.8% | 22.6% | 51.88 / 46.92 ms |
| RAF | window / temperature | 44.9% | 11.2% | 70.94 / 72.13 ms |
| RAF | window / texture | 55.0% | 18.3% | 39.67 / 40.63 ms |
| NEF | fit / exposure | 55.5% | 32.3% | 40.34 / 47.82 ms |
| NEF | fit / temperature | 45.9% | 14.8% | 61.61 / 65.03 ms |
| NEF | fit / texture | 72.4% | 32.2% | 45.21 / 39.40 ms |
| NEF | window / exposure | 62.3% | 23.7% | 44.55 / 49.79 ms |
| NEF | window / temperature | 37.4% | 8.8% | 72.25 / 72.99 ms |
| NEF | window / texture | 64.8% | 15.6% | 41.94 / 39.22 ms |

The serial request harness targets 30 Hz and is not phase-locked to display ticks. Presentation phase varies. Four cells have slower Draft medians, so the original Draft-below-Full latency requirement is unproven. The later brief's universal 20% CPU/wall reduction is also unmet, as is M6's under-25-ms goal. White balance retains costly native upstream work to preserve quality. Render CPU savings are not whole-process CPU or energy savings.

Ordinary 60-Hz exposure sweeps are a separate workload. In 24 paired reduced-output runs, median-of-three mapped presentation was 33.90 ms RAF fit, 33.92 ms RAF window, 35.26 ms NEF fit and 34.03 ms NEF window. The first NEF fit pair remained around 50 ms in both ownership modes; repeats were faster. Individual results, presented/overtaken counts, p95 and footprint remain in the evidence. These medians do not establish a Draft-vs-Full latency win.

## Canonical Release confirmation

The final normal app, built from canonical source and launched without rendering overrides, completed eight 30-second runs with nominal thermals. All counted Draft outputs were mapped and physically reduced; normal storage remained f32. Three fit runs per RAW gave RAF presented medians of 34.155, 34.149 and 34.035 ms, and NEF medians of 33.946, 33.896 and 33.986 ms. One 100% window confirmation per RAW gave 33.968 and 34.019 ms. Fit runs rendered 1,799–1,801 Drafts; window runs rendered 1,796 and 1,791. P95 ranged from 50.572 to 51.646 ms. Presented/overtaken counts and costs are preserved in the raw logs.

This confirms the integrated artifact's ordinary sweep behavior; it does not replace the matched Full comparison or close E4's latency criterion. Evidence: `e45-resume/production-normal/summary.json`, `production-normal/app-sha256.json`, and `reviewed-framework-sha256.json`. Window repeat evidence belongs to the earlier 24-run paired diagnostic series; these two canonical window runs are confirmation only.

## Memory and remaining work

The 2360×1573 fit output becomes 1180×787: about 14.2 MiB becomes 3.5 MiB of RGBA8 payload. Readback and copied data shrink, but native stages remain and FrameSink retains three buffers sized for maximum Full output. Its allocated capacity is not quartered. No proportional process-memory saving is claimed.

The user accepted E4 activation on 2026-10-17; no further activation decision is pending. Further latency work must preserve f32, exact Full restoration, ownership, cadence and validation. Fused finish/reduction, histogram sharding, alternate workgroups, extra CPU scans and reordered separable sums were tested and rejected; none is in the production candidate.

Evidence: `.superpowers/sdd/2026-10-12-m6/e45-resume/production-gpu-raw.log`, `production-ffi.log`, `production-device-quality-summary.json`, `matched-final-summary.json`, `matched-final-trials.csv`, `clean-acceptance/summary.json` and source/library manifests. The matched harness used the archived one-bank diagnostic shader with the same clean two-pass policy; canonical-library checks are identified separately. The superseded report is preserved in `e45-resume/previous-reports/draft-raster.md`.

The final physical app suite passes 311 tests in 44 suites (Debug, 76.936 s). The latest reviewed canonical library also produces 25 iPad Full PNGs byte-identical to the existing iPad references. Evidence: `e45-resume/production-app-tests-debug.log` and `production-ipad-golden-verification.json`. The Release performance build is separate.

## E4 presentation follow-up on 2026-10-17

A fresh investigation reproduces the old serial 30-Hz comparison problem and identifies display-phase imbalance plus warmup contamination. A supplementary phase-balanced 10-Hz comparison on RAF/NEF, three repetitions each, measures Draft below Full in all 12 median and p95 comparisons. This supports the benefit within that controlled workload; it does not prove a continuous-input latency improvement or the under-25-ms goal. Three canvas variants regress ordinary 60-Hz latency or presentation throughput and were rejected. No new production optimization was integrated. See [E4 latency follow-up](e4-latency-followup.md) for methods, limits and retained evidence.
