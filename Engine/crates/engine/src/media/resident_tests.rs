//! Modified for Unveil (2026-10-14): source ownership, exact recovery and eviction gates.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) fn gpu_available() -> bool {
    let available = lightcraft_gpu::available();
    assert!(
        available || std::env::var_os("UNVEIL_REQUIRE_GPU").is_none(),
        "a GPU adapter is required for ownership validation: {:?}",
        lightcraft_gpu::unavailable_reason()
    );
    if !available {
        eprintln!("skipped: no GPU adapter for source ownership");
    }
    available
}

fn pattern(seed: usize) -> Rgb32f {
    let mut image = Rgb32f::new(64, 48);
    for (i, pixel) in image.data.iter_mut().enumerate() {
        *pixel = [((i + seed) % 97) as f32 / 100.0, ((i * 3 + seed) % 83) as f32 / 100.0, 0.1];
    }
    image
}

#[test]
fn cold_promotion_releases_host_before_render_and_recovers_exactly_on_failure() {
    if !gpu_available() {
        return;
    }
    use lightcraft_raster::memory_trace;
    use resident::Promotion;
    static HOST: std::sync::Mutex<std::sync::Weak<Rgb32f>> = std::sync::Mutex::new(std::sync::Weak::new());
    static OBSERVED: AtomicUsize = AtomicUsize::new(0);
    memory_trace::set_sink(|event| {
        if event.owner == "promotion-render-begin" {
            assert!(HOST.lock().unwrap().upgrade().is_none(), "CPU pixels must end before render scratch starts");
            OBSERVED.fetch_add(1, Ordering::Relaxed);
        }
    });
    memory_trace::set_enabled(true);
    for fault in [None, Some(lightcraft_gpu::Fault::Limit(16)), Some(lightcraft_gpu::Fault::DropWork)] {
        lightcraft_gpu::reset_failures();
        let source = Arc::new(pattern(11));
        *HOST.lock().unwrap() = Arc::downgrade(&source);
        let reload = SourceRef::File {
            path: "owned-source".into(),
            max_edge: 2560,
            loader: Some(Arc::new(|_, _| Ok((pattern(11), SourceInfo::default())))),
            fallback: None,
            denoise: None,
        };
        if let Some(fault) = fault {
            lightcraft_gpu::inject_fault(fault);
        }
        let result = resident::promote_owned(
            SourceId::new(),
            Arc::new(resident::SourcePolicy::default()),
            DecodedSource::new(source, Some(SourceInfo::default())),
            reload,
            SourceInfo::default(),
            &DevelopSettings::default(),
            &RenderRequest::fit(64, 48),
            None,
            false,
        );
        match (fault, result) {
            (None, Promotion::Device(_, owner)) => assert_eq!(*owner.recover().unwrap().decoded.image, pattern(11)),
            (Some(_), Promotion::Host(host)) => assert_eq!(*host.image, pattern(11)),
            (_, Promotion::Failed(error)) => panic!("unexpected recovery failure: {error}"),
            _ => panic!("wrong ownership after promotion"),
        }
    }
    memory_trace::set_enabled(false);
    lightcraft_gpu::reset_failures();
    assert_eq!(OBSERVED.load(Ordering::Relaxed), 3);
}

fn fixture_job(session: &mut crate::Session, seed: Arc<AtomicUsize>, loads: Arc<AtomicUsize>) -> RenderJob {
    let id = session.active().unwrap();
    let mut job = session.render_job(id, 640, 480, false, true).unwrap();
    job.source = SourceRef::File {
        path: "resident-test-fixture".into(),
        max_edge: 2560,
        loader: Some(Arc::new(move |_, _| {
            loads.fetch_add(1, Ordering::Relaxed);
            if seed.load(Ordering::Relaxed) == usize::MAX {
                return Err("original is offline".into());
            }
            Ok((pattern(seed.load(Ordering::Relaxed)), SourceInfo::default()))
        })),
        fallback: None,
        denoise: None,
    };
    job
}

fn promoted_fixture() -> (crate::Session, Arc<ResidentSource>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let mut session = crate::Session::with_demo();
    let seed = Arc::new(AtomicUsize::new(0));
    let loads = Arc::new(AtomicUsize::new(0));
    let result = fixture_job(&mut session, seed.clone(), loads.clone()).run();
    let resident = result.resident.as_ref().expect("validated GPU promotion").clone();
    session.accept(&result);
    (session, resident, seed, loads)
}

