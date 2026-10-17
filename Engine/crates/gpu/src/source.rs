// Modified by the Unveil authors, 2026: immutable originals, checked half storage and exact reload policy; see Engine/CHANGES.md.
//! Source ownership is independent of view stages and never retains the uploaded CPU allocation.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// SourceStorage preserves the exact retry decision across source recovery and re-upload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceStorage {
    /// Use optional half storage when both the device and every candidate's range allow it.
    #[default]
    Automatic,
    /// Preserve exact original samples and keep all derived stages in f32.
    F32,
}

use lightcraft_pipeline::{SourceExtent, SourceId};
use lightcraft_raster::Rgb32f;

/// GpuSource owns one immutable original. Only its identity and dimensions escape the GPU module;
/// consumers cannot overwrite the buffer or mistake an edited stage for the original.
pub struct GpuSource {
    id: SourceId,
    extent: SourceExtent,
    force_f32: AtomicBool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) buffer: Arc<crate::ctx::Buf>,
    #[cfg(not(target_arch = "wasm32"))]
    generation: u64,
}

/// SourceFault exercises a failed ownership transfer or a partially completed recovery.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFault {
    Upload,
    ReadbackAfter(usize),
}

thread_local! {
    static FAULT: std::cell::Cell<Option<SourceFault>> = const { std::cell::Cell::new(None) };
}

/// inject_source_fault affects the next matching source operation on this thread only.
#[doc(hidden)]
pub fn inject_source_fault(fault: SourceFault) {
    FAULT.with(|f| f.set(Some(fault)));
}

#[cfg(not(target_arch = "wasm32"))]
fn take_fault(upload: bool) -> Option<SourceFault> {
    FAULT.with(|f| match f.get() {
        Some(value) if matches!(value, SourceFault::Upload) == upload => f.take(),
        _ => None,
    })
}

impl GpuSource {
    /// id identifies decoded content, independently of its current CPU or GPU residency.
    pub fn id(&self) -> SourceId {
        self.id
    }

    /// extent is the original decoded raster size, before framing or resampling.
    pub fn extent(&self) -> SourceExtent {
        self.extent
    }

