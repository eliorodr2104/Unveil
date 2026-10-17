# Decode memory (M6 E9)

Measured on 2026-10-17 on iPad Air 11-inch M2, 8 GB, iPad14,8, iPadOS 27.0.1 (24A446).
Release builds use the default Rust CPU flags. P1, E1, E3, E4 and E5 are held constant;
normal rendering remains f32. These changes do not enable E2 half storage.

The E9 comparison holds the preceding GPU reserve policy constant. E10's additional idle
reclamation is measured separately in [GPU pool budget](gpu-pool-budget.md).

## Mechanism

Three lifetimes contributed to the first full-resolution zoom peak:

1. AHD collected every completed RGB tile before copying it to another whole-image allocation.
   It now writes into disjoint row bands as tiles complete. Tile arithmetic, the 128-pixel tile
   size and six-pixel halo are unchanged. The regression test bounds completed tiles by worker
   count and compares all four Bayer phases and partial image edges exactly.
2. A same-size `fit` cloned the completed RGB original. `fit_owned` consumes an already correctly
   sized image. Resizing still uses the existing filter.
3. The cold CPU original overlapped its exact GPU upload and the first render's scratch. Cold
   promotion now consumes the CPU owner after a successful exact upload, before scratch is
   allocated. GPU failure recovers exact pixels from the immutable original, or reloads the
   pinned original recipe. It does not publish lossy proxy pixels as an exact original.

Reducing live overlap alone did not return enough memory on the iPad. Selected large temporary
buffers therefore use explicit returnable storage: normalized mosaic, X-Trans green, demosaiced
RGB and highlight reconstruction planes. `raster::Buffer<T>` distinguishes adopted ordinary
vectors from vectors using the `sysmem::Returnable` allocator. No global allocator changes.

For Apple allocations at least 1 MiB and aligned within the actual system page size, the
allocator pairs an anonymous private mapping with unmapping on final deallocation. Other
layouts and other platforms use the system allocator. Routing depends only on layout; a
failed mapping returns an allocation error rather than silently changing the freeing route.
The vector owns initialized elements and drops them once. Arc and ordinary slice borrows
govern lifetime. The safe raster, RAW and GPU crates retain `forbid(unsafe_code)`.

Infallible image constructors retain ordinary vector allocation-failure behavior. This change
does not claim recoverable out-of-memory handling for every image operation. The explicit
vector's fallible reserve API covers overflow/allocation errors. The macOS-only allocator
pressure-relief helper remains best effort; selected iOS buffers no longer depend on it.

## Physical memory

Each row summarizes three fresh processes and libraries, all nominal thermal state. Values
are MiB, shown as median [minimum–maximum]. Peak is the kernel lifetime peak, not the largest
100-ms sample. Fit is the settled initial preview; return is ten seconds after zooming out.

| RAW / state | Unchanged control | E9, old GPU reserve policy |
|---|---:|---:|
| RAF lifetime peak | 2264.2 [2263.8–2267.8] | 1465.1 [1457.4–1465.9] |
| RAF initial fit | 516.4 [515.7–516.4] | 516.2 [515.9–516.9] |
| RAF return +10 s | 322.5 [321.5–323.2] | 321.6 [321.5–360.0] |
| NEF lifetime peak | 3740.1 [3730.2–3748.8] | 2006.9 [2006.6–2007.6] |
| NEF initial fit | 569.7 [569.4–569.8] | 569.7 [569.4–569.7] |
| NEF return +10 s | 341.2 [341.1–342.0] | 338.5 [338.2–345.0] |

All three E9 runs per RAW meet the 1536/2560 MiB peak limits and return below initial fit +100 MiB.
Initial fit medians did not rise. The RAF candidate maximum is 0.52 MiB above the control maximum;
the raw values remain available rather than rounding this away. Fit was not inflated to satisfy
the return-to-fit test.

On this OS the unchanged control also eventually returned below initial fit. E0's earlier
0.4–0.5 GiB retention is not reproduced in every fresh run on 27.0.1. The stable control peak,
intermediate failed candidates, live-capacity events and returned pages support the chosen
mechanism; they do not establish a universal allocator-retention constant.

The control's historical runner sampled fit around +18 s when timeline output was enabled;
the new runner samples at +5 and +15 s relative to its first Full frame. The returned-fit
points use the same +10/+30 s offsets. This timing difference is retained in the raw logs.

## Attribution and rejected intermediate results

