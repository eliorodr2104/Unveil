# Baseline profiling report (T16, spec 6.3)

What the v0 engine spends its time on, on the iPad, while a slider is dragged and while a photo opens. It is the
evidence base for sub-project 2 (Metal-direct engine, unified memory, NEON, data layout, spec 1). Companion document:
[`data-structures.md`](data-structures.md) (every hot buffer with its size and cost). Latency numbers themselves are in
[`measurements.md`](measurements.md); this report explains them.

Every number is marked **(m)** measured (from a trace named in the text) or **(c)** computed (from sizes in the code or
from other measured numbers). Sizes are decimal: 1 MB = 10^6 bytes.

## Summary

1. **Half of every render re-uploads a source that never changes.** The FFI builds every preview job without a stage
   cache (`Engine/ffi/src/session.rs:403-414`, `stages: None`), so each render uploads the whole preview-level
   `Rgb32f` source (52.4 MB NEF, 34.6 MB RAF (c)) through `wgpu::Device::create_buffer_init`, which first zero-fills a
   fresh staging mapping and then copies into it. On the NEF that is 4.79 ms of `__bzero` + 1.52 ms of `memmove` per
   render on the render thread **(m)**, 39.8 % of all process CPU in the sweep **(m)**, plus 1.35 ms of GPU blit and
   then sampling, white balance and log-luminance passes that recompute the same pixels (3.4 ms GPU) **(m)**. Upstream
   already has the fix (`RenderJob::with_stages`, `GpuStages`): exposure drags would rerun only the `main` kernel.
2. **A render is ~20 ms, split evenly between CPU and GPU, never overlapped.** NEF: 9.8 ms CPU on `unveil-render` +
   9.4 ms GPU per render, 20.6 ms period, 48.5 renders/s **(m)**. Only ~2 ms of the CPU half is real work (`plan`,
   finish parameters); the rest is moving bytes (zero-fill, copies, readback, histogram, a scan for unwritten pixels).
3. **Draft is not cheaper than full on the engine side: it is the same render.** Same output size (2360 x 1573), same
   four kernels in every one of 2551 renders **(m)**; `draft()` only flips `Quality`, which changes nothing in an
   exposure drag (`pipeline/src/local.rs:271`). Latency is ~40 % render, ~15 % waiting behind the render in flight and
   ~45 % display path (hop to main, display-link tick, vsync and compositor), so it lands on 3 or 4 display frames for
   both qualities.
4. **The NEF is one frame slower only because its source is bigger.** Both RAWs render the same 2360 x 1573 output; the
   NEF's preview source is 2560 x 1707 (binned 2x then fitted) against 2080 x 1386 for the X-Trans RAF (binned 3x)
   (c). The extra 17.8 MB upload costs ~2.2 ms CPU and ~0.4 ms GPU per render **(m)**, which moves more requests past
   a vsync boundary.
5. **A warm NEF open decodes the file three times, serially, on one core.** `library.import` and `photo.relink` each
   run a full lossless NEF decode just to probe the file (`nef::decode` ignores the header-only mode), and the first
   render decodes it again: ~255 ms each (180 ms Huffman + 75 ms scalar `white_from_data`) out of 1086 ms **(m)**.
6. **Thermal: the sweep load (0.8 CPU cores + ~45 % GPU, ~35 GB/s of memory traffic (c)) reaches `serious` in under
   5 minutes.** The latency metric did not move at `serious` and neither did CPU time per frame (9.8 to 10.2 ms in all
   seven NEF sweeps **(m)**), but sub-project 2 must control thermal state in every comparison, and cutting the byte
   moving is also the main energy lever.
7. **The `apple-m1` A/B shows no difference in CPU per frame either** (9.94 against 10.10 ms median **(m)**): 75 % of
   the render thread runs in `libsystem_platform` memset/memcpy, which compiler flags do not touch.

Ranked targets for sub-project 2 (section 10): (1) keep the source and stages resident, ~11 ms of 20.6 ms per render;
(2) decode each RAW once per open, ~510 ms of a warm NEF open; (3) zero-copy output into shared memory, ~2.1 ms per
render and 82 MB of buffers; (4) GPU data layout and dispatch (fusion, f16, one command buffer), ~1.5 to 3 ms per
render; (5) drop or move the CPU histogram, ~1.4 ms per render.

## 1. Sources and method

| Item | Value |
|---|---|
| Device, OS, build | iPad Air 11-inch (M2), iPadOS 27.0 (24A437), Release, Mach-O UUID `369AA564-59AF-3DA3-8565-493522C96D86` (see `measurements.md` Setup) |
| Time Profiler traces | `Baseline/traces/time-48mp.trace` (NEF), `time-24mp.trace` (RAF): 85 s, 1 kHz sampling, running threads only (`all-thread-states NO`), each holding one warm open and one 60 s sweep |
| Metal System Trace | `Baseline/traces/metal-48mp.trace`, `metal-24mp.trace`: same scenario, GPU intervals per encoder (`metal-gpu-intervals`) |
| A/B traces | `Baseline/traces/ab/nef-{m1,default}-time-{1,2,3}.trace` (thermal `serious`) |
| Phases | from the `com.unveil` signposts in the same trace (`OSSignpostIntervals`): open = `OpenToFirstFrame`; import = the `ImportCopy` and `Command` intervals inside it; first render = the open's `SliderToFrame full`; sweep = first draft request to last frame |
| Preview size | `previewPixels = min(nativeBounds long edge, 2560)` = **2360** on this iPad (`Unveil/App/SceneDelegate.swift:58-60`); output 2360 x 1573 for both RAWs (3:2) (c) |

**Symbolication (what failed, what worked).**

- `xcrun xctrace symbolicate --input Baseline/traces/time-48mp.trace --output <scratch> --dsym Baseline/traced-build/Unveil.app.dSYM`
  failed: `Cannot resymbolicate this trace: No dSYMs were found or relevant to this trace.` The dSYM is the right one
  (`dwarfdump --uuid`: app and dSYM both `369AA564-...`). The cause is in every export's stderr:
  `Timeline modification failed -- dylibs overlap by 0x2124000: 0x1000ac000-0x1021d0000 ... Unveil.app/Unveil` twice.
  `--terminate-existing` relaunched the app at the same address, xctrace refused to add the image a second time, and
  the app's frames were stored as bare addresses with no binary, so there is nothing for the dSYM to match.
- What worked: the load address is in that same warning (0x1000ac000 for `time-48mp`, 0x102e80000 for `time-24mp`).
  The scratch script feeds every unique app address to `atos -i -o <dSYM>/Contents/Resources/DWARF/Unveil -arch arm64
  -l <load>` (2044 and 2089 addresses, inline frames from the line tables included). System frames are resolved with
  `atos -arch arm64e` against `~/Library/Developer/Xcode/iOS DeviceSupport/iPad14,8 27.0 (24A437)/arm64e/Symbols`, at
  each image's `load-addr` from the export. Rust v0 names (`_R...`) are demangled by Xcode's `c++filt`, Swift names by
  `swift-demangle`; `rustfilt` was not needed and not installed. Sanity check: the hot stacks read
  `RenderJob::run > develop > lightcraft_gpu::render > create_buffer_init > device_create_buffer > __bzero`, which is
  what the code does.
