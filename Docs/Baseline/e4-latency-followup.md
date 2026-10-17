# E4 latency follow-up

Measured on 2026-10-17, physical iPad Air 11-inch M2, Release, f32, with the completed E9/E10 engine. **No new production optimization was accepted.** Three canvas experiments failed ordinary 60-Hz input checks and remain isolated. The app source and the production Rust libraries were not changed by this follow-up.

The original E4 presentation comparison has a display-phase confound. A corrected supplementary comparison supports Draft's benefit on the tested workloads, but it does not establish lower latency in every continuous-input case or satisfy the under-25-ms objective. E4's prior user acceptance remains unchanged; the request to improve continuous-input latency is still open.

## Reproduction and attribution

The original serial 30-Hz harness, rebuilt against current E9/E10 source, reproduces the problematic result: RAF window exposure Draft 53.326 ms versus Full 40.616 ms; fit texture 40.030 ms versus 39.838 ms. A second traced run reverses other comparisons without changing the renderer. Each two-second trial samples almost the same display phase repeatedly, while the next quality starts at another phase.

Separate generation-level timing measures request, publication, main-thread hop, drawable acquisition, canvas GPU work, actual display time and presented callback. In a traced serial RAF fit exposure control, render-to-publication is 10.47 ms, main-hop-to-acquisition 13.86 ms, acquisition-to-encoding 0.42 ms, commit-to-display 20.00 ms and display-to-callback 0.46 ms. Faster rendering can therefore be masked by a different wait for the next refresh.

The old cost window also included the final warmup frame: 61 ended intervals for 60 measured requests. The supplementary harness drains warmup before starting cost collection and requires exactly 60 intervals, 60 presented outcomes and 60 input records. It retains the historical callback-based SliderToFrame metric and separately records input-to-actual-presentation; it does not silently substitute one for the other.

## Rejected presentation changes

All changes below were in `/private/tmp/unveil-e4-latency`, disabled by default and absent from the canonical app.

| Variant | RAF fit median / p95 | Presented Drafts / 30 s | Decision |
|---|---:|---:|---|
| Existing canvas | 34.07 / 50.97 ms | 1,434 | Control |
| Draw immediately after publication at rest | 50.41 / 51.88 ms | 1,766 | Reject: latency regresses |
| Immediate, hold one reservation through presentation | 50.70 / 52.19 ms | 898 | Reject: presentation cadence halves; all 29 periodic Full intervals overtaken |
| Immediate, native two-drawable queue | 50.58 / 52.02 ms | 898 | Reject: same cadence and Full regression |

The unrestricted immediate variant also regresses NEF fit and both 100% windows to about 50 ms. Input/render cadence stays approximately 1800 Drafts/30 s, so reduced presentation cadence cannot be hidden behind render counts. GPU-end-to-display grows from 19.26 to 32.01 ms on the RAF fit comparison; GPU drawing remains about 0.6 ms and request-to-publication about 12 ms. Earlier submission alone queues more finished frames. Limiting the queue in either tested way introduces an unacceptable throughput loss. No fourth canvas policy was integrated or claimed.

## Supplementary comparison with balanced refresh phases

A diagnostic CAD link runs at 60 Hz. Every sixth tick starts a request opportunity; fifteen offsets from 0 to 14/900 seconds span one refresh, repeated four times for 60 changing edits. The 10-Hz cadence lets both qualities finish, including white balance. RAF and NEF each run three times; the second repetition starts with Full. Fit and 100% window use exposure, temperature and texture. This is a controlled serial quality comparison, **not the original 30-Hz workload or the ordinary 60-Hz coalesced input sweep**. Lower load can change CPU/GPU frequency and per-frame costs; absolute timings must not be compared across these workloads as a speedup.

All 72 trials are complete with nominal thermals. All 4,320 measured frames are presented, with no overtaken intervals. Each trial contains exactly four input samples at each of the fifteen planned phases. The largest difference between the paired median phase errors is 0.083 ms. Draft is below Full in 12/12 median comparisons and 12/12 p95 comparisons (median of three per-run statistics). Input-to-actual-presentation also favors Draft in 12/12 comparisons, excluding a request-timestamp-only explanation.

These are differences between Draft and Full in unchanged production behavior, **not before/after gains from a new optimization**.

| RAW | View / edit | Draft / Full median, ms | Draft / Full p95, ms | Draft / Full render CPU, ms |
|---|---|---:|---:|---:|
| RAF | fit / exposure | 40.28 / 48.70 | 48.78 / 56.17 | 3.33 / 12.66 |
| RAF | fit / temperature | 62.57 / 75.24 | 70.19 / 83.83 | 5.76 / 17.27 |
| RAF | fit / texture | 41.16 / 49.08 | 49.11 / 55.96 | 3.32 / 12.68 |
| RAF | window / exposure | 45.66 / 50.65 | 52.85 / 58.47 | 3.05 / 10.50 |
| RAF | window / temperature | 71.61 / 79.31 | 79.25 / 86.87 | 5.65 / 14.65 |
| RAF | window / texture | 45.10 / 50.40 | 52.21 / 57.76 | 3.18 / 10.53 |
| NEF | fit / exposure | 40.31 / 48.86 | 48.27 / 56.70 | 3.43 / 12.76 |
| NEF | fit / temperature | 62.43 / 75.55 | 69.66 / 83.90 | 5.71 / 17.19 |
| NEF | fit / texture | 40.05 / 49.13 | 47.81 / 55.65 | 3.22 / 12.76 |
| NEF | window / exposure | 45.80 / 49.61 | 53.44 / 57.97 | 3.17 / 10.19 |
| NEF | window / temperature | 71.76 / 79.70 | 79.81 / 86.78 | 5.59 / 14.71 |
| NEF | window / texture | 46.46 / 50.28 | 52.44 / 59.03 | 3.12 / 10.46 |

The original presentation-latency limitation should therefore be described with its measurement scope, rather than assuming the reduced renderer itself is slower. The supplementary result does not erase the original 30-Hz logs, establish universal per-frame superiority, retroactively pass the universal 20% proposal, or close the under 25 ms goal. Repeated ordinary controls remain around 34 ms median and 51 ms p95.

## Reproduction and retained state

Evidence: `.superpowers/sdd/2026-10-12-m6/e4-latency/`. `run.py` drives serial matched trials; `run-sweep.py` drives the real 60-Hz coalesced path. `run-balanced.py` records the six phase-balanced runs. `analyze-balanced.py` validates frame counts, phase coverage, thermal state and presentation outcomes, then writes the paired summaries. Exact commands, raw logs, executable hashes and archived diagnostic source are retained. The initial `baseline-matched-raf-1.log` is invalid: the normal app had no matched-harness launch handler; it produced no accepted cost result.

`balanced-diagnostic-source.zip` contains the diagnostic application source and matched harness. Link its `Frameworks` directory to the canonical f32 XCFramework before building; the matched flags do not exist in the normal app. The inactive canvas experiment switches are diagnostic only. `baseline-source-manifest.json` and the final source check distinguish canonical source from those experiments. No golden references, normal storage policy, staging or commits were changed. Prior image acceptance remains applicable to the unchanged app; no new Lightroom parity claim is made.
