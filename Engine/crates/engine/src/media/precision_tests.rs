//! Modified for Unveil (2026-10-15): exact half-source recovery and persistent f32 retry policy.

use super::*;
use lightcraft_gpu::SourceStorage;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn half_source_available() -> bool {
    if std::env::var("UNVEIL_E2_HALF").unwrap_or_else(|_| "off".into()) != "source" {
        eprintln!("skipped: set UNVEIL_E2_HALF=source for half-source engine validation");
        return false;
    }
    if !super::resident_tests::gpu_available() {
        return false;
    }
    let source = lightcraft_gpu::upload_source(SourceId::new(), &exact_pixels()).unwrap();
    let active = !source.can_read_exact();
    assert!(active || std::env::var_os("UNVEIL_REQUIRE_GPU").is_none(), "half source kernels are required for this validation");
    if !active {
        eprintln!("skipped: half source kernels are unavailable");
    }
    active
}

fn exact_pixels() -> Rgb32f {
    Rgb32f { width: 1, height: 1, data: vec![[0.1800123, -0.0, 0.5100247]].into() }
}

fn hdr_pixels() -> Rgb32f {
    Rgb32f { width: 1, height: 1, data: vec![[60000.125, 60000.25, 60000.5]].into() }
}

fn overrange_pixels() -> Rgb32f {
    Rgb32f { width: 1, height: 1, data: vec![[70000.125, 70000.25, 70000.5]].into() }
}

fn textured_pixels() -> Rgb32f {
    Rgb32f { width: 64, height: 48, data: (0..64 * 48).map(|i| [0.05 + (i % 71) as f32 / 100.0, 0.1800123, 0.5100247]).collect() }
}

fn fixture(pixels: fn() -> Rgb32f) -> (crate::Session, PhotoId, Arc<AtomicUsize>) {
    let mut session = crate::Session::new();
    let id = PhotoId(1);
    let mut photo = Photo::new(id, Source::File { path: "precision-fixture.raw".into() }, "precision-fixture.raw", "RAW", 1, 1, "");
    photo.kind = MediaKind::Raw;
    photo.content_hash = Some("original".into());
    session.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    let loads = Arc::new(AtomicUsize::new(0));
    let count = loads.clone();
    session.media.file_loader = Some(Arc::new(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        Ok((pixels(), SourceInfo { raw: true, ..Default::default() }))
    }));
    (session, id, loads)
}

fn job(session: &mut crate::Session, id: PhotoId, overflow: bool) -> RenderJob {
    let mut job = session.render_job(id, 640, 480, false, true).unwrap();
    assert_eq!(job.level, SourceLevel::Preview);
    job.request = RenderRequest::fit(1, 1);
    let mut settings = DevelopSettings::default();
    if overflow {
        settings.wb.mode = lightcraft_develop::WbMode::Custom;
        settings.wb.temp = 2000.0;
    }
    job.settings = Arc::new(settings);
    job
}