    /// bytes counts the original's payload once, separately from view-derived stages.
    pub fn bytes(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.buffer.len * 4
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.extent.width * self.extent.height * 12
        }
    }

    /// can_read_exact distinguishes exact f32 storage from quantized half sources.
    pub fn can_read_exact(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.buffer.half_len.is_none()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// storage_policy is sticky for this content: F32 follows an explicit request or a rejected
    /// derived half stage. Recovery must preserve it when uploading a new source handle.
    /// Automatic may still contain f32 when only the original exceeds the half storage range.
    pub fn storage_policy(&self) -> SourceStorage {
        if self.force_f32.load(Ordering::Relaxed) { SourceStorage::F32 } else { SourceStorage::Automatic }
    }

    pub(crate) fn require_f32(&self) {
        self.force_f32.store(true, Ordering::Relaxed);
    }

    /// read_original recovers original f32 bits into one CPU destination with at most 16 MiB of
    /// reusable staging. It never returns partially copied or zero-filled data after a failure.
    /// GPU access must remain allowed; a caller with a suspended or lost device must reload.
    pub fn read_original(&self) -> Result<Rgb32f, String> {
        if !self.can_read_exact() {
            return Err("NeedsReload: half storage cannot recover exact original f32 samples".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let gpu = self.device()?;
            checked(gpu, || {
                let pixels = self.extent.width.checked_mul(self.extent.height).ok_or("source dimensions overflow")?;
                let mut data = Vec::new();
                data.try_reserve_exact(pixels).map_err(|e| format!("CPU source allocation failed: {e}"))?;
                data.resize(pixels, [0f32; 3]);
                let fault = take_fault(false);
                gpu.read_original_into(&self.buffer, bytemuck::cast_slice_mut(&mut data), |chunk| {
                    self.device()?;
                    if fault == Some(SourceFault::ReadbackAfter(chunk)) {
                        return Err("injected original readback failure".into());
                    }
                    Ok(())
                })?;
                self.device()?;
                Ok(Rgb32f { width: self.extent.width, height: self.extent.height, data: data.into() })
            })
        }
        #[cfg(target_arch = "wasm32")]
        Err("the browser build has no resident GPU source".into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn device(&self) -> Result<&'static crate::ctx::Gpu, String> {
        if !crate::enabled() {
            return Err(crate::unavailable_reason().unwrap_or_else(|| "GPU access is unavailable".into()));
        }
        let gpu = crate::existing_device().ok_or("the source device is unavailable")?;
        if self.generation != gpu.generation {
            return Err("the source belongs to a different device generation".into());
        }
        Ok(gpu)
    }

    pub(crate) fn allows_half_stages(&self) -> bool {
        self.storage_policy() == SourceStorage::Automatic
    }

    /// upload runs inside its caller's error scopes. The caller accepts the handle only after
    /// those scopes succeed, so placeholder storage can never become a resident original.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn upload(gpu: &crate::ctx::Gpu, id: SourceId, image: &Rgb32f) -> Result<Arc<Self>, String> {
        Self::upload_with_storage(gpu, id, image, SourceStorage::Automatic)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn upload_with_storage(gpu: &crate::ctx::Gpu, id: SourceId, image: &Rgb32f, storage: SourceStorage) -> Result<Arc<Self>, String> {
        let pixels = image.width.checked_mul(image.height).filter(|n| *n > 0).ok_or("invalid source dimensions")?;
        let words = pixels.checked_mul(3).ok_or("source size overflow")?;
        if image.data.len() != pixels || !gpu.fits(words) {
            return Err("source data length is invalid or exceeds the GPU buffer limit".into());
        }
        if take_fault(true).is_some() {
            return Err("injected source upload failure".into());
        }
        let mut cx = crate::render::Cx::new(gpu);
        let buffer = Arc::new(cx.upload_source(image, storage == SourceStorage::Automatic));
        crate::render::record_source_upload();
        Ok(Arc::new(Self {
            id,
            extent: SourceExtent { width: image.width, height: image.height },
            force_f32: AtomicBool::new(storage == SourceStorage::F32),
            buffer,
            generation: gpu.generation,
        }))
    }
}

/// upload_source validates an original upload without retaining CPU pixels. The engine promotes
/// its cached source only after a subsequent render has also completed and passed validation.
pub fn upload_source(id: SourceId, image: &Rgb32f) -> Result<Arc<GpuSource>, String> {
    upload_source_with_storage(id, image, SourceStorage::Automatic)
}

/// upload_source_with_storage accepts an explicit f32 retry without retaining a quantized shadow.
/// The engine must retain this policy with recovered source state to prevent repeated reloads.
pub fn upload_source_with_storage(id: SourceId, image: &Rgb32f, storage: SourceStorage) -> Result<Arc<GpuSource>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !crate::enabled() {
            return Err(crate::unavailable_reason().unwrap_or_else(|| "GPU access is unavailable".into()));
        }
        let gpu = crate::device().ok_or_else(|| crate::unavailable_reason().unwrap_or_else(|| "no GPU device".into()))?;
        checked(gpu, || {
            let source = GpuSource::upload_with_storage(gpu, id, image, storage)?;
            gpu.submit([gpu.encoder().finish()]);
            source.device()?;
            Ok(source)
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (id, image, storage);
        Err("the browser build has no resident GPU source".into())
    }
}

/// checked captures device errors and panics without manufacturing usable source data.
#[cfg(not(target_arch = "wasm32"))]
fn checked<T>(gpu: &crate::ctx::Gpu, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _ = crate::ctx::take_failure();
    let _render = crate::ctx::RenderScope::new(gpu);
    let errors = crate::ctx::ErrorScopes::push(gpu);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
    let failure = errors.pop().or_else(crate::ctx::take_failure);
    let _ = crate::ctx::take_failure();
    if let Some(failure) = failure {
        if failure.kind == crate::ctx::FailKind::Fatal {
            crate::mark_broken(&failure.reason);
        }
        return Err(failure.reason);
    }
    match result {
        Ok(result) => result,
        Err(_) => {
            crate::mark_broken("GPU source operation panicked");
            Err("GPU source operation panicked".into())
        }
    }
}
