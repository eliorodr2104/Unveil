// Modified by the Unveil authors, 2026: pooled uploads and bounded original/mapped readback; see Engine/CHANGES.md.
//! The wgpu device, the compiled kernels, buffers, dispatch and readback.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use wgpu::util::DeviceExt;

/// A kernel module: WGSL source, its storage bindings (name, writable, element type) after the
/// parameter block `P` (binding 0), and its entry points.
struct Module {
    src: &'static str,
    bindings: &'static [(&'static str, bool, &'static str)],
    entries: &'static [&'static str],
}

const FINISH_BINDINGS: &[(&str, bool, &str)] = &[
    ("img", false, "float_storage"),
    ("log_l", false, "float_storage"),
    ("base_p", false, "float_storage"),
    ("clar", false, "float_storage"),
    ("tex", false, "float_storage"),
    ("dark", false, "float_storage"),
    ("masks", false, "f32"),
    ("aux", false, "f32"),
    ("out", true, "u32"),
];

const MODULES: &[Module] = &[
    Module {
        src: include_str!("wgsl/display_reduce.wgsl"),
        bindings: &[("src", false, "u32"), ("dst", true, "u32"), ("bad", true, "atomic<u32>")],
        #[cfg(not(test))]
        entries: &["display_reduce_checked_stats"],
        #[cfg(test)]
        entries: &[
            "display_reduce",
            "display_reduce_checked",
            "display_reduce_fast",
            "display_reduce_checked_fast",
            "display_reduce_checked_stats",
            "display_validate",
        ],
    },
    Module { src: include_str!("wgsl/finish.wgsl"), bindings: FINISH_BINDINGS, entries: &["main"] },
    Module { src: include_str!("wgsl/finish.wgsl"), bindings: FINISH_BINDINGS, entries: &["main_unmasked"] },
    Module { src: include_str!("wgsl/blur.wgsl"), bindings: &[("src", false, "float_storage"), ("dst", true, "f32")], entries: &["box_h", "box_v"] },
    Module {
        src: include_str!("wgsl/resize.wgsl"),
        bindings: &[("src", false, "float_storage"), ("table", false, "u32"), ("dst", true, "f32")],
        entries: &["resize_h", "resize_v"],
    },
    Module {
        src: include_str!("wgsl/map.wgsl"),
        bindings: &[("a", false, "float_storage"), ("b", false, "f32"), ("c", false, "f32"), ("dst", true, "f32")],
        entries: &[
            "expand_storage",
            "log_lum_k",
            "dark_k",
            "guided_pre",
            "guided_ab",
            "guided_apply",
            "wb_k",
            "nr_lum",
            "chroma_k",
            "nr_col",
            "subsample",
            "redeye_k",
            "xguided_pre",
            "xguided_ab",
            "xguided_apply",
        ],
    },
    Module {
        src: include_str!("wgsl/mask.wgsl"),
        bindings: &[
            ("img", false, "float_storage"),
            ("log_l", false, "float_storage"),
            ("aux", false, "f32"),
            ("c", true, "f32"),
            ("alpha", true, "f32"),
        ],
        entries: &["shape", "combine", "finalize"],
    },
    Module {
        src: include_str!("wgsl/geom.wgsl"),
        bindings: &[("src", false, "float_storage"), ("dst", true, "f32"), ("coverage", false, "u32")],
        entries: &["orient", "sample_affine", "sample_warp"],
    },
];

struct Kernel {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    nbuf: usize,
}

/// A device buffer of `len` 32-bit values.
///
/// Dropped buffers are recycled: they first wait in a per-thread list (commands recorded on this
/// thread may still use them) and become reusable once this thread submits ([`Gpu::submitted`]),
/// as the queue then runs every earlier use before any later one. Reuse matters: allocating (and
/// zero-filling) a fresh 100–300 MB buffer per pass costs as much as the pass itself.
pub struct Buf {
    buf: Option<Tracked>,
    pub len: usize,
    pub half_len: Option<usize>,
}

/// An owned, mapped staging buffer. Views borrow it only during `with_bytes`; dropping a stale
/// render or unwinding a consumer releases its mapping without an extra CPU allocation.
pub(crate) struct MappedReadback {
    buffer: wgpu::Buffer,
    size: u64,
}

impl MappedReadback {
    pub(crate) fn with_bytes<R>(&self, consume: impl FnOnce(&[u8]) -> R) -> Result<R, String> {
        if self.size == 0 {
            return Ok(consume(&[]));
        }
        let view = self.buffer.slice(..self.size).get_mapped_range().map_err(|e| format!("mapped readback: {e:?}"))?;
        Ok(consume(&view))
    }
}

impl Drop for MappedReadback {
    fn drop(&mut self) {
        self.buffer.unmap();
    }
}

impl Buf {
    pub fn raw(&self) -> &wgpu::Buffer {
        // `buf` is `Some` from construction (`Gpu::buffer`/`upload`) until `Drop` takes it back
        // for the pool, so a live `Buf` always has it.
        #[allow(clippy::expect_used)]
        &self.buf.as_ref().expect("live buffer").0
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        let Some(b) = self.buf.take() else { return };
        // Outside a render this thread has no commands recorded (only renders record), so the
        // buffer is reusable at once; without this, buffers dropped on threads that rarely
        // submit (the UI thread clearing a view's stages) would wait indefinitely.
        let Ok(in_render) = IN_RENDER.try_with(|c| c.get() > 0) else {
            // thread teardown: wgpu's thread-locals may be gone too, and dropping (or trimming
            // the pool, which drops) could abort — leak it (the device owns its memory)
            std::mem::forget(b);
            return;
        };
        if !in_render && let Some(g) = crate::existing_device() {
            g.pool([b]);
            return;
        }
        // during thread teardown the list may already be gone: then the buffer leaks
        let size = b.0.size();
        let mut b = Some(b);
        let _ = RETIRED.try_with(|r| r.0.borrow_mut().extend(b.take()));
        match b {
            Some(b) => std::mem::forget(b),
            None => {
                RETIRED_BYTES.fetch_add(size, Ordering::Relaxed);
            }
        }
    }
}