The diagnostic-only control retains the original allocation behavior. A bounded event stream
tracks selected capacities, and a separate 100-ms sampler reads process footprint, internal,
compressed, Metal allocation and kernel lifetime peak. It does not walk VM regions per sample.
Detailed VM maps are taken only at the named memory points.

| Observation | Control | E9 |
|---|---:|---:|
| RAF scoped CPU capacity peak | 594.1 MiB | 546.0 MiB |
| NEF scoped CPU capacity peak | 1308.9 MiB | 794.5 MiB |
| RAF diagnostic kernel peak | 2263.8 MiB | 1457.1 MiB |
| NEF diagnostic kernel peak | 3748.3 MiB | 2006.2 MiB |

The RAF control's 594.1 MiB capacity event contains the RGB original and its identity-resize
clone. NEF's 1308.9 MiB event contains completed tiles, output RGB, normalized mosaic and sensor
samples. The candidate's NEF peak has bounded worker scratch instead of a second image of
completed tiles. Its RAF peak still includes output RGB, normalized mosaic, green and sensor
samples; the later upload no longer overlaps the CPU RGB original with render scratch.

Capacities are scoped observations, not a full heap census or physical-page accounting.
Handoff ends an observed scope without claiming the storage was freed. Some scope guards
end just before the corresponding local vector is destroyed. Timeline CSVs retain the time
difference to the nearest process sample; brief peaks are supplied by the kernel counter.
All four diagnostic traces report zero dropped events and no incomplete live map.

The diagnostic-only control's peaks (2263.8/3748.3 MiB) lie within the three unprofiled
control ranges. Its first windows (0.795/1.464 s) also lie within the control ranges
(0.782–0.898/1.263–1.696 s). This is a one-run-per-RAW overhead check, not proof of zero
instrumentation cost. Formal peak acceptance uses the unprofiled runs.

The named memory maps retain a separate physical accounting check: internal + compressed +
engine GPU + other Metal, excluding the FrameSink's reserved buffers from other Metal because
CPU-touched sink pages are already on the internal side. Its residual is -21.5 to +23.1 MiB
for the diagnostic control, -21.7 to +8.5 MiB for diagnostic E9, and -23.2 to +10.7 MiB for the
integrated E9/E10 runs (`memory-accounting.csv`). This residual and uninstrumented CPU allocations
remain explicit. Scoped pixel capacities cannot partition UIKit, caches, all decoder/driver
internals or allocator-retained pages, and are not subtracted from the lifetime peak to invent
an exact retained-malloc total.

Single-run intermediate candidates explain why all three changes are needed:

| Candidate | RAF peak / return +10 s | NEF peak / return +10 s |
|---|---:|---:|
| AHD bands and consuming fit only | 1967.9 / 718.3 MiB | 2716.2 / 875.6 MiB |
| Add selected returnable storage | 1737.7 / 324.1 MiB | 2505.5 / 338.5 MiB |
| Also consume cold CPU owner before rendering | 1457.1 / 321.8 MiB | 2006.2 / 341.6 MiB |

The first two candidates are not accepted final results. Their logs and source identities are
preserved under the M6 evidence directory.

## Output and ownership verification

All five full-resolution oriented camera-RGB fingerprints match the frozen control bit for bit
(RAF, NEF, ARW, CR3 and DNG). All 25 current Mac Full PNGs match the existing references byte for
byte. All 25 physical iPad PNGs also match their own existing references byte for byte.
Reference files were not regenerated. The cross-platform comparator passes all 25 pairs.
Its old four-decimal display rounded the mean to zero: at nine decimals the largest mean is
0.000014451, p99 is zero, and the largest single-pixel delta-E is 1.163544411. These sparse
differences already exist between the Mac/iPad references because each candidate is byte-identical
to its own reference. E9 introduces no new difference. The brief's literal mean-zero wording is
therefore not claimed at full precision; the stronger per-platform identity result is retained.

The physical app suite passes 312 tests in 44 suites in 75.671 seconds. The suspend/refusal
test now also checks exact Full pixels and dimensions after resume. It uses a generated PNG;
real-RAW suspend/recovery remains covered by the host FFI tests, separately from device zooms.

