// Modified by the Unveil authors, 2026: resident sources and mapped display output; see Engine/CHANGES.md.
//! The GPU renderer: the CPU pipeline's stages, evaluated on the device where a kernel exists and
//! on the CPU otherwise (per-stage hybrid). Stage results stay on the device and are cached per
//! view exactly like [`lightcraft_pipeline::StageCache`] (same keys, see [`lightcraft_pipeline::Plan`]).

use std::sync::{Arc, Mutex, Weak};

use lightcraft_develop::DevelopSettings;
use lightcraft_geom::Orientation;
use lightcraft_pipeline::finish::{FinishParams, MASK_TERMS, mask_terms};
use lightcraft_pipeline::geometry::SampleMode;
use lightcraft_pipeline::{Plan, RenderRequest, Rendered, SourceExtent, SourceId, SourceInfo, local};
use lightcraft_raster::resample::Filter;
use lightcraft_raster::{Histogram, Plane, Rgb32f};

use crate::ctx::{Buf, FailKind, Gpu, MappedReadback, fail, groups1, groups2};
use crate::params::{Present, finish_block};
use crate::{DisplayImage, GpuSource};

/// Device-resident stages of one view, kept next to the CPU [`lightcraft_pipeline::StageCache`]
/// (as its extension). Holds the last two output sizes, like the CPU cache.
#[derive(Default)]
pub struct GpuStages {
    entries: Mutex<Vec<Entry>>,
    /// The uploaded source (a view renders one photo at several sizes).
    source: Mutex<Option<LegacySource>>,
}

/// LegacySource keeps pointer identity valid without keeping the CPU allocation alive.
struct LegacySource {
    host: Weak<Rgb32f>,
    id: SourceId,
    device: Option<Arc<GpuSource>>,
}

#[derive(Clone)]
struct Entry {
    src: SourceId,
    original: Option<Weak<GpuSource>>,
    geo: u64,
    sampled: Arc<Buf>,
    lin: Option<(u64, Arc<Buf>)>,
    planes: Planes,
}

impl Entry {
    /// belongs_to distinguishes a current upload from an earlier allocation of the same content.
    /// Reusing an identity sample from that allocation would keep the old original alive.
    fn belongs_to(&self, source: &Arc<GpuSource>) -> bool {
        self.src == source.id() && self.original.as_ref().is_some_and(|original| original.ptr_eq(&Arc::downgrade(source)))
    }
}

/// Spatial planes of one linear image (`key` = its `lin_key`), each tagged with its radius.
#[derive(Clone, Default)]
struct Planes {
    key: u64,
    log_l: Option<Arc<Buf>>,
    base: Option<(u32, Arc<Buf>)>,
    clarity: Option<(u32, Arc<Buf>)>,
    texture: Option<(u32, Arc<Buf>)>,
    dark: Option<(u32, Arc<Buf>, f32)>,
    chroma: Option<(u32, Arc<Buf>)>,
}

impl Planes {
    /// is_f32 permits forced-f32 retries to reuse their already validated spatial caches.
    fn is_f32(&self) -> bool {
        self.log_l
            .iter()
            .chain(self.base.iter().map(|(_, b)| b))
            .chain(self.clarity.iter().map(|(_, b)| b))
            .chain(self.texture.iter().map(|(_, b)| b))
            .chain(self.dark.iter().map(|(_, b, _)| b))
            .chain(self.chroma.iter().map(|(_, b)| b))
            .all(|buffer| buffer.half_len.is_none())
    }
}

impl GpuStages {
    fn get(&self, src: SourceId, geo: u64) -> Option<Entry> {
        self.lock().iter().find(|e| e.geo == geo && e.src == src).cloned()
    }

    fn put(&self, e: Entry) {
        let mut v = self.lock();
        v.retain(|o| !(o.geo == e.geo && o.src == e.src));
        v.push(e);
        while v.len() > 2 {
            v.remove(0);
        }
    }

    fn legacy_source(&self, gpu: &Gpu, src: &Arc<Rgb32f>) -> Option<(SourceId, Option<Arc<GpuSource>>)> {
        let mut g = self.source.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = &mut *g
            && entry.host.upgrade().is_some_and(|host| Arc::ptr_eq(&host, src))
        {
            if entry.device.as_ref().is_some_and(|source| !source.allows_half_stages() && !source.can_read_exact()) {
                entry.device = Some(GpuSource::upload_with_storage(gpu, entry.id, src, crate::SourceStorage::F32).ok()?);
            }
            return Some((entry.id, entry.device.clone()));
        }
        *g = None;
        let id = SourceId::new();
        let device = if gpu.fits(src.data.len().checked_mul(3)?) && src.data.len() * 12 <= RETAIN_SOURCE_BYTES {
            Some(GpuSource::upload(gpu, id, src).ok()?)
        } else {
            None
        };
        *g = Some(LegacySource { host: Arc::downgrade(src), id, device: device.clone() });
        Some((id, device))
    }

