# Hot data-structure map (T16, spec 6.4)

Every structure or buffer that a photo open or a slider drag touches in v0, with its type and place in the code, its
layout, element type, size, how often it is allocated, the share of time it costs in the profile, and what a low-level
redesign could change. It is the basis for the sub-project 2 proposals, read together with
[`profiling.md`](profiling.md), which holds the evidence behind the time shares.

## Conventions

- **(c)** computed from sizes in the code, **(m)** measured in the traces named in `profiling.md` (Time Profiler for
  CPU, Metal System Trace for GPU). Sizes are decimal, 1 MB = 10^6 bytes.
- **26 MP** is the RAF of the baseline set (Fujifilm X-T4, X-Trans, 6240 x 4160, 26.1 MP nominal); **46 MP** is the
  NEF (Nikon Z 7, Bayer, 8256 x 5504 image area, 45.7 MP nominal). Mosaic sizes use the nominal pixel counts (c).
- **Preview sizes**: 2048 px is the golden export (2048 x 1365 = 2.80 MP); 2360 px is the canvas preview on this iPad
  (`min(nativeBounds long edge, 2560)`, `Unveil/App/SceneDelegate.swift:58-60`; 2360 x 1573 = 3.71 MP). Both RAWs give
  the same output size at a given preview size (3:2).
- **Preview-level source**: any request up to 2560 px renders from the `SourceLevel::Preview` source
  (`engine/src/media.rs:89-104`), whose size depends on the camera, not on the request: the RAF bins 3x to
  2080 x 1386 (X-Trans cannot bin 2x) and the NEF bins 2x to 4128 x 2752 and is fitted to 2560 x 1707
  (`engine/src/files.rs:209-215`, `raw/src/binned.rs:21-28`) (c; the measured upload costs scale with this 1.52 area
  ratio (m)).
- **Time share**: per render as a share of the NEF render-thread CPU (9.80 ms) or of the GPU time per render
  (9.41 ms); per open in ms per decode. Paths are relative to `Engine/crates/` unless they start with `Engine/ffi` or
  `Unveil/`.

## 1. Open path (per decode)

A warm NEF open decodes three times (import probe, relink probe, first render), a cold one twice; the RAF is probed
without pixels and decoded once (profiling section 3).

| Structure | Type, file:line | Layout | Element, B/px | 26 MP (RAF) | 46 MP (NEF) | Allocated | Time share | Low-level redesign |
|---|---|---|---|---|---|---|---|---|
| File bytes | `Vec<u8>` from `std::fs::read`, `engine/src/files.rs:538` (render load), `:547` (probe) | packed container stream | u8; 1.07 / 1.28 B per pixel (m) | 27.9 MB (m) | 58.7 MB (m) | per read: 3 per warm open (import, relink, render), 2 cold | SipHash content hash 22 ms (RAF), 37 ms (NEF) per read (m) | map the file (`mmap`/`Data(contentsOf:options:.alwaysMapped)`), hash and decode once per open and keep the result |
| RAW mosaic | `RawData::U16(Vec<u16>)` in `RawImage`, `raw/src/lib.rs:343-344`, `:532-559`; filled by `raw/src/vendor/nefc.rs:186` (NEF), per-stripe `Vec<u16>` then copied into the mosaic at `raw/src/vendor/rafc.rs:95-99` (RAF) | planar, one CFA plane, row-major | u16, 2 B/px | 52.2 MB, plus 52.2 MB of stripe buffers (c) | 91.4 MB (c) | per decode | NEF: Huffman decode ~180 ms on one core + `white_from_data` ~75 ms per decode (m), 87 % of the import CPU; RAF: 1368 ms CPU over 8 cores (~200 ms wall) + 40 ms white level (m) | decode once; NEON `white_from_data` (scalar today because of the `u16 -> f32` filter, `raw/src/vendor/mod.rs:40-66`); decode RAF stripes in place, not into separate `Vec`s; write the mosaic into a shared `MTLBuffer` so binning can run on the GPU |
| Binned camera RGB | `Rgb32f` (`Image<[f32; 3]>`, `raster/src/lib.rs:48-54`) made at `raw/src/binned.rs:65`; per block-row scratch `Vec<f32>` at `:68` | interleaved RGB (AoS), row-major | f32, 12 B/px | 2080 x 1386: 34.6 MB (c) | 4128 x 2752: 136.3 MB (c) | per decode | NEF 321 ms CPU (~41 ms wall on 8 cores), RAF 179 ms CPU (m) | bin straight to the preview size (the NEF bins to 3.3x the pixels it keeps, then resizes); planar SoA or f16 for NEON; or bin on the GPU from the shared mosaic |
| Highlight rebuild grids | `cv: Vec<[f32; 3]>`, `cval: Vec<bool>`, `raw/src/highlight.rs:126-127` | coarse grid, interleaved | f32 x 3, bool | small (coarse grid) (c) | small (c) | per decode | NEF ~56 ms CPU, RAF ~13 ms (m) | fold into the binning pass |
| Colour transform | in place over the binned `Rgb32f`, `engine/src/files.rs:313-330` (matrix, DNG profile tables, camera hue/sat map) | interleaved | f32 | 0 extra | 0 extra | none | RAF 291 ms CPU (`HsvTable::apply`, `HueSat::apply`, `powf`), NEF 48 ms (m) | 3D LUT on the GPU or NEON with a precomputed table instead of per-pixel `powf` |
| Fit to the preview level | `resample::resize`, `raster/src/resample.rs:118-125`: temp `Image::new(w, src_h)` at `:124` plus the output; same size returns `img.clone()` at `:121` | interleaved | f32, 12 B/px | clone of 34.6 MB (same size) (c) | temp 2560 x 2752: 84.5 MB + out 52.4 MB (c) | per decode | NEF 55 ms CPU (m) | skip the identity clone (take ownership); fuse the fit into the binning |
| Embedded JPEG for the camera look | decoded by `files::decode_raw_preview` at a reduced size for 96 px proxies, `engine/src/camera_preview.rs:40`, `:67` | interleaved | u8 RGB | reduced (not sized here) | reduced (not sized here) | per decode | `fit_preview` 63 ms (RAF), 89 ms (NEF) per decode, on one core (m) | cache the fitted look per photo (it depends only on the file) |
| Preview-level source | `Arc<Rgb32f>` in `DecodedSource`, `engine/src/media.rs:167-175`; cached in `MediaCache.previews`, `media.rs:310` (insert `:520-535`, up to 4 photos within the cache share of the budget); passed to jobs as `SourceRef::Loaded` (an `Arc` clone, `media.rs:252`) | interleaved RGB, linear Rec.2020 | f32, 12 B/px | 2080 x 1386: 34.6 MB (c) | 2560 x 1707: 52.4 MB (c) | per photo (kept) | the same for 2048 and 2360 previews; its per-render upload is row "Source upload" below | allocate it once in a page-aligned shared `MTLBuffer` (`makeBuffer(bytesNoCopy:)`) so the GPU reads it where it is; f16 halves it (26.2 MB NEF); a texture would give hardware bilinear sampling to `sample_affine` |