The final diagnostic build also completes three fit/100% cycles per RAW in one process.
RAF Full windows take 0.803/1.042/1.046 s; NEF 1.236/1.435/1.423 s. Sample-decode counters at
the windows are 1/2/3: the first zoom reuses prepared samples, while subsequent zooms follow
the existing full-source release and 30-second prepared-source expiry. No extra sample decode
is observed within a cycle. The later zooms are deliberately cold again. GPU source uploads are
one at fit and two/three/four cumulatively at the three windows, with no additional upload while
idle. Full CPU/GPU owners and window stages are zero after every return to fit.

The three-cycle lifetime peaks are 1432.3 MiB RAF and 1925.1 MiB NEF, still below their limits.
Returned-fit +30 s is 321.3/346.3/348.6 MiB RAF and 338.4/363.2/363.0 MiB NEF. The roughly
25 MiB rise after the second cycle is retained in the results; all returns stay below initial
fit +100 MiB, and the third cycle remains near the second. Every settled reserve and retired
count is zero. This is one three-cycle run per RAW, not a claim about arbitrarily long sessions.

The first cycle trial had a diagnostic bug: its waiter could accept the previous window's
Full metadata on later zooms. Those logs remain in `e10-cycles/` with an invalid-timing marker.
The corrected waiter requires a strictly newer generation, and `e10-cycles-fresh/` contains
the accepted repeat. Single-cycle runs had no earlier window and are unaffected.

Tests cover explicit backing, clone/borrow/drop lifetime, initialized-element destruction,
zero-size and over-aligned elements, capacity overflow, zero-copy vector adoption, orientation,
tile edges and exact recovery after an injected first-render failure. The latter observes
that the cold CPU owner is gone before render scratch, then verifies exact CPU fallback.

The relevant host runs report 515 engine passes (six ignored), 64 GPU passes (two ignored),
79 FFI passes (three ignored), 15 raster unit plus two storage passes, and seven sysmem passes.
The final RAW library run passes 147 tests with two ignored. The storage tests also exercise
growth and shrinkage across the mapping threshold in both directions.
The separate optional-half compatibility run passes 17 tests; it does not enable half in the
app. The broad RAW integration target still references a pre-existing absent
`Engine/docs/raf-corpus.sha256`; relevant RAW library and selected integration tests are run
explicitly instead. This is not reported as a passing complete RAW package suite.

## Reproduction and artifacts

Canonical evidence root: `.superpowers/sdd/2026-10-12-m6/e910-implementation/`.
The source baseline is a frozen pre-E9 checkout and manifest, not a reset of the dirty project.
Fixture hashes and dimensions are in [RAW set](raw-set.md).

- Control: `control-memory-trusted/`, `profile-control-memory/`, `e9-control-latency/`.
- E9-only candidate: `e9-owned-memory/`, `e9-owned-first-memory/`, source and archive manifests.
- Attribution: `allocation-attribution.json`, `control-{raf,nef}-timeline.csv`,
  `e9-{raf,nef}-timeline.csv`.
- Output: `native-pixels-comparison.json`, `mac-golden-comparison.json`.
- Integrated E9/E10 app and framework identity: `integrated-build.json`, `integrated-sources.json`.

The harness records each exact device command beside its immutable log. For a fresh output
directory and an already built app:

```sh
python3 .superpowers/sdd/2026-10-12-m6/e910-implementation/run-memory.py \
  /private/tmp/unveil-new-memory-run --app /absolute/path/Unveil.app --runs 3
```

`--profile` adds bounded decode observations; leave it off for the formal memory runs.
`--cycles 3 --runs 1` exercises repeated fit/full transitions. Device commands run serially.

## Sustained latency with the previous reserve policy

The unchanged control and E9 with the preceding GPU reserve policy each have three nominal
30-second sweeps per RAW and mode. Median-of-run Draft presentation and p95 are:

| RAW / mode | Control median / p95 | E9 median / p95 |
|---|---:|---:|
| RAF fit | 33.958 / 50.799 ms | 33.917 / 50.793 ms |
| RAF 100% | 33.937 / 50.554 ms | 33.974 / 50.578 ms |
| NEF fit | 33.891 / 50.795 ms | 33.849 / 50.782 ms |
| NEF 100% | 34.010 / 50.743 ms | 34.107 / 50.866 ms |

The paired runs retain request, presented and overtaken counts, render CPU/wall and process
CPU in `e9-latency-comparison.json`. Median delivered counts remain within the control range
or improve. Individual later runs have tails outside the earlier narrow three-run ranges;
these are retained, not discarded. The E10 same-binary comparisons and reverse-order repeats
investigate that variation separately. These results do not establish a universal speedup.
