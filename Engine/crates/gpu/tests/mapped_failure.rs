//! Failed work must never be published as a mapped display image.

use lightcraft_develop::DevelopSettings;
use lightcraft_gpu::Fault;
use lightcraft_pipeline::{RenderRequest, SourceInfo};
use std::sync::Arc;

#[test]
fn mapped_output_preserves_gpu_failure_and_recovery() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for mapped failure");
        return;
    }
    let source = Arc::new(lightcraft_scenes::demo_library()[0].render(257, 193));
    let settings = DevelopSettings::default();
    let info = SourceInfo { raw: true, ..Default::default() };
    let request = RenderRequest::fit(257, 193);
    for fault in [Fault::Limit(1024), Fault::Validation, Fault::DropWork] {
        lightcraft_gpu::inject_fault(fault);
        assert!(lightcraft_gpu::render_mapped(&source, &info, &settings, &request, None).is_none());
        assert!(lightcraft_gpu::last_fallback().is_some());
        lightcraft_gpu::reset_failures();
        let frame = lightcraft_gpu::render_mapped(&source, &info, &settings, &request, None).expect("recovered mapped render");
        assert!(frame.image.is_mapped());
        frame
            .image
            .with_pixels(|pixels, stride| {
                assert_eq!(pixels.len(), frame.image.height * stride);
                assert!(pixels.chunks_exact(4).all(|p| p[3] == 255));
            })
            .unwrap();
    }
}
