//! Modified for Unveil (2026-10-15): device ownership, exact recovery and persistent precision fallback.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, Weak};

use lightcraft_pipeline::{PlanningFacts, SourceExtent, plan_with_facts};

use super::*;

/// A precision rejection belongs to the source content, independent of cache residency.
#[derive(Default)]
pub(crate) struct SourcePolicy {
    forced_f32: AtomicBool,
    pub(super) content: Option<Hash128>,
}

impl SourcePolicy {
    pub(super) fn new(content: Hash128) -> Self {
        Self { forced_f32: AtomicBool::new(false), content: Some(content) }
    }

    pub(super) fn storage(&self) -> lightcraft_gpu::SourceStorage {
        if self.forced_f32.load(Ordering::Relaxed) { lightcraft_gpu::SourceStorage::F32 } else { lightcraft_gpu::SourceStorage::Automatic }
    }

    fn require_f32(&self) {
        self.forced_f32.store(true, Ordering::Relaxed);
    }
}

/// Live source owners, separate from derived render stages and the GPU's free pool.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceResidency {
    pub cpu_pixels: usize,
    pub gpu_pixels: usize,
    pub recovery_pixels: usize,
    pub planning_facts: usize,
    pub readbacks: usize,
    pub reloads: usize,
}

impl SourceResidency {
    pub(super) fn add(&mut self, other: Self) {
        self.cpu_pixels += other.cpu_pixels;
        self.gpu_pixels += other.gpu_pixels;
        self.recovery_pixels += other.recovery_pixels;
        self.planning_facts += other.planning_facts;
        self.readbacks += other.readbacks;
        self.reloads += other.reloads;
    }
}

#[derive(Clone)]
pub(super) enum CachedSource {
    Host { id: SourceId, source: DecodedSource },
    Device(Arc<ResidentSource>),
}

impl CachedSource {
    pub(super) fn new(source: DecodedSource) -> Self {
        Self::Host { id: SourceId::new(), source }
    }

    pub(super) fn host(&self) -> Result<DecodedSource, String> {
        match self {
            Self::Host { source, .. } => Ok(source.clone()),
            Self::Device(source) => source.recover().map(|r| r.decoded),
        }
    }

    pub(super) fn id(&self) -> SourceId {
        match self {
            Self::Host { id, .. } => *id,
            Self::Device(source) => source.id(),
        }
    }

    pub(super) fn info_or(&self, header: SourceInfo) -> SourceInfo {
        match self {
            Self::Host { source, .. } => source.info_or(header),
            Self::Device(source) => source.info.clone(),
        }
    }

    pub(super) fn has_info(&self) -> bool {
        match self {
            Self::Host { source, .. } => source.info.is_some(),
            Self::Device(_) => true,
        }
    }

    pub(super) fn cpu_bytes(&self) -> usize {
        match self {
            Self::Host { source, .. } => source.bytes(),
            Self::Device(source) => source.cpu_bytes(),
        }
    }

    pub(super) fn bytes(&self) -> usize {
        self.cpu_bytes() + self.residency().gpu_pixels
    }

    pub(super) fn residency(&self) -> SourceResidency {
        match self {
            Self::Host { source, .. } => {
                SourceResidency { cpu_pixels: source.image.data.len() * 12 + source.denoised.as_ref().map_or(0, |t| t.bytes()), ..Default::default() }
            }
            Self::Device(source) => SourceResidency {
                gpu_pixels: source.source.bytes(),
                recovery_pixels: source.recovery.lock().unwrap_or_else(|e| e.into_inner()).pixels.upgrade().map_or(0, |image| image.data.len() * 12),
                planning_facts: source.facts.lock().unwrap_or_else(|e| e.into_inner()).bytes(),
                readbacks: source.readbacks.load(Ordering::Relaxed),
                reloads: source.reloads.load(Ordering::Relaxed),
                ..Default::default()
            },
        }
    }
}

