//! Optional fixture gate for mapped display output: five RAWs, five presets, fit and native windows.

mod common;
use lightcraft_engine::{
    Session,
    pipeline::{PixelWindow, StageCache},
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

const PRESETS: &[(&str, &[(&str, f64)])] = &[
    ("neutral", &[]),
    ("exposure", &[("light.exposure", 1.0)]),
    ("contrast", &[("light.contrast", 60.0)]),
    ("temperature", &[("wb.temp", 4000.0)]),
    ("tones", &[("light.shadows", 50.0), ("light.highlights", -50.0)]),
];

#[test]
fn mapped_raw_fit_and_native_windows_match_owned_pixels_exactly() {
    let Some(raw_dir) = std::env::var_os("UNVEIL_RAW_DIR") else {
        eprintln!("skipped: UNVEIL_RAW_DIR is not set");
        return;
    };
    assert!(
        lightcraft_gpu::available(),
        "the fixture gate requires a real GPU"
    );
    eprintln!("mapped RAW adapter: {:?}", lightcraft_gpu::adapter_name());
    let mut raws: Vec<PathBuf> = std::fs::read_dir(raw_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|s| {
                ["cr3", "raf", "arw", "nef", "dng"].contains(&s.to_string_lossy().to_lowercase().as_str())
            })
        })
        .collect();
    raws.sort();
    assert_eq!(raws.len(), 5, "use the documented five-file fixture set");
    let mut comparisons = 0;
    for raw in raws {
        let dir = common::tempdir_lite::Dir::new();
        let mut session = Session::new().with_fs();
        session.open_library(dir.path(), false).unwrap();
        session.xmp.auto_write = false;
        let imported = session
            .execute(
                "library.import",
                &json!({"paths":[raw], "mode":"add", "onDeleted":"restore"}),
            )
            .unwrap();
        let id = imported["imported"][0].as_u64().unwrap();
        session
            .execute("library.select", &json!({"ids":[id], "active":id}))
            .unwrap();
        let id = session.active().unwrap();
        let photo = session.catalog.photo(id).unwrap();
        let native = (photo.width as usize, photo.height as usize);
        for (preset, edits) in PRESETS {
            session.execute("develop.reset", &json!({})).unwrap();
            for (control, value) in *edits {
                session
                    .execute("develop.set", &json!({"control":control,"value":value}))
                    .unwrap();
            }
            for window in [false, true] {
                let stages = Arc::new(StageCache::default());
                let make_job = |session: &mut Session| {
                    if window {
                        session
                            .region_job(
                                id,
                                native.0,
                                native.1,
                                PixelWindow {
                                    x: native.0 / 3,
                                    y: native.1 / 3,
                                    w: 901,
                                    h: 703,
                                },
                                true,
                            )
                            .unwrap()
                    } else {
                        session.render_job(id, 2048, 2048, false, true).unwrap()
                    }
                    .with_stages(stages.clone())
                };
                let expected = make_job(&mut session).run();
                session.accept(&expected);
                let expected = expected.rendered.expect("owned RAW output");
                let actual = make_job(&mut session).run_for_display(true);
                session.accept(&actual);
                let actual = actual.rendered.expect("mapped RAW output");
                assert!(
                    actual.image.is_mapped(),
                    "{} {preset} window={window}: mapped path required",
                    raw.display()
                );
                assert_eq!(actual.histogram, expected.histogram);
                let image = actual.image.into_owned().unwrap();
                assert_eq!(
                    (image.width, image.height),
                    (expected.image.width, expected.image.height)
                );
                assert_eq!(
                    image.data,
                    expected.image.data,
                    "{} {preset} window={window}",
                    raw.display()
                );
                comparisons += 1;
                eprintln!(
                    "mapped RAW exact: {} {preset} window={window} {}x{}",
                    raw.file_name().unwrap().to_string_lossy(),
                    image.width,
                    image.height
                );
            }
        }
    }
    assert_eq!(comparisons, 50);
}
