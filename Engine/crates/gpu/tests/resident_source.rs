// Modified by the Unveil authors, 2026: immutable resident-source checks; see Engine/CHANGES.md.
//! Explicit f32 originals retain exact bytes; Automatic matches the legacy precision policy.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_gpu::{GpuSource, SourceStorage};
use lightcraft_pipeline::{RenderRequest, Rendered, SourceId, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

// Exact-bit recovery and render-cache assertions need an explicitly exact source, even when
// Automatic storage defaults to half. Its uncached control must use that same policy.
fn exact_source(id: SourceId, image: &Rgb32f) -> Arc<GpuSource> {
    lightcraft_gpu::upload_source_with_storage(id, image, SourceStorage::F32).unwrap()
}

fn exact_render(image: &Rgb32f, info: &SourceInfo, settings: &DevelopSettings, request: &RenderRequest) -> Rendered {
    let source = exact_source(SourceId::new(), image);
    let plan = lightcraft_pipeline::plan(image, info, settings, request);
    let rendered = lightcraft_gpu::render_prepared(&source, info, &plan, request, None, false).unwrap();
    Rendered { display_size: rendered.display_size, image: rendered.image.into_owned().unwrap(), histogram: rendered.histogram, deep: rendered.deep }
}

#[test]
fn resident_originals_are_exact_independent_and_reusable() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for resident sources");
        return;
    }
    eprintln!("resident source adapter: {:?}", lightcraft_gpu::adapter_name());
    let bits = [0, 0x8000_0000, 1, 0x807f_ffff, 0x7f80_0000, 0xff80_0000, 0x7fc0_1234, 0xffc0_5678, 0x477f_e001];
    for pixels in [9, (16 << 20) / 12 + 1] {
        let image = Arc::new(Rgb32f {
            width: pixels,
            height: 1,
            data: (0..pixels).map(|i| std::array::from_fn(|c| f32::from_bits(bits[(3 * i + c) % bits.len()]))).collect(),
        });
        let weak = Arc::downgrade(&image);
        let id = SourceId::new();
        let source = exact_source(id, &image);
        assert_eq!(source.id(), id);
        assert_eq!((source.extent().width, source.extent().height), (pixels, 1));
        assert_eq!(source.bytes(), pixels * 12);
        assert!(source.can_read_exact());
        drop(image);
        assert!(weak.upgrade().is_none(), "upload must not keep CPU pixels alive");
        let recovered = source.read_original().expect("exact original readback");
        for (i, pixel) in recovered.data.iter().enumerate() {
            for c in 0..3 {
                assert_eq!(pixel[c].to_bits(), bits[(3 * i + c) % bits.len()]);
            }
        }
        if pixels > 9 {
            lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::ReadbackAfter(1));
            assert!(source.read_original().is_err(), "a failure after the first chunk must not return a partial image");
            lightcraft_gpu::reset_failures();
            assert!(source.read_original().is_ok(), "failed readback must leave immutable storage intact");
        }
    }

    let image = Arc::new(lightcraft_scenes::demo_library()[2].render(317, 239));
    let weak = Arc::downgrade(&image);
    let source = exact_source(SourceId::new(), &image);
    let info = SourceInfo { raw: true, ..Default::default() };
    let settings = DevelopSettings::default();
    let stages = StageCache::default();
    let request = RenderRequest::fit(317, 239);
    let _ = lightcraft_pipeline::render_cached(&image, &info, &settings, &request, &stages);
    assert!(!stages.is_empty(), "exercise removal of a previous CPU fallback's source pin");
    for (w, h) in [(317, 239), (173, 131), (317, 239)] {
        let request = RenderRequest::fit(w, h);
        let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
        let expected = exact_render(&image, &info, &settings, &request);
        let uploads = lightcraft_gpu::source_uploads();
        let actual = lightcraft_gpu::render_prepared(&source, &info, &plan, &request, Some(&stages), true).unwrap();
        assert_eq!(lightcraft_gpu::source_uploads(), uploads, "prepared renders reuse their immutable source");
        assert!(stages.is_empty(), "accepted GPU output must clear CPU stages without clearing GPU stages");
        assert!(lightcraft_gpu::stage_bytes(&stages) > 0);
        assert_eq!(actual.histogram, expected.histogram);
        assert_eq!(actual.image.into_owned().unwrap().data, expected.image.data);
    }
    let expected = image.data.clone();
    let plan = lightcraft_pipeline::plan(&image, &info, &settings, &request);
    let expected_frame = exact_render(&image, &info, &settings, &request);
    drop(image);
    assert!(weak.upgrade().is_none(), "resident stages must not pin a CPU source");
    let frame = lightcraft_gpu::render_prepared(&source, &info, &plan, &request, Some(&stages), true).unwrap();
    assert_eq!(frame.image.into_owned().unwrap().data, expected_frame.image.data, "render after CPU release");
    assert_eq!(source.read_original().unwrap().data, expected);
    lightcraft_gpu::set_enabled(false);
    assert!(source.read_original().is_err(), "disabled GPU access requires the engine reload recipe");
    lightcraft_gpu::set_enabled(true);
    let invalid = Rgb32f { width: 2, height: 2, data: vec![[0.0; 3]].into() };
    assert!(lightcraft_gpu::upload_source(SourceId::new(), &invalid).is_err());
    lightcraft_gpu::inject_source_fault(lightcraft_gpu::SourceFault::Upload);
    assert!(lightcraft_gpu::upload_source(SourceId::new(), &source.read_original().unwrap()).is_err());
    lightcraft_gpu::reset_failures();
}

