# Equivalence: iPad vs Mac

Date: 2026-10-09. iPad Air 11" (M2), Release build at the relink fix on top of c82b505, engine on the GPU (Apple M2 GPU, Metal). Mac: M4, engine on the GPU (Apple M4, Metal). Five CC0 RAWs (see raw-set.md) under five presets, full renders at 2048 px, RGB8 PNG. Thresholds (spec 3.4): mean ΔE2000 <= 0.5 and p99 <= 2.0.

iPad export: `xcrun devicectl device process launch --terminate-existing --console --device <id> com.eliorodr2104.unveil -- -UnveilRun golden` (25 written, 0 failed, 18 s wall clock including launch). Comparison: `cargo run --manifest-path Engine/Cargo.toml -p unveil-ffi --release --example compare_golden -- Baseline/golden/mac Baseline/golden/ipad`.

Result: every pair has mean and p99 ΔE 0.0000; the maximum single-pixel ΔE is 0.54 to 1.16, below the threshold where a difference becomes visible. The two GPUs agree to the 8-bit output except for isolated pixels.

| file | preset | size | mean | p99 | max | verdict |
|---|---|---|---|---|---|---|
| Canon_EOS_R5_RAW_ISO_100_nocrop_nodual | contrast | 2048x1366 | 0.0000 | 0.0000 | 1.0860 | PASS |
| Canon_EOS_R5_RAW_ISO_100_nocrop_nodual | exposure | 2048x1366 | 0.0000 | 0.0000 | 0.9070 | PASS |
| Canon_EOS_R5_RAW_ISO_100_nocrop_nodual | neutral | 2048x1366 | 0.0000 | 0.0000 | 1.0530 | PASS |
| Canon_EOS_R5_RAW_ISO_100_nocrop_nodual | temperature | 2048x1366 | 0.0000 | 0.0000 | 0.9560 | PASS |
| Canon_EOS_R5_RAW_ISO_100_nocrop_nodual | tones | 2048x1366 | 0.0000 | 0.0000 | 1.0634 | PASS |
| DSCF0267 | contrast | 2048x1365 | 0.0000 | 0.0000 | 0.8485 | PASS |
| DSCF0267 | exposure | 2048x1365 | 0.0000 | 0.0000 | 0.6087 | PASS |
| DSCF0267 | neutral | 2048x1365 | 0.0000 | 0.0000 | 0.5950 | PASS |
| DSCF0267 | temperature | 2048x1365 | 0.0000 | 0.0000 | 0.5693 | PASS |
| DSCF0267 | tones | 2048x1365 | 0.0000 | 0.0000 | 1.0639 | PASS |
| ILCE-7M4_DSC06674_LossLess-Large | contrast | 2048x1366 | 0.0000 | 0.0000 | 0.8463 | PASS |
| ILCE-7M4_DSC06674_LossLess-Large | exposure | 2048x1366 | 0.0000 | 0.0000 | 0.5420 | PASS |
| ILCE-7M4_DSC06674_LossLess-Large | neutral | 2048x1366 | 0.0000 | 0.0000 | 0.9861 | PASS |
| ILCE-7M4_DSC06674_LossLess-Large | temperature | 2048x1366 | 0.0000 | 0.0000 | 0.8404 | PASS |
| ILCE-7M4_DSC06674_LossLess-Large | tones | 2048x1366 | 0.0000 | 0.0000 | 1.1062 | PASS |
| Z7-14bit-lossless-L | contrast | 2048x1366 | 0.0000 | 0.0000 | 1.0232 | PASS |
| Z7-14bit-lossless-L | exposure | 2048x1366 | 0.0000 | 0.0000 | 0.9666 | PASS |
| Z7-14bit-lossless-L | neutral | 2048x1366 | 0.0000 | 0.0000 | 1.1168 | PASS |
| Z7-14bit-lossless-L | temperature | 2048x1366 | 0.0000 | 0.0000 | 0.9000 | PASS |
| Z7-14bit-lossless-L | tones | 2048x1366 | 0.0000 | 0.0000 | 1.1008 | PASS |
| iPhone12Pro_IMG_1361 | contrast | 1536x2048 | 0.0000 | 0.0000 | 0.9063 | PASS |
| iPhone12Pro_IMG_1361 | exposure | 1536x2048 | 0.0000 | 0.0000 | 1.0629 | PASS |
| iPhone12Pro_IMG_1361 | neutral | 1536x2048 | 0.0000 | 0.0000 | 1.1560 | PASS |
| iPhone12Pro_IMG_1361 | temperature | 1536x2048 | 0.0000 | 0.0000 | 0.8591 | PASS |
| iPhone12Pro_IMG_1361 | tones | 1536x2048 | 0.0000 | 0.0000 | 1.0708 | PASS |

25 of 25 pairs pass (mean <= 0.5, p99 <= 2).