- `<deduplicated_symbol>` is a linker-folded function; its callers' inline frames place it (for example
  `render.rs:596`, the source upload).
- The A/B traces carry overlap warnings for other images, and xctrace symbolicated the app partly and inconsistently
  there, so section 9 uses thread names and signposts only, no symbols.

**Caveats.** At 1 kHz a sample is 1 ms; costs under a millisecond per render are estimated from how many renders a
sample hit (for example 787 hits in 2914 frame copies gives ~0.27 ms). Blocked time (waiting for the GPU, fsync) is not
sampled. The Metal System Trace slows the CPU side (42.5 renders/s against 48.5 for the NEF), so GPU kernel times come
from it, CPU times and render counts from the Time Profiler runs. The opens in these traces are **warm** (the library
already knows the photo, so `photo.relink` runs); cold opens were recorded only with the Logging template (no
samples), see section 3.

## 2. Where the time goes during the sweeps

60 s, 3600 draft requests at 60 Hz plus 60 full ones. Per thread **(m)**:

| Thread | NEF (`time-48mp`) ms | RAF (`time-24mp`) ms | What it does |
|---|---|---|---|
| `unveil-render` | 28564 | 23987 | `RenderJob::run`: the whole render, CPU side, and the waits on the GPU |
| Main | 7092 | 7051 | SwiftUI `AdjustmentPanel` updates 5291 / 5229 ms (the sweep moves a SwiftUI slider 60 times a second), `CanvasView` encode 212 / 213 ms (~0.07 ms per draw) |
| 5 unnamed dispatch threads | 10364 | 9360 | Metal submission and completion: `-[_MTLCommandQueue _submitAvailableCommandBuffers]` and IOGPU calls, 5932 / 6435 ms |
| `unveil-engine` | 1441 | 1526 | FrameSink row copy 787 / 973 ms; `develop.set` commands 589 / 495 ms, of which `Session::persist` (journal append + `sync_data`) 427 / 384 ms |
| Total process | 47469 (0.79 cores) | 41931 (0.70 cores) | |

Renders: 2914 in 60.0 s on the NEF (48.5/s, period 20.6 ms) and 3238 on the RAF (53.9/s, period 18.5 ms) **(m)**,
counted as separate upload bursts on `unveil-render`. They match the presented frames (2906 and 3233 (m)): nearly
every render reaches the screen.

### NEF sweep (`time-48mp`)

Window 60047 ms wall, 47469 ms CPU (47469 samples at 1 ms).

| # | Exclusive (leaf) | ms | % CPU |
|---|---|---|---|
| 1 | `__bzero` | 13970 | 29.4 |
| 2 | `_platform_memmove` | 8664 | 18.3 |
| 3 | `mach_msg2_trap` | 5026 | 10.6 |
| 4 | `<lightcraft_raster::histogram::Histogram>::of_srgb8` | 4152 | 8.7 |
| 5 | `(kernel, no user stack)` | 3113 | 6.6 |
| 6 | `lightcraft_gpu::render::render` | 1024 | 2.2 |
| 7 | `start_wqthread` | 799 | 1.7 |
| 8 | `iokit_user_client_trap` | 432 | 0.9 |
| 9 | `kevent_id` | 361 | 0.8 |
| 10 | `bool swift::RefCounts<swift::RefCountBitsT<(swift::RefCountInlinedness)1>>::doDecrementSlow<(swift::Perform...` | 263 | 0.6 |
| 11 | `<deduplicated_symbol>` | 246 | 0.5 |
| 12 | `swift::RefCounts<swift::RefCountBitsT<(swift::RefCountInlinedness)1>>::incrementSlow(swift::RefCountBitsT<(...` | 241 | 0.5 |
| 13 | `_platform_memset` | 209 | 0.4 |
| 14 | `objc_msgSend` | 183 | 0.4 |
| 15 | `write` | 182 | 0.4 |
| 16 | `powf` | 179 | 0.4 |
| 17 | `__fcntl` | 141 | 0.3 |
| 18 | `swift_getGenericMetadata` | 123 | 0.3 |
| 19 | `std::__1::pair<swift::GenericCacheEntry*, swift::MetadataResponse> swift::LockingConcurrentMap<swift::Gener...` | 119 | 0.3 |
| 20 | `_xzm_malloc_tc` | 99 | 0.2 |

| # | Inclusive | ms | % CPU |
|---|---|---|---|
| 1 | `<lightcraft_engine::media::RenderJob>::run` | 28555 | 60.2 |
| 2 | `lightcraft_gpu::render` (+1 callers with the same samples) | 28553 | 60.2 |
| 3 | `lightcraft_gpu::render::render` | 28539 | 60.1 |
| 4 | `<wgpu::api::device::Device as wgpu::util::device::DeviceExt>::create_buffer_init` | 14467 | 30.5 |
| 5 | `<wgpu::backend::wgpu_core::CoreDevice as wgpu::dispatch::DeviceInterface>::create_buffer` (+1 callers with the same samples) | 14315 | 30.2 |
| 6 | `<wgpu_core::global::Global>::device_create_buffer` | 14314 | 30.2 |
| 7 | `__bzero` | 13970 | 29.4 |
| 8 | `_platform_memmove` | 8664 | 18.3 |
| 9 | `__CFRunLoopDoObservers` | 5296 | 11.2 |
| 10 | `closure #1 in closure #1 in _UIHostingView.beginTransaction()` | 5269 | 11.1 |
| 11 | `ViewGraphRootValueUpdater.updateGraph<A>(body:)` | 5267 | 11.1 |
| 12 | `GraphHost.flushTransactions()` | 5266 | 11.1 |
| 13 | `ViewGraphRootValueUpdater._updateViewGraph<A>(body:)` | 5266 | 11.1 |
| 14 | `@objc closure #1 in static NSRunLoop.addObserver(_:)` | 5266 | 11.1 |
| 15 | `specialized static NSRunLoop.flushObservers()` | 5265 | 11.1 |
| 16 | `partial apply for closure #1 in ViewGraphRootValueUpdater.updateGraph<A>(body:)` | 5260 | 11.1 |
| 17 | `AG::Graph::UpdateStack::update()` | 5116 | 10.8 |
| 18 | `-[_MTLCommandQueue _submitAvailableCommandBuffers]` | 5073 | 10.7 |
| 19 | `-[IOGPUMetalCommandQueue submitCommandBuffers:count:]` | 5056 | 10.7 |
| 20 | `-[IOGPUMetalCommandQueue _submitCommandBuffers:count:]` | 5055 | 10.6 |

The anonymous leaves, attributed by their callers **(m)**: `__bzero` 13954 ms is wgpu zero-filling the staging
mapping in `device_create_buffer` under `create_buffer_init` from `render.rs:596` (the source upload). `_platform_memmove`
8664 ms is the source copy into that mapping (4437 ms), the readback `to_vec` at `render.rs:162`/`ctx.rs:565` (3265 ms)
and the FrameSink row copy on the engine thread (769 ms). `mach_msg2_trap` is 4643 ms of `IOGPUCommandQueueSubmitCommandBuffers`
on the Metal dispatch threads. `lightcraft_gpu::render::render` exclusive time is the inlined `unwritten` scan
(`render.rs:733`, 942 ms).

### RAF sweep (`time-24mp`)

Window 60044 ms wall, 41931 ms CPU (41931 samples at 1 ms).

