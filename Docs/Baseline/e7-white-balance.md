# E7: Edited status after a JPEG white balance change

Measured on 2026-10-14 at 13:57:15 UTC on the arm64 macOS 27.0.1 host, Rust release build.
`ProcessInfo.processInfo.thermalState` returned 0 (nominal) before and after the run.

The production change is restricted to `DevelopSettings::is_unedited`: it ignores stored WB
numbers only in As Shot mode. JPEG Custom and preset WB changes now reach the catalog's `edited`
summary and filters. No pixel-processing source changed for this ticket.

## Correctness

Both new regressions failed before the production fix and passed afterward. The JPEG test imports
an actual 48 x 32 JPEG through `uv_execute`, then checks `catalog.query`'s summary and both edited
filters after independent temperature and tint changes, undo, redo, As Shot reset, Daylight and
full reset. This is the summary field read by the app's `EngineManager.isEdited`.

```sh
cd /Users/mac/Personal_Projects/Unveil/Engine
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo test -p lightcraft-develop -p lightcraft-catalog
cargo test -p unveil-ffi --features test-hooks --test white_balance
```

Result: 18 develop tests, 108 catalog tests and 1 JPEG FFI test passed; 1 existing catalog test
was ignored. No failures. RAW import defaults, old As Shot values and full reset remain covered
by the catalog suite.

## Isolated predicate timing

The previous `is_unedited` implementation is identical at v0 `fc020cd` and the pre-ticket main
`79be4e46f5880ce6bb83b2fb04043418043c3727`. The benchmark compiles that function body as the baseline
and calls the current crate's public `is_unedited` as the corrected side. Both wrappers are
`inline(never)`, and inputs and outputs pass through `std::hint::black_box`.

Each case warms both functions once, then records 21 rounds of 200,000 calls per side, alternating
which side runs first. The table contains the median and minimum ns/call across those rounds.
Summary statistics: [e7-white-balance.csv](e7-white-balance.csv).

| Settings | Baseline median ns | Corrected median ns | Baseline minimum ns | Corrected minimum ns |
|---|---:|---:|---:|---:|
| Default JPEG | 210.58 | 194.57 | 184.30 | 135.70 |
| JPEG with Custom WB at 8000 K | 174.81 | 140.81 | 141.76 | 106.89 |
| RAW As Shot at 5200 K, tint 4, import detail defaults | 176.33 | 167.60 | 131.36 | 123.77 |

No regression was observed in this isolated predicate measurement. These nanosecond results are
not app, render or import timings. Host scheduling/frequency were not pinned, and absolute times
varied between runs. An earlier run overlapping P1's Xcode build was discarded from the table;
this rerun started after the coordinator confirmed that build had finished.

The temporary Cargo runner uses a path dependency on the current `Engine/crates/develop` and was
built and invoked with:

```sh
PATH=/opt/homebrew/opt/rustup/bin:$PATH \
CARGO_TARGET_DIR=/Users/mac/Personal_Projects/Unveil/Engine/target \
cargo build --offline --release --manifest-path /tmp/unveil-e7-bench/Cargo.toml
/Users/mac/Personal_Projects/Unveil/Engine/target/release/unveil-e7-bench
xcrun swift -module-cache-path /tmp/unveil-e7-swift-cache \
  -e 'import Foundation; print(ProcessInfo.processInfo.thermalState.rawValue)'
```

The runner source and detailed red/green evidence are preserved in the working report directory
`.superpowers/sdd/2026-10-12-m6/` as `e7-benchmark.rs` and `task-e7-report.md`.

No iPad UI interaction, target-device timing, RAW export or golden regeneration was performed for
E7. The device remained reserved for P1. The existing optional craft-fonts build warning is
unrelated to this metadata change.
