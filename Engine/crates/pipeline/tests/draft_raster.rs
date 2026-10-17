use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::{Orientation, Point, Rect};
use lightcraft_pipeline::{DeepSamples, OutputDepth, PixelWindow, RasterScale, RenderRequest, SourceInfo, frame_for, plan, render};
use lightcraft_raster::Rgb32f;

fn assert_mapping(settings: &DevelopSettings, request: RenderRequest) {
    let src = Rgb32f::from_fn(513, 341, |_, _| [0.2; 3]);
    let info = SourceInfo::default();
    let p = plan(&src, &info, settings, &request);
    let full = frame_for(&src, &info, &p.settings, request.apply_crop);
    let (fw, fh) = full.fit(request.max_w, request.max_h);
    let win = request.window.unwrap_or(PixelWindow { x: 0, y: 0, w: fw, h: fh }).clamped(fw, fh);
    assert_eq!(p.display_size, (win.w, win.h));
    let keep = p.keep.unwrap_or(PixelWindow { x: 0, y: 0, w: p.w, h: p.h });
    assert_eq!((keep.w, keep.h), (win.w, win.h), "display reduction keeps native prepared samples");
    let actual = p.frame.out_to_norm(p.w, p.h);
    let expected = full.out_to_norm(fw, fh);
    for (x, y) in [(0.0, 0.0), (0.5, 0.5), (keep.w as f64 - 0.5, keep.h as f64 - 0.5), (keep.w as f64, keep.h as f64)] {
        let a = actual.apply(Point::new(keep.x as f64 + x, keep.y as f64 + y));
        let b = expected.apply(Point::new(win.x as f64 + x * win.w as f64 / keep.w as f64, win.y as f64 + y * win.h as f64 / keep.h as f64));
        assert!((a.x - b.x).abs() < 1e-10 && (a.y - b.y).abs() < 1e-10, "mapping changed: {a:?} vs {b:?}");
    }
    assert!(keep.x + keep.w <= p.w && keep.y + keep.h <= p.h);
}

#[test]
fn half_delivery_keeps_native_fit_and_odd_window_coverage() {
    let settings = DevelopSettings::default();
    let request = RenderRequest { raster_scale: RasterScale::Half, ..RenderRequest::fit(2051, 1367) };
    assert_mapping(&settings, request);
    for win in [
        PixelWindow { x: 51, y: 43, w: 173, h: 111 },
        PixelWindow { x: 2048, y: 1360, w: 99, h: 99 },
        PixelWindow { x: 0, y: 0, w: 1, h: 1 },
        PixelWindow { x: usize::MAX, y: usize::MAX, w: usize::MAX, h: usize::MAX },
    ] {
        assert_mapping(&settings, RenderRequest { window: Some(win), ..request });
    }
}

#[test]
fn crop_rotation_and_flips_preserve_logical_coverage() {
    let mut settings = DevelopSettings::default();
    settings.crop.geometry.rect = Rect { x0: 0.1, y0: 0.15, x1: 0.85, y1: 0.9 };
    settings.crop.geometry.angle = 4.0;
    for orientation in [Orientation::Rotate90, Orientation::Rotate270, Orientation::FlipH, Orientation::Transpose] {
        settings.orientation = orientation;
        for (h, v) in [(false, false), (true, false), (false, true), (true, true)] {
            settings.crop.flip_h = h;
            settings.crop.flip_v = v;
            assert_mapping(
                &settings,
                RenderRequest {
                    raster_scale: RasterScale::Half,
                    window: Some(PixelWindow { x: 51, y: 43, w: 173, h: 111 }),
                    ..RenderRequest::fit(2051, 1367)
                },
            );
        }
    }
}

#[test]
fn native_raster_remains_identity() {
    assert_mapping(&DevelopSettings::default(), RenderRequest::fit(2051, 1367));
    assert_mapping(
        &DevelopSettings::default(),
        RenderRequest { window: Some(PixelWindow { x: 51, y: 43, w: 173, h: 111 }), ..RenderRequest::fit(2051, 1367) },
    );
}