fn assert_exact(actual: &Rgb32f, expected: &Rgb32f) {
    assert_eq!((actual.width, actual.height), (expected.width, expected.height));
    for (actual, expected) in actual.data.iter().flatten().zip(expected.data.iter().flatten()) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

fn assert_f32_owner(result: &RenderResult) -> &Arc<ResidentSource> {
    assert!(result.rendered.is_ok());
    assert!(result.loaded.is_none(), "a successful exact GPU retry must release its CPU source");
    let owner = result.resident.as_ref().expect("successful f32 retry");
    assert!(owner.source.can_read_exact());
    assert_eq!(owner.source.storage_policy(), SourceStorage::F32);
    owner
}

struct SmartFixture(std::path::PathBuf);

impl SmartFixture {
    fn new(session: &mut crate::Session, id: PhotoId) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("unveil-recovery-smart-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(crate::smart::file_name(session.catalog.photo(id).unwrap()));
        std::fs::write(&path, crate::smart::encode(&textured_pixels(), None).unwrap()).unwrap();
        session.media.smart_dir = Some(dir.clone());
        session.media.availability.set_probe(Arc::new(|_| true));
        Self(dir)
    }
}

impl Drop for SmartFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn original_availability(session: &mut crate::Session, online: bool) -> Arc<AtomicBool> {
    let available = Arc::new(AtomicBool::new(online));
    let state = available.clone();
    let loader = session.media.file_loader.clone().unwrap();
    session.media.file_loader =
        Some(Arc::new(move |path, edge| if state.load(Ordering::Relaxed) { loader(path, edge) } else { Err("original is offline".into()) }));
    available
}

#[test]
fn ordinary_offline_loading_keeps_its_lossy_smart_preview_fallback() {
    let (mut session, id, _) = fixture(textured_pixels);
    let _smart = SmartFixture::new(&mut session, id);
    let _online = original_availability(&mut session, false);
    let photo = session.catalog.photo(id).unwrap().clone();
    let decoded = session.media.source_ref(&photo, SourceLevel::Preview).load_source().unwrap();
    assert_eq!((decoded.image.width, decoded.image.height), (64, 48));
    assert!(decoded.info.is_none());
    assert_ne!(*decoded.image, textured_pixels(), "the real JPEG proxy must differ from the original samples");
}

#[test]
fn exact_half_recovery_rejects_same_size_smart_fallback_without_retiring_the_original() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(textured_pixels);
    let _smart = SmartFixture::new(&mut session, id);
    let online = original_availability(&mut session, true);
    let first = job(&mut session, id, false).run();
    let owner = first.resident.as_ref().unwrap().clone();
    assert!(!owner.source.can_read_exact());
    session.accept(&first);
    drop(first);
    let exact = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_exact(&exact, &textured_pixels());
    drop(exact);
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    online.store(false, Ordering::Relaxed);
    let error = session.source_now(id, SourceLevel::Preview).err().expect("an offline original cannot be recovered from a lossy proxy");
    assert!(error.contains("original is offline"), "{error}");
    let mut missing_facts = job(&mut session, id, false);
    Arc::make_mut(&mut missing_facts.settings).effects.dehaze = 25.0;
    missing_facts.request = RenderRequest::fit(64, 48);
    missing_facts.request.window = Some(lightcraft_pipeline::PixelWindow { x: 8, y: 6, w: 32, h: 24 });
    let retry = missing_facts.clone();
    let failed = missing_facts.run();
    assert!(failed.rendered.is_err(), "new facts must not be computed from the proxy");
    session.accept(&failed);
    drop(failed);
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.recovery_pixels), (0, 0));
    let uploads = lightcraft_gpu::source_uploads();
    let next = job(&mut session, id, false).run();
    assert_eq!(next.resident.as_ref().unwrap().id(), owner.id());
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
    online.store(true, Ordering::Relaxed);
    let recovered = retry.run();
    assert_eq!(recovered.resident.as_ref().unwrap().id(), owner.id());
    assert_eq!(loads.load(Ordering::Relaxed), 3, "failed recovery must not publish planner facts");
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
}

#[test]
fn half_smart_resident_recovers_its_proxy_when_the_original_returns() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(textured_pixels);
    let _smart = SmartFixture::new(&mut session, id);
    let online = original_availability(&mut session, false);
    let photo = session.catalog.photo(id).unwrap().clone();
    let proxy = session.media.source_ref(&photo, SourceLevel::Preview).load().unwrap();
    let first = job(&mut session, id, false).run();
    let owner_id = first.resident.as_ref().unwrap().id();
    session.accept(&first);
    drop(first);
    online.store(true, Ordering::Relaxed);
    let recovered = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_exact(&recovered, &proxy);
    assert_eq!(loads.load(Ordering::Relaxed), 0, "recovery must keep the resident's selected source");
    drop(recovered);
    let next = job(&mut session, id, false).run();
    assert_eq!(next.resident.as_ref().unwrap().id(), owner_id);
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 0);
}

#[test]
fn half_promotion_of_cached_original_keeps_original_recovery_when_availability_changes() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, _) = fixture(textured_pixels);
    let _smart = SmartFixture::new(&mut session, id);
    let online = original_availability(&mut session, true);
    assert_exact(&session.source_now(id, SourceLevel::Preview).unwrap(), &textured_pixels());
    online.store(false, Ordering::Relaxed);
    session.media.availability.set_probe(Arc::new(|path| path.ends_with(".lcsp")));
    let first = job(&mut session, id, false).run();
    assert!(first.resident.is_some(), "the already cached original can still be promoted");
    session.accept(&first);
    drop(first);
    assert!(session.source_now(id, SourceLevel::Preview).is_err(), "the promoted original must not reload from a proxy");
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.recovery_pixels), (0, 0));
    assert!(usage.gpu_pixels > 0);
}