#[test]
fn successful_device_promotion_releases_host_pixels_and_cpu_source_now_is_transient() {
    if !gpu_available() {
        return;
    }
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    let mut pixels = Rgb32f::new(64, 48);
    for (i, pixel) in pixels.data.iter_mut().enumerate() {
        *pixel = [i as f32 / 4096.0, 0.2, 0.4];
    }
    let expected = pixels.clone();
    let source = Arc::new(pixels);
    let weak = Arc::downgrade(&source);
    session.media.insert(id, SourceLevel::Preview, source);
    let stages = Arc::new(StageCache::default());
    let result = session.render_job(id, 640, 480, false, true).unwrap().with_stages(stages.clone()).run();
    assert!(result.rendered.is_ok());
    assert!(result.resident.is_some(), "a validated GPU render must offer promotion");
    session.accept(&result);
    drop(result);
    assert!(weak.upgrade().is_none(), "neither cache, GPU stages nor result may pin the CPU original");
    assert_eq!(session.media.residency(SourceLevel::Preview).cpu_pixels, 0);
    let recovered = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_eq!(*recovered, expected);
    assert_eq!(session.media.residency(SourceLevel::Preview).cpu_pixels, 0, "analysis borrows temporary CPU pixels");
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 64 * 48 * 12);
    drop(recovered);
    assert_eq!(session.media.residency(SourceLevel::Preview).recovery_pixels, 0);
    let next = session.render_job(id, 640, 480, false, true).unwrap().with_stages(stages).run();
    assert!(next.rendered.is_ok());
    assert!(next.loaded.is_none());
}

#[test]
fn releasing_full_source_rejects_an_older_completion_without_losing_preview() {
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    session.media.insert(id, SourceLevel::Preview, Arc::new(Rgb32f::new(8, 8)));
    let mut old = session.render_job(id, 4000, 3000, false, true).unwrap();
    old.source = SourceRef::File {
        path: "test".into(),
        max_edge: usize::MAX,
        loader: Some(Arc::new(|_, _| Ok((Rgb32f::new(8, 8), SourceInfo::default())))),
        fallback: None,
        denoise: None,
    };
    session.media.release_full_source();
    let result = old.run();
    session.accept(&result);
    assert!(!session.media.has_source(id, SourceLevel::Full));
    assert!(session.media.has_source(id, SourceLevel::Preview));
}

#[test]
fn failed_upload_keeps_the_host_original_and_retry_can_promote() {
    if !gpu_available() {
        return;
    }
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    let original = Arc::new(pattern(0));
    let weak = Arc::downgrade(&original);
    session.media.insert(id, SourceLevel::Preview, original);
    let job = session.render_job(id, 640, 480, false, true).unwrap();
    let expected = lightcraft_pipeline::render(&pattern(0), &job.info, &job.settings, &job.request);
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::Upload);
    let failed = job.run();
    assert_eq!(failed.rendered.as_ref().unwrap().image.data, expected.image.data);
    assert!(failed.resident.is_none());
    session.accept(&failed);
    assert!(weak.upgrade().is_some(), "upload failure cannot discard the only valid pixels");
    let retry = session.render_job(id, 640, 480, false, true).unwrap().run();
    assert!(retry.resident.is_some());
    session.accept(&retry);
    drop(retry);
    assert!(weak.upgrade().is_none());
}

#[test]
fn exact_cpu_export_recovers_once_and_does_not_replace_device_ownership() {
    if !gpu_available() {
        return;
    }
    let (mut session, resident, _, loads) = promoted_fixture();
    let id = session.active().unwrap();
    let stages = Arc::new(StageCache::default());
    let mut job = session.render_job(id, 640, 480, false, true).unwrap().with_stages(stages.clone());
    job.request.depth = lightcraft_pipeline::OutputDepth::F32Linear;
    let expected = lightcraft_pipeline::render(&pattern(0), &SourceInfo::default(), &job.settings, &job.request);
    let result = job.run();
    assert_eq!(result.rendered.as_ref().unwrap().deep.as_ref().unwrap().samples, expected.deep.unwrap().samples);
    assert!(result.loaded.is_none());
    assert_eq!(result.resident.as_ref().unwrap().id(), resident.id());
    session.accept(&result);
    drop(result);
    assert!(stages.is_empty(), "CPU fallback must release its original source pin");
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.readbacks, usage.reloads), (0, 1, 0));
    assert_eq!(loads.load(Ordering::Relaxed), 1);
}