    /// adopt_source removes compatibility ownership only after a resident render is validated.
    /// Old identity samples may alias an entire original, so their entries must leave as well.
    pub(crate) fn adopt_source(&self, source: &Arc<GpuSource>) {
        let legacy = self.source.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(legacy) = legacy {
            release_shared_source_for(legacy.id);
        }
        self.lock().retain(|entry| {
            if entry.belongs_to(source) {
                true
            } else {
                release_shared_source_for(entry.src);
                false
            }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of cached output sizes.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// bytes counts derived storage once. A sampled alias is source storage while its original
    /// owner exists; after eviction, any alias still retained here is counted as a stage.
    pub fn bytes(&self) -> usize {
        let mut seen: Vec<*const Buf> = Vec::new();
        let mut total = 0;
        let mut add = |b: &Arc<Buf>| {
            let p = Arc::as_ptr(b);
            if !seen.contains(&p) {
                seen.push(p);
                total += b.len * 4;
            }
        };
        for e in self.lock().iter() {
            let original = e.original.as_ref().and_then(Weak::upgrade);
            if !original.as_ref().is_some_and(|source| Arc::ptr_eq(&source.buffer, &e.sampled)) {
                add(&e.sampled);
            }
            if let Some((_, b)) = &e.lin {
                add(b);
            }
            let p = &e.planes;
            p.log_l.iter().for_each(&mut add);
            p.base.iter().for_each(|(_, b)| add(b));
            p.clarity.iter().for_each(|(_, b)| add(b));
            p.texture.iter().for_each(|(_, b)| add(b));
            p.dark.iter().for_each(|(_, b, _)| add(b));
            p.chroma.iter().for_each(|(_, b)| add(b));
        }
        total
    }

    /// Drop everything (e.g. when memory is needed).
    pub fn clear(&self) {
        self.lock().clear();
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A render in progress: the device and the commands recorded so far. Reading a result back
/// submits everything recorded before it.
pub(crate) struct Cx<'a> {
    pub gpu: &'a Gpu,
    enc: Option<wgpu::CommandEncoder>,
    /// Kernel invocations recorded since the last submit.
    pending: u64,
    storage_enabled: bool,
    half_status: Option<Buf>,
}

/// Kernel invocations recorded before a render submits them (under one full-size pass over a
/// 24 MP export, several passes over a preview): no single submission runs long.
const FLUSH_INVOCATIONS: u64 = 16 << 20;

/// Pixels per dispatch of the per-pixel stage (the heaviest kernel).
const BAND_PIXELS: usize = 4 << 20;

impl<'a> Cx<'a> {
    pub fn new(gpu: &'a Gpu) -> Cx<'a> {
        Cx { gpu, enc: Some(gpu.encoder()), pending: 0, storage_enabled: true, half_status: None }
    }

    /// Record kernel `name` (see [`Gpu::run`]).
    pub fn run(&mut self, name: &str, p: &[u32], bufs: &[Option<&Buf>], groups: [u32; 3]) {
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        self.gpu.run(enc, name, p, bufs, groups);
        // every kernel runs 256 invocations per workgroup
        let pixels_per_lane = if name == "display_reduce_checked_stats" { crate::reduced_stats::TILE.pow(2) as u64 } else { 1 };
        self.pending += groups.iter().map(|g| *g as u64).product::<u64>() * 256 * pixels_per_lane;
        if self.pending >= FLUSH_INVOCATIONS {
            self.flush();
        }
    }

    /// Submit what is recorded and read back `len` values of `b`.
    pub fn read<T: bytemuck::Pod>(&mut self, b: &Buf, len: usize) -> Vec<T> {
        self.pending = 0;
        let enc = self.enc.take().unwrap_or_else(|| self.gpu.encoder());
        self.gpu.finish_and_read(enc, b, len)
    }

    fn map(&mut self, b: &Buf, len: usize, tail: usize) -> Option<MappedReadback> {
        self.pending = 0;
        let enc = self.enc.take().unwrap_or_else(|| self.gpu.encoder());
        self.gpu.finish_and_map_padded(enc, b, len, tail)
    }

    /// Submit what is recorded so far without waiting. Renders submit stage by stage and every
    /// [`FLUSH_INVOCATIONS`]: long submissions are what GPU watchdogs reset (Windows TDR, the
    /// i915 hang check; issue #78's iGPU spent seconds on an export recorded as one submission),
    /// and buffers dropped by earlier work become reusable by later work (lower peak memory).
    pub fn flush(&mut self) {
        self.pending = 0;
        if let Some(enc) = self.enc.take() {
            self.gpu.submit([enc.finish()]);
        }
    }

    /// Submit what is recorded and wait for it (profiling only: stage timings).
    pub fn sync(&mut self) {
        self.pending = 0;
        if let Some(enc) = self.enc.take() {
            self.gpu.submit([enc.finish()]);
        }
        let _ = self.gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// A zero-filled buffer (the clear is recorded).
    pub fn zeroed(&mut self, len: usize) -> Buf {
        let out = self.gpu.buffer(len);
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.clear_buffer(out.raw(), 0, None);
        out
    }

    /// A copy of `b` (recorded).
    pub fn copy(&mut self, b: &Buf) -> Buf {
        if let Some(n) = b.half_len {
            let out = self.gpu.buffer(n);
            map(self, "expand_storage", n, &[], [Some(b), None, None], &out);
            return out;
        }
        let out = self.gpu.buffer(b.len);
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_buffer(b.raw(), 0, out.raw(), 0, (b.len * 4) as u64);
        out
    }

    /// `len` values of `b` from `offset` (both in 32-bit words), as a new buffer (recorded).
    pub fn slice(&mut self, b: &Buf, offset: usize, len: usize) -> Buf {
        assert!(b.half_len.is_none(), "slice offsets require f32 storage");
        let out = self.gpu.buffer(len);
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_buffer(b.raw(), (offset * 4) as u64, out.raw(), 0, (len * 4) as u64);
        out
    }

    /// Copy all of `src` into `dst` at `offset` (32-bit words; recorded).
    pub fn copy_into(&mut self, src: &Buf, dst: &Buf, offset: usize) {
        if src.half_len.is_some() {
            let expanded = self.copy(src);
            return self.copy_into(&expanded, dst, offset);
        }
        assert!(dst.half_len.is_none(), "copy destination requires f32 storage");
        let enc = self.enc.get_or_insert_with(|| self.gpu.encoder());
        enc.copy_buffer_to_buffer(src.raw(), 0, dst.raw(), (offset * 4) as u64, (src.len * 4) as u64);
    }

    fn pack_candidate(&mut self, original: &Buf) -> Buf {
        let n = original.len;
        let mut packed = self.gpu.buffer(n.div_ceil(2));
        packed.half_len = Some(n);
        let bad = self.half_status.take().unwrap_or_else(|| self.zeroed(1));
        self.run("pack_half", &[n as u32], &[Some(original), Some(&packed), Some(&bad)], groups1(n));
        self.half_status = Some(bad);
        packed
    }

    fn compact(&mut self, original: Buf, plane: bool) -> Buf {
        if !self.storage_enabled || !self.gpu.half_storage || (plane && !self.gpu.half_planes) || original.half_len.is_some() {
            return original;
        }
        self.pack_candidate(&original)
    }

    /// upload_source validates half storage before publishing an immutable source.
    pub(crate) fn upload_source(&mut self, src: &Rgb32f, allow_half: bool) -> Buf {
        let original = self.gpu.upload(rgb_words(src));
        if !allow_half || !self.gpu.half_source {
            return original;
        }
        assert!(self.half_status.is_none(), "source precedes derived-stage candidates");
        let packed = self.pack_candidate(&original);
        let status = self.half_status.take().expect("source range guard");
        let rejected: Vec<u32> = self.read(&status, 1);
        if rejected == [0] && !crate::ctx::failed() { packed } else { original }
    }

    pub fn read_rgb(&mut self, b: &Buf, w: usize, h: usize) -> Rgb32f {
        if b.half_len.is_some() {
            let expanded = self.copy(b);
            return self.read_rgb(&expanded, w, h);
        }
        Rgb32f { width: w, height: h, data: self.read(b, w * h * 3).into() }
    }

    pub fn read_plane(&mut self, b: &Buf, w: usize, h: usize) -> Plane {
        if b.half_len.is_some() {
            let expanded = self.copy(b);
            return self.read_plane(&expanded, w, h);
        }
        Plane { width: w, height: h, data: self.read(b, w * h).into() }
    }
}

pub(crate) fn rgb_words(img: &Rgb32f) -> &[f32] {
    bytemuck::cast_slice(&img.data)
}

// ---------------------------------------------------------------------------------------------
// Filters (twins of `lightcraft_raster::blur` / `resample` and `lightcraft_pipeline::local`)

/// Pixels each blur thread slides its running sum over (at least; longer for large radii, so the
/// priming sum stays a small share of the work).
const CHUNK: usize = 16;

/// Gaussian blur of a `w × h` image of `nc` interleaved channels: three box passes each way
/// (`lightcraft_raster::blur::gaussian`).
pub(crate) fn gaussian(cx: &mut Cx<'_>, src: &Buf, w: usize, h: usize, nc: usize, sigma: f32) -> Buf {
    if sigma <= 0.3 || w * h == 0 {
        return cx.copy(src);
    }
    let radii = lightcraft_raster::blur::box_radii(sigma);
    let passes: Vec<(&str, usize)> = radii.iter().map(|r| ("box_h", *r)).chain(radii.iter().map(|r| ("box_v", *r))).filter(|p| p.1 > 0).collect();
    if passes.is_empty() {
        return cx.copy(src);
    }
    // ping-pong between two buffers
    let mut bufs = [cx.gpu.buffer(w * h * nc), cx.gpu.buffer(w * h * nc)];
    for (i, (k, r)) in passes.iter().enumerate() {
        let chunk = (2 * r + 1).clamp(CHUNK, 128);
        let groups = if *k == "box_h" { groups2(w.div_ceil(chunk), h, [64, 4]) } else { groups2(w, h.div_ceil(chunk), [64, 4]) };
        let p = [w as u32, h as u32, nc as u32, *r as u32, chunk as u32];
        let (out, prev) = (&bufs[i % 2], &bufs[(i + 1) % 2]);
        cx.run(k, &p, &[Some(if i == 0 { src } else { prev }), Some(out)], groups);
    }
    let last = (passes.len() - 1) % 2;
    let [a, b] = std::mem::replace(&mut bufs, [cx.gpu.buffer(0), cx.gpu.buffer(0)]);
    if last == 0 { a } else { b }
}

/// Resample taps of `lightcraft_raster::resample::resize`, packed for the `resize_*` kernels.
fn taps(src: usize, dst: usize, filter: Filter) -> Vec<u32> {
    let w = lightcraft_raster::resample::weights(src, dst, filter);
    let mut t = Vec::with_capacity(dst * 3);
    let mut weights = Vec::new();
    let base = dst * 3;
    for (lo, ws) in &w {
        t.extend_from_slice(&[*lo as u32, ws.len() as u32, (base + weights.len()) as u32]);
        weights.extend(ws.iter().map(|v| v.to_bits()));
    }
    t.extend(weights);
    t
}

/// `lightcraft_raster::resample::resize` of an `nc`-channel image.
pub(crate) fn resize(cx: &mut Cx<'_>, src: &Buf, (sw, sh): (usize, usize), (dw, dh): (usize, usize), nc: usize, filter: Filter) -> Buf {
    let (dw, dh) = (dw.max(1), dh.max(1));
    if (sw, sh) == (dw, dh) {
        return cx.copy(src);
    }
    let tx = cx.gpu.upload(&taps(sw, dw, filter));
    let tmp = cx.gpu.buffer(dw * sh * nc);
    cx.run("resize_h", &[sw as u32, sh as u32, dw as u32, nc as u32], &[Some(src), Some(&tx), Some(&tmp)], groups2(dw, sh, [16, 16]));
    let ty = cx.gpu.upload(&taps(sh, dh, filter));
    let out = cx.gpu.buffer(dw * dh * nc);
    cx.run("resize_v", &[dw as u32, sh as u32, dh as u32, nc as u32], &[Some(&tmp), Some(&ty), Some(&out)], groups2(dw, dh, [16, 16]));
    out
}

/// An element-wise kernel of the `map` module over `n` items.
pub(crate) fn map(cx: &mut Cx<'_>, k: &str, n: usize, extra: &[u32], ins: [Option<&Buf>; 3], out: &Buf) {
    let mut p = vec![n as u32];
    p.extend_from_slice(extra);
    cx.run(k, &p, &[ins[0], ins[1], ins[2], Some(out)], groups1(n));
}

/// Self-guided filter of a plane (`local::guided`).
fn guided(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    let ab = guided_coeffs(cx, p, w, h, sigma, eps);
    let q = cx.gpu.buffer(w * h);
    map(cx, "guided_apply", w * h, &[], [Some(p), Some(&ab), None], &q);
    q
}

/// Blurred (a, b) coefficients, interleaved.
fn guided_coeffs(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    let n = w * h;
    let pp = cx.gpu.buffer(2 * n);
    map(cx, "guided_pre", n, &[], [Some(p), None, None], &pp);
    let m = gaussian(cx, &pp, w, h, 2, sigma);
    let ab = cx.gpu.buffer(2 * n);
    map(cx, "guided_ab", n, &[eps.to_bits()], [Some(&m), None, None], &ab);
    gaussian(cx, &ab, w, h, 2, sigma)
}

/// Fast guided filter (`local::guided_fast`): coefficients on a subsampled grid.
fn guided_fast(cx: &mut Cx<'_>, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    let s = local::guided_fast_step(sigma);
    if s <= 1 {
        return guided(cx, p, w, h, sigma, eps);
    }
    let (lw, lh) = (w.div_ceil(s).max(1), h.div_ceil(s).max(1));
    let lo = resize(cx, p, (w, h), (lw, lh), 1, Filter::Box);
    let ab = guided_coeffs(cx, &lo, lw, lh, sigma / s as f32, eps);
    let up = resize(cx, &ab, (lw, lh), (w, h), 2, Filter::Bilinear);
    let q = cx.gpu.buffer(w * h);
    map(cx, "guided_apply", w * h, &[], [Some(p), Some(&up), None], &q);
    q
}

// ---------------------------------------------------------------------------------------------
// Geometry (`Frame::sample`)

/// Integer pixel map `(x, y) → (m0·x + m1·y + m2, m3·x + m4·y + m5)` from an oriented image back
/// to the source of `w × h` (`Image::oriented`: optional horizontal flip, then quarter turns).
pub(crate) fn orient_map(o: Orientation, w: usize, h: usize) -> [i32; 6] {
    let (flip, turns) = o.to_parts();
    let (w, h) = (w as i32, h as i32);
    let mut ops: Vec<fn(i32, i32) -> [i32; 6]> = Vec::new();
    if flip {
        ops.push(|w, _| [-1, 0, w - 1, 0, 1, 0]);
    }
    let cw: fn(i32, i32) -> [i32; 6] = |_, h| [0, 1, 0, -1, 0, h - 1];
    let half: fn(i32, i32) -> [i32; 6] = |w, h| [-1, 0, w - 1, 0, -1, h - 1];
    match turns {
        1 => ops.push(cw),
        2 => ops.push(half),
        3 => {
            ops.push(half);
            ops.push(cw);
        }
        _ => {}
    }
    // compose: T ← T ∘ op, tracking each op's input size
    let mut t = [1, 0, 0, 0, 1, 0];
    let (mut cw_, mut ch_) = (w, h);
    for op in ops {
        let o = op(cw_, ch_);
        t = [
            t[0] * o[0] + t[1] * o[3],
            t[0] * o[1] + t[1] * o[4],
            t[0] * o[2] + t[1] * o[5] + t[2],
            t[3] * o[0] + t[4] * o[3],
            t[3] * o[1] + t[4] * o[4],
            t[3] * o[2] + t[4] * o[5] + t[5],
        ];
        if o[0] == 0 {
            (cw_, ch_) = (ch_, cw_);
        }
    }
    t
}

fn affine_bits(a: &lightcraft_geom::Affine) -> [u32; 6] {
    a.0.map(|v| (v as f32).to_bits())
}

/// The `sample_warp` kernel's parameters (layout documented in `geom.wgsl`).
fn warp_params(
    wp: &lightcraft_pipeline::optics::Warp,
    base: (usize, usize),
    out: (usize, usize),
    o2t: &lightcraft_geom::Affine,
    sx: f64,
    sy: f64,
) -> Vec<u32> {
    let f = |v: f64| (v as f32).to_bits();
    let mut p = vec![base.0 as u32, base.1 as u32, out.0 as u32, out.1 as u32];
    p.extend(affine_bits(o2t));
    p.extend([f(sx), f(sy), f(wp.w), f(wp.h)]);
    let persp = wp.persp != lightcraft_geom::Homography::IDENTITY;
    p.push(persp as u32);
    p.extend(wp.persp_inv.0.map(f));
    p.push(f(wp.k1));
    p.extend(wp.ca.map(f));
    p.push(f(wp.lens_dist));
    let warp = wp.lens.and_then(|l| l.warp);
    p.push(warp.is_some() as u32);
    let w = warp.unwrap_or_default();
    p.extend(w.planes.iter().flatten().map(|v| f(*v)));
    p.extend([f(w.center.x), f(w.center.y), f(w.radius)]);
    p.extend([f(wp.vig_stops), f(wp.vig_power), f(wp.lens_vig)]);
    let vig = wp.lens.and_then(|l| l.vignette);
    p.push(vig.is_some() as u32);
    let v = vig.unwrap_or_default();
    p.extend(v.k.map(f));
    p.extend([f(v.center.x), f(v.center.y), f(v.radius)]);
    p.push(wp.per_channel() as u32);
    p.push(wp.has_gain() as u32);
    debug_assert_eq!(p.len(), 65);
    p
}

/// Output rows per block of [`coverage_mask`]; a block is one mask word (32 px) wide. 16 rows measured
/// ~20 % faster than 8 at 6000 × 4000 (fewer interval evaluations, few more edge pixels).
const COVER_ROWS: usize = 16;

/// The largest source (bytes on the device) a view's stages keep uploaded between renders.
const RETAIN_SOURCE_BYTES: usize = 96 << 20;

/// Original uploads across resident and compatibility callers (diagnostics and tests).
static SOURCE_UPLOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The one big source (the original a zoom window is cut from) kept on the device for all the
/// views of the photo: the Before and After windows and every pan share it, instead of each
/// view's stages holding (or re-uploading) 288 MB of a 24 MP photo. It does not count as a view's
/// stages; it is released with the buffer pool when the app idles.
static SHARED_SOURCE: Mutex<Option<(Weak<Rgb32f>, Arc<GpuSource>)>> = Mutex::new(None);

fn shared_source(gpu: &Gpu, src: &Arc<Rgb32f>, id: SourceId) -> Option<Arc<GpuSource>> {
    let mut g = SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((weak, source)) = &mut *g
        && weak.upgrade().is_some_and(|s| Arc::ptr_eq(&s, src))
    {
        if !source.allows_half_stages() && !source.can_read_exact() {
            *source = GpuSource::upload_with_storage(gpu, source.id(), src, crate::SourceStorage::F32).ok()?;
        }
        return Some(source.clone());
    }
    *g = None;
    let source = GpuSource::upload(gpu, id, src).ok()?;
    *g = Some((Arc::downgrade(src), source.clone()));
    Some(source)
}

/// Free the shared big source (see [`shared_source`]).
pub(crate) fn release_shared_source() {
    *SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// release_shared_source_for retires only the legacy original replaced by resident adoption.
/// A different photo's compatibility cache may still be serving another caller.
fn release_shared_source_for(id: SourceId) {
    let mut shared = SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner());
    if shared.as_ref().is_some_and(|(_, source)| source.id() == id) {
        *shared = None;
    }
}

/// Device bytes of the shared big source, 0 when there is none.
pub(crate) fn shared_source_bytes() -> usize {
    SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |(_, source)| source.bytes())
}

pub(crate) fn source_uploads() -> u64 {
    SOURCE_UPLOADS.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn record_source_upload() {
    SOURCE_UPLOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The reference framing decision ([`Warp::covers`](lightcraft_pipeline::optics::Warp::covers)) for every
/// output pixel, one bit per pixel, rows padded to 32-bit words. Blocks of 32 × [`COVER_ROWS`] pixels whose
/// interval bounds place them clearly inside or outside the image are filled at once
/// ([`Warp::block_coverage`](lightcraft_pipeline::optics::Warp::block_coverage), the same formulas evaluated
/// with outward-rounded intervals); only blocks near the image edge evaluate each pixel. Same bits as
/// evaluating every pixel.
fn coverage_mask(wp: &lightcraft_pipeline::optics::Warp, o2t: &lightcraft_geom::Affine, w: usize, h: usize) -> Vec<u32> {
    let words = w.div_ceil(32);
    let mut coverage = vec![0u32; words * h];
    if words == 0 {
        return coverage;
    }
    lightcraft_raster::par_rows(&mut coverage, words * COVER_ROWS, |band, rows| {
        let y0 = band * COVER_ROWS;
        let y1 = y0 + rows.len() / words;
        for word in 0..words {
            let (x0, x1) = (word * 32, (word * 32 + 32).min(w));
            let full = if x1 - x0 == 32 { u32::MAX } else { (1u32 << (x1 - x0)) - 1 };
            let block = wp.block_coverage(o2t, x0, x1, y0, y1);
            for (y, row) in (y0..y1).zip(rows.chunks_exact_mut(words)) {
                let Some(bits) = row.get_mut(word) else { continue };
                *bits = match block {
                    Some(true) => full,
                    Some(false) => 0,
                    None => (x0..x1).filter(|&x| wp.covers(o2t, x, y)).fold(0, |b, x| b | 1 << (x - x0)),
                };
            }
        }
    });
    coverage
}

/// Resample the source into the output frame (`Frame::sample`). May return the source buffer
/// itself when the frame is the identity.
fn sample(cx: &mut Cx<'_>, src: SourceExtent, src_buf: Arc<Buf>, plan: &Plan<'_>) -> Arc<Buf> {
    let (w, h) = (plan.w, plan.h);
    let fr = &plan.frame;
    let sp = fr.sample_plan(src.width, src.height, w, h);
    let oriented = if fr.orient == Orientation::Normal {
        src_buf
    } else {
        let out = cx.gpu.buffer(sp.ow * sp.oh * 3);
        let mut p = vec![src.width as u32, src.height as u32, sp.ow as u32, sp.oh as u32];
        p.extend(orient_map(fr.orient, src.width, src.height).map(|v| v as u32));
        cx.run("orient", &p, &[Some(&src_buf), Some(&out), None], groups2(sp.ow, sp.oh, [16, 16]));
        Arc::new(out)
    };
    let (base, (bw, bh)) = match sp.prefilter {
        Some(d) => (Arc::new(resize(cx, &oriented, (sp.ow, sp.oh), d, 3, Filter::Mitchell)), d),
        None => (oriented, (sp.ow, sp.oh)),
    };
    let mut affine = |xf: &lightcraft_geom::Affine| {
        let out = cx.gpu.buffer(w * h * 3);
        let mut p = vec![bw as u32, bh as u32, w as u32, h as u32];
        p.extend(affine_bits(xf));
        cx.run("sample_affine", &p, &[Some(&base), Some(&out), None], groups2(w, h, [16, 16]));
        Arc::new(out)
    };
    match (&sp.mode, fr.warp.as_ref()) {
        (SampleMode::Copy, _) => base,
        (SampleMode::Affine(xf), _) => affine(xf),
        // `sample_plan` plans a warp only when there is one; without it the same mapping is affine
        (SampleMode::Warp(o2t), None) => affine(&(lightcraft_geom::Affine::scale(sp.sx, sp.sy) * *o2t)),
        (SampleMode::Warp(o2t), Some(wp)) => {
            let out = cx.gpu.buffer(w * h * 3);
            let p = warp_params(wp, (bw, bh), (w, h), o2t, sp.sx, sp.sy);
            // f32 rounding can cross a source edge. Keep the reference f64
            // framing decision in a bit mask; color sampling stays on the GPU.
            let coverage = cx.gpu.upload(&coverage_mask(wp, o2t, w, h));
            cx.run("sample_warp", &p, &[Some(&base), Some(&out), Some(&coverage)], groups2(w, h, [16, 16]));
            Arc::new(out)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The render

/// CPU copies made along the way (for stages that still run on the CPU).
#[derive(Default)]
struct Host {
    sampled: Option<Rgb32f>,
    lin: Option<Arc<Rgb32f>>,
    log_l: Option<Arc<Plane>>,
}

pub(crate) fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIGHTCRAFT_PROFILE").is_some())
}

/// Render on `gpu`, reusing `stages` (if given). `None` (or a result the caller discards) when the
/// render failed: the reason is recorded with [`crate::ctx::fail`].
pub fn render(
    gpu: &Gpu,
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&GpuStages>,
    fault: Option<crate::Fault>,
    retain_mapping: bool,
) -> Option<Rendered<DisplayImage>> {
    let plan = lightcraft_pipeline::plan(src, info, s, req);
    let (id, cached_source) = match stages {
        Some(cache) => cache.legacy_source(gpu, src)?,
        None => (SourceId::new(), None),
    };
    let original = if let Some(source) = cached_source {
        Some(source)
    } else if gpu.fits(src.data.len().checked_mul(3)?) {
        Some(if stages.is_some() { shared_source(gpu, src, id)? } else { GpuSource::upload(gpu, id, src).ok()? })
    } else {
        None
    };
    let id = original.as_ref().map_or(id, |source| source.id());
    render_plan(
        gpu,
        id,
        SourceExtent { width: src.width, height: src.height },
        original.as_ref(),
        Some(src),
        info,
        &plan,
        req,
        stages,
        fault,
        retain_mapping,
        original.as_ref().is_none_or(|source| source.allows_half_stages()),
    )
}

/// render_prepared renders an already-resolved plan without retaining or reading CPU originals.
pub(crate) fn render_prepared(
    gpu: &Gpu,
    source: &Arc<GpuSource>,
    info: &SourceInfo,
    plan: &Plan<'_>,
    req: &RenderRequest,
    stages: Option<&GpuStages>,
    fault: Option<crate::Fault>,
    retain_mapping: bool,
) -> Option<Rendered<DisplayImage>> {
    if !source.allows_half_stages() && !source.can_read_exact() {
        fail(FailKind::Precision, "half storage was rejected; reload exact original and retry f32".into());
        return None;
    }
    render_plan(gpu, source.id(), source.extent(), Some(source), None, info, plan, req, stages, fault, retain_mapping, source.allows_half_stages())
}

/// render_plan shares all pixel work between CPU-owned compatibility and resident sources.
fn render_plan(
    gpu: &Gpu,
    id: SourceId,
    extent: SourceExtent,
    original: Option<&Arc<GpuSource>>,
    host_source: Option<&Arc<Rgb32f>>,
    info: &SourceInfo,
    plan: &Plan<'_>,
    req: &RenderRequest,
    stages: Option<&GpuStages>,
    fault: Option<crate::Fault>,
    retain_mapping: bool,
    allow_half: bool,
) -> Option<Rendered<DisplayImage>> {
    let mut t = profiling().then(std::time::Instant::now);
    // Each stage is submitted on its own; under `LIGHTCRAFT_PROFILE` also waited for, so the
    // timings are real.
    let lap = |what: &str, t: &mut Option<std::time::Instant>, cx: &mut Cx<'_>| match t {
        Some(t) => {
            cx.sync();
            eprintln!("  gpu {what}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            *t = std::time::Instant::now();
        }
        None => cx.flush(),
    };
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let _limit = match fault {
        Some(crate::Fault::Limit(bytes)) => Some(crate::ctx::LimitOverride::new(bytes)),
        _ => None,
    };
    if !gpu.fits(n * 3) {
        fail(
            FailKind::Limit,
            format!("{w}×{h} needs {} MiB buffers, over the device's {} MiB storage-buffer limit", (n * 12).div_ceil(1 << 20), gpu.limit() >> 20),
        );
        return None;
    }
    let s = &*plan.settings;
    let cached = stages.and_then(|c| c.get(id, plan.geo)).filter(|entry| original.is_none_or(|source| entry.belongs_to(source)));
    let mut host = Host::default();
    let mut cx = Cx::new(gpu);
    cx.storage_enabled = allow_half;
    lap("plan", &mut t, &mut cx);

    // 1. geometry (on the CPU when the source exceeds the device's buffer limit)
    let sampled = match &cached {
        Some(e) => e.sampled.clone(),
        None => match original {
            Some(source) => sample(&mut cx, extent, source.buffer.clone(), plan),
            None => {
                let img = plan.frame.sample(host_source?, w, h);
                let b = Arc::new(gpu.upload(rgb_words(&img)));
                host.sampled = Some(img);
                b
            }
        },
    };
    lap("sample", &mut t, &mut cx);

    // 2. white balance, defringe, spots, noise reduction
    let lin = match cached.as_ref().and_then(|e| e.lin.clone()).filter(|(k, b)| *k == plan.lin_key && (allow_half || b.half_len.is_none())) {
        Some((_, b)) => b,
        None => {
            let b = linear(&mut cx, &sampled, info, &plan, &mut host);
            Arc::new(cx.compact(b, false))
        }
    };
    lap("wb/nr", &mut t, &mut cx);

    // 3. spatial planes
    let mut planes = match cached.map(|e| e.planes) {
        Some(p) if p.key == plan.lin_key && (allow_half || p.is_f32()) => p,
        _ => Planes { key: plan.lin_key, ..Default::default() },
    };
    let mut prep = prepare(&mut cx, &lin, &plan, req, &mut planes);
    // a window dehazes with the whole frame's airlight
    if let Some(a) = plan.fixed_air {
        prep.air = a;
    }
    lap("planes", &mut t, &mut cx);
    let pending = Entry { src: id, original: original.map(Arc::downgrade), geo: plan.geo, sampled, lin: Some((plan.lin_key, lin.clone())), planes };

    // 4. masks
    let (masks, terms) = masks(&mut cx, &lin, &prep, &plan, &mut host);
    lap("masks", &mut t, &mut cx);

    // 5. per-pixel stage
    let fp = FinishParams::new(s, &plan.frame, info, w, h, plan.px_per_long, prep.air, req.space);
    let present = Present {
        clarity: prep.clarity.is_some(),
        texture: prep.texture.is_some(),
        dark: prep.dark.is_some(),
        chroma: masks.is_some() && prep.chroma.is_some(),
    };
    let (p, aux) = finish_block(&fp, &terms, &present);
    let aux = gpu.upload(&aux);
    // cleared, so that work which never ran cannot pass for an image (see the check below)
    let has_half_status = cx.half_status.is_some();
    let out = cx.zeroed(n + usize::from(has_half_status));
    if fault == Some(crate::Fault::Validation) {
        // a dispatch the device rejects (too many workgroups)
        cx.run("box_h", &[0; 5], &[Some(&aux), Some(&out)], [gpu.device.limits().max_compute_workgroups_per_dimension + 1, 1, 1]);
    }
    // in bands of rows: each dispatch (and submission) stays short on a slow GPU
    let rows = (BAND_PIXELS / w.max(1) / 16 * 16).max(16); // whole workgroups: bands never overlap
    let y0_at = crate::params::index("Y0").unwrap_or(0);
    let mut p = p;
    for y0 in (0..h).step_by(rows).filter(|_| fault != Some(crate::Fault::DropWork)) {
        p[y0_at] = y0 as u32;
        cx.run(
            gpu.finish_kernel(terms.is_empty()),
            &p,
            &[
                Some(&lin),
                Some(&prep.log_l),
                Some(&prep.base),
                prep.clarity.as_deref(),
                prep.texture.as_deref(),
                prep.dark.as_deref(),
                masks.as_ref(),
                Some(&aux),
                Some(&out),
            ],
            groups2(w, rows.min(h - y0), [16, 16]),
        );
    }
    // the alpha a mask overlay shows: copied out before the readback below submits
    let overlay_mask = req.overlay.mask(s).map(|m| {
        let list = s.masks.iter().filter(|m| m.visible && !m.components.is_empty());
        match (list.map(|m| m.id).position(|id| id == m.id), masks.as_ref()) {
            (Some(mi), Some(all)) => Ok(cx.slice(all, mi * n, n)),
            _ => Err(m),
        }
    });
    let post_color = lightcraft_pipeline::post_color_half(req) && req.overlay == lightcraft_pipeline::Overlay::None;
    let stats_words = if post_color { crate::reduced_stats::WORDS } else { 0 };
    let (mut reduced, mut reduced_stats) = (None, None);
    let (read_w, read_h, read_keep) = if post_color {
        let (x, y, sw, sh) = plan.keep.map_or((0, 0, w, h), |k| (k.x, k.y, k.w, k.h));
        let (dw, dh) = (sw.div_ceil(2), sh.div_ceil(2));
        let output = cx.zeroed(dw * dh + usize::from(has_half_status) + stats_words);
        let stats = crate::reduced_stats::ReducedStats::new(&mut cx, dw, dh);
        let params = [w, h, x, y, sw, sh, dw, dh, n, stats.step].map(|v| v as u32);
        let tile = 16 * crate::reduced_stats::TILE;
        cx.run("display_reduce_checked_stats", &params, &[Some(&out), Some(&output), Some(&stats.buffer)], groups2(dw, dh, [tile, tile]));
        cx.copy_into(&stats.buffer, &output, dw * dh + usize::from(has_half_status));
        reduced_stats = Some(stats);
        reduced = Some(output);
        (dw, dh, None)
    } else {
        (w, h, plan.keep)
    };
    let read_n = read_w * read_h;
    let read_out = reduced.as_ref().unwrap_or(&out);
    if let Some(status) = cx.half_status.take() {
        cx.copy_into(&status, read_out, read_n);
    }
    let readback = cx.map(read_out, read_n + usize::from(has_half_status) + stats_words, read_keep.map_or(0, |k| k.x))?;
    lap("finish + readback", &mut t, &mut cx);
    if crate::ctx::failed() {
        return None;
    }
    // The per-pixel kernel writes alpha 255 everywhere and `out` was cleared: a pixel with any
    // other alpha means work silently did not run (a driver that reset or dropped a submission
    // without reporting it — issue #78's all-black exports).
    let checked = readback.with_bytes(|bytes| {
        // Validate the range word in the first completed mapping view, before inspecting pixels.
        if has_half_status && bytes[read_n * 4..read_n * 4 + 4].iter().any(|byte| *byte != 0) {
            return Ok(None);
        }
        if let Some(stats) = &reduced_stats {
            let at = (read_n + usize::from(has_half_status)) * 4;
            return stats.decode(&bytes[at..at + stats_words * 4]).map(Some);
        }
        let pixels: &[[u8; 4]] = bytemuck::cast_slice(&bytes[..read_n * 4]);
        let missing = unwritten(pixels);
        let histogram = match read_keep {
            Some(k) => Histogram::of_srgb8_view(k.w, k.h, |x, y| pixels[(k.y + y) * read_w + k.x + x]),
            None => Histogram::of_srgb8_view(read_w, read_h, |x, y| pixels[y * read_w + x]),
        };
        Ok(Some((missing, histogram)))
    });
    let (unwritten, histogram) = match checked {
        Ok(Ok(Some(value))) => value,
        Ok(Ok(None)) => {
            // None of this candidate has been published. Release its mapping and derived
            // storage before obtaining an exact source or asking the engine to reload it.
            drop(overlay_mask);
            drop(fp);
            drop((readback, cx, pending, lin, prep, masks, aux, out, host, reduced, reduced_stats));
            if let Some(source) = original {
                source.require_f32();
            }
            if original.is_none_or(|source| source.can_read_exact()) {
                return render_plan(gpu, id, extent, original, host_source, info, plan, req, stages, fault, retain_mapping, false);
            }
            let Some(host_source) = host_source else {
                fail(FailKind::Precision, "half storage range exceeded; reload exact original and retry f32".into());
                return None;
            };
            let exact = GpuSource::upload_with_storage(gpu, id, host_source, crate::SourceStorage::F32).ok()?;
            let rendered = render_plan(gpu, id, extent, Some(&exact), Some(host_source), info, plan, req, stages, fault, retain_mapping, false);
            if rendered.is_some() {
                if let Some(cache) = stages {
                    cache.adopt_source(&exact);
                    let device = (exact.bytes() <= RETAIN_SOURCE_BYTES).then(|| exact.clone());
                    *cache.source.lock().unwrap_or_else(|e| e.into_inner()) = Some(LegacySource { host: Arc::downgrade(host_source), id, device });
                    if exact.bytes() > RETAIN_SOURCE_BYTES {
                        *SHARED_SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Arc::downgrade(host_source), exact));
                    }
                }
            }
            return rendered;
        }
        Ok(Err(reason)) | Err(reason) => {
            fail(FailKind::Fatal, reason);
            return None;
        }
    };
    if unwritten > 0 {
        fail(FailKind::Fatal, format!("the GPU returned an incomplete image ({unwritten} of {n} pixels unwritten)"));
        return None;
    }
    lap("histogram", &mut t, &mut cx);
    // An entirely black result (upstream work that did not run would give that too): redo it on
    // the CPU, which costs time only for the rare photo that really is black.
    if n >= 4096 && [&histogram.r, &histogram.g, &histogram.b].iter().all(|c| c.first() == Some(&histogram.total)) {
        fail(FailKind::Blank, "the GPU result is entirely black; rendering it on the CPU to be sure".into());
        return None;
    }
    if let Some(cache) = stages {
        cache.put(pending);
    }
    if post_color {
        let image = DisplayImage::mapped(readback, read_w, read_h, read_w * 4, 0);
        let image = if retain_mapping {
            image
        } else {
            match image.into_owned() {
                Ok(image) => image.into(),
                Err(reason) => {
                    fail(FailKind::Fatal, reason);
                    return None;
                }
            }
        };
        return Some(Rendered { display_size: plan.display_size, image, histogram, deep: None });
    }
    if retain_mapping && req.overlay == lightcraft_pipeline::Overlay::None {
        let (width, height, offset) = plan.keep.map_or((w, h, 0), |k| (k.w, k.h, (k.y * w + k.x) * 4));
        let image = DisplayImage::mapped(readback, width, height, w * 4, offset);
        return Some(Rendered { display_size: plan.display_size, image, histogram, deep: None });
    }
    let mut image = match DisplayImage::mapped(readback, w, h, w * 4, 0).into_owned() {
        Ok(image) => image,
        Err(reason) => {
            fail(FailKind::Fatal, reason);
            return None;
        }
    };
    let overlay_mask = overlay_mask.map(|r| match r {
        Ok(b) => cx.read_plane(&b, w, h),
        // a hidden mask: evaluate it on the CPU
        Err(m) => {
            let img = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(&lin, w, h))).clone();
            let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(&prep.log_l, w, h))).clone();
            lightcraft_pipeline::masks::evaluate_one(m, &plan.frame, w, h, &img, &l, s.light.exposure as f32, plan.mattes.as_deref())
        }
    });
    let histogram = if lightcraft_pipeline::post_color_half(req) {
        let visible = plan.keep.map_or_else(|| image.clone(), |k| image.crop(k.x, k.y, k.w, k.h));
        Histogram::of_srgb8(&lightcraft_pipeline::reduce_display_half(&visible))
    } else {
        histogram
    };
    lightcraft_pipeline::visualize::apply(&mut image, req.overlay, &plan, overlay_mask.as_ref());
    // spots grew the window the render worked on: cut it back to the request
    if let Some(k) = plan.keep {
        image = image.crop(k.x, k.y, k.w, k.h);
    }
    let image = if lightcraft_pipeline::post_color_half(req) { lightcraft_pipeline::reduce_display_half(&image) } else { image };
    Some(Rendered { display_size: plan.display_size, image: image.into(), histogram, deep: None })
}

/// Pixels whose alpha is not 255 (cheap: no early exit, so it vectorizes).
fn unwritten(data: &[[u8; 4]]) -> usize {
    data.chunks(4096).filter(|c| c.iter().fold(255u8, |a, p| a & p[3]) != 255).map(|c| c.iter().filter(|p| p[3] != 255).count()).sum()
}

/// The white-balanced, retouched, denoised image.
fn linear(cx: &mut Cx<'_>, sampled: &Buf, info: &SourceInfo, plan: &Plan<'_>, host: &mut Host) -> Buf {
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let s = &*plan.settings;
    let img = if lightcraft_pipeline::lin_needs_cpu(s) {
        // defringe / spot removal: CPU
        let mut img = match host.sampled.take() {
            Some(i) => i,
            None => cx.read_rgb(sampled, w, h),
        };
        lightcraft_pipeline::lin_cpu(&mut img, info, plan);
        cx.gpu.upload(rgb_words(&img))
    } else {
        let m = local::wb_matrix_for(info, s);
        let mut p = vec![m.is_some() as u32];
        p.extend(m.unwrap_or_default().iter().flatten().map(|v| v.to_bits()));
        let out = cx.gpu.buffer(n * 3);
        map(cx, "wb_k", n, &p, [Some(sampled), None, None], &out);
        if plan.eyes.is_empty() {
            out
        } else {
            let mut p = vec![w as u32, plan.eyes.len() as u32];
            p.extend(plan.eyes.iter().flat_map(|e| e.words()).map(f32::to_bits));
            let eyed = cx.gpu.buffer(n * 3);
            map(cx, "redeye_k", n, &p, [Some(&out), None, None], &eyed);
            eyed
        }
    };
    denoise(cx, img, plan)
}

/// Noise reduction (`local::denoise`).
fn denoise(cx: &mut Cx<'_>, img: Buf, plan: &Plan<'_>) -> Buf {
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let (lum, col) = local::nr_params(&plan.settings, plan.src_long, plan.frame.output_long(w, h));
    let mut img = img;
    if let Some(nr) = lum {
        let l = cx.gpu.buffer(n);
        map(cx, "log_lum_k", n, &[], [Some(&img), None, None], &l);
        let f = guided(cx, &l, w, h, nr.sigma, nr.eps);
        let out = cx.gpu.buffer(n * 3);
        map(cx, "nr_lum", n, &[nr.k.to_bits()], [Some(&img), Some(&l), Some(&f)], &out);
        img = out;
    }
    if let Some(nr) = col {
        let chroma = cx.gpu.buffer(n * 3);
        map(cx, "chroma_k", n, &[], [Some(&img), None, None], &chroma);
        let b = gaussian(cx, &chroma, w, h, 3, nr.sigma);
        let out = cx.gpu.buffer(n * 3);
        map(cx, "nr_col", n, &[nr.t.to_bits()], [Some(&img), Some(&chroma), Some(&b)], &out);
        img = out;
    }
    img
}

/// The planes the per-pixel stage reads.
struct Prep {
    log_l: Arc<Buf>,
    base: Arc<Buf>,
    clarity: Option<Arc<Buf>>,
    texture: Option<Arc<Buf>>,
    dark: Option<Arc<Buf>>,
    /// Blurred chromaticity (local Moiré / Noise).
    chroma: Option<Arc<Buf>>,
    air: f32,
}

fn plane_at(slot: &mut Option<(u32, Arc<Buf>)>, sigma: f32, cx: &mut Cx<'_>, f: impl FnOnce(&mut Cx<'_>) -> Buf) -> Arc<Buf> {
    match slot {
        Some((k, p)) if *k == sigma.to_bits() => p.clone(),
        _ => {
            let b = f(cx);
            let p = Arc::new(cx.compact(b, true));
            *slot = Some((sigma.to_bits(), p.clone()));
            p
        }
    }
}

/// The spatial planes (`local::prepare`), reusing those in `planes`.
fn prepare(cx: &mut Cx<'_>, lin: &Buf, plan: &Plan<'_>, req: &RenderRequest, planes: &mut Planes) -> Prep {
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let sig = local::plane_sigmas(&plan.settings, plan.px_per_long, req.quality);
    let log_l = match &planes.log_l {
        Some(b) => b.clone(),
        None => {
            let l = cx.gpu.buffer(n);
            map(cx, "log_lum_k", n, &[], [Some(lin), None, None], &l);
            let b = Arc::new(cx.compact(l, true));
            planes.log_l = Some(b.clone());
            b
        }
    };
    let base = match sig.base {
        Some(sg) => plane_at(&mut planes.base, sg, cx, |cx| guided_fast(cx, &log_l, w, h, sg, local::BASE_EPS)),
        None => log_l.clone(),
    };
    let clarity = sig.clarity.map(|sg| plane_at(&mut planes.clarity, sg, cx, |cx| guided_fast(cx, &log_l, w, h, sg, local::CLARITY_EPS)));
    let texture = sig.texture.map(|sg| plane_at(&mut planes.texture, sg, cx, |cx| gaussian(cx, &log_l, w, h, 1, sg)));
    let (dark, air) = match sig.dark {
        None => (None, 1.0),
        Some(sg) => match &planes.dark {
            Some((k, d, air)) if *k == sg.to_bits() => (Some(d.clone()), *air),
            _ => {
                let d0 = cx.gpu.buffer(n);
                map(cx, "dark_k", n, &[], [Some(lin), None, None], &d0);
                let d = gaussian(cx, &d0, w, h, 1, sg);
                // airlight: a percentile of every 7th value, as on the CPU
                let m = n.div_ceil(local::AIRLIGHT_STEP);
                let sub = cx.gpu.buffer(m);
                map(cx, "subsample", m, &[local::AIRLIGHT_STEP as u32], [Some(&d), None, None], &sub);
                let air = local::airlight_of(cx.read(&sub, m));
                cx.flush();
                let b = Arc::new(cx.compact(d, true));
                planes.dark = Some((sg.to_bits(), b.clone(), air));
                (Some(b), air)
            }
        },
    };
    let chroma = sig.chroma.map(|sg| {
        plane_at(&mut planes.chroma, sg, cx, |cx| {
            let ch = cx.gpu.buffer(n * 3);
            map(cx, "chroma_k", n, &[], [Some(lin), None, None], &ch);
            gaussian(cx, &ch, w, h, 3, sg)
        })
    });
    Prep { log_l, base, clarity, texture, dark, chroma, air }
}

/// Most brush dabs the mask kernel evaluates per pixel (more: the CPU rasterizes the brush).
const MAX_DABS: usize = 4096;

/// The `shape` kernel's brush data for `dabs`: stroke records (12 words: dab offset, dab count,
/// r, hard, flow, density, erase, bbox x0 y0 x1 y1, auto) then the dab centres.
fn brush_aux(dabs: &[&lightcraft_pipeline::masks::BrushDabs], erase: bool) -> Vec<f32> {
    let mut aux = Vec::new();
    let mut off = 12 * dabs.len();
    for d in dabs {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for q in &d.dabs {
            (x0, y0, x1, y1) = (x0.min(q.x - d.r), y0.min(q.y - d.r), x1.max(q.x + d.r), y1.max(q.y + d.r));
        }
        aux.extend([off as f32, d.dabs.len() as f32, d.r as f32, d.hard as f32, d.flow, d.density, (erase && d.erase) as u8 as f32]);
        aux.extend([x0, y0, x1, y1].map(|v| v as f32));
        aux.push(d.auto as u8 as f32);
        off += 2 * d.dabs.len();
    }
    for d in dabs {
        aux.extend(d.dabs.iter().flat_map(|q| [q.x as f32, q.y as f32]));
    }
    aux
}

/// Cross guided filter (`masks::guided_cross`) of `p` steered by `guide`, max-combined with `p`.
fn guided_cross_max(cx: &mut Cx<'_>, guide: &Buf, p: &Buf, w: usize, h: usize, sigma: f32, eps: f32) -> Buf {
    let n = w * h;
    let (m0, m1) = (cx.gpu.buffer(2 * n), cx.gpu.buffer(2 * n));
    map(cx, "xguided_pre", n, &[0], [Some(guide), Some(p), None], &m0);
    map(cx, "xguided_pre", n, &[1], [Some(guide), Some(p), None], &m1);
    let (b0, b1) = (gaussian(cx, &m0, w, h, 2, sigma), gaussian(cx, &m1, w, h, 2, sigma));
    let ab = cx.gpu.buffer(2 * n);
    map(cx, "xguided_ab", n, &[eps.to_bits()], [Some(&b0), Some(&b1), None], &ab);
    let ab = gaussian(cx, &ab, w, h, 2, sigma);
    let q = cx.gpu.buffer(n);
    map(cx, "xguided_apply", n, &[], [Some(guide), Some(&ab), Some(p)], &q);
    q
}

/// Evaluate the masks (`lightcraft_pipeline::masks::evaluate`): their alpha planes (concatenated,
/// followed by the blurred chromaticity when local Moiré / Noise need it) and their adjustment
/// terms. Shapes without a kernel (Sky, Subject, …) run on the CPU.
fn masks(cx: &mut Cx<'_>, lin: &Buf, prep: &Prep, plan: &Plan<'_>, host: &mut Host) -> (Option<Buf>, Vec<[f32; MASK_TERMS]>) {
    use lightcraft_develop::{MaskOp, MaskShape};
    let s = &*plan.settings;
    let list: Vec<_> = s.masks.iter().filter(|m| m.visible && !m.components.is_empty()).collect();
    if list.is_empty() {
        return (None, Vec::new());
    }
    let (w, h) = (plan.w, plan.h);
    let n = w * h;
    let frame = &plan.frame;
    let ev = s.light.exposure as f32;
    let long = frame.ow.max(frame.oh);
    let [a, b, c, d, e, f] = frame.out_to_norm(w, h).0;
    let (kx, ky) = (frame.ow / long, frame.oh / long);
    let aff = [a * kx, b * ky, c * kx, d * ky, e * kx, f * ky].map(|v| (v as f32).to_bits());
    let alpha = cx.zeroed(n * list.len() + if prep.chroma.is_some() { 3 * n } else { 0 });
    if let Some(ch) = &prep.chroma {
        cx.copy_into(ch, &alpha, n * list.len());
    }
    let comp_plane = cx.gpu.buffer(n);
    for (mi, m) in list.iter().enumerate() {
        let mut first = true;
        for comp in &m.components {
            let op = match comp.op {
                _ if first && comp.op != MaskOp::Intersect => 0u32,
                MaskOp::Add => 1,
                MaskOp::Subtract => 2,
                MaskOp::Intersect => 3,
            };
            first = false;
            let mut p = vec![w as u32, h as u32, 0, comp.invert as u32];
            p.extend_from_slice(&aff);
            let f32s = |v: &[f64]| v.iter().map(|x| (*x as f32).to_bits()).collect::<Vec<u32>>();
            let mut aux: Vec<f32> = Vec::new();
            let mut own: Option<Buf> = None;
            let kind = match &comp.shape {
                MaskShape::Linear { start, end } => {
                    let (a, b) = (frame.norm_to_long(*start), frame.norm_to_long(*end));
                    let d = b - a;
                    p.extend(f32s(&[a.x, a.y, d.x, d.y, d.dot(d).max(1e-12)]));
                    Some(0)
                }
                MaskShape::Radial { center, rx, ry, angle, feather, invert } => {
                    let c = frame.norm_to_long(*center);
                    let (sn, co) = (-angle.to_radians()).sin_cos();
                    p.extend(f32s(&[c.x, c.y, co, sn, rx.max(1e-6), ry.max(1e-6), (*feather / 100.0).clamp(0.0, 1.0)]));
                    p.push(*invert as u32);
                    Some(1)
                }
                MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
                    p.extend(f32s(&[*lo, *hi, lo_feather.max(1e-3), hi_feather.max(1e-3)]));
                    p.push(ev.to_bits());
                    Some(2)
                }
                MaskShape::ColorRange { samples, refine } => {
                    let tol = lightcraft_pipeline::masks::color_range_tolerance(*refine);
                    p.extend([tol.to_bits(), ev.exp2().to_bits(), samples.len() as u32]);
                    aux.extend(samples.iter().flat_map(|s| s.map(|v| v as f32)));
                    Some(3)
                }
                MaskShape::Brush { strokes } => {
                    let dabs: Vec<_> = strokes.iter().map(|st| lightcraft_pipeline::masks::brush_dabs(st, frame, w, h)).collect();
                    if dabs.iter().map(|d| d.dabs.len()).sum::<usize>() > MAX_DABS {
                        None
                    } else if dabs.iter().any(|d| d.auto) {
                        // Auto Mask: stroke by stroke (each one refined before it's combined)
                        let brush = cx.zeroed(n);
                        let stroke = cx.gpu.buffer(n);
                        for d in &dabs {
                            let mut q = p.clone();
                            q[2] = 4;
                            q[3] = 0;
                            q.push(1);
                            let aux = cx.gpu.upload(&brush_aux(&[d], false));
                            cx.run("shape", &q, &[Some(lin), Some(&prep.log_l), Some(&aux), Some(&stroke), Some(&brush)], groups2(w, h, [16, 16]));
                            let refined = d.auto.then(|| {
                                let (sigma, eps) = lightcraft_pipeline::masks::auto_refine(d.r);
                                guided_cross_max(cx, &prep.log_l, &stroke, w, h, sigma, eps)
                            });
                            let op = if d.erase { 2 } else { 1 };
                            cx.run(
                                "combine",
                                &[n as u32, 0, 0, op],
                                &[None, None, None, Some(refined.as_ref().unwrap_or(&stroke)), Some(&brush)],
                                groups1(n),
                            );
                        }
                        if comp.invert {
                            cx.run("finalize", &[n as u32, 0, 0, 1, 1f32.to_bits()], &[None, None, None, Some(&stroke), Some(&brush)], groups1(n));
                        }
                        own = Some(brush);
                        None
                    } else {
                        p.push(dabs.len() as u32);
                        aux = brush_aux(&dabs.iter().collect::<Vec<_>>(), true);
                        Some(4)
                    }
                }
                _ => None,
            };
            let plane = match kind {
                _ if own.is_some() => own,
                Some(k) => {
                    p[2] = k;
                    let aux = (!aux.is_empty()).then(|| cx.gpu.upload(&aux));
                    cx.run("shape", &p, &[Some(lin), Some(&prep.log_l), aux.as_ref(), Some(&comp_plane), Some(&alpha)], groups2(w, h, [16, 16]));
                    None
                }
                None => {
                    // no kernel: evaluate on the CPU
                    let img = host.lin.get_or_insert_with(|| Arc::new(cx.read_rgb(lin, w, h))).clone();
                    let l = host.log_l.get_or_insert_with(|| Arc::new(cx.read_plane(&prep.log_l, w, h))).clone();
                    let mut v = lightcraft_pipeline::masks::shape_alpha(&comp.shape, frame, w, h, &img, &l, ev, plan.mattes.as_deref());
                    if comp.invert {
                        v.data.iter_mut().for_each(|x| *x = 1.0 - *x);
                    }
                    Some(cx.gpu.upload(&v.data))
                }
            };
            let cbuf = plane.as_ref().unwrap_or(&comp_plane);
            let k = [n as u32, 0, mi as u32, op];
            cx.run("combine", &k, &[None, None, None, Some(cbuf), Some(&alpha)], groups1(n));
        }
        let amt = (m.adjust.amount / 100.0) as f32;
        cx.run(
            "finalize",
            &[n as u32, 0, mi as u32, m.invert as u32, amt.to_bits()],
            &[None, None, None, Some(&comp_plane), Some(&alpha)],
            groups1(n),
        );
    }
    let terms = list.iter().map(|m| mask_terms(&m.adjust)).collect();
    (Some(alpha), terms)
}

#[cfg(test)]
mod tests {
    #[test]
    fn resident_adoption_releases_legacy_originals_and_aliases() {
        let Some(gpu) = crate::device() else {
            eprintln!("skipped: no GPU adapter for resident adoption");
            return;
        };
        let image = Arc::new(lightcraft_scenes::demo_library()[2].render(101, 79));
        let info = SourceInfo::default();
        let settings = DevelopSettings::default();
        let request = RenderRequest::fit(101, 79);
        for shared in [false, true] {
            let cache = lightcraft_pipeline::StageCache::default();
            let stages = cache.extension::<GpuStages>();
            let expected = crate::render(&image, &info, &settings, &request, Some(&cache)).unwrap();
            let previous = stages.source.lock().unwrap().as_ref().unwrap().device.as_ref().unwrap().clone();
            let old_source = Arc::downgrade(&previous);
            let old_buffer = Arc::downgrade(&previous.buffer);
            if shared {
                *SHARED_SOURCE.lock().unwrap() = Some((Arc::downgrade(&image), previous.clone()));
                stages.source.lock().unwrap().as_mut().unwrap().device = None;
            }
            drop(previous);
            let resident = crate::upload_source(SourceId::new(), &image).unwrap();
            let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
            let actual = crate::render_prepared(&resident, &info, &plan, &request, Some(&cache), true).unwrap();
            assert_eq!(actual.image.into_owned().unwrap().data, expected.image.data);
            assert!(stages.source.lock().unwrap().is_none(), "successful adoption must remove the legacy source slot");
            assert!(stages.lock().iter().all(|entry| entry.src == resident.id()), "old identity stages can retain original aliases");
            assert!(old_source.upgrade().is_none(), "legacy source ownership must end, including the shared compatibility cache");
            assert!(old_buffer.upgrade().is_none(), "identity-sampled stages must not keep the previous original alive");
        }
        let cache = lightcraft_pipeline::StageCache::default();
        let id = SourceId::new();
        let original = crate::upload_source(id, &image).unwrap();
        let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
        crate::render_prepared(&original, &info, &plan, &request, Some(&cache), true).unwrap();
        let smaller = RenderRequest::fit(73, 59);
        let smaller_plan = lightcraft_pipeline::plan(&image, &info, &settings, &smaller);
        crate::render_prepared(&original, &info, &smaller_plan, &smaller, Some(&cache), true).unwrap();
        let old_buffer = Arc::downgrade(&original.buffer);
        drop(original);
        let recovered = crate::upload_source(id, &image).unwrap();
        crate::render_prepared(&recovered, &info, &smaller_plan, &smaller, Some(&cache), true).unwrap();
        assert!(old_buffer.upgrade().is_none(), "re-uploading the same content must retire an old original alias");
        gpu.trim(0);
    }

