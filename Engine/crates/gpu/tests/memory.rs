// Modified for Unveil (2026-10-17): memory ownership and reserve policy; see Engine/CHANGES.md.
//! Device-memory bookkeeping: a view's stages are counted, released buffers are pooled within
//! the pool limit, and trimming frees them. Skips when no GPU adapter exists.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache};

struct RestorePoolLimit(u64);

impl Drop for RestorePoolLimit {
    fn drop(&mut self) {
        lightcraft_gpu::trim_pool(0);
        lightcraft_gpu::set_pool_limit(self.0);
    }
}

#[test]
fn stages_are_counted_and_the_pool_is_bounded_and_trimmed() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    let _restore = RestorePoolLimit(lightcraft_gpu::pool_limit());
    let limit = 64u64 << 20;
    lightcraft_gpu::set_pool_limit(limit);
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.effects.clarity = 30.0;
    let stages = StageCache::default();
    // photos of slightly different sizes, like stepping through a library
    for (i, (w, h)) in [(1200usize, 800usize), (1190, 810), (1210, 790), (800, 1200)].into_iter().enumerate() {
        let src = Arc::new(lightcraft_scenes::demo_library()[i].render(w, h));
        let r = lightcraft_gpu::render(&src, &info, &s, &RenderRequest::fit(w, h), Some(&stages)).expect("gpu render");
        assert_eq!((r.image.width, r.image.height), (w, h));
        let m = lightcraft_gpu::memory();
        assert!(m.pooled <= limit, "pool {} over its limit", m.pooled);
        assert_eq!(m.retired, 0, "a finished render leaves nothing retired");
    }
    let held = lightcraft_gpu::stage_bytes(&stages);
    assert!(held >= 1200 * 800 * 3 * 4, "the view's stages hold at least its resampled source: {held}");
    let before = lightcraft_gpu::memory().allocated;
    stages.clear();
    let m = lightcraft_gpu::memory();
    assert_eq!(lightcraft_gpu::stage_bytes(&stages), 0);
    assert!(m.pooled <= limit && m.allocated <= before);
    lightcraft_gpu::trim_pool(0);
    let m = lightcraft_gpu::memory();
    assert_eq!(m.pooled, 0);
    assert!(m.allocated < before, "trimming frees the pooled buffers: {} → {}", before, m.allocated);
}

#[test]
fn trimming_only_free_reserve_preserves_sources_stages_and_exact_pixels() {
    assert!(lightcraft_gpu::available(), "actual GPU required");
    let _restore = RestorePoolLimit(lightcraft_gpu::pool_limit());
    lightcraft_gpu::set_pool_limit(64 << 20);
    let image = Arc::new(lightcraft_scenes::demo_library()[2].render(4000, 2400));
    let info = SourceInfo { raw: true, ..Default::default() };
    let settings = DevelopSettings::default();
    let stages = StageCache::default();
    let request =
        RenderRequest { window: Some(lightcraft_pipeline::PixelWindow { x: 1000, y: 900, w: 640, h: 480 }), ..RenderRequest::fit(4000, 2400) };
    let expected = lightcraft_gpu::render(&image, &info, &settings, &request, Some(&stages)).unwrap();
    let uploads = lightcraft_gpu::source_uploads();
    let original = lightcraft_gpu::shared_source_bytes();
    let held = lightcraft_gpu::stage_bytes(&stages);
    assert!(original > 0 && held > 0);
    for cap in [0, 8 << 20, 1024, 0, 64 << 20] {
        lightcraft_gpu::set_pool_limit(cap);
        lightcraft_gpu::trim_reusable(0);
        let memory = lightcraft_gpu::memory();
        assert_eq!(memory.pooled, 0);
        assert_eq!(memory.retired, 0);
        assert!(memory.allocated >= original as u64);
        assert_eq!(lightcraft_gpu::shared_source_bytes(), original);
        assert_eq!(lightcraft_gpu::stage_bytes(&stages), held);
        let result = lightcraft_gpu::render(&image, &info, &settings, &request, Some(&stages)).unwrap();
        assert_eq!(result.image, expected.image);
        assert_eq!(lightcraft_gpu::source_uploads(), uploads);
        assert!(lightcraft_gpu::memory().pooled <= cap);
    }
    drop(stages);
    lightcraft_gpu::trim_pool(0);
}
