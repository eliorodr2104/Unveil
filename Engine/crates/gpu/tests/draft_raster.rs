//! Reduced display views share one resident source and bound retained native stage sizes.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_gpu::{GpuStages, render_prepared, source_uploads, upload_source};
use lightcraft_pipeline::{PlanningFacts, RasterScale, RenderRequest, SourceExtent, SourceId, SourceInfo, StageCache, plan_with_facts};

#[test]
fn reduced_views_reuse_one_original_and_two_native_stage_sizes() {
    assert!(lightcraft_gpu::available(), "this acceptance test requires a GPU adapter");
    let host = Arc::new(lightcraft_scenes::demo_library()[2].render(513, 341));
    let weak = Arc::downgrade(&host);
    let id = SourceId::new();
    let before = source_uploads();
    let source = upload_source(id, &host).expect("source upload");
    let mut facts = PlanningFacts::new(id, SourceExtent::from(&*host));
    drop(host);
    assert!(weak.upgrade().is_none());
    let info = SourceInfo::default();
    let settings = DevelopSettings::default();
    let stages = StageCache::default();
    for (index, (w, h)) in [(513, 341), (401, 257), (1025, 683)].into_iter().enumerate() {
        let request = RenderRequest { raster_scale: RasterScale::Half, ..RenderRequest::fit(w, h) };
        let plan = plan_with_facts(&info, &settings, &request, &mut facts, None).expect("neutral plan needs no original");
        let rendered = render_prepared(&source, &info, &plan, &request, Some(&stages), true).expect("resident draft");
        assert_eq!(rendered.display_size, plan.display_size);
        assert_eq!((rendered.image.width, rendered.image.height), (plan.display_size.0.div_ceil(2), plan.display_size.1.div_ceil(2)));
        assert_eq!(stages.peek_extension::<GpuStages>().unwrap().len(), (index + 1).min(2));
        let bytes = lightcraft_gpu::stage_bytes(&stages);
        let native = RenderRequest { raster_scale: RasterScale::Native, ..request };
        let full = render_prepared(&source, &info, &plan, &native, Some(&stages), true).expect("native Full");
        assert_eq!((full.image.width, full.image.height), plan.display_size);
        assert_eq!(lightcraft_gpu::stage_bytes(&stages), bytes, "Full must reuse the same native stages");
    }
    assert_eq!(source_uploads(), before + 1);
    assert_eq!(facts.bytes(), 0);
    stages.clear();
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), 0);
    if source.can_read_exact() {
        assert_eq!(source.read_original().expect("unchanged original").data.len(), 513 * 341);
    } else {
        assert!(source.read_original().unwrap_err().starts_with("NeedsReload:"));
    }
}
