// Modified by the Unveil authors, 2026: half-source reload and deferred-range checks.
use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_gpu::{SourceStorage, upload_source, upload_source_with_storage};
use lightcraft_pipeline::{RenderRequest, SourceId, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

fn mode() -> String {
    std::env::var("UNVEIL_E2_HALF").unwrap_or_else(|_| "off".into())
}

fn half_requested() -> bool {
    matches!(mode().as_str(), "lin" | "all" | "source")
        && std::env::var_os("UNVEIL_E2_NO_F16").is_none()
        && std::env::var_os("UNVEIL_E2_FAIL_HALF").is_none()
}

#[test]
fn cached_storage_matches_capability_without_replacing_original() {
    assert!(lightcraft_gpu::available(), "a real GPU is required");
    let (w, h) = (33, 17);
    let image = Arc::new(Rgb32f { width: w, height: h, data: (0..w * h).map(|i| [0.05 + i as f32 / 2000.0, 0.18, 0.4]).collect() });
    let source = upload_source(SourceId::new(), &image).unwrap();
    let stages = StageCache::default();
    let info = SourceInfo::default();
    let settings = DevelopSettings::default();
    let request = RenderRequest::fit(w, h);
    let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
    let result = lightcraft_gpu::render_prepared(&source, &info, &plan, &request, Some(&stages), true).unwrap();
    assert_eq!((result.image.width, result.image.height), (w, h));
    let source_half = half_requested() && mode() == "source";
    assert_eq!(source.bytes(), if source_half { 3368 } else { 6732 });
    assert_eq!(source.can_read_exact(), !source_half);
    let stage_bytes = if !half_requested() {
        8976
    } else if mode() == "lin" {
        5612
    } else {
        4492
    };
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), stage_bytes);
    assert_eq!(lightcraft_gpu::last_fallback(), None);
}

#[test]
fn half_original_requires_reload_and_forced_f32_preserves_every_bit() {
    assert!(lightcraft_gpu::available());
    let image =
        Rgb32f { width: 3, height: 1, data: vec![[0.18, -0.0, 65504.0], [-1.0, 0.5, 1.00048828125], [5.960464477539063e-8, 1e-9, 2.0]].into() };
    let source = upload_source(SourceId::new(), &image).unwrap();
    if !source.can_read_exact() {
        assert_eq!(source.bytes(), 20);
        assert!(source.read_original().unwrap_err().starts_with("NeedsReload:"));
    }
    assert_eq!(source.storage_policy(), SourceStorage::Automatic);
    let exact = upload_source_with_storage(source.id(), &image, SourceStorage::F32).unwrap();
    assert_eq!(exact.storage_policy(), SourceStorage::F32);
    assert!(exact.can_read_exact());
    assert_eq!(exact.bytes(), 36);
    for (actual, expected) in exact.read_original().unwrap().data.iter().flatten().zip(image.data.iter().flatten()) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

#[test]
fn source_range_rejection_keeps_exact_f32_samples() {
    assert!(lightcraft_gpu::available());
    for value in [65505.0, -65505.0, 1e10, -1e10, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(0x7fc01234), f32::from_bits(0xffc05678)] {
        let image = Rgb32f { width: 1, height: 1, data: vec![[value, 0.18, -0.0]].into() };
        let source = upload_source(SourceId::new(), &image).unwrap();
        assert!(source.can_read_exact());
        assert_eq!(source.storage_policy(), SourceStorage::Automatic);
        assert_eq!(source.bytes(), 12);
        for (actual, expected) in source.read_original().unwrap().data[0].iter().zip(image.data[0]) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
}

#[test]
fn resident_derived_overflow_rejects_before_publication_and_retries_exactly() {
    assert!(lightcraft_gpu::available());
    let image = Rgb32f { width: 1, height: 1, data: vec![[60000.125, 60000.25, 60000.5]].into() };
    let source = upload_source(SourceId::new(), &image).unwrap();
    if source.can_read_exact() {
        return;
    }
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut settings = DevelopSettings::default();
    settings.wb.mode = lightcraft_develop::WbMode::Custom;
    settings.wb.temp = 2000.0;
    let request = RenderRequest::fit(1, 1);
    let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
    let stages = StageCache::default();
    assert!(lightcraft_gpu::render_prepared(&source, &info, &plan, &request, Some(&stages), true).is_none());
    assert_eq!(source.storage_policy(), SourceStorage::F32);
    assert!(lightcraft_gpu::render_prepared(&source, &info, &plan, &request, Some(&stages), true).is_none());
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), 0);
    assert!(lightcraft_gpu::enabled(), "precision rejection is recoverable");
    assert!(lightcraft_gpu::last_fallback().unwrap().contains("retry f32"));
    assert!(source.read_original().unwrap_err().starts_with("NeedsReload:"));
    let exact = upload_source_with_storage(source.id(), &image, SourceStorage::F32).unwrap();
    lightcraft_gpu::reset_failures();
    let first = lightcraft_gpu::render_prepared(&exact, &info, &plan, &request, Some(&stages), true).unwrap();
    let uploads = lightcraft_gpu::source_uploads();
    let second = lightcraft_gpu::render_prepared(&exact, &info, &plan, &request, Some(&stages), true).unwrap();
    assert_eq!(first.image.into_owned().unwrap().data, second.image.into_owned().unwrap().data);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads, "forced f32 must not repeat half/reload");
    assert_eq!(lightcraft_gpu::last_fallback(), None);
    assert_eq!(exact.read_original().unwrap().data, image.data);
}