## 2. Render path (per render, during a drag)

The v0 FFI attaches no stage cache (`Engine/ffi/src/session.rs:403-414` builds the job with `render_job`, whose
`stages` is `None`, `engine/src/media.rs:972`), so everything below is recomputed for every request; ~48 renders per
second during the sweep (m). Buffers made with `Gpu::buffer` (`gpu/src/ctx.rs:367-397`) come from a recycling pool
when one of a close size is free; `upload` and the readback staging buffer are always new.

| Structure | Type, file:line | Layout | Element, B/px | 26 MP (RAF) | 46 MP (NEF) | 2048 / 2360 preview | Allocated | Time share | Low-level redesign |
|---|---|---|---|---|---|---|---|---|---|
| Source upload (staging + device copy) | `Gpu::upload` (`device.create_buffer_init`), `gpu/src/ctx.rs:443-458`, called at `gpu/src/render.rs:596`, `:602` | flat 32-bit words of the interleaved source | f32, 12 B per source pixel | 34.6 MB staging + 34.6 MB device (c) | 52.4 MB + 52.4 MB (c) | independent of the preview size | per render, both new | CPU 6.49 ms NEF / 4.26 ms RAF per render (zero-fill 4.79, copy 1.52, other 0.18): **66 % of the render thread**, 39.8 % of all process CPU (m); GPU blit 1.35 / 0.96 ms (14 %) (m) | keep it resident (upstream `GpuStages`, `render.rs:21-25`, keeps a source up to 96 MB, `:418`); in a Metal-direct engine, no upload at all (shared buffer from decode time) |
| Sampled image | `Buf` from `cx.gpu.buffer(w * h * 3)`, `gpu/src/render.rs:511` (`sample`, `:493-532`) | interleaved RGB | f32, 12 B/px | as preview | as preview | 33.5 MB / 44.5 MB (c) | per render (pool) | `sample_affine` 1.43 ms NEF, 1.38 RAF (15 % of GPU) (m) | cache per view (the frame does not change during a tone drag); f16; sample through a texture (hardware bilinear); fuse with the next two passes |
| White-balanced image (`lin`) | `Buf`, `gpu/src/render.rs:753` (`linear`, `:737-766`) | interleaved RGB | f32, 12 B/px | as preview | as preview | 33.5 MB / 44.5 MB (c) | per render (pool) | `wb_k` 1.22 / 1.23 ms (13 % of GPU) (m) | fuse into the sampling pass (one write instead of two); f16; cache |
| Log-luminance plane | `Buf` of `n` floats, `gpu/src/render.rs:824` (`prepare`); the CPU twin is `Plane` (`Image<f32>`, `raster/src/lib.rs:56`); `base` aliases it when highlights and shadows are 0 (`:831-834`) | planar, one channel | f32, 4 B/px | as preview | as preview | 11.2 MB / 14.85 MB (c) | per render (pool) | `log_lum_k` 0.71 / 0.70 ms (7.5 % of GPU) (m) | compute in the fused pass; f16 (log values need little precision); cache |
| Optional spatial planes (base, clarity, texture, dark, chroma) | `Buf`s from `prepare`, `gpu/src/render.rs:817-865`, guided-filter temporaries of 2 channels (`:305-313`) | planar (chroma: interleaved 3) | f32, 4 B/px (chroma 12) | as preview | as preview | 11.2 / 14.85 MB each, temporaries 2x (c) | per render, only when the settings need them; **not allocated in the sweep** (exposure only: no such kernel in the trace (m)) | 0 in the sweep (m) | cache per radius (`GpuStages` does), f16, tiled guided filter in threadgroup memory |
| RGBA8 output on the GPU | `Buf` `out` from `cx.zeroed(n)`, `gpu/src/render.rs:652`, written by the `main` kernel in bands (`:658-679`) | interleaved RGBA8, packed in u32 | u8 x 4, 4 B/px | as preview | as preview | 11.2 MB / 14.85 MB (c) | per render (pool) + a GPU clear | `main` 4.17 ms NEF, 4.62 RAF (44 to 49 % of GPU) + clear 0.4 ms (m) | let `main` write into the shared buffer or texture the canvas samples (no clear, no readback); f16 output later for EDR |
| Parameter and aux buffers | per-dispatch storage buffer + bind group, `gpu/src/ctx.rs:504-514`; aux `render.rs:650` | words | u32 / f32 | bytes to KB | bytes to KB | bytes to KB | per dispatch (4 kernels per render in the sweep (m)) | parameter blits ~0.2 ms GPU; with the other submissions, ~3.5 ms CPU per render on the Metal dispatch threads (m) | one argument buffer or `setBytes` per encoder, one command buffer per render |
| Readback staging | `MAP_READ` buffer, `gpu/src/ctx.rs:533-538` | interleaved RGBA8 | 4 B/px | as preview | as preview | 11.2 MB / 14.85 MB (c) | per render, new | GPU blit 0.34 / 0.38 ms (3.6 % of GPU) (m) | disappears with a zero-copy output |
| RGBA8 result (`Rendered.image`) | `Rgba8` = `Image<[u8; 4]>` (`raster/src/lib.rs:55`), filled by `to_vec` at `gpu/src/ctx.rs:565` via `render.rs:688-689`; `Rendered`, `pipeline/src/lib.rs:170-176`; moved to the engine thread in a `Box` (`Engine/ffi/src/render_worker.rs:48-56`), passed to the callback as a pointer (`Engine/ffi/src/session.rs:56-73`) | interleaved RGBA8, row-major, stride `width * 4` | u8 x 4, 4 B/px | as preview | as preview | 11.2 MB / 14.85 MB (c) | per render, new | `to_vec` 1.12 / 1.15 ms + unwritten scan 0.35 / 0.30 ms + histogram 1.42 / 1.25 ms read it: ~2.9 ms, **30 % of the render thread** (m) | zero-copy output; check completeness with a GPU counter instead of a CPU scan |
| Histogram | `Histogram` (4 x `Vec<u32>` of 256), `raster/src/histogram.rs:8-35`, built at `gpu/src/render.rs:702-705` | 4 arrays | u32 | 4 KB (c) | 4 KB (c) | 4 KB (c) | per render | 1.42 ms NEF, 1.25 RAF (14 % of the render thread, 8.7 to 9.7 % of all process CPU) (m); v0 never returns it | skip it for previews, or compute it in `main` with threadgroup atomics |
| Job and settings | `RenderJob`, `engine/src/media.rs:652-676`, `Arc<DevelopSettings>`; settings hash via JSON serialization, `develop/src/lib.rs:49`, cached per photo in `SettingsHashes` (`media.rs:32-60`) | struct | | < 1 KB | < 1 KB | < 1 KB | per request | `build_job` 48 ms per minute, ~0.015 ms per request (m) | none needed |