| # | Exclusive (leaf) | ms | % CPU |
|---|---|---|---|
| 1 | `__bzero` | 10245 | 24.4 |
| 2 | `_platform_memmove` | 7894 | 18.8 |
| 3 | `mach_msg2_trap` | 5344 | 12.7 |
| 4 | `<lightcraft_raster::histogram::Histogram>::of_srgb8` | 4050 | 9.7 |
| 5 | `(kernel, no user stack)` | 1435 | 3.4 |
| 6 | `lightcraft_gpu::render::render` | 979 | 2.3 |
| 7 | `start_wqthread` | 917 | 2.2 |
| 8 | `iokit_user_client_trap` | 613 | 1.5 |
| 9 | `kevent_id` | 363 | 0.9 |
| 10 | `<deduplicated_symbol>` | 249 | 0.6 |
| 11 | `bool swift::RefCounts<swift::RefCountBitsT<(swift::RefCountInlinedness)1>>::doDecrementSlow<(swift::Perform...` | 235 | 0.6 |
| 12 | `_platform_memset` | 224 | 0.5 |
| 13 | `powf` | 223 | 0.5 |
| 14 | `swift::RefCounts<swift::RefCountBitsT<(swift::RefCountInlinedness)1>>::incrementSlow(swift::RefCountBitsT<(...` | 203 | 0.5 |
| 15 | `objc_msgSend` | 187 | 0.4 |
| 16 | `write` | 152 | 0.4 |
| 17 | `__fcntl` | 149 | 0.4 |
| 18 | `swift_getGenericMetadata` | 121 | 0.3 |
| 19 | `std::__1::pair<swift::GenericCacheEntry*, swift::MetadataResponse> swift::LockingConcurrentMap<swift::Gener...` | 114 | 0.3 |
| 20 | `<lightcraft_pipeline::tone::CameraTone>::apply` | 113 | 0.3 |

| # | Inclusive | ms | % CPU |
|---|---|---|---|
| 1 | `<lightcraft_engine::media::RenderJob>::run` | 23973 | 57.2 |
| 2 | `lightcraft_engine::media::develop` | 23970 | 57.2 |
| 3 | `lightcraft_gpu::render` | 23969 | 57.2 |
| 4 | `lightcraft_gpu::render::render` | 23935 | 57.1 |
| 5 | `<wgpu::api::device::Device as wgpu::util::device::DeviceExt>::create_buffer_init` | 10751 | 25.6 |
| 6 | `<wgpu::backend::wgpu_core::CoreDevice as wgpu::dispatch::DeviceInterface>::create_buffer` (+1 callers with the same samples) | 10627 | 25.3 |
| 7 | `<wgpu_core::global::Global>::device_create_buffer` | 10626 | 25.3 |
| 8 | `__bzero` | 10245 | 24.4 |
| 9 | `_platform_memmove` | 7894 | 18.8 |
| 10 | `-[_MTLCommandQueue _submitAvailableCommandBuffers]` | 5473 | 13.1 |
| 11 | `-[IOGPUMetalCommandQueue submitCommandBuffers:count:]` | 5460 | 13.0 |
| 12 | `-[IOGPUMetalCommandQueue _submitCommandBuffers:count:]` | 5459 | 13.0 |
| 13 | `IOGPUCommandQueueSubmitCommandBuffers` | 5439 | 13.0 |
| 14 | `mach_msg2_trap` | 5344 | 12.7 |
| 15 | `__CFRunLoopDoObservers` | 5226 | 12.5 |
| 16 | `IOConnectCallMethod` | 5219 | 12.4 |
| 17 | `io_connect_method` | 5216 | 12.4 |
| 18 | `closure #1 in closure #1 in _UIHostingView.beginTransaction()` | 5212 | 12.4 |
| 19 | `ViewGraphRootValueUpdater.updateGraph<A>(body:)` | 5210 | 12.4 |
| 20 | `ViewGraphRootValueUpdater._updateViewGraph<A>(body:)` | 5208 | 12.4 |

Same picture, smaller upload: `__bzero` 10245 ms and source `memmove` 3045 ms for a 34.6 MB source.

## 3. Opens

The two Time Profiler traces each start with a **warm** open (the library knew the photo): NEF 1086 ms, RAF 511 ms
**(m)**, in line with the warm medians of `measurements.md` (1089.9 and 517.0 ms). Timeline of the NEF open in 20 ms
buckets (samples of all threads; `openline.py`) **(m)**:

| Open time (ms) | What runs | Thread(s) |
|---|---|---|
| 0 to 80 | ImportCopy (APFS clone, ~5 ms), content hash (SipHash of the 58.7 MB file) | engine |
| 80 to 340 | `library.import` probe: full `nefc::decode` (Huffman) ~180 ms, then `white_from_data` ~75 ms | engine, one core |
| 340 to 620 | `photo.relink`: content hash again, the same full decode and `white_from_data` again | engine, one core |
| 640 to 890 | first render, source load: the same decode a third time | one rayon worker |
| 890 to 940 | camera-look fit (`fit_preview`: embedded JPEG decode, proxies) | one rayon worker |
| 940 to 1030 | `develop_binned` (2x bin of the mosaic), highlight rebuild, colour matrix, fit to 2560 | 8 rayon workers |
| 1040 to 1086 | GPU render (upload, kernels, readback), frame delivery, present | render, engine, main |

There is **no demosaic** on this path: preview-level sources bin the mosaic straight to size (`files.rs:288-292`); AHD
runs only for levels above 2560 px. The RAF timeline: content hash (60 ms, no pixel probe for RAF), then the first
render: `rafc` decode on all 8 cores (80 to 300 ms, 1368 ms CPU), `white_from_data` 40 ms, camera-look fit 50 ms,
bin + colour 80 ms, GPU and present ~40 ms **(m)**.

Cold opens (`measurements.md` section A, Logging template, no samples) differ by not running `photo.relink`: NEF cold
865.5 ms = import 344.1 + first render 512.1, which is two decodes instead of three (c).

### NEF open, import commands (`ImportCopy`, `library.import`, `photo.relink`)

Window 630 ms wall, 579 ms CPU (579 samples at 1 ms).

| # | Exclusive (leaf) | ms | % CPU |
|---|---|---|---|
| 1 | `lightcraft_raw::vendor::nefc::decode` | 363 | 62.7 |
| 2 | `lightcraft_raw::vendor::white_from_data` | 138 | 23.8 |
| 3 | `<siphasher::sip128::Hasher<siphasher::sip128::Sip13Rounds> as core::hash::Hasher>::write` | 37 | 6.4 |
| 4 | `(kernel, no user stack)` | 21 | 3.6 |
| 5 | `_platform_memmove` | 8 | 1.4 |
| 6 | `stat` | 3 | 0.5 |
| 7 | `read` | 2 | 0.3 |
| 8 | `lstat` | 1 | 0.2 |
| 9 | `fstatat` | 1 | 0.2 |
| 10 | `clonefileat` | 1 | 0.2 |
| 11 | `_kernelrpc_mach_vm_deallocate_trap` | 1 | 0.2 |
| 12 | `<lightcraft_tiff::reader::Ctx>::ifd` | 1 | 0.2 |
| 13 | `_xzm_free_tc` | 1 | 0.2 |
| 14 | `madvise` | 1 | 0.2 |