    #[test]
    fn source_alias_bytes_are_counted_once_across_owner_release() {
        let Some(gpu) = crate::device() else {
            eprintln!("skipped: no GPU adapter for source accounting");
            return;
        };
        let image = lightcraft_raster::Rgb32f { width: 3, height: 3, data: vec![[0.5; 3]; 9].into() };
        let source = crate::upload_source(lightcraft_pipeline::SourceId::new(), &image).unwrap();
        let stages = super::GpuStages::default();
        stages.put(super::Entry {
            src: source.id(),
            original: Some(std::sync::Arc::downgrade(&source)),
            geo: 1,
            sampled: source.buffer.clone(),
            lin: None,
            planes: super::Planes::default(),
        });
        // The 27 half scalars occupy 54 bytes, rounded to a 4-byte buffer boundary.
        let expected_bytes = if source.can_read_exact() { 108 } else { 56 };
        assert_eq!(source.bytes(), expected_bytes);
        assert_eq!(stages.bytes(), 0, "the immutable source must not also be counted as a sampled stage");
        drop(source);
        assert_eq!(stages.bytes(), expected_bytes, "an alias that outlives the original owner remains accounted");
        stages.clear();
        assert_eq!(stages.bytes(), 0);
        gpu.trim(0);
    }

    use super::*;