/// The immutable device original, exact decoder facts and a recipe that owns no decoded pixels.
/// Planning memo entries are bounded by the pipeline; a CPU analysis borrows recovered pixels only
/// for its call. A failed device readback reloads the source and starts a new identity.
pub struct ResidentSource {
    pub(super) source: Arc<lightcraft_gpu::GpuSource>,
    pub(super) info: SourceInfo,
    facts: Mutex<PlanningFacts>,
    pub(super) reload: SourceRef,
    pub(super) policy: Arc<SourcePolicy>,
    recovery: Mutex<Recovery>,
    recovered: Condvar,
    readbacks: AtomicUsize,
    reloads: AtomicUsize,
}

#[derive(Default)]
struct Recovery {
    pixels: Weak<Rgb32f>,
    info: Option<SourceInfo>,
    camera_tone: Option<lightcraft_pipeline::tone::CameraTone>,
    denoised: Option<Weak<Twin>>,
    kind: RecoveryKind,
    busy: bool,
}

impl Recovery {
    fn get(&self) -> Option<Recovered> {
        let image = self.pixels.upgrade()?;
        let denoised = match &self.denoised {
            Some(twin) => Some(twin.upgrade()?),
            None => None,
        };
        Some(Recovered { decoded: DecodedSource { image, info: self.info.clone(), camera_tone: self.camera_tone, denoised }, kind: self.kind })
    }

    fn remember(&mut self, recovered: &Recovered) {
        self.pixels = Arc::downgrade(&recovered.decoded.image);
        self.info = recovered.decoded.info.clone();
        self.camera_tone = recovered.decoded.camera_tone;
        self.denoised = recovered.decoded.denoised.as_ref().map(Arc::downgrade);
        self.kind = recovered.kind;
    }
}

struct RecoveryClaim<'a>(&'a ResidentSource);

impl Drop for RecoveryClaim<'_> {
    fn drop(&mut self) {
        self.0.recovery.lock().unwrap_or_else(|e| e.into_inner()).busy = false;
        self.0.recovered.notify_all();
    }
}

/// Every recovery contains exact decoded CPU samples. Only device failure or changed decoder
/// facts retire the owner; healthy half storage reloads under the current catalog content identity.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum RecoveryKind {
    Readback,
    HalfReload,
    #[default]
    Replacement,
}

impl RecoveryKind {
    pub(super) fn reuses_device(self) -> bool {
        self != Self::Replacement
    }
}

pub(super) struct Recovered {
    pub(super) decoded: DecodedSource,
    pub(super) kind: RecoveryKind,
}

impl ResidentSource {
    pub fn id(&self) -> SourceId {
        self.source.id()
    }

    pub(super) fn needs_f32_retry(&self) -> bool {
        if self.source.storage_policy() == lightcraft_gpu::SourceStorage::F32 {
            self.policy.require_f32();
        }
        self.policy.storage() == lightcraft_gpu::SourceStorage::F32
            && (self.source.storage_policy() != lightcraft_gpu::SourceStorage::F32 || !self.source.can_read_exact())
    }