#[test]
fn f32_smart_readback_keeps_proxy_recovery_after_cpu_mode_and_repromotion() {
    if std::env::var("UNVEIL_E2_HALF").unwrap_or_else(|_| "off".into()) != "off" {
        eprintln!("skipped: set UNVEIL_E2_HALF=off for exact f32 proxy readback validation");
        return;
    }
    if !super::resident_tests::gpu_available() {
        return;
    }
    let (mut session, id, loads) = fixture(textured_pixels);
    let _smart = SmartFixture::new(&mut session, id);
    let online = original_availability(&mut session, false);
    let photo = session.catalog.photo(id).unwrap().clone();
    let proxy = session.media.source_ref(&photo, SourceLevel::Preview).load().unwrap();
    let first = job(&mut session, id, false).run();
    assert!(first.resident.as_ref().unwrap().source.can_read_exact());
    session.accept(&first);
    drop(first);
    online.store(true, Ordering::Relaxed);
    let cpu = job(&mut session, id, false).with_proof(Some(lightcraft_pipeline::Proof::default())).run();
    assert!(cpu.rendered.is_ok() && cpu.loaded.is_some());
    session.accept(&cpu);
    drop(cpu);
    let next = job(&mut session, id, false).run();
    assert_exact(&next.resident.as_ref().unwrap().source.read_original().unwrap(), &proxy);
    session.accept(&next);
    drop(next);
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::ReadbackAfter(0));
    let recovered = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_exact(&recovered, &proxy);
    assert_eq!(loads.load(Ordering::Relaxed), 0);
}

#[test]
fn first_precision_rejection_retries_from_host_without_an_extra_decode() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(hdr_pixels);
    let stages = Arc::new(StageCache::default());
    let uploads = lightcraft_gpu::source_uploads();
    let result = job(&mut session, id, true).with_stages(stages.clone()).run();
    let source_id = assert_f32_owner(&result).id();
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 2, "one rejected half upload and one exact retry");
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    assert_exact(&result.resident.as_ref().unwrap().source.read_original().unwrap(), &hdr_pixels());
    session.accept(&result);
    drop(result);
    let uploads = lightcraft_gpu::source_uploads();
    for _ in 0..3 {
        let result = job(&mut session, id, true).with_stages(stages.clone()).run();
        assert_eq!(assert_f32_owner(&result).id(), source_id);
        session.accept(&result);
    }
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
    assert_eq!(session.media.residency(SourceLevel::Preview).cpu_pixels, 0);
    assert_eq!(stages.bytes(), 0, "GPU stages must not pin the host used for retry");
}

#[test]
fn resident_precision_rejection_reloads_once_and_survives_eviction_until_content_changes() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(hdr_pixels);
    let first = job(&mut session, id, false).run();
    let half = first.resident.as_ref().unwrap().clone();
    assert!(!half.source.can_read_exact());
    session.accept(&first);
    drop(first);
    let stages = Arc::new(StageCache::default());
    let uploads = lightcraft_gpu::source_uploads();
    let retry = job(&mut session, id, true).with_stages(stages.clone()).run();
    let exact_id = assert_f32_owner(&retry).id();
    assert_ne!(exact_id, half.id(), "a recipe reload starts a new decoded identity");
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 1);
    session.accept(&retry);
    drop(retry);
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 0);
    for _ in 0..3 {
        let result = job(&mut session, id, true).with_stages(stages.clone()).run();
        assert_eq!(assert_f32_owner(&result).id(), exact_id);
        session.accept(&result);
    }
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 1);
    session.media.release_device_sources();
    let reload = job(&mut session, id, true).run();
    assert_ne!(assert_f32_owner(&reload).id(), exact_id);
    assert_eq!(loads.load(Ordering::Relaxed), 3);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 2, "eviction must not retry half storage");
    session.accept(&reload);
    drop(reload);
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::ReadbackAfter(0));
    let recovered = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_exact(&recovered, &hdr_pixels());
    drop(recovered);
    let promoted = job(&mut session, id, true).run();
    assert_f32_owner(&promoted);
    assert_eq!(loads.load(Ordering::Relaxed), 4);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 3, "readback recovery must retain forced f32 too");
    session.accept(&promoted);
    drop(promoted);
    session
        .catalog
        .apply(lightcraft_catalog::Op::SetContent {
            id,
            width: 1,
            height: 1,
            file_size: 12,
            content_hash: Some("replacement".into()),
            preview_only: None,
        })
        .unwrap();
    session.media.forget(id);
    let replacement = job(&mut session, id, false).run();
    let owner = replacement.resident.as_ref().unwrap();
    assert!(!owner.source.can_read_exact(), "new content gets its own automatic precision policy");
    assert_eq!(owner.source.storage_policy(), SourceStorage::Automatic);
}

