# GPU reserve (M6 E10)

Measured on 2026-10-17 on iPad Air 11-inch M2, 8 GB, iPad14,8, iPadOS 27.0.1 (24A446).
Release uses default Rust CPU flags and f32 storage. Memory, lifetime, output and physical app
checks pass; sustained latency results and their presentation-phase limits follow.

The reusable GPU reserve has a byte limit of `min(engine budget / 8, 320 MiB)` while editing.
The 320 MiB limit accommodates the previously measured 304 MiB f32 window reserve; a separate
128 MiB experiment checks the cost of a smaller cap. This is not a limit on live GPU sources,
render stages, driver allocations or total process memory.

After the last render completes, the engine thread arms one three-second idle deadline.
New render work, photo replacement and suspension cancel the old deadline. The command loop
uses its existing bounded receive timeout; no recurring timer or main-actor GPU wait is added.
Read-only diagnostic traffic cannot indefinitely defer a due trim.

The deadline calls `trim_reusable(0)`, which releases only free pool entries. It does not call
the older `trim_pool(0)` operation that also releases the legacy shared source. Live original
handles, stage caches and buffers retained by unsubmitted commands remain owned and usable.
The existing window-release and suspend paths keep their stronger release semantics.

Budget reductions trim reusable bytes immediately, and subsequent buffer returns enforce the
current cap. Zero is valid. Tests render a second window after repeated zero/nonzero trims and
compare exact pixels, source bytes, stage bytes and source-upload count. Another test holds an
unsubmitted copy across a trim, then submits and reads it back before allowing its storage to
be reused. GPU tests run serially on actual Metal.

Pool diagnostics are disabled on normal launches. The opt-in collector has at most 128 size/
usage buckets and records allocation, reuse, return and free counts plus high-water bytes.
Its counters do not redefine device ownership. Diagnostic overrides require an enabled
diagnostic session and exist for paired experiments in one binary.

## Memory results

Three fresh-process/library runs per RAW, nominal thermal state, with the same E9 implementation.
MiB below are median [minimum–maximum]. E9-only retains the preceding pool policy.

| Moment | E9-only RAF | E9 + E10 RAF | E9-only NEF | E9 + E10 NEF |
|---|---:|---:|---:|---:|
| Fit +5 s footprint | 554.6 [554.0–579.0] | 418.0 [416.6–418.1] | 629.0 [629.0–638.6] | 577.7 [573.7–592.9] |
| Fit +15 s footprint | 516.2 [515.9–516.9] | 331.7 [331.6–331.9] | 569.7 [569.4–569.7] | 384.8 [384.7–385.3] |
| Initial idle fit reserve | 184.1 | 0 | 184.2 | 0 |
| Return to fit +10 s footprint | 321.6 [321.5–360.0] | 321.2 [321.2–323.3] | 338.5 [338.2–345.0] | 338.0 [337.8–348.3] |
| Lifetime peak footprint | 1465.1 [1457.4–1465.9] | 1365.6 [1365.5–1366.5] | 2006.9 [2006.6–2007.6] | 1908.0 [1905.2–1908.5] |

All six runs have zero pooled and retired bytes at fit +5/+15 s, settled 100%, and both returned-fit
points. The +15 s saving is about 185 MiB per RAW. Other CPU memory is still settling at +5 s,
so its process-footprint delta is smaller and variable even though the reserve is already empty.
The return-to-fit path already cleared the reserve before E10; no comparable extra saving is
claimed there. Integrated peaks include E9 and must not be attributed to E10 alone.

Three isolated RAF trims release 187.678 MiB of counted reserve and reduce footprint by
187.656 MiB each. Two valid initial NEF observations release 187.795 MiB and reduce footprint
by 173.563 and 187.781 MiB, within the 32 MiB attribution tolerance. Sources and stages remain
unchanged, and the first edit afterwards leaves the source-upload counter at one and causes
no decode. The third NEF observation is excluded from attribution: footprint falls 230.406 MiB
while 56.78 MiB of CPU internal memory also disappears (56 MiB in MALLOC_LARGE). Its log is
preserved. Its replacement releases the same 187.795 MiB and reduces footprint by 187.781 MiB,
with CPU internal memory changing only +0.047 MiB and compressed memory unchanged. There are
therefore three valid NEF attributions as well as the three RAF observations.