## 3. Display path (fixed)

| Structure | Type, file:line | Layout | Element, B/px | 26 MP (RAF) | 46 MP (NEF) | 2048 / 2360 preview | Allocated | Time share | Low-level redesign |
|---|---|---|---|---|---|---|---|---|---|
| FrameSink buffers | 2 x `MTLBuffer` `.storageModeShared`, `Unveil/Core/Engine/FrameSink.swift:35`, allocated at `:57-73` for `maxPixels` 2560 (`Unveil/App/AppDelegate.swift:18`) | interleaved RGBA8, rows padded to `minimumLinearTextureAlignment(for: .rgba8Unorm)` (`:114`) | u8 x 4, 4 B/px | as preview | as preview | capacity 2 x 26.2 MB (2560 rows of 10,240 B each) (c); used per frame 11.2 MB / 14.85 MB (2360 x 4 = 9,440 B rows) (c) | once at launch; **outside the engine memory budget** (progress note T10.1/T15) | row copy `FrameSink.swift:151-165` on the engine thread: ~0.27 ms NEF, ~0.30 ms RAF per frame (m, sampled; 787 and 973 ms per minute) | make it the engine's render target (the engine writes, the canvas samples, no copy); a ring of 3 with in-use fences instead of 2 removes the tear risk too |
| Canvas textures | `MTLTexture` views over the sink buffers, `makeTexture(descriptor:offset:bytesPerRow:)`, `Unveil/Views/Editor/CanvasView.swift:342-354`, cached per buffer at `:51`, `:316-334` | linear texture, RGBA8 | 4 B/px | 0 extra (zero-copy) | 0 extra | 0 extra | rebuilt only when the frame geometry changes | encode ~0.07 ms per draw on main, draw 0.35 / 0.39 ms GPU (m) | sample the engine's output texture directly |
| Drawables | `CAMetalLayer` `bgra8Unorm`, `CanvasView.swift:38`, sized to the view in pixels at `:194-203` | 2D, BGRA8 | 4 B/px | | | 2360 x 1640: 15.5 MB each, up to 3 (CAMetalLayer default): 46.4 MB (c) | owned by the system, per frame acquire | in the draw above | `rgba16Float` doubles them if EDR comes |