#[test]
fn successful_internal_f32_retry_persists_without_reuploading_the_current_handle() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(overrange_pixels);
    let uploads = lightcraft_gpu::source_uploads();
    let first = job(&mut session, id, true).run();
    let owner_id = assert_f32_owner(&first).id();
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 1, "an already exact source retries derived stages internally");
    session.accept(&first);
    drop(first);
    let next = job(&mut session, id, true).run();
    assert_eq!(assert_f32_owner(&next).id(), owner_id);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 1);
    session.accept(&next);
    drop(next);
    session.media.release_device_sources();
    let request = job(&mut session, id, false);
    assert_eq!(request.source_policy.storage(), SourceStorage::F32, "successful internal fallback must survive eviction");
    let reloaded = request.run();
    assert_f32_owner(&reloaded);
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 2);
}

#[test]
fn one_shot_half_source_analysis_reloads_exact_bits_and_preserves_only_the_device_owner() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(exact_pixels);
    let stages = Arc::new(StageCache::default());
    let result = job(&mut session, id, false).with_stages(stages.clone()).run();
    let half_id = result.resident.as_ref().unwrap().id();
    assert!(!result.resident.as_ref().unwrap().source.can_read_exact());
    session.accept(&result);
    drop(result);
    let first = session.source_now(id, SourceLevel::Preview).unwrap();
    let second = session.media.get(id, SourceLevel::Preview).unwrap();
    let photo = session.catalog.photo(id).unwrap().clone();
    let decoded = session.media.source_ref(&photo, SourceLevel::Preview).load_source().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&first, &decoded.image));
    assert_exact(&first, &exact_pixels());
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.recovery_pixels), (0, 12));
    assert!(usage.gpu_pixels > 0);
    let weak = Arc::downgrade(&first);
    drop((first, second, decoded));
    assert!(weak.upgrade().is_none());
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 0);
    let uploads = lightcraft_gpu::source_uploads();
    let next = job(&mut session, id, false).with_stages(stages).run();
    assert_eq!(next.resident.as_ref().unwrap().id(), half_id);
    assert!(next.loaded.is_none());
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads, "analysis must not require another GPU upload");
}

#[test]
fn overlapping_half_recovery_shares_one_exact_recipe_reload_without_pinning_pixels() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(exact_pixels);
    let result = job(&mut session, id, false).run();
    let resident = result.resident.as_ref().unwrap().clone();
    session.accept(&result);
    drop(result);
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let resident = resident.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let recovered = resident.recover().unwrap();
                assert_eq!(recovered.kind, resident::RecoveryKind::HalfReload);
                assert!(recovered.decoded.info.as_ref().unwrap().raw);
                recovered.decoded.image
            })
        })
        .collect();
    barrier.wait();
    let images: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    assert!(images.iter().all(|image| Arc::ptr_eq(image, &images[0])));
    assert_exact(&images[0], &exact_pixels());
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(session.media.residency(SourceLevel::Preview).reloads, 1);
    let weak = Arc::downgrade(&images[0]);
    drop(images);
    assert!(weak.upgrade().is_none());
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 0);
}