    #[test]
    fn coverage_mask_matches_per_pixel_decision() {
        use lightcraft_develop::{EmbeddedLens, EmbeddedWarp};
        let lens = EmbeddedLens {
            warp: Some(EmbeddedWarp {
                planes: [[1.0, -0.03, 0.01, 0.0, 0.001, -0.002]; 3],
                center: lightcraft_geom::Point::new(0.52, 0.48),
                radius: 0.6,
            }),
            vignette: None,
        };
        let mut s = DevelopSettings::default();
        s.optics.lens_profile = true;
        s.optics.distortion = -40.0;
        s.geometry.horizontal = -15.0;
        s.geometry.rotate = 3.0;
        for orientation in [Orientation::Normal, Orientation::Rotate270] {
            s.orientation = orientation;
            let f = lightcraft_pipeline::geometry::Frame::with_lens(900, 600, &s, true, Some(&lens));
            let wp = f.warp.as_ref().expect("warp");
            // widths that do and don't fill the last word; heights that do and don't fill the last band
            for (w, h) in [f.fit(700, 700), (333, 221), (64, 16), (1, 1)] {
                let o2t = f.out_to_oriented(w, h);
                let mask = coverage_mask(wp, &o2t, w, h);
                let words = w.div_ceil(32);
                assert_eq!(mask.len(), words * h);
                for y in 0..h {
                    for x in 0..w {
                        let bit = mask[y * words + x / 32] >> (x % 32) & 1 == 1;
                        assert_eq!(bit, wp.covers(&o2t, x, y), "{orientation:?} {w}x{h}: pixel {x},{y}");
                    }
                    let pad = mask[y * words + words - 1] >> (w % 32);
                    assert!(w.is_multiple_of(32) || pad == 0, "padding bits set");
                }
            }
        }
    }