| # | Inclusive | ms | % CPU |
|---|---|---|---|
| 1 | `<lightcraft_engine::Session>::execute` (+1 callers with the same samples) | 554 | 95.7 |
| 2 | `lightcraft_engine::files::fs_hooks::{closure#1}` (+1 callers with the same samples) | 552 | 95.3 |
| 3 | `lightcraft_engine::files::probe_bytes` | 550 | 95.0 |
| 4 | `lightcraft_raw::vendor::nef::decode` (+1 callers with the same samples) | 502 | 86.7 |
| 5 | `lightcraft_raw::vendor::nefc::decode` | 364 | 62.9 |
| 6 | `lightcraft_engine::import::import_with` (+1 callers with the same samples) | 281 | 48.5 |
| 7 | `<lightcraft_engine::import::ImportJob>::prepare_files` | 280 | 48.4 |
| 8 | `lightcraft_engine::cmd::missing::relink` | 273 | 47.2 |
| 9 | `lightcraft_raw::vendor::white_from_data` | 138 | 23.8 |
| 10 | `<siphasher::sip128::Hasher<siphasher::sip128::Sip13Rounds> as core::hash::Hasher>::write` (+1 callers with the same samples) | 37 | 6.4 |
| 11 | `_platform_memmove` | 8 | 1.4 |
| 12 | `lightcraft_meta::extract` | 7 | 1.2 |
| 13 | `PhotoImporter.importCopy(of:)` (+4 callers with the same samples) | 4 | 0.7 |
| 14 | `stat` | 3 | 0.5 |
| 15 | `specialized static _FileOperations.linkOrCopyFile<A>(_:dst:with:delegate:)` (+2 callers with the same samples) | 3 | 0.5 |
| 16 | `read` (+1 callers with the same samples) | 2 | 0.3 |
| 17 | `_xzm_segment_group_alloc_huge_chunk` (+1 callers with the same samples) | 2 | 0.3 |
| 18 | `lightcraft_meta::container::embedded` | 2 | 0.3 |
| 19 | `@objc _NSFileManagerBridge.fileExists(atPath:)` (+6 callers with the same samples) | 1 | 0.2 |
| 20 | `lstat` | 1 | 0.2 |

### NEF open, first full render (source load and GPU render)

Window 453 ms wall, 942 ms CPU (942 samples at 1 ms).

| # | Exclusive (leaf) | ms | % CPU |
|---|---|---|---|
| 1 | `<&<lightcraft_raw::RawImage>::develop_binned::{closure#3} as core::ops::function::FnMut<((usize, &mut [[f32...` | 321 | 34.1 |
| 2 | `lightcraft_raw::vendor::nefc::decode` | 180 | 19.1 |
| 3 | `lightcraft_raw::vendor::white_from_data` | 69 | 7.3 |
| 4 | `<&lightcraft_raster::par_rows<[f32; 3], <lightcraft_raster::Image<[f32; 3]>>::map_in_place<lightcraft_engin...` | 48 | 5.1 |
| 5 | `<jpeg_decoder::decoder::Decoder<&[u8]>>::decode_scan` | 42 | 4.5 |
| 6 | `<&lightcraft_raw::highlight::downsample<lightcraft_raw::highlight::reconstruct::{closure#2}>::{closure#0} a...` | 40 | 4.2 |
| 7 | `_platform_memset_pattern16` | 37 | 3.9 |
| 8 | `<&lightcraft_raster::par_rows<[f32; 3], lightcraft_raster::resample::resize<[f32; 3]>::{closure#0}>::{closu...` | 32 | 3.4 |
| 9 | `<&lightcraft_raster::par_rows<[f32; 3], lightcraft_raster::resample::resize<[f32; 3]>::{closure#1}>::{closu...` | 23 | 2.4 |
| 10 | `swtch_pri` | 20 | 2.1 |
| 11 | `<&lightcraft_raw::highlight::reconstruct::{closure#4} as core::ops::function::FnMut<((usize, &mut [[f32; 3]...` | 16 | 1.7 |
| 12 | `madvise` | 15 | 1.6 |
| 13 | `rayon::iter::plumbing::bridge_producer_consumer::helper::<rayon::slice::IterProducer<[f32; 3]>, rayon::iter...` | 15 | 1.6 |
| 14 | `(kernel, no user stack)` | 10 | 1.1 |
| 15 | `_platform_memmove` | 7 | 0.7 |
| 16 | `<jpeg_decoder::huffman::HuffmanDecoder>::decode::<&[u8]>` | 6 | 0.6 |
| 17 | `__bzero` | 5 | 0.5 |
| 18 | `mach_msg2_trap` | 4 | 0.4 |
| 19 | `<jpeg_decoder::worker::rayon::ImmediateWorker>::append_row_locked` | 4 | 0.4 |
| 20 | `<&lightcraft_engine::camera_preview::fit_hue_sat::{closure#2} as core::ops::function::FnMut<(usize,)>>::cal...` | 4 | 0.4 |

| # | Inclusive | ms | % CPU |
|---|---|---|---|
| 1 | `lightcraft_engine::files::load_bytes_now` | 398 | 42.3 |
| 2 | `<&<lightcraft_raw::RawImage>::develop_binned::{closure#3} as core::ops::function::FnMut<((usize, &mut [[f32...` | 331 | 35.1 |
| 3 | `lightcraft_raw::vendor::nef::decode` | 250 | 26.5 |
| 4 | `lightcraft_raw::vendor::nefc::decode` | 181 | 19.2 |
| 5 | `lightcraft_engine::camera_preview::fit_preview` | 89 | 9.4 |
| 6 | `lightcraft_engine::camera_preview::proxies` | 82 | 8.7 |
| 7 | `lightcraft_raw::vendor::white_from_data` | 69 | 7.3 |
| 8 | `lightcraft_engine::files::decode_raw_preview` | 63 | 6.7 |
| 9 | `lightcraft_codecs::jpeg::decode_with_fallback` (+1 callers with the same samples) | 62 | 6.6 |
| 10 | `<jpeg_decoder::worker::WorkerScope>::get_or_init_worker::<core::result::Result<(core::option::Option<jpeg_d...` (+2 callers with the same samples) | 60 | 6.4 |
| 11 | `<jpeg_decoder::decoder::Decoder<&[u8]>>::decode_scan` | 59 | 6.3 |
| 12 | `<rayon::iter::plumbing::bridge::Callback<rayon::iter::for_each::ForEachConsumer<<lightcraft_raw::RawImage>:...` (+1 callers with the same samples) | 50 | 5.3 |
| 13 | `<&lightcraft_raster::par_rows<[f32; 3], <lightcraft_raster::Image<[f32; 3]>>::map_in_place<lightcraft_engin...` | 48 | 5.1 |
| 14 | `<&lightcraft_raw::highlight::downsample<lightcraft_raw::highlight::reconstruct::{closure#2}>::{closure#0} a...` (+1 callers with the same samples) | 40 | 4.2 |
| 15 | `_platform_memset_pattern16` | 37 | 3.9 |
| 16 | `<&lightcraft_raster::par_rows<[f32; 3], lightcraft_raster::resample::resize<[f32; 3]>::{closure#0}>::{closu...` | 32 | 3.4 |
| 17 | `lightcraft_engine::files::load_bytes_now::{closure#11}` | 28 | 3.0 |
| 18 | `<&lightcraft_raster::par_rows<[f32; 3], lightcraft_raster::resample::resize<[f32; 3]>::{closure#1}>::{closu...` | 23 | 2.4 |
| 19 | `swtch_pri` | 20 | 2.1 |
| 20 | `<&lightcraft_raw::highlight::reconstruct::{closure#4} as core::ops::function::FnMut<((usize, &mut [[f32; 3]...` | 16 | 1.7 |