/// Buffers dropped on a thread since its last submit.
struct Retired(std::cell::RefCell<Vec<Tracked>>);

impl Drop for Retired {
    /// At thread exit wgpu's own thread-locals may already be destroyed, and dropping a buffer
    /// then aborts the process: leak the few buffers left instead (the device owns their memory).
    fn drop(&mut self) {
        for b in self.0.get_mut().drain(..) {
            RETIRED_BYTES.fetch_sub(b.0.size(), Ordering::Relaxed);
            std::mem::forget(b);
        }
    }
}

/// This thread's retired buffers (empty during thread teardown).
fn take_retired() -> Vec<Tracked> {
    let retired = RETIRED.try_with(|r| std::mem::take(&mut *r.0.borrow_mut())).unwrap_or_default();
    RETIRED_BYTES.fetch_sub(retired.iter().map(|b| b.0.size()).sum(), Ordering::Relaxed);
    retired
}

thread_local! {
    /// Renders running on this thread (they may hold recorded, unsubmitted commands).
    static IN_RENDER: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Marks a render on this thread; when it ends, the buffers it released join the pool (everything
/// it recorded has been submitted by then).
pub(crate) struct RenderScope<'a>(&'a Gpu);

impl<'a> RenderScope<'a> {
    pub fn new(g: &'a Gpu) -> RenderScope<'a> {
        IN_RENDER.with(|c| c.set(c.get() + 1));
        RenderScope(g)
    }
}

impl Drop for RenderScope<'_> {
    fn drop(&mut self) {
        let outer = IN_RENDER.with(|c| {
            c.set(c.get().saturating_sub(1));
            c.get() == 0
        });
        if outer {
            self.0.pool(take_retired());
        }
    }
}

/// A device buffer, counted in [`ALLOCATED`] while it exists.
pub(crate) struct Tracked(wgpu::Buffer);

impl Tracked {
    fn new(b: wgpu::Buffer) -> Tracked {
        ALLOCATED.fetch_add(b.size(), Ordering::Relaxed);
        crate::pool_stats::record(b.size(), b.usage().bits(), crate::pool_stats::Operation::Allocate);
        Tracked(b)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        crate::pool_stats::record(self.0.size(), self.0.usage().bits(), crate::pool_stats::Operation::Free);
        ALLOCATED.fetch_sub(self.0.size(), Ordering::Relaxed);
    }
}

thread_local! {
    static RETIRED: Retired = const { Retired(std::cell::RefCell::new(Vec::new())) };
}

/// Bytes of device buffers that exist (in use, retired or pooled).
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
/// Bytes waiting in per-thread retired lists.
static RETIRED_BYTES: AtomicU64 = AtomicU64::new(0);
/// Bytes in the free pool.
static POOLED: AtomicU64 = AtomicU64::new(0);
/// Most bytes kept in the free pool (see [`crate::set_pool_limit`]).
pub(crate) static POOL_LIMIT: AtomicU64 = AtomicU64::new(DEFAULT_POOL_BYTES);

/// Uploads smaller than this (bytes) reuse pooled buffers (see [`Gpu::upload`]).
const SMALL_UPLOAD: usize = 1 << 20;

/// Default for the most bytes kept in the free pool (apps derive it from their memory budget).
pub(crate) const DEFAULT_POOL_BYTES: u64 = 256 << 20;

/// Device buffers held by the renderer (bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuMemory {
    /// Every buffer that exists (in use + retired + pooled).
    pub allocated: u64,
    /// Of which recycled buffers waiting in the free pool.
    pub pooled: u64,
    /// Of which buffers dropped since their thread's last submit (pooled at its next submit).
    pub retired: u64,
}

pub(crate) fn memory() -> GpuMemory {
    GpuMemory { allocated: ALLOCATED.load(Ordering::Relaxed), pooled: POOLED.load(Ordering::Relaxed), retired: RETIRED_BYTES.load(Ordering::Relaxed) }
}

pub struct Gpu {
    pub generation: u64,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    kernels: HashMap<&'static str, Kernel>,
    dummy: wgpu::Buffer,
    /// Recycled buffers (see [`Buf`]).
    free: std::sync::Mutex<Vec<Tracked>>,
    /// Largest storage buffer the device accepts (bytes).
    pub max_buffer: u64,
    pub half_storage: bool,
    pub half_planes: bool,
    pub half_source: bool,
}

