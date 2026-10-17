use lightcraft_develop::{DevelopSettings, RedEye, Spot, Upright, WbMode};
use lightcraft_geom::{Point, Rect};
use lightcraft_pipeline::{PixelWindow, PlanningFacts, RenderRequest, SourceExtent, SourceId, SourceInfo, plan, plan_with_facts};
use lightcraft_raster::Rgb32f;

fn source() -> Rgb32f {
    lightcraft_scenes::demo_library()[2].render(321, 209)
}

fn assert_plan(source: &Rgb32f, info: &SourceInfo, settings: &DevelopSettings, request: &RenderRequest, facts: &mut PlanningFacts) {
    let reference = plan(source, info, settings, request);
    let actual = plan_with_facts(info, settings, request, facts, None).expect("all required facts are resident");
    assert_eq!(&*reference.settings, &*actual.settings);
    assert_eq!(format!("{:?}", reference.frame), format!("{:?}", actual.frame));
    assert_eq!((reference.w, reference.h, reference.keep), (actual.w, actual.h, actual.keep));
    assert_eq!((reference.geo, reference.lin_key), (actual.geo, actual.lin_key));
    assert_eq!(reference.eyes, actual.eyes);
    assert_eq!(reference.fixed_air, actual.fixed_air);
    assert_eq!(reference.px_per_long, actual.px_per_long);
    assert!(facts.bytes() <= 8 << 20);
}

#[test]
fn neutral_and_exposure_plans_need_no_source_pixels() {
    let source = source();
    let info = SourceInfo::default();
    let mut facts = PlanningFacts::new(SourceId::new(), SourceExtent::from(&source));
    let mut settings = DevelopSettings::default();
    for exposure in [-2.0, 0.0, 1.3] {
        settings.light.exposure = exposure;
        assert_plan(&source, &info, &settings, &RenderRequest::fit(640, 480), &mut facts);
    }
    assert_eq!(facts.bytes(), 0);
}

#[test]
fn exact_pixel_facts_survive_source_release_and_parameter_changes() {
    let source = source();
    let info = SourceInfo { raw: true, ..Default::default() };
    let request = RenderRequest { window: Some(PixelWindow { x: 41, y: 23, w: 173, h: 111 }), ..RenderRequest::fit(1025, 683) };
    let mut settings = DevelopSettings::default();
    settings.optics.remove_ca = true;
    settings.geometry.upright = Upright::Auto;
    settings.effects.dehaze = 20.0;
    settings.spots.push(Spot { points: vec![Point::new(0.5, 0.5)], size: 0.03, ..Default::default() });
    settings.red_eye.push(RedEye { center: Point::new(0.5, 0.5), rx: 0.04, ry: 0.03, ..Default::default() });
    let mut facts = PlanningFacts::new(SourceId::new(), SourceExtent::from(&source));
    assert!(plan_with_facts(&info, &settings, &request, &mut facts, None).is_err());
    plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
    assert_plan(&source, &info, &settings, &request, &mut facts);
    for exposure in [-0.7, 0.2, 1.0] {
        settings.light.exposure = exposure;
        settings.wb.mode = WbMode::Custom;
        settings.wb.temp = 4900.0 + exposure * 1000.0;
        settings.effects.dehaze = 35.0 + exposure;
        settings.red_eye[0].darken = 60.0 + exposure;
        settings.red_eye[0].pupil_size = 20.0 + exposure;
        assert_plan(&source, &info, &settings, &request, &mut facts);
    }
    settings.geometry.upright = Upright::Level;
    plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
    assert_plan(&source, &info, &settings, &request, &mut facts);
    facts.clear();
    assert_eq!(facts.bytes(), 0);
    assert!(plan_with_facts(&info, &settings, &request, &mut facts, None).is_err());
}

#[test]
fn crop_changes_rebuild_only_the_bounded_frame_proxy() {
    let source = source();
    let info = SourceInfo::default();
    let mut settings = DevelopSettings::default();
    settings.effects.dehaze = 10.0;
    let request = RenderRequest { window: Some(PixelWindow { x: 41, y: 23, w: 173, h: 111 }), ..RenderRequest::fit(1025, 683) };
    let mut facts = PlanningFacts::new(SourceId::new(), SourceExtent::from(&source));
    for step in 0..12 {
        settings.crop.geometry.rect = Rect { x0: step as f64 * 0.01, y0: 0.05, x1: 0.95, y1: 0.9 };
        assert!(plan_with_facts(&info, &settings, &request, &mut facts, None).is_err());
        plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
        assert_plan(&source, &info, &settings, &request, &mut facts);
    }
}

#[test]
fn pixel_dependencies_invalidate_and_recover_without_retaining_the_source() {
    use lightcraft_develop::{EmbeddedLens, EmbeddedWarp};
    use lightcraft_pipeline::NeedsPixels;
    use std::sync::Arc;
    let source = Arc::new(source());
    let weak = Arc::downgrade(&source);
    let mut info = SourceInfo::default();
    let request = RenderRequest::fit(640, 480);
    let mut settings = DevelopSettings::default();
    let mut facts = PlanningFacts::new(SourceId::new(), SourceExtent::from(&*source));
    settings.optics.remove_ca = true;
    assert!(matches!(plan_with_facts(&info, &settings, &request, &mut facts, None), Err(NeedsPixels::LateralCa)));
    plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
    assert_plan(&source, &info, &settings, &request, &mut facts);
    settings.geometry.upright = Upright::Auto;
    settings.optics.distortion = 35.0;
    for enabled in [true, false, true] {
        settings.set_section_enabled("optics", enabled);
        plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
        assert_plan(&source, &info, &settings, &request, &mut facts);
    }
    settings.optics.lens_profile = true;
    for coefficient in [0.1, -0.1] {
        info.lens = Some(EmbeddedLens {
            warp: Some(EmbeddedWarp { planes: [[1.0, coefficient, 0.0, 0.0, 0.0, 0.0]; 3], center: Point::new(0.5, 0.5), radius: 0.5 }),
            ..Default::default()
        });
        assert!(matches!(plan_with_facts(&info, &settings, &request, &mut facts, None), Err(NeedsPixels::Upright)));
        plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
        assert_plan(&source, &info, &settings, &request, &mut facts);
    }
    settings.red_eye.push(RedEye { center: Point::new(0.5, 0.5), rx: 0.04, ry: 0.03, ..Default::default() });
    for (x, rx, pet) in [(0.5, 0.04, false), (0.6, 0.04, false), (0.6, 0.06, false), (0.6, 0.06, true)] {
        settings.red_eye[0].center.x = x;
        settings.red_eye[0].rx = rx;
        settings.red_eye[0].pet = pet;
        assert!(matches!(plan_with_facts(&info, &settings, &request, &mut facts, None), Err(NeedsPixels::RedEye)));
        plan_with_facts(&info, &settings, &request, &mut facts, Some(&source)).unwrap();
        assert_plan(&source, &info, &settings, &request, &mut facts);
    }
    drop(source);
    assert!(weak.upgrade().is_none(), "planning facts must not keep the original alive");
    assert!(plan_with_facts(&info, &settings, &request, &mut facts, None).is_ok());
}

#[test]
fn a_different_extent_requires_a_new_source_identity() {
    let source = source();
    let mut facts = PlanningFacts::new(SourceId::new(), SourceExtent { width: 1, height: 1 });
    assert!(matches!(
        plan_with_facts(&SourceInfo::default(), &DevelopSettings::default(), &RenderRequest::fit(640, 480), &mut facts, Some(&source)),
        Err(lightcraft_pipeline::NeedsPixels::SourceExtent)
    ));
}