#[test]
fn partial_readback_failure_reloads_once_and_changed_content_gets_a_new_identity() {
    if !gpu_available() {
        return;
    }
    let (mut session, resident, seed, loads) = promoted_fixture();
    let id = session.active().unwrap();
    seed.store(29, Ordering::Relaxed);
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::ReadbackAfter(0));
    let recovered = session.source_now(id, SourceLevel::Preview).unwrap();
    assert_eq!(*recovered, pattern(29), "no partial old readback may escape as recovered pixels");
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    let replacement = session.render_job(id, 640, 480, false, true).unwrap().run();
    assert_ne!(replacement.resident.as_ref().unwrap().id(), resident.id());
    session.accept(&replacement);
    drop(replacement);
    assert_eq!(session.media.residency(SourceLevel::Preview).cpu_pixels, 0);
    assert_eq!(loads.load(Ordering::Relaxed), 2, "promotion of reloaded pixels must not decode again");
}

#[test]
fn overlapping_analysis_calls_share_one_temporary_recovery_and_release_it() {
    if !gpu_available() {
        return;
    }
    let (session, resident, _, loads) = promoted_fixture();
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let resident = resident.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                resident.recover().unwrap().decoded.image
            })
        })
        .collect();
    barrier.wait();
    let images: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    assert!(images.iter().all(|image| Arc::ptr_eq(image, &images[0])));
    let weak = Arc::downgrade(&images[0]);
    assert_eq!(session.media.residency(SourceLevel::Preview).readbacks, 1);
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    drop(images);
    assert!(weak.upgrade().is_none(), "the source must hold only a weak recovery lease");
}

#[test]
fn dehaze_fact_activation_recovers_once_then_white_balance_and_histogram_reuse_it() {
    if !gpu_available() {
        return;
    }
    let (mut session, resident, _, loads) = promoted_fixture();
    let id = session.active().unwrap();
    let mut settings = DevelopSettings::default();
    settings.effects.dehaze = 25.0;
    let stages = Arc::new(StageCache::default());
    for temp in [5200.0, 4200.0, 6300.0] {
        settings.wb.mode = lightcraft_develop::WbMode::Custom;
        settings.wb.temp = temp;
        let mut job = session.render_job(id, 640, 480, false, true).unwrap().with_stages(stages.clone());
        job.settings = Arc::new(settings.clone());
        job.request.window = Some(lightcraft_pipeline::PixelWindow { x: 12, y: 9, w: 29, h: 21 });
        let expected = lightcraft_gpu::render(&Arc::new(pattern(0)), &SourceInfo::default(), &settings, &job.request, None).unwrap();
        let result = job.run();
        assert_eq!(result.resident.as_ref().unwrap().id(), resident.id());
        assert_eq!(result.rendered.as_ref().unwrap().image.data, expected.image.data);
        session.accept(&result);
    }
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!((usage.cpu_pixels, usage.readbacks, usage.reloads), (0, 1, 0));
    assert!(usage.planning_facts <= 8 << 20);
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    let mut histogram = session.render_job(id, 640, 480, false, true).unwrap().with_stages(stages);
    histogram.request.max_w = 512;
    histogram.request.max_h = 512;
    let uploads = lightcraft_gpu::source_uploads();
    let result = histogram.run();
    assert_eq!(result.resident.as_ref().unwrap().id(), resident.id());
    assert_eq!(lightcraft_gpu::source_uploads(), uploads, "histogram uses the preview original");
}

#[test]
fn replacement_and_suspension_reject_late_device_completions() {
    if !gpu_available() {
        return;
    }
    let (mut session, resident, _, _) = promoted_fixture();
    let id = session.active().unwrap();
    let old = session.render_job(id, 640, 480, false, true).unwrap();
    session.media.forget(id);
    session.media.insert(id, SourceLevel::Preview, Arc::new(pattern(19)));
    session.accept(&old.run());
    let current = session.render_job(id, 640, 480, false, true).unwrap().run();
    assert_ne!(current.resident.as_ref().unwrap().id(), resident.id());
    session.accept(&current);
    let in_flight = session.render_job(id, 640, 480, false, true).unwrap();
    session.media.release_device_sources();
    session.accept(&in_flight.run());
    assert!(!session.media.has_source(id, SourceLevel::Preview));
}