/// WGSL constants shared by every module (generated from the CPU pipeline's values).
fn constants() -> String {
    use lightcraft_pipeline::finish::{GRAIN_HASH, MASK_SUMS, MASK_TERMS, SRGB_LUT_N};
    use lightcraft_pipeline::tone::{CHROMA_N, LUT_MAX_EV, LUT_MIN_EV, LUT_N};
    let mut s = String::new();
    s += &format!("const REDUCTION_TILE: u32 = {}u;\n", crate::reduced_stats::TILE);
    let [to_lms, from_lms, to_lab, from_lab] = lightcraft_color::perceptual::oklab_matrices();
    for (name, m) in [("OK_TO_LMS", to_lms), ("OK_FROM_LMS", from_lms), ("OK_TO_LAB", to_lab), ("OK_FROM_LAB", from_lab)] {
        let rows: Vec<String> = m.iter().map(|r| format!("vec3<f32>({:?}, {:?}, {:?})", r[0], r[1], r[2])).collect();
        s += &format!("const {name} = array<vec3<f32>, 3>({});\n", rows.join(", "));
    }
    s += &format!(
        "const TONE_MIN_EV: f32 = {LUT_MIN_EV:?};\nconst TONE_MAX_EV: f32 = {LUT_MAX_EV:?};\nconst TONE_N: u32 = {LUT_N}u;\nconst CHROMA_N: u32 = {CHROMA_N}u;\n"
    );
    s += &format!("const TONE_MIN_GAIN: f32 = {:?};\n", 2f32.powf(LUT_MIN_EV));
    let b = lightcraft_pipeline::geometry::BLANK;
    s += &format!("const BLANK_R: f32 = {:?};\nconst BLANK_G: f32 = {:?};\nconst BLANK_B: f32 = {:?};\n", b[0], b[1], b[2]);
    s += &format!(
        "const SRGB_N: u32 = {SRGB_LUT_N}u;\nconst CURVE_N: u32 = {}u;\nconst MASK_TERMS: u32 = {MASK_TERMS}u;\nconst MASK_SUMS: u32 = {MASK_SUMS}u;\n",
        crate::params::CURVE_N
    );
    s += &format!("const POINT_WORDS: u32 = {}u;\n", lightcraft_pipeline::colorops::POINT_WORDS);
    s += &format!("const EYE_WORDS: u32 = {}u;\n", lightcraft_pipeline::redeye::EYE_WORDS);
    use lightcraft_pipeline::masks::{AUTO_TOL_CHROMA, AUTO_TOL_EV};
    s += &format!("const AUTO_TOL_EV: f32 = {AUTO_TOL_EV:?};\nconst AUTO_TOL_CHROMA: f32 = {AUTO_TOL_CHROMA:?};\n");
    s += &format!("const SHADOW_TINT_K: f32 = {:?};\n", lightcraft_pipeline::colorops::SHADOW_TINT);
    for (i, h) in GRAIN_HASH.iter().enumerate() {
        s += &format!("const GRAIN_H{i}: u32 = {h}u;\n");
    }
    for (name, idx) in crate::params::finish_fields() {
        s += &format!("const F_{name}: u32 = {idx}u;\n");
    }
    s
}

/// Storage loads are fixed when the device is created; all shader arithmetic stays f32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StorageLoads {
    Dynamic,
    F32,
}

fn module_source(m: &Module, consts: &str, loads: StorageLoads) -> String {
    let mut s = if m.bindings.iter().any(|(_, _, ty)| *ty == "f16") { String::from("enable f16;\n") } else { String::new() };
    s += consts;
    s += "@group(0) @binding(0) var<storage, read> P: array<u32>;\n";
    for (i, (name, write, ty)) in m.bindings.iter().enumerate() {
        let access = if *write { "read_write" } else { "read" };
        let stored = if *ty != "float_storage" {
            *ty
        } else if loads == StorageLoads::F32 {
            "f32"
        } else {
            "u32"
        };
        s += &format!("@group(0) @binding({}) var<storage, {access}> {name}: array<{stored}>;\n", i + 1);
        if *ty == "float_storage" {
            if loads == StorageLoads::F32 {
                s += &format!("fn load_{name}(i: u32) -> f32 {{ return {name}[i]; }}\n");
            } else {
                s += &format!(
                    "fn load_{name}(i: u32) -> f32 {{\n  if ((P[arrayLength(&P)-1u] & {}u) != 0u) {{ return unpack2x16float({name}[i/2u])[i%2u]; }}\n  return bitcast<f32>({name}[i]);\n}}\n",
                    1u32 << i
                );
            }
        }
    }
    s += include_str!("wgsl/common.wgsl");
    if m.entries == ["main_unmasked"] {
        // Compile out the local-mask accumulator only when the host proves there are no terms.
        // The generic kernel remains available for actual masks and optional compile failures.
        s += &m.src.replace("fn main(", "fn main_unmasked(").replace("let nm = pu(F_NMASK);", "let nm = 0u;");
    } else {
        s += m.src;
    }
    s
}

/// Contain optional compilation failures so initialization can retry the dynamic f32 loads.
fn compile_module(
    device: &wgpu::Device,
    m: &Module,
    consts: &str,
    loads: StorageLoads,
    inject_failure: bool,
) -> Result<(wgpu::BindGroupLayout, Vec<wgpu::ComputePipeline>), String> {
    let scopes =
        [wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Validation, wgpu::ErrorFilter::Internal].map(|filter| device.push_error_scope(filter));
    let mut src = module_source(m, consts, loads);
    if inject_failure {
        src += "invalid optional storage shader";
    }
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(src.into()) });
    let mut entries = vec![storage_entry(0, true)];
    entries.extend(m.bindings.iter().enumerate().map(|(i, (_, w, _))| storage_entry(i as u32 + 1, !w)));
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &entries });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    let pipelines = m
        .entries
        .iter()
        .map(|e| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(e),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(e),
                compilation_options: Default::default(),
                cache: None,
            })
        })
        .collect();
    let errors: Vec<_> = scopes.into_iter().rev().filter_map(|scope| pollster::block_on(scope.pop())).collect();
    if errors.is_empty() { Ok((layout, pipelines)) } else { Err(format!("{errors:?}")) }
}

