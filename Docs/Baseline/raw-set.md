# Baseline RAW set and presets

The five RAW files the Mac and iPad renders are compared on. They live in `Baseline/raw/`, which git ignores.
All come from https://raw.pixls.us (CC0, public domain). The coordinator's download log records the file names, not
per-file URLs, so each row gives the site and the source file name.

## Files

| File in `Baseline/raw/` | Camera | Format | MP | Source file name on raw.pixls.us | sha256 |
|---|---|---|---|---|---|
| `DSCF0267.RAF` | Fujifilm X-T4 (X-Trans) | RAF | 26 | DSCF0267.RAF | `07ea642656074834ffe5c661870a612ffa2e80fc1218a05166ee966bbe5f6fae` |
| `ILCE-7M4_DSC06674_LossLess-Large.ARW` | Sony A7 IV | ARW, lossless large | 33 | ILCE-7M4_DSC06674_FullFrame-LossLess-Compressed-Large.ARW | `851b43c2116c4139104a5036f83ac3b6a148789b2142214dd7192c13972b25b6` |
| `Canon_EOS_R5_RAW_ISO_100_nocrop_nodual.CR3` | Canon EOS R5 | CR3 | 45 | Canon_EOS_R5_RAW_ISO_100_nocrop_nodual.CR3 | `21430c36387efe65bd09ac0fcd724bfd959d93dd391ec811a1b741c8663becb5` |
| `Z7-14bit-lossless-L.NEF` | Nikon Z 7 | NEF, 14-bit lossless | 45.7 | 3-Nikon-Z7-RAW-14bit-lossless-compressed-L.NEF | `f0e756dbfd00f3b71cb3d8aa5b9e2e1be32f1052a6c4380e09ebc7c5e7748114` |
| `iPhone12Pro_IMG_1361.DNG` | Apple iPhone 12 Pro (ProRAW) | DNG | 12 | IMG_1361.DNG | `e91e77a4533ed7cce551d83330676ea5c47dd5e55fb38adda7819366afdbdfc2` |

The RAF stands for the 24 MP class and the NEF for the 48 MP class (spec 3.2), so the "two around 24 MP" rule
of the brief is met only in that sense. One file is X-Trans (RAF), and all five formats are covered.

## Presets

Identical on Mac and iPad, copied by hand into `Engine/ffi/tests/golden.rs` and the Swift exporter.

| Preset | `develop.set` calls |
|---|---|
| `neutral` | none |
| `exposure` | `light.exposure` = +1.0 |
| `contrast` | `light.contrast` = +60 |
| `temperature` | `wb.temp` = 4000 |
| `tones` | `light.shadows` = +50, `light.highlights` = -50 |

## Render sequence

For each RAW, in this order, with the same parameters `EngineManager` sends:

1. `library.import` `{"paths":[<absolute path>],"mode":"add","onDeleted":"restore"}`. The id is `imported[0]`, else
   `restored[0]`, else `duplicates[0].existing`.
2. `library.select` `{"ids":[id],"active":id}`.
3. For each preset: `develop.reset {}` (the library keeps edits, so presets must not stack), then the preset's
   `develop.set` calls, then `uv_request_preview(2048, false)` (full render, not draft). Wait for a frame whose
   generation is at least the one returned.

## Output

RGB8 PNG, alpha dropped, named `<raw file name without extension>__<preset>.png`: 5 files by 5 presets, 25 PNGs.
Mac: `Baseline/golden/mac/`. iPad: `Baseline/golden/ipad/`. Compare with the `compare_golden` example
(mean CIEDE2000 at most 0.5, p99 at most 2.0, same dimensions).