    fn cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.facts.lock().unwrap_or_else(|e| e.into_inner()).bytes()
            + self.info.mattes.as_ref().map_or(0, |m| m.bytes())
    }

    pub(super) fn recover(&self) -> Result<Recovered, String> {
        let mut recovery = self.recovery.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(mut recovered) = recovery.get() {
                if !lightcraft_gpu::enabled() {
                    recovered.kind = RecoveryKind::Replacement;
                }
                return Ok(recovered);
            }
            if !recovery.busy {
                break;
            }
            recovery = self.recovered.wait(recovery).unwrap_or_else(|e| e.into_inner());
        }
        recovery.busy = true;
        drop(recovery);
        // Neither the recovery lock nor planner lock is held while waiting for the device.
        let _claim = RecoveryClaim(self);
        let expected_half_reload = !self.source.can_read_exact() && lightcraft_gpu::available();
        let recovered = match self.source.read_original() {
            Ok(image) => {
                self.readbacks.fetch_add(1, Ordering::Relaxed);
                let image = Arc::new(image);
                let mut decoded = DecodedSource::new(image, Some(self.info.clone()));
                if matches!(self.reload, SourceRef::Smart { .. }) {
                    decoded.info = None;
                    decoded.camera_tone = self.info.camera_tone;
                }
                Ok(Recovered { decoded, kind: RecoveryKind::Readback })
            }
            Err(read_error) => {
                self.reloads.fetch_add(1, Ordering::Relaxed);
                self.reload
                    .load_source()
                    .map(|decoded| {
                        let same_interpretation = decoded.denoised.is_none()
                            && SourceExtent::from(decoded.image.as_ref()) == self.source.extent()
                            && decoded.info_or(self.info.clone()) == self.info;
                        let kind = if expected_half_reload && lightcraft_gpu::enabled() && same_interpretation {
                            RecoveryKind::HalfReload
                        } else {
                            RecoveryKind::Replacement
                        };
                        Recovered { decoded, kind }
                    })
                    .map_err(|reload_error| format!("source readback: {read_error}; reload: {reload_error}"))
            }
        }?;
        self.recovery.lock().unwrap_or_else(|e| e.into_inner()).remember(&recovered);
        Ok(recovered)
    }

    /// Recover at most once for missing planner facts, then reuse the temporary for CPU fallback.
    pub(super) fn render(
        &self,
        settings: &DevelopSettings,
        request: &RenderRequest,
        stages: Option<&StageCache>,
        mapped: bool,
        recovered: &mut Option<Recovered>,
    ) -> Option<Rendered<lightcraft_gpu::DisplayImage>> {
        if !eligible(settings, request) || !lightcraft_gpu::available() {
            return None;
        }
        if self.needs_f32_retry() {
            return None;
        }
        let effective = settings.effective();
        let settings = &effective;
        let mut facts = self.facts.lock().unwrap_or_else(|e| e.into_inner());
        let plan = match plan_with_facts(&self.info, settings, request, &mut facts, None) {
            Ok(plan) => plan,
            Err(_) => {
                drop(facts);
                *recovered = Some(self.recover().ok()?);
                let host = recovered.as_ref()?;
                if !host.kind.reuses_device() {
                    return None;
                }
                facts = self.facts.lock().unwrap_or_else(|e| e.into_inner());
                plan_with_facts(&self.info, settings, request, &mut facts, Some(&host.decoded.image)).ok()?
            }
        };
        drop(facts);
        let rendered = lightcraft_gpu::render_prepared(&self.source, &self.info, &plan, request, stages, mapped)?;
        if self.source.storage_policy() == lightcraft_gpu::SourceStorage::F32 {
            self.policy.require_f32();
        }
        if let Some(stages) = stages {
            stages.clear_cpu();
        }
        Some(rendered)
    }
}

pub(super) fn eligible(settings: &DevelopSettings, request: &RenderRequest) -> bool {
    !lightcraft_pipeline::lut::is_lut_profile(&settings.profile.id)
        && !settings.masks.iter().any(|mask| mask.visible && mask.refine > 0.0)
        && request.proof.is_none()
}

/// An unsuccessful promotion either preserves exact host pixels or reports why recovery failed.
pub(super) enum Promotion {
    Device(Rendered<lightcraft_gpu::DisplayImage>, Arc<ResidentSource>),
    Host(DecodedSource),
    Failed(String),
}

/// Borrowed callers still keep their recovery lease until the result is known.
pub(super) fn promote(
    id: SourceId,
    policy: Arc<SourcePolicy>,
    decoded: &DecodedSource,
    reload: SourceRef,
    info: SourceInfo,
    settings: &DevelopSettings,
    request: &RenderRequest,
    stages: Option<&StageCache>,
    mapped: bool,
) -> Option<(Rendered<lightcraft_gpu::DisplayImage>, Arc<ResidentSource>)> {
    match promote_owned(id, policy, decoded.clone(), reload, info, settings, request, stages, mapped) {
        Promotion::Device(rendered, owner) => Some((rendered, owner)),
        Promotion::Host(_) | Promotion::Failed(_) => None,
    }
}