Of the 942 ms of CPU in this window, `load_bytes_now` (decode, bin, colour, fit) is the critical path at 398 ms on one
thread plus the parallel bin and colour work; the GPU render's CPU side is under 15 ms **(m)**.

### RAF open, import commands

Window 78 ms wall, 43 ms CPU (43 samples at 1 ms).

| # | Exclusive (leaf) | ms | % CPU |
|---|---|---|---|
| 1 | `<siphasher::sip128::Hasher<siphasher::sip128::Sip13Rounds> as core::hash::Hasher>::write` | 22 | 51.2 |
| 2 | `(kernel, no user stack)` | 12 | 27.9 |
| 3 | `read` | 2 | 4.7 |
| 4 | `mkdirat` | 1 | 2.3 |
| 5 | `lstat` | 1 | 2.3 |
| 6 | `clonefileat` | 1 | 2.3 |
| 7 | `swift_dynamicCast` | 1 | 2.3 |
| 8 | `lightcraft_tiff::reader::parse` | 1 | 2.3 |
| 9 | `lightcraft_raw::probe_info` | 1 | 2.3 |
| 10 | `write` | 1 | 2.3 |

| # | Inclusive | ms | % CPU |
|---|---|---|---|
| 1 | `<lightcraft_engine::Session>::execute` (+1 callers with the same samples) | 27 | 62.8 |
| 2 | `lightcraft_engine::files::fs_hooks::{closure#1}` (+1 callers with the same samples) | 26 | 60.5 |
| 3 | `lightcraft_engine::files::probe_bytes` | 24 | 55.8 |
| 4 | `<siphasher::sip128::Hasher<siphasher::sip128::Sip13Rounds> as core::hash::Hasher>::write` (+1 callers with the same samples) | 22 | 51.2 |
| 5 | `<lightcraft_engine::import::ImportJob>::prepare_files` (+2 callers with the same samples) | 15 | 34.9 |
| 6 | `lightcraft_engine::cmd::missing::relink` | 11 | 25.6 |
| 7 | `PhotoImporter.importCopy(of:)` (+4 callers with the same samples) | 3 | 7.0 |
| 8 | `specialized static _FileOperations.linkOrCopyFile<A>(_:dst:with:delegate:)` (+2 callers with the same samples) | 2 | 4.7 |
| 9 | `read` (+1 callers with the same samples) | 2 | 4.7 |
| 10 | `mkdirat` (+7 callers with the same samples) | 1 | 2.3 |
| 11 | `lstat` | 1 | 2.3 |
| 12 | `clonefileat` | 1 | 2.3 |
| 13 | `swift_dynamicCast` (+16 callers with the same samples) | 1 | 2.3 |
| 14 | `lightcraft_tiff::reader::parse` (+4 callers with the same samples) | 1 | 2.3 |
| 15 | `lightcraft_raw::probe_info` | 1 | 2.3 |
| 16 | `write` (+4 callers with the same samples) | 1 | 2.3 |

### RAF open, first full render

Window 428 ms wall, 2065 ms CPU (2065 samples at 1 ms).

| # | Exclusive (leaf) | ms | % CPU |
|---|---|---|---|
| 1 | `<&lightcraft_raw::vendor::rafc::decode::{closure#10} as core::ops::function::FnMut<(&lightcraft_raw::vendor...` | 1368 | 66.2 |
| 2 | `<&<lightcraft_raw::RawImage>::develop_binned::{closure#3} as core::ops::function::FnMut<((usize, &mut [[f32...` | 179 | 8.7 |
| 3 | `<lightcraft_raw::profile::HsvTable>::apply` | 133 | 6.4 |
| 4 | `powf` | 56 | 2.7 |
| 5 | `<lightcraft_raw::profile::HsvTable>::lookup` | 50 | 2.4 |
| 6 | `<lightcraft_engine::camera_preview::HueSat>::apply` | 42 | 2.0 |
| 7 | `lightcraft_raw::vendor::white_from_data` | 41 | 2.0 |
| 8 | `<jpeg_decoder::decoder::Decoder<&[u8]>>::decode_scan` | 31 | 1.5 |
| 9 | `swtch_pri` | 17 | 0.8 |
| 10 | `_platform_memmove` | 14 | 0.7 |
| 11 | `<&lightcraft_raw::highlight::downsample<lightcraft_raw::highlight::reconstruct::{closure#2}>::{closure#0} a...` | 13 | 0.6 |
| 12 | `_platform_memset` | 12 | 0.6 |
| 13 | `_platform_memset_pattern16` | 10 | 0.5 |
| 14 | `_xzm_xzone_malloc_tiny` | 8 | 0.4 |
| 15 | `(kernel, no user stack)` | 7 | 0.3 |
| 16 | `jpeg_decoder::decoder::color_convert_line_ycbcr` | 6 | 0.3 |
| 17 | `madvise` | 6 | 0.3 |
| 18 | `<&lightcraft_raster::par_rows<[f32; 3], <lightcraft_raster::Image<[f32; 3]>>::map_in_place<lightcraft_engin...` | 6 | 0.3 |
| 19 | `<jpeg_decoder::huffman::HuffmanDecoder>::read_bits::<&[u8]>` | 5 | 0.2 |
| 20 | `<jpeg_decoder::huffman::HuffmanDecoder>::decode::<&[u8]>` | 4 | 0.2 |

| # | Inclusive | ms | % CPU |
|---|---|---|---|
| 1 | `<alloc::vec::Vec<alloc::vec::Vec<u16>> as alloc::vec::spec_extend::SpecExtend<alloc::vec::Vec<u16>, core::i...` | 1408 | 68.2 |
| 2 | `<&lightcraft_raw::vendor::rafc::decode::{closure#10} as core::ops::function::FnMut<(&lightcraft_raw::vendor...` | 1397 | 67.7 |
| 3 | `lightcraft_engine::files::load_bytes_now` | 336 | 16.3 |
| 4 | `<&lightcraft_raster::par_rows<[f32; 3], <lightcraft_raster::Image<[f32; 3]>>::map_in_place<lightcraft_engin...` | 291 | 14.1 |
| 5 | `<lightcraft_engine::camera_preview::HueSat>::apply` | 282 | 13.7 |
| 6 | `<lightcraft_raw::profile::HsvTable>::apply` | 241 | 11.7 |
| 7 | `lightcraft_raw::decode_with` | 220 | 10.7 |
| 8 | `<&<lightcraft_raw::RawImage>::develop_binned::{closure#3} as core::ops::function::FnMut<((usize, &mut [[f32...` | 186 | 9.0 |
| 9 | `<alloc::vec::Vec<alloc::vec::Vec<u16>> as rayon::iter::ParallelExtend<alloc::vec::Vec<u16>>>::par_extend::<...` (+2 callers with the same samples) | 169 | 8.2 |
| 10 | `lightcraft_engine::camera_preview::fit_preview` | 63 | 3.1 |
| 11 | `lightcraft_engine::camera_preview::proxies` | 61 | 3.0 |
| 12 | `powf` | 56 | 2.7 |
| 13 | `<lightcraft_raw::profile::HsvTable>::lookup` | 50 | 2.4 |
| 14 | `lightcraft_codecs::jpeg::decode_with_fallback` (+3 callers with the same samples) | 48 | 2.3 |
| 15 | `<jpeg_decoder::decoder::Decoder<&[u8]>>::decode_internal` (+1 callers with the same samples) | 47 | 2.3 |
| 16 | `<jpeg_decoder::worker::WorkerScope>::get_or_init_worker::<core::result::Result<(core::option::Option<jpeg_d...` | 46 | 2.2 |
| 17 | `<jpeg_decoder::decoder::Decoder<&[u8]>>::decode_scan` | 45 | 2.2 |
| 18 | `lightcraft_raw::vendor::white_from_data` | 41 | 2.0 |
| 19 | `lightcraft_engine::files::load_bytes_now::{closure#11}` | 38 | 1.8 |
| 20 | `<rayon::iter::plumbing::bridge::Callback<rayon::iter::for_each::ForEachConsumer<lightcraft_raster::par_rows...` | 34 | 1.6 |