    /// The perspective horizon in view: its denominator is exactly 0 on output row 75 (clamped to 1e-300) and
    /// changes sign there, so the blocks around it are undecided (clamp, and division by a range that may hold
    /// zero) and are decided per pixel, while blocks clear of it are filled at once.
    #[test]
    fn coverage_mask_with_the_horizon_in_view() {
        use lightcraft_geom::{Affine, Homography};
        let (w, h) = (900, 600);
        let mut wp = lightcraft_pipeline::optics::Warp::identity(w as f64, h as f64);
        // centred coordinates: v = (y − 300) / 450, denominator 2·v + m8, exactly 0 at y = 75.5
        let m8 = -((75.5 - 300.0) / 450.0 * 2.0);
        wp.persp_inv = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, m8]);
        wp.persp = wp.persp_inv.inverse().expect("invertible");
        let o2t = Affine::IDENTITY;
        assert_eq!(wp.block_coverage(&o2t, 0, 32, 64, 80), None, "the block holding the horizon");
        assert_eq!(wp.block_coverage(&o2t, 0, 32, 0, 16), Some(false), "beyond the horizon");
        assert_eq!(wp.block_coverage(&o2t, 448, 480, 288, 304), Some(true), "the centre");
        let mask = coverage_mask(&wp, &o2t, w, h);
        let words = w.div_ceil(32);
        let mut covered = 0;
        for y in 0..h {
            for x in 0..w {
                let bit = mask[y * words + x / 32] >> (x % 32) & 1 == 1;
                assert_eq!(bit, wp.covers(&o2t, x, y), "pixel {x},{y}");
                covered += bit as usize;
            }
        }
        assert!(covered > 0 && covered < w * h, "{covered} px covered");
    }

    /// Kernel timings on a 24 MP plane: `cargo test --release -p lightcraft-gpu -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_kernels() {
        let Some(gpu) = crate::device() else { return };
        let _scope = crate::ctx::RenderScope::new(gpu);
        let (w, h) = (6000, 4000);
        let src = gpu.upload(&vec![0.5f32; w * h * 3]);
        let time = |name: &str, f: &mut dyn FnMut(&mut Cx<'_>)| {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let mut cx = Cx::new(gpu);
                cx.sync();
                let t = std::time::Instant::now();
                f(&mut cx);
                cx.sync();
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
            }
            eprintln!("{name:<40} {best:.1} ms");
        };
        for nc in [1, 2, 3] {
            time(&format!("gaussian nc={nc} sigma=2"), &mut |cx| drop(gaussian(cx, &src, w, h, nc, 2.0)));
        }
        time("map log_lum", &mut |cx| {
            let o = cx.gpu.buffer(w * h);
            map(cx, "log_lum_k", w * h, &[], [Some(&src), None, None], &o);
        });
        time("guided sigma=1.75", &mut |cx| drop(guided(cx, &src, w, h, 1.75, 0.01)));
    }

    #[test]
    fn orient_map_matches_image_oriented() {
        use Orientation::*;
        let img = Rgb32f::from_fn(5, 3, |x, y| [x as f32, y as f32, 0.0]);
        for o in [Normal, Rotate90, Rotate180, Rotate270, FlipH, Transverse, FlipV, Transpose] {
            let r = img.oriented(o);
            let m = orient_map(o, 5, 3);
            for y in 0..r.height {
                for x in 0..r.width {
                    let (x, y) = (x as i32, y as i32);
                    let (sx, sy) = (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
                    assert_eq!(r.get(x as usize, y as usize), img.get(sx as usize, sy as usize), "{o:?} at {x},{y}");
                }
            }
        }
    }
}

