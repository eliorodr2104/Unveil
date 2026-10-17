//! Mapped output keeps the ordinary renderer's pixels and histogram across thread handoff.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot};
use lightcraft_geom::Point;
use lightcraft_pipeline::{PixelWindow, RenderRequest, SourceInfo};

#[test]
fn mapped_frames_survive_worker_handoff_and_match_owned_output() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for mapped output");
        return;
    }
    eprintln!("mapped output adapter: {:?}", lightcraft_gpu::adapter_name());
    let source = Arc::new(lightcraft_scenes::demo_library()[2].render(317, 239));
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut settings = DevelopSettings::default();
    settings.light.exposure = 0.37;
    settings.effects.clarity = 12.0;
    for window in [None, Some(PixelWindow { x: 51, y: 43, w: 173, h: 111 })] {
        let mut request = RenderRequest::fit(317, 239);
        request.window = window;
        let expected = lightcraft_gpu::render(&source, &info, &settings, &request, None).expect("owned render");
        let source = source.clone();
        let settings = settings.clone();
        let info = info.clone();
        let actual = std::thread::spawn(move || lightcraft_gpu::render_mapped(&source, &info, &settings, &request, None).expect("mapped render"))
            .join()
            .expect("render worker");
        assert!(actual.image.is_mapped(), "the test must exercise mapped delivery");
        assert_eq!((actual.image.width, actual.image.height), (expected.image.width, expected.image.height));
        assert_eq!(actual.histogram, expected.histogram);
        actual
            .image
            .with_pixels(|bytes, stride| {
                for y in 0..actual.image.height {
                    assert_eq!(
                        &bytes[y * stride..y * stride + actual.image.width * 4],
                        expected.image.data[y * actual.image.width..(y + 1) * actual.image.width].as_flattened()
                    );
                }
            })
            .expect("mapped pixels remain valid on the receiving thread");
    }
}

#[test]
fn a_spot_expanded_crop_keeps_the_last_stride_readable() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter for cropped mapped output");
        return;
    }
    let source = Arc::new(lightcraft_scenes::demo_library()[2].render(317, 239));
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut settings = DevelopSettings::default();
    settings.spots.push(Spot {
        points: vec![Point::new(0.55, 0.45)],
        source_offset: Some(Point::new(-0.42, -0.12)),
        size: 0.06,
        ..Default::default()
    });
    let mut request = RenderRequest::fit(317, 239);
    request.window = Some(PixelWindow { x: 150, y: 70, w: 167, h: 169 });
    let plan = lightcraft_pipeline::plan(&source, &info, &settings, &request);
    let keep = plan.keep.expect("the source of the spot must extend the window");
    assert!(keep.x > 0);
    assert_eq!(keep.y + keep.h, plan.h, "exercise the padded final row");
    let expected = lightcraft_gpu::render(&source, &info, &settings, &request, None).expect("owned crop");
    let actual = lightcraft_gpu::render_mapped(&source, &info, &settings, &request, None).expect("mapped crop");
    assert!(actual.image.is_mapped());
    assert_eq!(actual.histogram, expected.histogram);
    actual
        .image
        .with_pixels(|bytes, stride| {
            assert_eq!(bytes.len(), actual.image.height * stride);
            assert!(stride > actual.image.width * 4);
            for y in 0..actual.image.height {
                assert_eq!(
                    &bytes[y * stride..y * stride + actual.image.width * 4],
                    expected.image.data[y * actual.image.width..(y + 1) * actual.image.width].as_flattened()
                );
            }
        })
        .expect("all final-row padding must remain readable");
}