## 4. CPU against GPU per render and per `SliderToFrame`

**Per render, render thread (Time Profiler, `renders.py`)** **(m)**:

| Render-thread work | NEF ms/render | RAF ms/render | Where |
|---|---|---|---|
| Source upload: wgpu zero-fill of the staging mapping | 4.79 | 3.16 | `gpu/src/render.rs:596` > `ctx.rs:453` > wgpu-core `device_create_buffer` |
| Source upload: copy into the mapping | 1.52 | 0.94 | same call |
| Source upload: other (buffer creation, IOGPU) | 0.18 | 0.16 | same call |
| CPU histogram of the output | 1.42 | 1.25 | `render.rs:702-705` > `raster/src/histogram.rs:19` |
| Readback copy `to_vec` from the mapped staging buffer | 1.12 | 1.15 | `render.rs:688` > `ctx.rs:565` |
| Scan for unwritten pixels (alpha != 255) | 0.35 | 0.30 | `render.rs:697`, `:732` |
| Finish parameters, plan, other | 0.30 | 0.32 | `params::finish_block`, `pipeline::plan` |
| Queue submit and poll (this thread) | 0.12 | 0.12 | wgpu |
| **Total** | **9.80** | **7.41** | |

**Per render, GPU (Metal System Trace, `gpuan.py`, means over the sweep, renders counted by `main` dispatches)** **(m)**:

| GPU work | NEF ms/render | RAF ms/render |
|---|---|---|
| Upload and parameter blits (`(wgpu internal) PendingWrites`) | 1.55 | 1.16 |
| `sample_affine` (resample source to 2360 x 1573) | 1.43 | 1.38 |
| `wb_k` (white balance) | 1.22 | 1.23 |
| `log_lum_k` (log luminance plane) | 0.71 | 0.70 |
| `main` (per-pixel stage, RGBA8 out) | 4.17 (bimodal: 2.8 or 4.7) | 4.62 |
| Readback blit (`Signal`) | 0.34 | 0.38 |
| **Serial total** | **9.41** | **9.48** |
| Clear of the output buffer (runs beside `wb_k`) | 0.42 | 0.39 |
| GPU busy share of the sweep | 39 % | 45 % |
| Canvas draw (fragment pass of `CanvasView`) | 0.35 per draw | 0.39 per draw |

The kernels and their times are the same for both RAWs, because the output is the same size; only the upload differs.
The bimodal `main` time follows the GPU clock state. CPU (9.8) + GPU (9.4) = 19.2 ms against a 20.6 ms period on the
NEF: the two halves run one after the other, never overlapped, because one render is in flight at a time and each
render waits for its own readback **(c)**. The first upload blit reaches the GPU 2.4 ms (NEF) and 1.7 ms (RAF) after
it is committed (`CPU to GPU Latency` column) **(m)**.

**Per `SliderToFrame` (NEF, medians from `measurements.md`: 51.3 ms presented, 67.0 ms all-ended)**, an approximate
decomposition (medians do not add exactly) **(c from m)**:

| Part | ms | Evidence |
|---|---|---|
| Waiting for the render in flight | ~8 (0 to 16.7) | requests arrive every 16.7 ms, renders take 20.6 ms; the pending slot is overwritten when the wait exceeds 16.7 ms. Model: (20.6 - 16.7) / 20.6 = 19 % of requests overtaken; measured 740 / 3600 = 20.6 %. RAF: model 10 %, measured 11.7 % |
| Own render, CPU | 9.8 | table above |
| Own render, GPU | 9.4 | table above |
| Render GPU end to canvas draw on the GPU | 11.3 median, 21.7 p95 (MST) | includes the CPU tail of the render (histogram, `to_vec`, scan, ~2.9 ms), `RenderDone` to the engine thread, FrameSink copy (~0.3 ms), the async hop to main and the wait for the next display-link tick |
| Canvas draw to glass | ~10 to 25 | vsync and compositor; 60 Hz panel |

So about 19 ms (37 %) of a typical presented frame is render work, and of that only about half runs on the GPU. The
rest is queueing and the display path, which is why every latency lands on 51 or 67 ms (3 or 4 frames of 16.7 ms).

## 5. Every memory copy on the frame path

Per render, NEF / RAF, output 2360 x 1573 = 3,712,280 pixels (c):

| # | Copy | Bytes per render | Cost per render | Where |
|---|---|---|---|---|
| 1 | Zero-fill of the wgpu staging buffer for the source (`mapped_at_creation`) | 52.4 / 34.6 MB written | 4.79 / 3.16 ms CPU **(m)** | `render.rs:596` > `ctx.rs:453` > wgpu-core |
| 2 | Source `Rgb32f` copied into the staging mapping | 52.4 / 34.6 MB read + written | 1.52 / 0.94 ms CPU **(m)** | same |
| 3 | Staging to private device buffer (GPU blit, `PendingWrites`) | 52.4 / 34.6 MB | 1.35 / 0.96 ms GPU median **(m)** | wgpu queue |
| 4 | Clear of the RGBA8 output buffer (`cx.zeroed(n)`) | 14.85 MB written | 0.42 / 0.39 ms GPU, overlapped **(m)** | `render.rs:652` |
| 5 | Intermediate passes that rewrite the image: `sample_affine` 44.5 MB, `wb_k` 44.5 MB, `log_lum_k` 14.85 MB written | 104 MB written, ~150 MB read | 3.4 ms GPU **(m)** | `render.rs:511`, `:753`, `:824` |
| 6 | Readback: output to a fresh `MAP_READ` staging buffer (GPU blit, `Signal`) | 14.85 MB | 0.34 / 0.38 ms GPU **(m)** | `ctx.rs:533-540` |
| 7 | Readback: staging mapping to a new `Vec<[u8; 4]>` (`to_vec`) | 14.85 MB read + written | 1.12 / 1.15 ms CPU **(m)** | `ctx.rs:565` |
| 8 | Scan of the output for unwritten pixels | 14.85 MB read | 0.35 / 0.30 ms CPU **(m)** | `render.rs:697`, `:732` |
| 9 | Histogram of the output (every second row and column) | ~7.4 MB of cache lines read (c) | 1.42 / 1.25 ms CPU **(m)** | `render.rs:702-705` |
| 10 | `Rendered` to the engine thread and the C callback | 0 (moved in a `Box`, pointer passed) | none | `render_worker.rs:45-56`, `session.rs:56-73` |
| 11 | FrameSink row copy into the shared `MTLBuffer` (`copyMemory` per row) | 14.85 MB read + written | ~0.27 / 0.30 ms CPU **(m, sampled)**; floor 0.30 ms at 100 GB/s (c) | `FrameSink.swift:151-165` |
| 12 | Canvas: texture over the shared buffer | 0 (zero-copy `makeTexture(descriptor:offset:bytesPerRow:)`) | draw 0.35 / 0.39 ms GPU **(m)** | `CanvasView.swift:342-354` |