## 4. Present upstream but unused in v0

| Structure | Type, file:line | Size if attached (NEF, 2360 px) | Why it matters |
|---|---|---|---|
| CPU stage cache | `StageCache`, `pipeline/src/lib.rs:220-320` (sampled, linear, planes; capacity 2 output sizes) | used only by the CPU path | carries the GPU stages as its extension |
| GPU stage cache | `GpuStages`, `gpu/src/render.rs:18-124` (sampled, `lin`, planes, the uploaded source up to 96 MB, `:418`) | 44.5 + 44.5 + 14.85 + 52.4 = ~156 MB device memory per view (c); RAF ~139 MB | `RenderJob::with_stages` (`engine/src/media.rs:731-734`) would turn an exposure drag into one `main` dispatch plus readback: target 1 of `profiling.md` |

## 5. Others found on the hot path

| Structure | Type, file:line | Size | Allocated | Time share | Low-level redesign |
|---|---|---|---|---|---|
| Catalog journal record | JSON-encoded `Op` appended per `develop.set`, `catalog/src/journal.rs:458`, written with `sync_data` at `catalog/src/store.rs:127-138` | hundreds of bytes (c) | per slider tick (60/s) | `Session::persist` 427 ms per minute of CPU plus unsampled fsync waits; `develop.set` 3.0 ms median, before each request (m) | coalesce to the end of a drag (one record per gesture) |
| GPU buffer pool | `Gpu::free`, `gpu/src/ctx.rs:228`, reuse within +25 % of the size at `:373-388`, cap = budget / 8 (`engine/src/memory.rs:86`) | cap ~310 MB at this budget (one third of ~7.4 GB available, divided by 8) (c) | grows as renders retire buffers | avoids reallocating `sampled`, `lin`, `log_l`, `out`; uploads and readback staging bypass it, `IOGPUResourceCreate` ~270 ms per minute (m) | a fixed per-view arena sized once for the preview (two or three renders in flight), no byte-limited pool |

## 6. Footprint check

The measured steady `phys_footprint` during the NEF sweep is about 657 MiB, peak 880 MiB at the open (m,
`measurements.md`). The buffers above account for most of it (c, an upper-level estimate, not a measurement): the
per-render GPU set while a render is in flight is ~238 MB (two 52.4 MB source copies, 2 x 44.5 MB images, 3 x 14.85 MB
RGBA8 and plane buffers), the recycling pool may hold up to ~310 MB more, the cached source 52.4 MB, the FrameSink
52.4 MB, the drawables up to 46.4 MB and the RGBA8 result 14.85 MB, on top of the app and its frameworks. The open
peak adds the decode set: 58.7 MB file + 91.4 MB mosaic + 136.3 MB binned image + 137 MB of fit buffers for the NEF
(c). Targets 1 and 3 of `profiling.md` remove the per-render upload pair and the readback pair, about 135 MB of
allocation churn per render on the NEF, and the FrameSink copy (c).