/// Upload consumes the cold decode before render scratch is allocated. Only an exact f32 device
/// original may replace the CPU pixels at this boundary. A rejected frame recovers those exact
/// samples, or reloads through the original's pinned recipe if device access also failed.
pub(super) fn promote_owned(
    id: SourceId,
    policy: Arc<SourcePolicy>,
    decoded: DecodedSource,
    reload: SourceRef,
    info: SourceInfo,
    settings: &DevelopSettings,
    request: &RenderRequest,
    stages: Option<&StageCache>,
    mapped: bool,
) -> Promotion {
    if decoded.denoised.is_some() || !eligible(settings, request) || !lightcraft_gpu::available() {
        return Promotion::Host(decoded);
    }
    let effective = settings.effective();
    let settings = &effective;
    let storage = policy.storage();
    let host_memory = lightcraft_raster::memory_trace::Allocation::of("upload-host", &decoded.image.data);
    let prepared = (|| {
        let mut facts = PlanningFacts::new(id, SourceExtent::from(decoded.image.as_ref()));
        let plan = plan_with_facts(&info, settings, request, &mut facts, Some(&decoded.image)).ok()?;
        lightcraft_raster::memory_trace::checkpoint("upload-begin");
        let source = lightcraft_gpu::upload_source_with_storage(id, &decoded.image, storage).ok()?;
        lightcraft_raster::memory_trace::checkpoint("upload-end");
        Some((facts, plan, source))
    })();
    let Some((facts, plan, mut source)) = prepared else {
        host_memory.handoff();
        return Promotion::Host(decoded);
    };
    let reload = reload.exact_reload(&decoded);
    let decoded_info = decoded.info.clone();
    let camera_tone = decoded.camera_tone;
    let (host, host_memory) = if source.can_read_exact() {
        // create_buffer_init has copied the input into independently owned device storage. No
        // command, callback or planning fact borrows decoded.image after upload_source returns.
        let last_host_owner = Arc::strong_count(&decoded.image) == 1;
        drop(decoded);
        if last_host_owner {
            drop(host_memory);
        } else {
            host_memory.handoff();
        }
        crate::memory::release();
        (None, None)
    } else {
        (Some(decoded), Some(host_memory))
    };
    lightcraft_raster::memory_trace::checkpoint("promotion-render-begin");
    let rendered = match lightcraft_gpu::render_prepared(&source, &info, &plan, request, stages, mapped) {
        Some(rendered) => Some(rendered),
        None if storage != lightcraft_gpu::SourceStorage::F32 && source.storage_policy() == lightcraft_gpu::SourceStorage::F32 => {
            policy.require_f32();
            if let Some(decoded) = host.as_ref() {
                match lightcraft_gpu::upload_source_with_storage(id, &decoded.image, lightcraft_gpu::SourceStorage::F32) {
                    Ok(exact) => {
                        source = exact;
                        lightcraft_gpu::render_prepared(&source, &info, &plan, request, stages, mapped)
                    }
                    Err(_) => None,
                }
            } else {
                // The original already has exact storage; only its derived stages were rejected.
                lightcraft_gpu::render_prepared(&source, &info, &plan, request, stages, mapped)
            }
        }
        None => None,
    };
    if let Some(memory) = host_memory {
        memory.handoff();
    }
    let Some(rendered) = rendered else {
        if let Some(decoded) = host {
            return Promotion::Host(decoded);
        }
        return match source.read_original() {
            Ok(image) => Promotion::Host(DecodedSource { image: Arc::new(image), info: decoded_info, camera_tone, denoised: None }),
            Err(read_error) => match reload.load_source() {
                Ok(decoded) => Promotion::Host(decoded),
                Err(reload_error) => Promotion::Failed(format!("source readback: {read_error}; reload: {reload_error}")),
            },
        };
    };
    if source.storage_policy() == lightcraft_gpu::SourceStorage::F32 {
        policy.require_f32();
    }
    if let Some(stages) = stages {
        stages.clear_cpu();
    }
    let resident = Arc::new(ResidentSource {
        source,
        info,
        facts: Mutex::new(facts),
        reload,
        policy,
        recovery: Mutex::default(),
        recovered: Condvar::new(),
        readbacks: AtomicUsize::new(0),
        reloads: AtomicUsize::new(0),
    });
    Promotion::Device(rendered, resident)
}