#[test]
fn resident_ids_separate_equal_extents_and_failures_never_publish() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for source identity");
        return;
    }
    let first = Arc::new(lightcraft_scenes::demo_library()[1].render(133, 97));
    let second = Arc::new(lightcraft_scenes::demo_library()[2].render(133, 97));
    let sources = [&first, &second].map(|image| exact_source(SourceId::new(), image));
    assert_ne!(sources[0].id(), sources[1].id());
    let stages = StageCache::default();
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut settings = DevelopSettings::default();
    settings.effects.clarity = 23.0;
    let request = RenderRequest::fit(113, 79);
    for index in [0, 1, 0] {
        let image = [&first, &second][index];
        let plan = lightcraft_pipeline::plan(image, &info, &settings, &request);
        let expected = exact_render(image, &info, &settings, &request);
        let actual = lightcraft_gpu::render_prepared(&sources[index], &info, &plan, &request, Some(&stages), false).unwrap();
        assert_eq!(actual.image.into_owned().unwrap().data, expected.image.data);
    }
    let plan = lightcraft_pipeline::plan(&first, &info, &settings, &request);
    for fault in [lightcraft_gpu::Fault::Limit(1024), lightcraft_gpu::Fault::Validation, lightcraft_gpu::Fault::DropWork] {
        lightcraft_gpu::inject_fault(fault);
        assert!(lightcraft_gpu::render_prepared(&sources[0], &info, &plan, &request, Some(&stages), true).is_none());
        assert_eq!(lightcraft_gpu::stage_bytes(&stages), 0, "failed work must clear derived stages");
        lightcraft_gpu::reset_failures();
        assert_eq!(sources[0].read_original().unwrap().data, first.data, "render failure must not alter the original");
        assert!(lightcraft_gpu::render_prepared(&sources[0], &info, &plan, &request, Some(&stages), true).is_some());
    }
}

#[test]
fn legacy_stages_do_not_pin_the_original_cpu_allocation() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for legacy source ownership");
        return;
    }
    let image = Arc::new(lightcraft_scenes::demo_library()[2].render(101, 79));
    let weak = Arc::downgrade(&image);
    let stages = StageCache::default();
    lightcraft_gpu::render(&image, &SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(83, 61), Some(&stages)).unwrap();
    drop(image);
    assert!(weak.upgrade().is_none(), "legacy cache identity must be weak");
}