#[test]
fn disabled_sections_match_the_existing_render_path_after_promotion() {
    if !gpu_available() {
        return;
    }
    let (mut session, _, _, _) = promoted_fixture();
    let id = session.active().unwrap();
    let mut job = session.render_job(id, 640, 480, false, true).unwrap();
    let settings = Arc::make_mut(&mut job.settings);
    settings.light.exposure = 2.0;
    settings.set_section_enabled("light", false);
    let expected = lightcraft_gpu::render(&Arc::new(pattern(0)), &SourceInfo::default(), settings, &job.request, None).unwrap();
    let result = job.run();
    assert_eq!(result.rendered.unwrap().image.data, expected.image.data);
}

#[test]
fn proof_mode_recovers_once_then_keeps_the_cpu_original() {
    if !gpu_available() {
        return;
    }
    let (mut session, _, _, loads) = promoted_fixture();
    let id = session.active().unwrap();
    let stages = Arc::new(StageCache::default());
    let proof = lightcraft_pipeline::Proof::default();
    let mut expected = None;
    for _ in 0..3 {
        let job = session.render_job(id, 640, 480, false, true).unwrap().with_stages(stages.clone()).with_proof(Some(proof));
        let result = job.run();
        let image = result.rendered.as_ref().unwrap().image.clone();
        assert!(result.resident.is_none(), "persistent CPU editing must not retain a GPU original");
        if let Some(expected) = &expected {
            assert_eq!(&image, expected);
        } else {
            assert_eq!(session.media.residency(SourceLevel::Preview).readbacks, 1);
        }
        expected = Some(image);
        session.accept(&result);
    }
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    let usage = session.media.residency(SourceLevel::Preview);
    assert_eq!(usage.gpu_pixels, 0);
    assert_eq!(usage.cpu_pixels, 64 * 48 * 12);
    assert!(stages.peek_extension::<lightcraft_gpu::GpuStages>().is_none());
}

#[test]
fn cache_budget_eviction_rejects_an_older_full_completion() {
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    session.media.insert(id, SourceLevel::Full, Arc::new(pattern(0)));
    let mut job = session.render_job(id, 4000, 3000, false, true).unwrap();
    job.source = SourceRef::File {
        path: "resident-budget-fixture".into(),
        max_edge: usize::MAX,
        loader: Some(Arc::new(|_, _| Ok((pattern(0), SourceInfo::default())))),
        fallback: None,
        denoise: None,
    };
    session.media.set_budget(0);
    assert!(!session.media.has_source(id, SourceLevel::Full));
    session.media.set_budget(64 << 20);
    session.accept(&job.run());
    assert!(!session.media.has_source(id, SourceLevel::Full), "an evicted source cannot be revived by old work");
}

#[test]
fn failed_recovery_reports_the_loader_error_without_returning_black_pixels() {
    if !gpu_available() {
        return;
    }
    let (mut session, _, seed, loads) = promoted_fixture();
    let id = session.active().unwrap();
    seed.store(usize::MAX, Ordering::Relaxed);
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::ReadbackAfter(0));
    let error = session.source_now(id, SourceLevel::Preview).unwrap_err();
    assert!(error.contains("source readback") && error.contains("original is offline"), "{error}");
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    assert_eq!(session.media.residency(SourceLevel::Preview).cpu_pixels, 0);
}

#[test]
fn preview_capacity_and_same_photo_replacement_retire_old_jobs() {
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    session.media.preview_capacity = 1;
    session.media.insert(id, SourceLevel::Preview, Arc::new(pattern(0)));
    let preview_id = session.media.get_cached(id, SourceLevel::Preview).unwrap().id();
    session.media.insert(id, SourceLevel::Full, Arc::new(pattern(0)));
    assert_ne!(preview_id, session.media.get_cached(id, SourceLevel::Full).unwrap().id());
    let old = fixture_job(&mut session, Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    session.media.insert(id, SourceLevel::Preview, Arc::new(pattern(19)));
    session.accept(&old.run());
    assert_eq!(*session.source_now(id, SourceLevel::Preview).unwrap(), pattern(19));
    let evicted = fixture_job(&mut session, Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    session.media.insert(PhotoId(u64::MAX), SourceLevel::Preview, Arc::new(pattern(29)));
    session.accept(&evicted.run());
    assert!(!session.media.has_source(id, SourceLevel::Preview));
    assert_eq!(session.media.previews.len(), 1);
}