#[test]
fn legacy_derived_overflow_reuses_its_exact_retry() {
    assert!(lightcraft_gpu::available());
    let image = Arc::new(Rgb32f { width: 1, height: 1, data: vec![[60000.125, 60000.25, 60000.5]].into() });
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut settings = DevelopSettings::default();
    settings.wb.mode = lightcraft_develop::WbMode::Custom;
    settings.wb.temp = 2000.0;
    let request = RenderRequest::fit(1, 1);
    let stages = StageCache::default();
    let first = lightcraft_gpu::render_mapped(&image, &info, &settings, &request, Some(&stages)).unwrap();
    let uploads = lightcraft_gpu::source_uploads();
    let second = lightcraft_gpu::render_mapped(&image, &info, &settings, &request, Some(&stages)).unwrap();
    assert_eq!(first.image.into_owned().unwrap().data, second.image.into_owned().unwrap().data);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
    assert_eq!(lightcraft_gpu::last_fallback(), None);
}

#[test]
fn exact_source_derived_rejection_keeps_its_f32_retry_policy() {
    assert!(lightcraft_gpu::available());
    let image = Rgb32f { width: 1, height: 1, data: vec![[65505.0, 60000.0, 60000.0]].into() };
    let source = upload_source(SourceId::new(), &image).unwrap();
    assert!(source.can_read_exact());
    assert_eq!(source.storage_policy(), SourceStorage::Automatic);
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut settings = DevelopSettings::default();
    settings.wb.mode = lightcraft_develop::WbMode::Custom;
    settings.wb.temp = 2000.0;
    let request = RenderRequest::fit(1, 1);
    let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
    let stages = StageCache::default();
    let result = lightcraft_gpu::render_prepared(&source, &info, &plan, &request, Some(&stages), true).unwrap();
    assert_eq!(source.storage_policy(), if half_requested() { SourceStorage::F32 } else { SourceStorage::Automatic });
    assert_eq!(result.image.into_owned().unwrap().data, lightcraft_pipeline::render(&Arc::new(image), &info, &settings, &request).image.data);
    assert_eq!(lightcraft_gpu::last_fallback(), None);
}

#[test]
fn unrelated_render_failure_preserves_automatic_storage_policy() {
    assert!(lightcraft_gpu::available());
    let image = Rgb32f { width: 1, height: 1, data: vec![[0.18; 3]].into() };
    let source = upload_source(SourceId::new(), &image).unwrap();
    let info = SourceInfo::default();
    let settings = DevelopSettings::default();
    let request = RenderRequest::fit(1, 1);
    let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
    lightcraft_gpu::inject_fault(lightcraft_gpu::Fault::Limit(4));
    assert!(lightcraft_gpu::render_prepared(&source, &info, &plan, &request, None, true).is_none());
    assert_eq!(source.storage_policy(), SourceStorage::Automatic);
    assert!(lightcraft_gpu::enabled());
    lightcraft_gpu::reset_failures();
}