impl Gpu {
    pub fn finish_kernel(&self, no_masks: bool) -> &'static str {
        if no_masks && self.kernels.contains_key("main_unmasked") { "main_unmasked" } else { "main" }
    }

    /// Create a device on the best available adapter (no software fallback), or why there is none.
    pub fn new(backends: wgpu::Backends) -> Result<Gpu, String> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        // only these drivers are loaded (`crate::backend`: never Vulkan on Windows unless asked)
        desc.backends = backends;
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
        )
        .map_err(|e| format!("no GPU adapter among {backends:?} ({e})"))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            log::info!("gpu: only a software adapter ({}), using the CPU pipeline", info.name);
            return Err(format!("software adapter ({}) skipped: the CPU pipeline is faster", info.name));
        }
        let mut limits = adapter.limits();
        if limits.max_storage_buffers_per_shader_stage < 10 {
            log::info!("gpu: {} has too few storage buffers per stage", info.name);
            return Err(format!("{} has too few storage buffers per shader stage ({} < 10)", info.name, limits.max_storage_buffers_per_shader_stage));
        }
        if let Some((binding, buffer)) = limits_override() {
            limits.max_storage_buffer_binding_size = limits.max_storage_buffer_binding_size.min(binding);
            limits.max_buffer_size = limits.max_buffer_size.min(buffer);
            log::info!("gpu: LIGHTCRAFT_GPU_LIMITS: storage bindings ≤ {} MiB, buffers ≤ {} MiB", binding >> 20, buffer >> 20);
        }
        let mode = std::env::var("UNVEIL_E2_HALF").unwrap_or_default();
        let half_requested = matches!(mode.as_str(), "lin" | "all" | "source")
            && adapter.features().contains(wgpu::Features::SHADER_F16)
            && std::env::var_os("UNVEIL_E2_NO_F16").is_none();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lightcraft"),
            required_limits: limits.clone(),
            required_features: if half_requested { wgpu::Features::SHADER_F16 } else { wgpu::Features::empty() },
            ..Default::default()
        }))
        .map_err(|e| format!("{}: device creation failed ({e})", info.name))?;
        // Errors outside a render's error scopes (see `ErrorScopes`): log, and stop using the GPU.
        device.on_uncaptured_error(std::sync::Arc::new(|e| {
            log::error!("gpu: {e}");
            crate::mark_broken(&format!("device error: {e}"));
        }));
        // wgpu reports a lost device (driver reset, GPU hang, watchdog) only here — never as an
        // error — so without this callback a lost device went unnoticed.
        device.set_device_lost_callback(|reason, msg| {
            log::error!("gpu: device lost ({reason:?}): {msg}");
            crate::mark_broken(&format!("device lost ({reason:?}): {msg}"));
        });
        let consts = constants();
        let mut kernels = HashMap::new();
        // Resolve optional packing before choosing the storage loads for every base kernel.
        let mut half_storage = false;
        if half_requested {
            let module = Module {
                src: include_str!("wgsl/storage.wgsl"),
                bindings: &[("src", false, "f32"), ("dst", true, "f16"), ("bad", true, "atomic<u32>")],
                entries: &["pack_half"],
            };
            if let Ok((layout, mut pipelines)) =
                compile_module(&device, &module, &consts, StorageLoads::Dynamic, std::env::var_os("UNVEIL_E2_FAIL_HALF").is_some())
            {
                let Some(pipeline) = pipelines.pop() else { return Err("pack_half pipeline missing".into()) };
                kernels.insert("pack_half", Kernel { pipeline, layout, nbuf: 3 });
                half_storage = true;
            }
        }
        let fail_f32 = std::env::var_os("UNVEIL_E2_FAIL_F32_BASE").is_some();
        for m in MODULES {
            // A half-enabled device can hold packed and exact-retry sources together. Its
            // dynamic loads handle both; direct f32 loads are safe only without half storage.
            let loads =
                if !half_storage && m.bindings.iter().any(|(_, _, ty)| *ty == "float_storage") { StorageLoads::F32 } else { StorageLoads::Dynamic };
            let optional = m.entries == ["main_unmasked"];
            let fail_optional = optional && std::env::var_os("UNVEIL_E45_FAIL_UNMASKED").is_some();
            let compiled = compile_module(&device, m, &consts, loads, fail_optional || fail_f32 && loads == StorageLoads::F32).or_else(|error| {
                if loads == StorageLoads::F32 { compile_module(&device, m, &consts, StorageLoads::Dynamic, fail_optional) } else { Err(error) }
            });
            let (layout, pipelines) = match compiled {
                Ok(compiled) => compiled,
                Err(error) if optional => {
                    log::warn!("gpu: unmasked finish unavailable, keeping the generic kernel: {error}");
                    continue;
                }
                Err(error) => return Err(format!("{}: kernels failed to build ({error})", info.name)),
            };
            for (entry, pipeline) in m.entries.iter().zip(pipelines) {
                kernels.insert(*entry, Kernel { pipeline, layout: layout.clone(), nbuf: m.bindings.len() });
            }
        }
        let dummy = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dummy"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let max_buffer = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        log::info!("gpu: {} ({:?}), buffers ≤ {} MiB", info.name, info.backend, max_buffer >> 20);
        static GENERATION: AtomicU64 = AtomicU64::new(1);
        Ok(Gpu {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            device,
            queue,
            info,
            kernels,
            dummy,
            free: Default::default(),
            max_buffer,
            half_storage,
            half_planes: half_storage && matches!(mode.as_str(), "all" | "source"),
            half_source: half_storage && mode == "source",
        })
    }

    /// A buffer of `len` 32-bit values with undefined contents (a recycled one when possible).
    ///
    /// A buffer over the device limit fails the render ([`FailKind::Limit`]) instead of raising a
    /// validation error: the caller gets a placeholder and the render is redone on the CPU.
    pub fn buffer(&self, len: usize) -> Buf {
        let size = (len.max(1) * 4) as u64;
        if size > self.limit() {
            self.over_limit(size);
            return Buf { buf: Some(Tracked::new(self.placeholder())), len, half_len: None };
        }
        let recycled = {
            let mut f = self.free.lock().unwrap_or_else(|e| e.into_inner());
            // the smallest pooled buffer that holds `size` with at most 25 % to spare: photos of
            // slightly different sizes reuse each other's buffers instead of piling up new ones
            let b = f
                .iter()
                .enumerate()
                .filter(|(_, b)| b.0.size() >= size && b.0.size() <= size + size / 4)
                .min_by_key(|(_, b)| b.0.size())
                .map(|(i, _)| i)
                .map(|i| f.swap_remove(i));
            if let Some(b) = &b {
                crate::pool_stats::record(b.0.size(), b.0.usage().bits(), crate::pool_stats::Operation::Reuse);
                POOLED.fetch_sub(b.0.size(), Ordering::Relaxed);
            }
            b
        };
        let buf = recycled.unwrap_or_else(|| {
            Tracked::new(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }))
        });
        Buf { buf: Some(buf), len, half_len: None }
    }

    /// Submit command buffers, then make the buffers this thread dropped before reusable.
    pub fn submit(&self, cmds: impl IntoIterator<Item = wgpu::CommandBuffer>) {
        self.queue.submit(cmds);
        let retired = take_retired();
        self.pool(retired);
    }

    /// Put reusable buffers into the free pool (trimmed to its limit).
    fn pool(&self, bufs: impl IntoIterator<Item = Tracked>) {
        let mut added = false;
        {
            let mut f = self.free.lock().unwrap_or_else(|e| e.into_inner());
            for b in bufs {
                POOLED.fetch_add(b.0.size(), Ordering::Relaxed);
                crate::pool_stats::record(b.0.size(), b.0.usage().bits(), crate::pool_stats::Operation::Return);
                f.push(b);
                added = true;
            }
        }
        if added {
            self.trim(POOL_LIMIT.load(Ordering::Relaxed));
        }
    }

    /// Free pooled buffers (oldest first) until at most `keep` bytes stay pooled.
    pub fn trim(&self, keep: u64) {
        let mut f = self.free.lock().unwrap_or_else(|e| e.into_inner());
        let keep = keep.min(POOL_LIMIT.load(Ordering::Relaxed));
        let mut total: u64 = f.iter().map(|b| b.0.size()).sum();
        let mut dropped = Vec::new();
        while total > keep && !f.is_empty() {
            let b = f.remove(0);
            total -= b.0.size();
            POOLED.fetch_sub(b.0.size(), Ordering::Relaxed);
            dropped.push(b);
        }
        drop(f);
        if !dropped.is_empty() {
            drop(dropped);
            // let wgpu release the memory now rather than at its next maintenance
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
    }

    /// Upload 32-bit values.
    ///
    /// Small uploads (under [`SMALL_UPLOAD`]) go into a recycled buffer when one fits: a fresh one
    /// per call would join the free pool on drop and, never asked for again, pile up to its limit.
    /// The write lands before the next submit, after every earlier use of the recycled buffer.
    pub fn upload<T: bytemuck::Pod>(&self, data: &[T]) -> Buf {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let len = bytes.len() / 4;
        if bytes.is_empty() {
            return self.buffer(1);
        }
        if bytes.len() as u64 > self.limit() {
            self.over_limit(bytes.len() as u64);
            return Buf { buf: Some(Tracked::new(self.placeholder())), len, half_len: None };
        }
        // large uploads (the source) keep a fresh buffer: writing a recycled one needs a staging copy as big
        if bytes.len() < SMALL_UPLOAD && bytes.len().is_multiple_of(4) {
            let buf = self.buffer(len);
            self.queue.write_buffer(buf.raw(), 0, bytes);
            return buf;
        }
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        });
        Buf { buf: Some(Tracked::new(buf)), len, half_len: None }
    }

    fn over_limit(&self, size: u64) {
        fail(
            FailKind::Limit,
            format!("a {} MiB buffer exceeds the device's {} MiB storage-buffer limit", size.div_ceil(1 << 20), self.limit() >> 20),
        );
    }

    /// Stands in for a buffer that could not be created (the render has failed already).
    fn placeholder(&self) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("placeholder"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Whether a buffer of `len` 32-bit values fits the device limits.
    pub fn fits(&self, len: usize) -> bool {
        (len as u64) * 4 <= self.limit()
    }

    /// Largest buffer a render may use (bytes): the device limit, or a lower one set for the
    /// render on this thread ([`LimitOverride`]).
    pub fn limit(&self) -> u64 {
        LIMIT.try_with(|l| l.get()).ok().flatten().map_or(self.max_buffer, |l| l.min(self.max_buffer))
    }

    pub fn encoder(&self) -> wgpu::CommandEncoder {
        self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
    }

    /// Record kernel `name` with parameters `p` over buffers `bufs` (None = unused binding).
    pub fn run(&self, enc: &mut wgpu::CommandEncoder, name: &str, p: &[u32], bufs: &[Option<&Buf>], groups: [u32; 3]) {
        if failed() {
            return; // the render is redone on the CPU: don't spend the GPU on it
        }
        let Some(k) = self.kernels.get(name) else {
            // a kernel name typo: fail this render over to the CPU instead of crashing
            fail(FailKind::Fatal, format!("unknown kernel {name}"));
            return;
        };
        assert_eq!(bufs.len(), k.nbuf, "{name}: binding count");
        let mut p = p.to_vec();
        let formats = bufs.iter().enumerate().fold(0u32, |bits, (i, b)| bits | if b.is_some_and(|b| b.half_len.is_some()) { 1 << i } else { 0 });
        p.push(formats);
        let params = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&p),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }];
        for (i, b) in bufs.iter().enumerate() {
            let buf = b.map(|b| b.raw()).unwrap_or(&self.dummy);
            entries.push(wgpu::BindGroupEntry { binding: i as u32 + 1, resource: buf.as_entire_binding() });
        }
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &k.layout, entries: &entries });
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(name), timestamp_writes: None });
        pass.set_pipeline(&k.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(groups[0], groups[1], groups[2]);
    }

    /// Submit `enc`, then read back the first `len` 32-bit values of `src`.
    ///
    /// A failed readback (device lost, map error, timeout) fails the render ([`FailKind::Fatal`])
    /// and returns zeros, which the caller discards; after an earlier failure in the same render
    /// nothing more is submitted.
    pub fn finish_and_read<T: bytemuck::Pod>(&self, enc: wgpu::CommandEncoder, src: &Buf, len: usize) -> Vec<T> {
        self.with_mapped_readback(enc, src, len, |bytes| bytemuck::cast_slice(bytes).to_vec())
            .unwrap_or_else(|| vec![<T as bytemuck::Zeroable>::zeroed(); len])
    }

    /// read_original_into copies immutable source bytes through one reusable staging allocation.
    /// The destination is private to the caller until every chunk succeeds. No failed map can
    /// escape as zeros, and each BufferView drops before its staging storage is unmapped/reused.
    pub(crate) fn read_original_into(&self, src: &Buf, destination: &mut [u8], allowed: impl Fn(usize) -> Result<(), String>) -> Result<(), String> {
        const CHUNK: usize = 16 << 20;
        if destination.is_empty() || destination.len() != src.len.checked_mul(4).ok_or("source byte count overflow")? {
            return Err("original readback length does not match its source".into());
        }
        allowed(0)?;
        let staging = Tracked::new(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("original readback chunk"),
            size: destination.len().min(CHUNK) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        for (index, bytes) in destination.chunks_mut(CHUNK).enumerate() {
            allowed(index)?;
            let mut enc = self.encoder();
            enc.copy_buffer_to_buffer(src.raw(), (index * CHUNK) as u64, &staging.0, 0, bytes.len() as u64);
            self.submit([enc.finish()]);
            let slice = staging.0.slice(..bytes.len() as u64);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
            let result: Result<(), String> = (|| {
                self.device
                    .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(READBACK_TIMEOUT) })
                    .map_err(|e| format!("original readback wait failed: {e}"))?;
                rx.recv_timeout(std::time::Duration::from_secs(10))
                    .map_err(|e| format!("original readback callback failed: {e}"))?
                    .map_err(|e| format!("original readback map failed: {e}"))?;
                let mapped = slice.get_mapped_range().map_err(|e| format!("original readback view failed: {e:?}"))?;
                bytes.copy_from_slice(&mapped);
                Ok(())
            })();
            staging.0.unmap();
            if let Err(reason) = result {
                fail(FailKind::Fatal, reason.clone());
                return Err(reason);
            }
            allowed(index + 1)?;
        }
        Ok(())
    }

    /// Consume mapped GPU output without an intermediate owned image. `len` counts 32-bit words.
    /// The callback runs only after a successful readback and cannot retain the mapped bytes.
    pub fn with_mapped_readback<R>(&self, enc: wgpu::CommandEncoder, src: &Buf, len: usize, consume: impl FnOnce(&[u8]) -> R) -> Option<R> {
        match self.finish_and_map(enc, src, len)?.with_bytes(consume) {
            Ok(value) => Some(value),
            Err(reason) => {
                fail(FailKind::Fatal, reason);
                None
            }
        }
    }

    /// Submit and retain mapped staging storage for delivery on another thread.
    pub(crate) fn finish_and_map(&self, enc: wgpu::CommandEncoder, src: &Buf, len: usize) -> Option<MappedReadback> {
        self.finish_and_map_padded(enc, src, len, 0)
    }

    /// A cropped strided view may expose padding after its last row. Reserve that small tail so
    /// consumers can safely borrow `height * stride` bytes, as the frame ABI promises.
    pub(crate) fn finish_and_map_padded(&self, mut enc: wgpu::CommandEncoder, src: &Buf, len: usize, tail: usize) -> Option<MappedReadback> {
        assert!(len <= src.len, "readback of {len} values from a buffer of {}", src.len);
        if failed() {
            return None;
        }
        let size = ((len + tail) * 4) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: size.max(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(src.raw(), 0, &staging, 0, (len * 4) as u64);
        self.submit([enc.finish()]);
        let slice = staging.slice(..size.max(4));
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let failed = |why: String| {
            fail(FailKind::Fatal, format!("readback failed: {why}"));
            None
        };
        // A lost device makes the wait fail and the map callback never run: checking both keeps
        // this from blocking forever (or handing back an unwritten buffer as the image).
        if let Err(e) = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(READBACK_TIMEOUT) }) {
            return failed(format!("{e}"));
        }
        // another thread's poll may be the one that runs the callback: allow it a moment
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return failed(format!("{e}")),
            Err(e) => return failed(format!("map callback: {e}")),
        }
        Some(MappedReadback { buffer: staging, size })
    }
}