#[cfg(test)]
mod half_storage_tests {
    use super::*;

    #[test]
    fn odd_rgb_half_round_trip_and_copy_preserve_quantized_values() {
        let gpu = crate::device().expect("real GPU");
        if !gpu.half_storage {
            return;
        }
        let _scope = crate::ctx::RenderScope::new(gpu);
        let mut cx = Cx::new(gpu);
        let original = gpu.upload(&[0.18f32, -0.0, 65504.0, -1.0, 0.5, 1.00048828125, 5.960464477539063e-8, 1e-9, 2.0]);
        let packed = cx.compact(original, false);
        assert_eq!(packed.half_len, Some(9));
        assert_eq!(packed.len * 4, 20);
        let result = cx.read_rgb(&packed, 3, 1);
        let expected = [[0.1800537109375f32, -0.0, 65504.0], [-1.0, 0.5, 1.0], [5.960464477539063e-8, 0.0, 2.0]];
        for (actual, expected) in result.data.iter().flatten().zip(expected.iter().flatten()) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        let output = cx.zeroed(12);
        cx.copy_into(&packed, &output, 3);
        let copied: Vec<f32> = cx.read(&output, 12);
        assert_eq!(&copied[..3], &[0.0, 0.0, 0.0]);
        for (actual, expected) in copied[3..].iter().zip(expected.iter().flatten()) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn range_and_nonfinite_pack_rejection_preserves_original_bits() {
        let gpu = crate::device().expect("real GPU");
        if !gpu.half_storage {
            return;
        }
        let _scope = crate::ctx::RenderScope::new(gpu);
        for value in [65505.0, -65505.0, 1e10, -1e10, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(0x7fc01234), f32::from_bits(0xffc05678)] {
            let values = [value, 0.18, 0.4];
            let mut cx = Cx::new(gpu);
            let original = gpu.upload(&values);
            let retained = cx.copy(&original);
            let candidate = cx.compact(original, false);
            assert_eq!(candidate.half_len, Some(3));
            let status = cx.half_status.take().expect("pending range guard");
            let rejected: Vec<u32> = cx.read(&status, 1);
            assert_eq!(rejected, [1], "guard must reject {value:?}");
            let read: Vec<f32> = cx.read(&retained, 3);
            for (got, want) in read.iter().zip(values) {
                assert_eq!(got.to_bits(), want.to_bits());
            }
        }
    }
}

#[cfg(test)]
mod storage_policy_tests {
    use super::*;