Probe Full presentation after refill is 62–102 ms versus 89–119 ms for the warm edit.
Each value represents one discrete drag/release after a hold, not a slider-sweep median.
The refill creates four small/medium buffers instead of re-uploading the original; the live
source and stages are unaffected. Sustained slider results are reported separately.

Three repeated fit/100% cycles on each RAW retain zero pooled/retired bytes at every settled
point. Returned-fit +30 s is 321.3/346.3/348.6 MiB RAF and 338.4/363.2/363.0 MiB NEF; the small
second-cycle rise is retained rather than described as perfectly flat memory. All returns
meet initial fit +100 MiB. The accepted run requires a fresh Full generation at every zoom;
an earlier diagnostic run that could accept stale metadata is preserved as invalid for timing.

## Measurement workloads

The physical comparison keeps E9 and E1/E3/E4/E5 fixed and leaves E2 disabled. It includes the
old roughly 341 MiB reserve with idle trimming disabled, the candidate, and the smaller fixed
cap. Thirty-second sweeps use identical request cadence in fit Draft, fit mixed Draft/Full,
and 100% window Draft configurations. Only presented latency is used for the headline;
overtaken requests and delivered counts are retained separately.

`-UnveilPoolProbe` disables the automatic deadline for an isolated experiment: settle, edit,
hold, take a memory map, trim free bytes, take another map, then edit again. This checks the
process-footprint delta as well as counters, live owners and first-refill cost. It is not the
same workload as a sustained slider sweep.

Evidence and exact commands are under `.superpowers/sdd/2026-10-12-m6/e910-implementation/`:
`run-probe.py`, `run-pool.py`, `run-memory.py`, `integrated-build.json` and their output folders.
The final task report includes +5/+15 s memory, isolated trim attribution, first-refill
latency, allocation/reuse counts, repeated cycles, output identity and the app test result.

## Interaction and final qualification

The initial matrix contains 36 nominal 30-second sweeps, followed by twelve reverse-order
repetitions targeting NEF fit and RAF window tails. All have the expected source-upload counts
and settled reserve below the selected cap. Thirty-five initial cases free no buffers during
the sweep; one NEF window performs a single cap-enforcement free. Startup allocations are
two versus four at fit/mixed and thirteen versus thirteen to seventeen in window mode. There
is no continuing allocate/free cycle on every edit.

Initial Draft medians at fit/window remain about 34 ms. Initial NEF fit p95 and some RAF Full
tails were slower, so the targeted repetitions were required rather than declaring a pass
from renderer CPU alone. The NEF repeat has Draft median/p95 33.952/51.032 ms current versus
33.990/50.891 ms original; the candidate p95 lies within the control range. The RAF repeat
has window Draft 33.893/50.437 versus 33.906/50.610 ms, and periodic fit-Full p95 68.043 versus
67.945 ms. One candidate run still has an 82.851 ms tail; it remains in the report.

The original harness settles for exactly three seconds, the same boundary as idle trimming.
One window sweep therefore began before trim. A final six-case NEF mixed comparison adds the
existing timeline's three-second hold before the common settle. Median-of-run Draft median/p95
is 32.676/49.387 ms current versus 32.627/49.048 ms original; Full is 32.984/49.757 versus
32.996/49.495 ms. Candidate median and p95 lie within the control ranges at the median of runs.
An individual candidate still switches to the roughly 49 ms median phase. Total presented
frames range 1789–1793 current versus 1791–1793 original, with a small shift in the Draft/Full mix.

These cohorts do not establish a universal latency improvement or eliminate the previously
accepted E4 presentation-phase limit. The earlier tail penalty is not consistent across the
independent repeats. All raw outcomes, delivered/overtaken counts, matched CPU/wall figures and
allocation/reuse buckets are retained, including the slower cases. See `e10-paired/cpu-table.md`,
each cohort's `presentation-table.md` and `task-e10-report.md`.

The final Release app is installed and launched normally. Its 25 PNGs match the existing iPad
references byte for byte, as do all 25 Mac outputs against their references. The precise
cross-platform maximum mean delta-E is 0.000014451, with p99 zero; those sparse differences
already exist in the references and were rounded to zero by the older report. No precision
reduction or new output difference is introduced. The physical app suite passes 312 tests in
44 suites, including exact pixels after resume. Release and Debug builds have no compiler
warnings. Final artifact identity is in `cycle-verified-build.json` and source in
`final-source-manifest.json` / `final-source.zip`.