#[test]
fn display_reduction_does_not_change_high_depth_exports() {
    let source = Rgb32f::from_fn(47, 31, |x, y| [x as f32 / 46.0, y as f32 / 30.0, (x + y) as f32 / 76.0]);
    let info = SourceInfo::default();
    let mut settings = DevelopSettings::default();
    settings.crop.geometry.rect = Rect { x0: 0.1, y0: 0.15, x1: 0.85, y1: 0.9 };
    settings.crop.geometry.angle = 4.0;
    settings.crop.flip_h = true;
    for depth in [OutputDepth::U16, OutputDepth::F32Linear] {
        for window in [None, Some(PixelWindow { x: 3, y: 5, w: 17, h: 11 })] {
            let request = RenderRequest { depth, window, ..RenderRequest::fit(41, 29) };
            let native = render(&source, &info, &settings, &request);
            let half = render(&source, &info, &settings, &RenderRequest { raster_scale: RasterScale::Half, ..request });
            assert_eq!(half.display_size, native.display_size);
            assert_eq!((half.image.width, half.image.height), (native.image.width, native.image.height));
            assert_eq!(half.image.data, native.image.data);
            let (native, half) = (native.deep.unwrap(), half.deep.unwrap());
            assert_eq!((half.width, half.height), (native.width, native.height));
            match (native.samples, half.samples) {
                (DeepSamples::U16(native), DeepSamples::U16(half)) => assert_eq!(half, native),
                (DeepSamples::F32(native), DeepSamples::F32(half)) => {
                    let bits = |samples: Vec<f32>| samples.into_iter().map(f32::to_bits).collect::<Vec<_>>();
                    assert_eq!(bits(half), bits(native));
                }
                _ => panic!("display reduction changed the sample depth"),
            }
        }
    }
}

#[test]
fn spot_halo_keeps_exact_requested_cells() {
    let mut settings = DevelopSettings::default();
    settings.spots.push(Spot {
        mode: SpotMode::Clone,
        points: vec![Point::new(0.5, 0.5)],
        size: 0.06,
        feather: 30.0,
        opacity: 100.0,
        source_offset: Some(Point::new(-0.3, 0.0)),
    });
    assert_mapping(
        &settings,
        RenderRequest {
            raster_scale: RasterScale::Half,
            window: Some(PixelWindow { x: 951, y: 603, w: 173, h: 111 }),
            ..RenderRequest::fit(2051, 1367)
        },
    );
}

#[test]
fn a_halo_crossing_the_frame_edge_uses_native_samples() {
    let source = Rgb32f::from_fn(513, 341, |_, _| [0.2; 3]);
    let info = SourceInfo::default();
    let mut settings = DevelopSettings::default();
    settings.spots.push(Spot {
        mode: SpotMode::Clone,
        points: vec![Point::new(0.06, 0.1)],
        size: 0.08,
        feather: 30.0,
        opacity: 100.0,
        source_offset: Some(Point::new(-0.15, 0.0)),
    });
    let request = RenderRequest { window: Some(PixelWindow { x: 51, y: 43, w: 173, h: 111 }), ..RenderRequest::fit(2051, 1367) };
    let full = plan(&source, &info, &settings, &request);
    let reduced = plan(&source, &info, &settings, &RenderRequest { raster_scale: RasterScale::Half, ..request });
    assert!(full.keep.is_some(), "the fixture must need an expanded halo");
    assert_eq!((reduced.w, reduced.h, reduced.keep), (full.w, full.h, full.keep));
    assert_eq!(format!("{:?}", reduced.frame), format!("{:?}", full.frame));
}

#[test]
fn auto_mask_without_spots_keeps_the_same_edge_policy() {
    use lightcraft_develop::{BrushStroke, Mask, MaskComponent, MaskOp, MaskShape};
    let source = Rgb32f::from_fn(513, 341, |_, _| [0.2; 3]);
    let info = SourceInfo::default();
    let settings = DevelopSettings {
        masks: vec![Mask {
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Brush {
                    strokes: vec![BrushStroke { points: vec![Point::new(0.06, 0.1)], size: 0.2, auto_mask: true, ..Default::default() }],
                },
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let request = RenderRequest { window: Some(PixelWindow { x: 51, y: 43, w: 173, h: 111 }), ..RenderRequest::fit(2051, 1367) };
    let full = plan(&source, &info, &settings, &request);
    let reduced = plan(&source, &info, &settings, &RenderRequest { raster_scale: RasterScale::Half, ..request });
    assert!(full.keep.is_some(), "the fixture must need an expanded Auto Mask halo");
    assert_eq!((reduced.w, reduced.h, reduced.keep), (full.w, full.h, full.keep));
    assert_eq!(format!("{:?}", reduced.frame), format!("{:?}", full.frame));
}