    #[test]
    fn forced_f32_reuses_derived_planes() {
        assert!(crate::available());
        let image = lightcraft_scenes::demo_library()[2].render(37, 29);
        let source = crate::upload_source_with_storage(SourceId::new(), &image, crate::SourceStorage::F32).unwrap();
        let info = SourceInfo::default();
        let settings = DevelopSettings::default();
        let request = RenderRequest::fit(37, 29);
        let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
        let cache = lightcraft_pipeline::StageCache::default();
        crate::render_prepared(&source, &info, &plan, &request, Some(&cache), true).unwrap();
        let stages = cache.peek_extension::<GpuStages>().unwrap();
        let before = stages.lock()[0].planes.log_l.clone().unwrap();
        crate::render_prepared(&source, &info, &plan, &request, Some(&cache), true).unwrap();
        let after = stages.lock()[0].planes.log_l.clone().unwrap();
        assert!(Arc::ptr_eq(&before, &after), "a forced-f32 retry must retain valid derived caches on later frames");
    }
}

#[cfg(test)]
mod post_color_tests {
    use super::*;
    use lightcraft_pipeline::{RasterScale, StageCache};

    fn reduce_words(width: usize, height: usize, keep: [usize; 4], pixels: &[u32], partial: bool) -> Vec<[u8; 4]> {
        reduce_words_with_entry(width, height, keep, pixels, partial, "display_reduce_checked")
    }

    fn reduce_words_with_entry(width: usize, height: usize, keep: [usize; 4], pixels: &[u32], partial: bool, entry: &str) -> Vec<[u8; 4]> {
        let gpu = crate::device().expect("real GPU required");
        let [x, y, sw, sh] = keep;
        let (dw, dh) = (sw.div_ceil(2), sh.div_ceil(2));
        let mut cx = Cx::new(gpu);
        let source = gpu.upload(pixels);
        let output = cx.zeroed(dw * dh);
        let params = [width as u32, height as u32, x as u32, y as u32, sw as u32, sh as u32, dw as u32, dh as u32];
        let groups = if partial { [1, 1, 1] } else { groups2(dw, dh, [16, 16]) };
        cx.run(entry, &params, &[Some(&source), Some(&output), None], groups);
        cx.map(&output, dw * dh, 0).unwrap().with_bytes(|bytes| bytemuck::cast_slice(bytes).to_vec()).unwrap()
    }