#[test]
fn resident_windows_before_after_and_hybrid_edits_reuse_an_unchanged_original() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for resident windows");
        return;
    }
    let image = Arc::new(lightcraft_scenes::demo_library()[2].render(317, 239));
    let original = image.data.clone();
    let weak = Arc::downgrade(&image);
    let source = exact_source(SourceId::new(), &image);
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut edited = DevelopSettings::default();
    edited.effects.clarity = 23.0;
    edited.optics.defringe_purple_amount = 5.0;
    edited.spots = vec![lightcraft_develop::Spot {
        points: vec![lightcraft_geom::Point::new(0.3, 0.3)],
        size: 0.03,
        source_offset: Some(lightcraft_geom::Point::new(0.1, 0.0)),
        ..Default::default()
    }];
    let settings = [DevelopSettings::default(), edited];
    let window = |x| RenderRequest { window: Some(lightcraft_pipeline::PixelWindow { x, y: 23, w: 131, h: 97 }), ..RenderRequest::fit(317, 239) };
    let jobs = [(0, window(27)), (1, window(27)), (1, window(71)), (0, RenderRequest::fit(173, 131)), (1, RenderRequest::fit(61, 47))];
    let plans: Vec<_> = jobs.iter().map(|(index, request)| lightcraft_pipeline::plan(&image, &info, &settings[*index], request)).collect();
    let expected: Vec<_> = jobs.iter().map(|(index, request)| exact_render(&image, &info, &settings[*index], request)).collect();
    let caches = [StageCache::default(), StageCache::default()];
    let uploads = lightcraft_gpu::source_uploads();
    drop(image);
    assert!(weak.upgrade().is_none());
    for _ in 0..2 {
        for (((index, request), plan), expected) in jobs.iter().zip(&plans).zip(&expected) {
            let rendered = lightcraft_gpu::render_prepared(&source, &info, plan, request, Some(&caches[*index]), true).unwrap();
            assert_eq!(rendered.histogram, expected.histogram);
            assert_eq!(rendered.image.into_owned().unwrap().data, expected.image.data);
        }
    }
    assert_eq!(lightcraft_gpu::source_uploads(), uploads, "windows and Before/After must share the existing original upload");
    let recovered = source.read_original().unwrap();
    for (actual, expected) in recovered.data.iter().zip(&original) {
        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits), "hybrid edits must leave original bits untouched");
    }
}

#[test]
fn automatic_residents_match_legacy_across_views_and_source_changes() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for Automatic source equivalence");
        return;
    }
    let images = [Arc::new(lightcraft_scenes::demo_library()[1].render(317, 239)), Arc::new(lightcraft_scenes::demo_library()[2].render(317, 239))];
    let originals = images.each_ref().map(|image| image.data.clone());
    let weak = images.each_ref().map(Arc::downgrade);
    let sources = images.each_ref().map(|image| lightcraft_gpu::upload_source(SourceId::new(), image).unwrap());
    assert_ne!(sources[0].id(), sources[1].id());
    eprintln!("Automatic resident exact storage: {:?}", sources.each_ref().map(|source| source.can_read_exact()));
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut edited = DevelopSettings::default();
    edited.effects.clarity = 23.0;
    edited.optics.defringe_purple_amount = 5.0;
    edited.spots = vec![lightcraft_develop::Spot {
        points: vec![lightcraft_geom::Point::new(0.3, 0.3)],
        size: 0.03,
        source_offset: Some(lightcraft_geom::Point::new(0.1, 0.0)),
        ..Default::default()
    }];
    let settings = [DevelopSettings::default(), edited];
    let window = |x| RenderRequest { window: Some(lightcraft_pipeline::PixelWindow { x, y: 23, w: 131, h: 97 }), ..RenderRequest::fit(317, 239) };
    let jobs = [
        (0, 0, RenderRequest::fit(317, 239)),
        (1, 0, RenderRequest::fit(317, 239)),
        (0, 1, window(27)),
        (0, 1, window(71)),
        (1, 1, window(27)),
        (0, 0, RenderRequest::fit(173, 131)),
        (1, 1, RenderRequest::fit(61, 47)),
    ];
    let plans: Vec<_> =
        jobs.iter().map(|(source, setting, request)| lightcraft_pipeline::plan(&images[*source], &info, &settings[*setting], request)).collect();
    let expected: Vec<_> = jobs
        .iter()
        .map(|(source, setting, request)| lightcraft_gpu::render(&images[*source], &info, &settings[*setting], request, None).unwrap())
        .collect();
    let caches = [StageCache::default(), StageCache::default()];
    let uploads = lightcraft_gpu::source_uploads();
    drop(images);
    assert!(weak.iter().all(|image| image.upgrade().is_none()), "Automatic sources must not pin CPU pixels");
    for _ in 0..2 {
        for (((source, setting, request), plan), expected) in jobs.iter().zip(&plans).zip(&expected) {
            let rendered = lightcraft_gpu::render_prepared(&sources[*source], &info, plan, request, Some(&caches[*setting]), true).unwrap();
            assert_eq!(rendered.histogram, expected.histogram);
            assert_eq!(rendered.image.into_owned().unwrap().data, expected.image.data);
        }
    }
    assert_eq!(lightcraft_gpu::source_uploads(), uploads, "Automatic prepared renders must reuse their uploads");
    for (source, original) in sources.iter().zip(&originals) {
        if source.can_read_exact() {
            assert_eq!(source.read_original().unwrap().data, *original);
        } else {
            assert!(source.read_original().unwrap_err().starts_with("NeedsReload:"), "half originals must never claim exact recovery");
        }
    }
}