#[test]
fn one_shot_half_source_deep_export_releases_its_cpu_pixels_and_reuses_the_device() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(exact_pixels);
    let stages = Arc::new(StageCache::default());
    let first = job(&mut session, id, false).with_stages(stages.clone()).run();
    let source_id = first.resident.as_ref().unwrap().id();
    session.accept(&first);
    drop(first);
    let uploads = lightcraft_gpu::source_uploads();
    let mut export = job(&mut session, id, false).with_stages(stages.clone());
    export.request.depth = lightcraft_pipeline::OutputDepth::F32Linear;
    let expected = lightcraft_pipeline::render(&exact_pixels(), &SourceInfo { raw: true, ..Default::default() }, &export.settings, &export.request);
    let result = export.run();
    assert_eq!(result.rendered.as_ref().unwrap().deep.as_ref().unwrap().samples, expected.deep.unwrap().samples);
    assert_eq!(result.resident.as_ref().unwrap().id(), source_id);
    assert!(result.loaded.is_none());
    session.accept(&result);
    drop(result);
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.recovery_pixels), (0, 0));
    assert_eq!(stages.bytes(), 0);
    let next = job(&mut session, id, false).with_stages(stages).run();
    assert_eq!(next.resident.as_ref().unwrap().id(), source_id);
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
}

#[test]
fn concurrent_missing_facts_and_analysis_share_a_temporary_half_reload() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(textured_pixels);
    let first = job(&mut session, id, false).run();
    let resident = first.resident.as_ref().unwrap().clone();
    session.accept(&first);
    drop(first);
    let mut request = job(&mut session, id, false);
    Arc::make_mut(&mut request.settings).effects.dehaze = 25.0;
    request.request = RenderRequest::fit(64, 48);
    request.request.window = Some(lightcraft_pipeline::PixelWindow { x: 8, y: 6, w: 32, h: 24 });
    let next_request = request.clone();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let analysis = {
        let resident = resident.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            resident.recover().unwrap().decoded.image
        })
    };
    let render = {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            request.run()
        })
    };
    barrier.wait();
    let pixels = analysis.join().unwrap();
    let result = render.join().unwrap();
    assert_eq!(result.resident.as_ref().unwrap().id(), resident.id());
    assert!(result.loaded.is_none());
    assert_exact(&pixels, &textured_pixels());
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    session.accept(&result);
    drop((pixels, result));
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.recovery_pixels), (0, 0));
    assert_eq!(next_request.run().resident.as_ref().unwrap().id(), resident.id());
    assert_eq!(loads.load(Ordering::Relaxed), 2, "new facts must survive after the temporary original is released");
}

#[test]
fn content_reload_retires_the_old_half_recovery_lease_and_detached_work() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, _) = fixture(exact_pixels);
    let first = job(&mut session, id, false).run();
    let old_id = first.resident.as_ref().unwrap().id();
    session.accept(&first);
    drop(first);
    let old_work = job(&mut session, id, false);
    let old_pixels = session.source_now(id, SourceLevel::Preview).unwrap();
    session
        .catalog
        .apply(lightcraft_catalog::Op::SetContent {
            id,
            width: 64,
            height: 48,
            file_size: 64 * 48 * 12,
            content_hash: Some("replacement".into()),
            preview_only: None,
        })
        .unwrap();
    session.media.file_loader = Some(Arc::new(|_, _| Ok((textured_pixels(), SourceInfo { raw: true, ..Default::default() }))));
    session.media.forget(id);
    session.accept(&old_work.run());
    assert!(!session.media.has_source(id, SourceLevel::Preview));
    let replacement = job(&mut session, id, false).run();
    assert_ne!(replacement.resident.as_ref().unwrap().id(), old_id);
    session.accept(&replacement);
    drop(replacement);
    let current = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_exact(&current, &textured_pixels());
    assert_exact(&old_pixels, &exact_pixels());
    drop((current, old_pixels));
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 0);
}