    #[test]
    fn fast_reduction_matches_exact_area_for_parity_crop_and_degenerate_grids() {
        assert!(crate::available());
        let mut cases: Vec<(usize, usize, [usize; 4])> = (1..=9).flat_map(|w| (1..=9).map(move |h| (w, h, [0, 0, w, h]))).collect();
        cases.extend([(2360, 1573, [0, 0, 2360, 1573]), (2424, 1706, [0, 0, 2424, 1706]), (907, 709, [3, 5, 901, 703])]);
        let mut changed_channels = 0usize;
        let mut checked_channels = 0usize;
        for (width, height, keep) in cases {
            let mut pixels: Vec<u32> = (0..width * height)
                .map(|i| 0xff000000 | ((i as u32 * 37) & 255) | (((i as u32 * 73) & 255) << 8) | (((i as u32 * 139) & 255) << 16))
                .collect();
            for missing in [false, true] {
                if missing {
                    let [x, y, w, h] = keep;
                    pixels[(y + h / 2) * width + x + w / 2] &= 0x00ffffff;
                }
                let expected = reduce_words(width, height, keep, &pixels, false);
                let fast = reduce_words_with_entry(width, height, keep, &pixels, false, "display_reduce_checked_fast");
                let exact = exact_area_words(width, keep, &pixels);
                for (i, ((got, old), reference)) in fast.iter().zip(&expected).zip(&exact).enumerate() {
                    assert_eq!(got[3], old[3], "alpha differs at {width}x{height} pixel {i}");
                    assert_eq!(got[3], reference[3], "alpha differs from exact coverage at {width}x{height} pixel {i}");
                    for c in 0..3 {
                        assert!(got[c].abs_diff(old[c]) <= 1, "old path differs at {width}x{height} pixel {i} channel {c}: {got:?} vs {old:?}");
                        assert!(
                            got[c].abs_diff(reference[c]) <= 1,
                            "exact area differs at {width}x{height} pixel {i} channel {c}: {got:?} vs {reference:?}"
                        );
                        changed_channels += usize::from(got[c] != old[c]);
                        checked_channels += 1;
                    }
                }
            }
        }
        eprintln!(
            "fast area: {checked_channels} RGB channels checked; {changed_channels} differ from generic by exactly one byte; alpha agrees exactly"
        );
    }

    // Integer area integrals keep this reference independent of shader f32 arithmetic.
    pub(super) fn exact_area_words(width: usize, keep: [usize; 4], pixels: &[u32]) -> Vec<[u8; 4]> {
        let [x, y, sw, sh] = keep;
        let (dw, dh) = (sw.div_ceil(2), sh.div_ceil(2));
        let denominator = (sw * sh) as u64;
        let mut out = Vec::with_capacity(dw * dh);
        for dy in 0..dh {
            let (y0, y1) = (dy * sh, (dy + 1) * sh);
            for dx in 0..dw {
                let (x0, x1) = (dx * sw, (dx + 1) * sw);
                let mut rgb = [0u64; 3];
                let mut alpha = 255;
                for sy in y0 / dh..y1.div_ceil(dh) {
                    let wy = (y1.min((sy + 1) * dh) - y0.max(sy * dh)) as u64;
                    for sx in x0 / dw..x1.div_ceil(dw) {
                        let wx = (x1.min((sx + 1) * dw) - x0.max(sx * dw)) as u64;
                        let px = pixels[(y + sy) * width + x + sx];
                        if px >> 24 != 255 {
                            alpha = 0;
                        }
                        for (c, sum) in rgb.iter_mut().enumerate() {
                            *sum += u64::from((px >> (8 * c)) & 255) * wx * wy;
                        }
                    }
                }
                out.push([
                    ((2 * rgb[0] + denominator) / (2 * denominator)) as u8,
                    ((2 * rgb[1] + denominator) / (2 * denominator)) as u8,
                    ((2 * rgb[2] + denominator) / (2 * denominator)) as u8,
                    alpha,
                ]);
            }
        }
        out
    }

    #[test]
    fn reduction_rejects_each_missing_visible_native_pixel() {
        assert!(crate::available());
        for (width, height, keep) in [(5, 7, [0, 0, 5, 7]), (9, 11, [2, 1, 5, 7])] {
            let pixels: Vec<u32> = (0..width * height).map(|i| 0xff503000 | (30 + i as u32 % 120)).collect();
            let folded = reduce_words(width, height, keep, &pixels, false);
            assert_eq!(unwritten(&folded), 0);
            assert_eq!(folded, reduce_words_with_entry(width, height, keep, &pixels, false, "display_reduce"));
            let [x, y, w, h] = keep;
            for sy in y..y + h {
                for sx in x..x + w {
                    let mut missing = pixels.clone();
                    missing[sy * width + sx] &= 0x00ffffff;
                    for entry in ["display_reduce_checked", "display_reduce_checked_fast"] {
                        assert!(
                            unwritten(&reduce_words_with_entry(width, height, keep, &missing, false, entry)) > 0,
                            "{entry} hid missing native pixel {sx},{sy}"
                        );
                    }
                }
            }
            if x != 0 {
                let expected = reduce_words(width, height, keep, &pixels, false);
                let mut outside = pixels.clone();
                outside[0] = 0;
                assert_eq!(reduce_words(width, height, keep, &outside, false), expected, "unused halo cannot change the visible result");
            }
        }
    }

    #[test]
    fn reduction_rejects_missing_output_workgroups() {
        assert!(crate::available());
        let (width, height) = (67, 47);
        let pixels = vec![0xff503028u32; width * height];
        for entry in ["display_reduce", "display_reduce_checked", "display_reduce_fast", "display_reduce_checked_fast"] {
            let result = reduce_words_with_entry(width, height, [0, 0, width, height], &pixels, true, entry);
            assert_eq!(unwritten(&result), width.div_ceil(2) * height.div_ceil(2) - 16 * 16);
            assert!(unwritten(&vec![[0; 4]; result.len()]) > 0, "a wholly omitted reduction must fail too");
        }
    }

    #[test]
    fn precision_rejection_precedes_missing_native_output() {
        assert!(crate::available());
        let image = Rgb32f { width: 1, height: 1, data: vec![[60000.125, 60000.25, 60000.5]].into() };
        let source = crate::upload_source(SourceId::new(), &image).unwrap();
        if source.can_read_exact() {
            return;
        }
        let info = SourceInfo { raw: true, ..Default::default() };
        let mut settings = DevelopSettings::default();
        settings.wb.mode = lightcraft_develop::WbMode::Custom;
        settings.wb.temp = 2000.0;
        let request = RenderRequest { raster_scale: RasterScale::Half, ..RenderRequest::fit(1, 1) };
        let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
        let cache = StageCache::default();
        crate::inject_fault(crate::Fault::DropWork);
        assert!(crate::render_prepared(&source, &info, &plan, &request, Some(&cache), true).is_none());
        assert!(crate::enabled(), "precision rejection must not mark the device broken");
        assert_eq!(source.storage_policy(), crate::SourceStorage::F32);
        assert!(crate::last_fallback().unwrap().contains("retry f32"));
        assert_eq!(crate::stage_bytes(&cache), 0);
        crate::reset_failures();
        let exact = crate::upload_source_with_storage(source.id(), &image, crate::SourceStorage::F32).unwrap();
        let actual = crate::render_prepared(&exact, &info, &plan, &request, Some(&cache), true).unwrap().image.into_owned().unwrap();
        let expected = lightcraft_pipeline::render(&Arc::new(image), &info, &settings, &request).image;
        assert_eq!(actual.data, expected.data);
    }

    #[test]
    fn missing_native_work_does_not_publish_prepared_stages() {
        assert!(crate::available());
        let source = Arc::new(Rgb32f { width: 97, height: 73, data: vec![[0.12, 0.18, 0.3]; 97 * 73].into() });
        let request = RenderRequest { raster_scale: RasterScale::Half, ..RenderRequest::fit(97, 73) };
        let cache = StageCache::default();
        crate::inject_fault(crate::Fault::DropWork);
        assert!(crate::render_mapped(&source, &SourceInfo::default(), &DevelopSettings::default(), &request, Some(&cache)).is_none());
        assert!(crate::last_fallback().unwrap().contains("incomplete"));
        assert_eq!(cache.peek_extension::<GpuStages>().unwrap().len(), 0);
        assert_eq!(crate::stage_bytes(&cache), 0);
        crate::reset_failures();
    }

    #[test]
    fn native_stages_are_reused_and_post_color_matches_cpu_area() {
        assert!(crate::available());
        let image = Arc::new(Rgb32f { width: 97, height: 73, data: (0..97 * 73).map(|i| [0.05 + (i % 97) as f32 / 120.0, 0.18, 0.3]).collect() });
        let info = SourceInfo::default();
        let mut settings = DevelopSettings::default();
        settings.effects.clarity = 20.0;
        let stages = StageCache::default();
        let full_request = RenderRequest::fit(97, 73);
        let full = crate::render_mapped(&image, &info, &settings, &full_request, Some(&stages)).unwrap().image.into_owned().unwrap();
        let cached = stages.peek_extension::<GpuStages>().unwrap();
        let before = cached.lock()[0].clone();
        let bytes = crate::stage_bytes(&stages);
        let uploads = crate::source_uploads();
        let request = RenderRequest { raster_scale: RasterScale::Half, ..full_request };
        let result = crate::render_mapped(&image, &info, &settings, &request, Some(&stages)).unwrap();
        assert_eq!(result.display_size, (97, 73));
        assert_eq!((result.image.width, result.image.height), (49, 37));
        let expected = lightcraft_pipeline::reduce_display_half(&full);
        let actual = result.image.into_owned().unwrap();
        assert!(actual.data.iter().zip(&expected.data).all(|(a, b)| (0..4).all(|c| a[c].abs_diff(b[c]) <= 1)));
        assert_eq!(result.histogram, Histogram::of_srgb8(&actual));
        let after = cached.lock()[0].clone();
        assert_eq!(cached.len(), 1);
        assert!(Arc::ptr_eq(&before.lin.unwrap().1, &after.lin.unwrap().1));
        assert!(Arc::ptr_eq(&before.planes.log_l.unwrap(), &after.planes.log_l.unwrap()));
        assert_eq!(crate::stage_bytes(&stages), bytes);
        assert_eq!(crate::source_uploads(), uploads);
        let overlay_request = RenderRequest { overlay: lightcraft_pipeline::Overlay::Spots(50), ..request };
        let overlay = crate::render_mapped(&image, &info, &settings, &overlay_request, Some(&stages)).unwrap();
        assert_eq!((overlay.image.width, overlay.image.height), (49, 37));
        assert_eq!(overlay.histogram, result.histogram, "diagnostic overlays do not enter the histogram");
        crate::inject_fault(crate::Fault::DropWork);
        assert!(crate::render_mapped(&image, &info, &settings, &request, Some(&stages)).is_none());
        assert!(crate::last_fallback().unwrap().contains("incomplete"));
        crate::reset_failures();
    }
}

#[cfg(test)]
#[path = "reduced_stats_tests.rs"]
mod reduced_stats_tests;