/// Longest wait for a render's GPU work: beyond it the render fails over to the CPU (drivers
/// normally reset a hung GPU much sooner).
const READBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `LIGHTCRAFT_GPU_LIMITS`: lower the device limits, e.g. to reproduce a smaller adapter.
/// `webgpu` / `downlevel` = 128 MiB storage bindings and 256 MiB buffers (the WebGPU defaults),
/// `<n>` = n MiB storage bindings and 2n MiB buffers. Returns (binding bytes, buffer bytes).
fn limits_override() -> Option<(u64, u64)> {
    parse_limits(&std::env::var("LIGHTCRAFT_GPU_LIMITS").ok()?)
}

fn parse_limits(v: &str) -> Option<(u64, u64)> {
    let v = v.trim().to_ascii_lowercase();
    match v.as_str() {
        "" => None,
        "webgpu" | "downlevel" | "default" => {
            let d = wgpu::Limits::default();
            Some((d.max_storage_buffer_binding_size, d.max_buffer_size))
        }
        n => {
            let mb: u64 = n.trim_end_matches("mib").trim_end_matches("mb").trim().parse().ok()?;
            let b = mb.max(1) << 20;
            Some((b, 2 * b))
        }
    }
}

/// How a GPU render failed (it is then redone on the CPU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailKind {
    Precision,
    /// A buffer over the device limits: this render only.
    Limit,
    /// The device ran out of memory: this render only (the buffer pool is emptied).
    OutOfMemory,
    /// The image came back entirely black: redone on the CPU in case the GPU dropped work.
    Blank,
    /// Device error or loss, readback failure, incomplete image: the GPU is not used again.
    Fatal,
}