Totals per render (c): about 240 MB (NEF) of CPU-side writes and reads that exist only to move or check bytes
(rows 1, 2, 7, 8, 9, 11), ~9.5 ms of CPU, and about 470 MB of GPU-side traffic (rows 3 to 6 plus `main`). At 48
renders/s that is ~35 GB/s of memory traffic, a third of the M2's ~100 GB/s, for an image that changes only in
exposure.

Copies on the open path, per decode (NEF / RAF) (c, costs (m) from section 3):

| Copy | Bytes | Where |
|---|---|---|
| File read into a `Vec<u8>` (three times in a warm NEF open, twice cold) | 58.7 / 27.9 MB | `engine/src/files.rs:538`, `:547` |
| Decode into the `u16` mosaic (RAF: per-stripe `Vec<u16>`, then copied into the full mosaic) | 91.4 / 52.2 MB, RAF + 52.2 MB stripe copy | `raw/src/vendor/nefc.rs:163`, `rafc.rs:33-100` |
| Bin into camera RGB `Rgb32f` | 136.3 / 34.6 MB | `raw/src/binned.rs:65` |
| Colour matrix in place | 0 | `files.rs:318` |
| Fit to 2560: NEF resize (temp 84.5 MB + out 52.4 MB); RAF same size, so `resize` clones 34.6 MB | 136.9 / 34.6 MB | `files.rs:332`, `raster/src/resample.rs:118-125` |
| Cache insert, job source | 0 (`Arc`) | `media.rs:252`, `:1207` |

## 6. Why draft equals full

Draft is **not cheaper on the engine side**. Evidence:

- The app asks for the same size for both: `requestPreview(maxPixels: previewPixels, draft: true/false)` with
  `previewPixels` = 2360 (`EditorViewModel.swift:115`, `:128`). The FFI passes it unchanged and only calls
  `job.draft()` (`session.rs:403-414`).
- `RenderJob::draft()` sets `Quality::Draft` and changes the cache key (`engine/src/media.rs:693-699`). The only
  reader of `Quality` in the pipeline and the GPU renderer is `plane_sigmas`, which caps the highlights/shadows base
  radius at 24 px (`pipeline/src/local.rs:271`). The sweep drags exposure, so that plane does not exist and the cap
  changes nothing; even with it, the pixel count would be the same.
- The Metal trace shows the same four kernels (`sample_affine`, `wb_k`, `log_lum_k`, `main`) in all 2551 NEF renders
  and 2893 RAF renders, with the stable per-kernel times of section 4 **(m)**. The 60 full requests are not
  distinguishable from the drafts.
- No stage is reused between a draft and the next full, because v0 attaches no `StageCache`.

So the bottleneck is elsewhere, in this order: (a) the fixed ~20 ms render, of which ~55 % re-uploads and re-derives
an unchanged source and ~15 % reads the result back and inspects it on the CPU; (b) one render in flight with 60 Hz
requests, which adds ~8 ms of queueing on average and overtakes 12 to 21 % of requests; (c) the display path
(~11 ms from render end to canvas draw, then vsync and compositor), which quantizes everything to 16.7 ms steps. A
genuinely cheaper draft (for example half the linear size, a quarter of the pixels) would cut the per-pixel GPU time,
the readback and the histogram by about 4x, but not the source upload, and not the display path.

**Why the NEF is one frame slower at the median.** Both outputs are 2360 x 1573. The sources differ: the Z 7 mosaic
(Bayer) bins 2x to 4128 x 2752 and is fitted to 2560 x 1707 (52.4 MB); the X-T4 mosaic (X-Trans, which cannot bin 2x)
bins 3x to 2080 x 1386 (34.6 MB) and is used as is (`files.rs:209-215`, `binned.rs:21-28`) (c). The measured upload
costs scale with that ratio (zero-fill 4.79 / 3.16 = 1.52, copy 1.52 / 0.94 = 1.62, area ratio 1.52) **(m)**. The NEF
render is therefore ~2.1 ms longer (20.6 against 18.5 ms period), which pushes more requests over a vsync boundary
and nearly doubles the overtaken share (20.6 % against 11.7 %).

## 7. Thermal finding and what it implies

From `measurements.md` section C **(m)**: the 600 s NEF soak went `nominal` to `fair` at 54.1 s and to `serious` at
284.1 s, and stayed `serious`; the iPad was on its charger. The load during a sweep (this report) is 0.79 CPU cores
(60 % of it the render thread) and a GPU busy 39 to 45 % of the time, at 48 renders/s **(m)**, moving ~35 GB/s of
memory (c).

What it implies:

1. **Throttling did not show in the metric yet.** The six A/B sweeps at `serious` gave the same latency as the
   `nominal` sweep, and the same CPU per frame (section 9). The render is ~20 ms and the display quantizes to 16.7 ms,
   so a moderate clock drop disappears in the rounding. It will not stay hidden once renders get shorter: a 10 ms
   render at reduced clocks may cross a vsync boundary where a nominal one does not.
2. **Every sub-project 2 comparison must control thermal state**: start from `nominal` (cool down between arms), record
   the state per run (the sampler already does), interleave arms (ABAB), and report CPU time per frame and GPU time
   per render next to the display-quantized latency, which is too coarse to see engine gains below one frame.
3. **Energy is dominated by byte moving, not by image math.** About 75 % of the render thread is memset and memcpy
   (section 2), and the GPU spends ~4.7 ms of 9.4 recomputing an unchanged source. Targets 1 and 3 cut heat as well as
   latency, which delays throttling in long editing sessions.
4. **Part of the load is the harness.** The SwiftUI panel update costs ~1.5 ms of main thread per 60 Hz tick
   (5.3 s per minute **(m)**). It is real in the app as well, but a test that drags a slider without SwiftUI would
   isolate the engine.

## 8. Other observations

- **Metal submission cost.** Each render submits several command buffers (upload, parameter writes, stage flushes,
  readback). The Metal dispatch threads spend 10.4 s of CPU per minute **(m)**, ~3.5 ms per render, mostly
  `IOGPUCommandQueueSubmitCommandBuffers` kernel calls. It is off the render thread but it is energy, and it adds the
  2.4 ms CPU-to-GPU latency of the first blit.