#[test]
fn active_f32_preference_survives_policy_lru_eviction_then_explicit_forget_clears_it() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(hdr_pixels);
    let first = job(&mut session, id, true).run();
    let resident = assert_f32_owner(&first).clone();
    session.accept(&first);
    drop(first);
    let mut photo = session.catalog.photo(id).unwrap().clone();
    for slot in 0..SOURCE_POLICY_ENTRIES {
        Arc::make_mut(&mut photo).id = PhotoId(slot as u64 + 2);
        session.media.source_policy(&photo, SourceLevel::Preview);
    }
    assert_eq!(session.media.source_policies.len(), SOURCE_POLICY_ENTRIES);
    assert!(!session.media.source_policies.contains_key(&id));
    let request = job(&mut session, id, true);
    assert!(Arc::ptr_eq(&request.source_policy, &resident.policy));
    let uploads = lightcraft_gpu::source_uploads();
    let next = request.run();
    assert_eq!(assert_f32_owner(&next).id(), resident.id());
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
    session.media.forget(id);
    assert!(!session.media.source_policies.contains_key(&id));
    assert_eq!(job(&mut session, id, false).source_policy.storage(), SourceStorage::Automatic);
    session.media.clear_sources();
    assert!(session.media.source_policies.is_empty());
}

#[test]
fn failed_f32_retry_keeps_exact_cpu_pixels_and_the_next_job_does_not_retry_half() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(hdr_pixels);
    let first = job(&mut session, id, false).run();
    session.accept(&first);
    drop(first);
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::Upload);
    let stages = Arc::new(StageCache::default());
    let request = job(&mut session, id, true).with_stages(stages.clone());
    let expected = lightcraft_pipeline::render(&hdr_pixels(), &SourceInfo { raw: true, ..Default::default() }, &request.settings, &request.request);
    let failed = request.run();
    assert_eq!(failed.rendered.as_ref().unwrap().image.data, expected.image.data);
    assert!(failed.resident.is_none());
    assert_exact(&failed.loaded.as_ref().unwrap().image, &hdr_pixels());
    session.accept(&failed);
    drop(failed);
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(session.media.residency(SourceLevel::Preview).gpu_pixels, 0);
    let uploads = lightcraft_gpu::source_uploads();
    let retry = job(&mut session, id, true).with_stages(stages.clone()).run();
    assert_f32_owner(&retry);
    session.accept(&retry);
    drop(retry);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads + 1);
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(session.media.residency(SourceLevel::Preview).cpu_pixels, 0);
    assert_eq!(stages.bytes(), 0);
}

#[test]
fn cpu_only_proof_reloads_a_half_source_once_and_uses_exact_samples() {
    if !half_source_available() {
        return;
    }
    let (mut session, id, loads) = fixture(exact_pixels);
    let first = job(&mut session, id, false).run();
    session.accept(&first);
    drop(first);
    let mut expected = None;
    for _ in 0..3 {
        let mut request = job(&mut session, id, false).with_proof(Some(lightcraft_pipeline::Proof::default()));
        request.request.depth = lightcraft_pipeline::OutputDepth::F32Linear;
        if expected.is_none() {
            expected = Some(lightcraft_pipeline::render(
                &exact_pixels(),
                &SourceInfo { raw: true, ..Default::default() },
                &request.settings,
                &request.request,
            ));
        }
        let result = request.run();
        assert!(result.resident.is_none());
        assert_eq!(result.rendered.as_ref().unwrap().deep.as_ref().unwrap().samples, expected.as_ref().unwrap().deep.as_ref().unwrap().samples);
        session.accept(&result);
    }
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.gpu_pixels), (12, 0));
}

#[test]
fn cpu_disabled_renders_exact_source_without_any_gpu_owner() {
    if std::env::var("LIGHTCRAFT_GPU").as_deref() != Ok("0") {
        eprintln!("skipped: set LIGHTCRAFT_GPU=0 for disabled-GPU validation");
        return;
    }
    let (mut session, id, loads) = fixture(exact_pixels);
    let uploads = lightcraft_gpu::source_uploads();
    for _ in 0..3 {
        let mut request = job(&mut session, id, false);
        request.request.depth = lightcraft_pipeline::OutputDepth::F32Linear;
        let expected =
            lightcraft_pipeline::render(&exact_pixels(), &SourceInfo { raw: true, ..Default::default() }, &request.settings, &request.request);
        let result = request.run();
        assert!(result.resident.is_none());
        assert_eq!(result.rendered.as_ref().unwrap().deep.as_ref().unwrap().samples, expected.deep.unwrap().samples);
        session.accept(&result);
    }
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.gpu_pixels), (12, 0));
}