#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub kind: FailKind,
    pub reason: String,
}

thread_local! {
    /// A lower buffer limit for the render on this thread (fault injection in tests).
    static LIMIT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Lowers [`Gpu::limit`] on this thread while it lives.
pub(crate) struct LimitOverride;

impl LimitOverride {
    pub fn new(bytes: u64) -> LimitOverride {
        LIMIT.with(|l| l.set(Some(bytes)));
        LimitOverride
    }
}

impl Drop for LimitOverride {
    fn drop(&mut self) {
        let _ = LIMIT.try_with(|l| l.set(None));
    }
}

thread_local! {
    /// The first failure of the render running on this thread.
    static FAILURE: std::cell::RefCell<Option<Failure>> = const { std::cell::RefCell::new(None) };
}

/// Record a failure of the render running on this thread (the first one is kept).
pub(crate) fn fail(kind: FailKind, reason: String) {
    let _ = FAILURE.try_with(|f| {
        let mut f = f.borrow_mut();
        if f.is_none() {
            *f = Some(Failure { kind, reason });
        }
    });
}

/// Has the render on this thread failed?
pub(crate) fn failed() -> bool {
    FAILURE.try_with(|f| f.borrow().is_some()).unwrap_or(false)
}

/// The render's failure, clearing it.
pub(crate) fn take_failure() -> Option<Failure> {
    FAILURE.try_with(|f| f.borrow_mut().take()).ok().flatten()
}

/// wgpu error scopes around a render: out-of-memory, validation and internal errors raised by
/// this thread's calls are captured (instead of reaching the uncaptured-error handler) and fail
/// the render.
pub(crate) struct ErrorScopes([wgpu::ErrorScopeGuard; 3]);

impl ErrorScopes {
    pub fn push(g: &Gpu) -> ErrorScopes {
        ErrorScopes([
            g.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
            g.device.push_error_scope(wgpu::ErrorFilter::Validation),
            g.device.push_error_scope(wgpu::ErrorFilter::Internal),
        ])
    }

    /// Pop the scopes: the most telling error captured (out of memory first — an allocation
    /// failure makes later commands invalid too — then internal, then validation).
    pub fn pop(self) -> Option<Failure> {
        let [oom, validation, internal] = self.0;
        // scopes pop in reverse order of their creation
        let internal = pollster::block_on(internal.pop());
        let validation = pollster::block_on(validation.pop());
        let oom = pollster::block_on(oom.pop());
        if let Some(e) = oom {
            return Some(Failure { kind: FailKind::OutOfMemory, reason: format!("out of device memory: {e}") });
        }
        internal.or(validation).map(|e| Failure { kind: FailKind::Fatal, reason: format!("device error: {e}") })
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

/// Workgroups covering a `w × h` grid with `wg`-sized groups.
pub fn groups2(w: usize, h: usize, wg: [usize; 2]) -> [u32; 3] {
    [w.div_ceil(wg[0]).max(1) as u32, h.div_ceil(wg[1]).max(1) as u32, 1]
}

/// Workgroups for a 1-D kernel of `n` threads (256 per group) on a 2-D grid (see `lin_index`).
pub fn groups1(n: usize) -> [u32; 3] {
    let g = n.div_ceil(256).max(1);
    let x = g.min(32768);
    [x as u32, g.div_ceil(x) as u32, 1]
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires a fresh process with UNVEIL_E45_FAIL_UNMASKED=1"]
    fn optional_unmasked_compile_failure_keeps_the_generic_gpu() {
        assert_eq!(std::env::var("UNVEIL_E45_FAIL_UNMASKED").as_deref(), Ok("1"));
        let gpu = super::Gpu::new(wgpu::Backends::METAL).expect("optional kernel must not disable the GPU");
        assert!(!gpu.kernels.contains_key("main_unmasked"), "injected compile failure must be observed");
        assert!(gpu.kernels.contains_key("main"));
        assert_eq!(gpu.finish_kernel(true), "main");
    }

    #[test]
    fn a_panicking_consumer_releases_its_view_and_dropping_the_owner_unmaps() {
        let Ok(gpu) = super::Gpu::new(wgpu::Backends::PRIMARY) else {
            eprintln!("skipped: no GPU adapter for mapped lifetime");
            return;
        };
        let source = gpu.upload(&[7u32, 11, 13]);
        let mapped = gpu.finish_and_map(gpu.encoder(), &source, 3).unwrap();
        let buffer = mapped.buffer.clone();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mapped.with_bytes(|_| panic!("consumer panic")).unwrap();
        }));
        assert!(panic.is_err());
        assert_eq!(mapped.with_bytes(|bytes| bytemuck::cast_slice::<u8, u32>(bytes).to_vec()).unwrap(), [7, 11, 13]);
        drop(mapped);
        assert!(buffer.slice(..12).get_mapped_range().is_err(), "stale-result drop must unmap the retained buffer");
    }

    #[test]
    fn mapped_readback_writes_padded_rows_without_an_intermediate_image() {
        let Ok(gpu) = super::Gpu::new(wgpu::Backends::PRIMARY) else {
            eprintln!("skipped: no GPU adapter for mapped readback");
            return;
        };
        eprintln!("mapped readback adapter: {}", gpu.info.name);
        let pixels: Vec<[u8; 4]> = (0u8..35).map(|n| [n, n.wrapping_mul(7), 255 - n, 255]).collect();
        let source = gpu.upload(&pixels);
        let mut destination = vec![0xA5u8; 5 * 64];
        let histogram = gpu.with_mapped_readback(gpu.encoder(), &source, pixels.len(), |bytes| {
            let mapped: &[[u8; 4]] = bytemuck::cast_slice(bytes);
            for y in 0..5 {
                destination[y * 64..y * 64 + 28].copy_from_slice(bytemuck::cast_slice(&mapped[y * 7..(y + 1) * 7]));
            }
            lightcraft_raster::Histogram::of_srgb8_view(7, 5, |x, y| mapped[y * 7 + x])
        });
        let owned = lightcraft_raster::Rgba8 { width: 7, height: 5, data: lightcraft_raster::Buffer::Heap(pixels.clone()) };
        assert_eq!(histogram, Some(lightcraft_raster::Histogram::of_srgb8(&owned)));
        for y in 0..5 {
            assert_eq!(&destination[y * 64..y * 64 + 28], bytemuck::cast_slice::<[u8; 4], u8>(&pixels[y * 7..(y + 1) * 7]));
            assert!(destination[y * 64 + 28..(y + 1) * 64].iter().all(|byte| *byte == 0xA5));
        }
        let second: Vec<[u8; 4]> = gpu.finish_and_read(gpu.encoder(), &source, pixels.len());
        assert_eq!(second, pixels, "borrowed consumption must not change the GPU source or owned readback");
    }

    #[test]
    fn trimming_free_buffers_cannot_recycle_an_unsubmitted_source() {
        let gpu = crate::device().expect("actual GPU required");
        crate::set_pool_limit(64 << 20);
        crate::trim_reusable(0);
        let scope = super::RenderScope::new(gpu);
        let source = gpu.upload(&[7u32, 11, 13, 17]);
        let output = gpu.buffer(4);
        let mut encoder = gpu.encoder();
        encoder.copy_buffer_to_buffer(source.raw(), 0, output.raw(), 0, 16);
        drop(source);
        let retired = crate::memory().retired;
        assert!(retired >= 16);
        crate::trim_reusable(0);
        assert_eq!(crate::memory().retired, retired);
        let other = gpu.upload(&[99u32, 99, 99, 99]);
        gpu.submit([encoder.finish()]);
        let result: Vec<u32> = gpu.finish_and_read(gpu.encoder(), &output, 4);
        assert_eq!(result, [7, 11, 13, 17]);
        drop((other, output));
        drop(scope);
        assert_eq!(crate::memory().retired, 0);
        crate::trim_reusable(0);
        crate::set_pool_limit(256 << 20);
    }

    #[test]
    fn limits_override_parses() {
        assert_eq!(super::parse_limits("webgpu"), Some((128 << 20, 256 << 20)));
        assert_eq!(super::parse_limits(" Downlevel "), Some((128 << 20, 256 << 20)));
        assert_eq!(super::parse_limits("64"), Some((64 << 20, 128 << 20)));
        assert_eq!(super::parse_limits("512MiB"), Some((512 << 20, 1024 << 20)));
        assert_eq!(super::parse_limits(""), None);
        assert_eq!(super::parse_limits("lots"), None);
    }

    #[test]
    fn dispatches_stay_within_the_workgroup_limit() {
        // 1-D kernels over a 100 MP image and 2-D ones over 16k × 16k stay under 65535 per axis
        assert!(super::groups1(100_000_000 * 3).iter().all(|g| *g <= 65535));
        assert!(super::groups2(16384, 16384, [16, 16]).iter().all(|g| *g <= 65535));
    }
}

#[cfg(test)]
#[path = "base_format_tests.rs"]
mod base_format_tests;