- **Per-tick persistence.** Every `develop.set` during a drag appends a JSON journal record and calls `sync_data`
  (`catalog/src/store.rs:127-138`; `Session::persist` 427 ms per minute of CPU, plus unsampled fsync waits). The
  command takes 3.0 ms median **(m)** and runs before the preview request of the same tick (`EditorViewModel.swift:107-116`),
  so the user-visible latency is ~3 ms longer than `SliderToFrame`, which starts at the request.
- **Buffers churn.** The source upload buffer, its staging buffer and the readback staging buffer are created new for
  every render (`ctx.rs:453`, `:533`); only `Gpu::buffer` recycles from the pool (`ctx.rs:367-397`). The
  `IOGPUResourceCreate` calls alone take ~270 ms per minute **(m)**.
- **`white_from_data`** (`raw/src/vendor/mod.rs:40-66`) scans the whole mosaic twice with a `u16 -> f32` comparison
  in the filter, which keeps it scalar: ~75 ms per NEF decode, ~40 ms per RAF decode **(m)**.

## 9. The A/B revisited: CPU time per frame

`measurements.md` could only say "no difference of a frame". CPU on `unveil-render` per presented frame, same traces,
thread names only **(m)**:

| Run | Thermal | Render-thread ms per presented frame | Process CPU (cores) |
|---|---|---|---|
| `time-48mp` (default) | nominal | 9.83 | 0.79 |
| `ab/nef-m1-time-1`, `-2`, `-3` | serious | 10.23, 9.94, 9.80 | 0.80, 0.79, 0.78 |
| `ab/nef-default-time-1`, `-2`, `-3` | serious | 10.13, 9.95, 10.10 | 0.80, 0.79, 0.80 |

Median `apple-m1` 9.94 ms against default 10.10 ms: within the run-to-run spread. Expected, since the render thread is
three quarters `libsystem_platform` memset/memcpy and the rest is mostly scalar histogram code that both targets
compile alike. Codegen flags become worth testing again only after targets 1, 3 and 5.

## 10. Five targets for sub-project 2, ranked by estimated recoverable time

| Rank | Target | Evidence | Estimated recoverable time |
|---|---|---|---|
| 1 | **Keep the source and unchanged stages resident on the GPU** (upstream `StageCache` + `GpuStages` through `RenderJob::with_stages`, then in a Metal-direct engine a source decoded straight into a shared `MTLBuffer`, never re-uploaded) | Per NEF render: 6.49 ms CPU in `create_buffer_init` (39.8 % of all process CPU), 1.35 ms upload blit, 3.36 ms of `sample_affine` + `wb_k` + `log_lum_k` whose inputs do not change during an exposure drag **(m)**. `GpuStages` keeps the uploaded source when it is at most 96 MB (`render.rs:418`), which holds for both RAWs (c) | **~11 ms of the 20.6 ms NEF render, ~8.5 of 18.5 ms RAF** (c from m). With the shorter render the queueing shrinks too; estimated median `SliderToFrame` about one display frame lower. Cost: the stages stay allocated between renders: ~156 MB (NEF) or ~139 MB (RAF) of device memory per view at 2360 px (sampled 44.5 + white-balanced 44.5 + log-luminance 14.85 + the source) (c), memory that today is allocated and pooled on every render anyway |
| 2 | **Decode each RAW once per open** (keep the decoded mosaic or the preview source from the probe, make the NEF probe header-only, do not re-probe pixels in `photo.relink`), and vectorize `white_from_data` with NEON | Warm NEF open 1086 ms: three full decodes of ~255 ms each, serial on one core (section 3) **(m)**; cold NEF 865 ms: two | **~510 ms of the warm NEF open (47 %), ~255 ms of the cold one (30 %)**; NEON `white_from_data` a further ~70 ms per remaining decode (c: 91 MB scan at memory speed is ~5 ms) |
| 3 | **Zero-copy output in unified memory**: `main` writes into a shared `MTLBuffer` or texture that the canvas samples directly (spec 4: "the copy will disappear") | Readback blit 0.34 ms GPU, `to_vec` 1.12 ms, unwritten scan 0.35 ms, FrameSink copy ~0.3 ms per render; a new 14.85 MB staging buffer and a 14.85 MB `Vec` per render, plus 2 x 26.2 MB FrameSink buffers outside the engine budget **(m, c)** | **~2.1 ms per render (~10 %)** and ~82 MB of buffers (c) |
| 4 | **GPU data layout and dispatch**: fuse `sample_affine` + `wb_k` + `log_lum_k` (one read of the source, one write), store the image and planes as f16, one command buffer per render with persistent parameter buffers (Metal-direct) | GPU per render 9.4 ms, all streaming kernels over f32 RGB buffers of 44.5 MB (~470 MB of traffic per render (c)); `main` 4.2 to 4.6 ms; ~3.5 ms of driver CPU per render on the dispatch threads and 2.4 ms CPU-to-GPU latency **(m)** | **~1.5 to 3 ms per render** after target 1 (f16 input to `main` ~1 to 1.5 ms, fewer submissions ~1 ms of latency) (c, least certain of the five) |
| 5 | **Drop the CPU histogram from previews, or compute it on the GPU** | `Histogram::of_srgb8` 1.42 ms (NEF) and 1.25 ms (RAF) per render, 8.7 to 9.7 % of all process CPU **(m)**; the v0 C ABI never returns it, so nothing uses it | **~1.4 ms per render (~7 %)**, trivially |

Not ranked but worth a decision in sub-project 2: a real draft (lower resolution while dragging) would divide the
per-pixel part of targets 3 to 5 by four; the display path (render end to canvas draw 11.3 ms median) belongs to the
UI side, which the spec keeps unchanged, but driving the draw from the frame callback instead of a hop to main and
the next display-link tick could save several milliseconds; and per-tick `develop.set` persistence adds ~3 ms before
each request.

## 11. Limits

- One trace per size and scenario; the per-render costs are stable inside each trace (thousands of renders), but
  run-to-run spread is known only from the A/B set (±0.2 ms CPU per frame).
- No sampled cold open; the cold breakdown comes from the Logging traces.
- Sub-millisecond costs (FrameSink copy, scan) are statistical estimates from 1 kHz sampling.
- GPU times come from the Metal System Trace run, CPU times from the Time Profiler run of the same scenario, not from
  the same run.
- The memory-traffic totals are computed from buffer sizes, not measured with GPU counters.

## Reproducing

Scripts are in the session scratchpad (`t16/bin/`), not in the repo, as the brief asks:

```bash
xcrun xctrace export --input Baseline/traces/time-48mp.trace --toc                       # also prints the load address
xcrun xctrace export --input Baseline/traces/time-48mp.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' > tp48.xml
python3 -I tp.py tp48.xml Baseline/traced-build/Unveil.app.dSYM/Contents/Resources/DWARF/Unveil s48.json 0x1000ac000
python3 -I an.py s48.json iv-time-48mp.json 20     # phases, per-thread, top 20 (intervals from parse.py of M3.5)
python3 -I renders.py s48.json iv-time-48mp.json   # per-render CPU on unveil-render
python3 -I openline.py s48.json iv-time-48mp.json  # open timeline
xcrun xctrace export --input Baseline/traces/metal-48mp.trace --xpath '//table[@schema="metal-gpu-intervals"]' > m48.xml
python3 -I gpu.py m48.xml g48.json && python3 -I gpuan.py g48.json iv-metal-48mp.json
```
