//! Optional RAW acceptance: reduced drafts share logical geometry and final Full pixels.
//! Full is area-averaged once onto the actual Draft grid; Draft pixels are compared unchanged.
//! The previous 256-pixel grid applied two area filters to Draft and one to Full; its results
//! are retained in the E4 investigation report. Mean <=0.5 and p99 <=2 thresholds are unchanged.
mod common;
use lightcraft_engine::{
    Session,
    pipeline::{PixelWindow, StageCache},
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use unveil_ffi::delta_e::{ciede2000, srgb8_to_lab};

fn common_grid(data: &[[u8; 4]], w: usize, h: usize, dw: usize, dh: usize) -> Vec<[u8; 3]> {
    let mut out = Vec::with_capacity(dw * dh);
    for y in 0..dh {
        let y0 = y as f64 * h as f64 / dh as f64;
        let y1 = (y + 1) as f64 * h as f64 / dh as f64;
        for x in 0..dw {
            let x0 = x as f64 * w as f64 / dw as f64;
            let x1 = (x + 1) as f64 * w as f64 / dw as f64;
            let mut rgb = [0.0; 3];
            for sy in y0.floor() as usize..(y1.ceil() as usize).min(h) {
                let wy = (y1.min((sy + 1) as f64) - y0.max(sy as f64)).max(0.0);
                for sx in x0.floor() as usize..(x1.ceil() as usize).min(w) {
                    let weight = wy * (x1.min((sx + 1) as f64) - x0.max(sx as f64)).max(0.0);
                    for c in 0..3 {
                        rgb[c] += f64::from(data[sy * w + sx][c]) * weight;
                    }
                }
            }
            out.push(rgb.map(|v| (v / ((x1 - x0) * (y1 - y0))).round().clamp(0.0, 255.0) as u8));
        }
    }
    out
}

// An optional independent f32 control is written once, then read by a separate half process.
// These temporary fixtures never replace the project's committed goldens.
fn full_reference(
    name: &str,
    preset: &str,
    window: bool,
    width: usize,
    height: usize,
    pixels: &[[u8; 4]],
) -> Vec<[u8; 4]> {
    let Some(directory) = std::env::var_os("UNVEIL_DRAFT_REFERENCE_DIR") else {
        return pixels.to_vec();
    };
    let directory = PathBuf::from(directory);
    let path = directory.join(format!(
        "{name}-{preset}-{}.rgba",
        if window { "window" } else { "fit" }
    ));
    if std::env::var_os("UNVEIL_DRAFT_WRITE_REFERENCE").is_some() {
        assert!(
            !matches!(
                std::env::var("UNVEIL_E2_HALF")
                    .unwrap_or_else(|_| "off".into())
                    .as_str(),
                "source" | "all" | "lin"
            ),
            "control must use f32 storage"
        );
        std::fs::create_dir_all(&directory).unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        let mut file = std::io::BufWriter::new(file);
        use std::io::Write;
        file.write_all(&(width as u32).to_le_bytes()).unwrap();
        file.write_all(&(height as u32).to_le_bytes()).unwrap();
        for pixel in pixels {
            file.write_all(pixel).unwrap();
        }
        file.flush().unwrap();
        return pixels.to_vec();
    }
    let bytes = std::fs::read(path).unwrap();
    let actual_width = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let actual_height = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!((actual_width, actual_height), (width, height));
    assert_eq!(bytes.len(), 8 + width * height * 4);
    bytes[8..]
        .chunks_exact(4)
        .map(|p| p.try_into().unwrap())
        .collect()
}

#[test]
#[ignore = "requires the five documented RAW fixtures and a real Metal GPU"]
fn reduced_raw_color_and_full_return() {
    let raw_dir = std::env::var_os("UNVEIL_RAW_DIR").expect("UNVEIL_RAW_DIR is required");
    assert!(lightcraft_gpu::available(), "real GPU required");
    let mut raws: Vec<PathBuf> = std::fs::read_dir(raw_dir)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|s| {
                ["cr3", "raf", "arw", "nef", "dng"].contains(&s.to_string_lossy().to_lowercase().as_str())
            })
        })
        .collect();
    raws.sort();
    assert_eq!(raws.len(), 5);
    let mut failures = Vec::new();
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
        for (preset, edits) in [
            ("neutral", vec![]),
            ("exposure", vec![("light.exposure", 1.0)]),
            ("temperature", vec![("wb.temp", 4000.0)]),
            ("contrast", vec![("light.contrast", 60.0)]),
            (
                "tones",
                vec![("light.shadows", 50.0), ("light.highlights", -50.0)],
            ),
            (
                "spatial",
                vec![
                    ("effects.texture", 30.0),
                    ("effects.clarity", 20.0),
                    ("detail.sharpenAmount", 40.0),
                ],
            ),
        ] {
            session.execute("develop.reset", &json!({})).unwrap();
            for (control, value) in edits {
                session
                    .execute("develop.set", &json!({"control":control,"value":value}))
                    .unwrap();
            }
            for window in [false, true] {
                let full_stages = Arc::new(StageCache::default());
                let job = |s: &mut Session| {
                    if window {
                        s.region_job(
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
                        s.render_job(id, 2048, 2048, false, true).unwrap()
                    }
                };
                let full = job(&mut session).with_stages(full_stages.clone()).run();
                session.accept(&full);
                let full = full.rendered.unwrap();
                let draft = job(&mut session)
                    .with_stages(full_stages.clone())
                    .reduced_draft()
                    .run();
                session.accept(&draft);
                let draft = draft.rendered.unwrap();
                assert_eq!(full.display_size, draft.display_size);
                assert_eq!(draft.image.width, full.image.width.div_ceil(2));
                assert_eq!(draft.image.height, full.image.height.div_ceil(2));
                {
                    let cpu = lightcraft_engine::pipeline::reduce_display_half(&full.image);
                    let max = cpu
                        .data
                        .iter()
                        .zip(&draft.image.data)
                        .flat_map(|(a, b)| (0..4).map(move |c| a[c].abs_diff(b[c])))
                        .max()
                        .unwrap();
                    assert!(
                        max <= 1,
                        "GPU post-color area reduction differs from exact CPU by {max} bytes"
                    );
                }
                // Compare the same physical pixel footprint once: Full area-reduced to Draft, Draft unchanged.
                let (dw, dh) = (draft.image.width, draft.image.height);
                let reference = full_reference(
                    &raw.file_name().unwrap().to_string_lossy(),
                    preset,
                    window,
                    full.image.width,
                    full.image.height,
                    &full.image.data,
                );
                let a = common_grid(&reference, full.image.width, full.image.height, dw, dh);
                let b = common_grid(&draft.image.data, draft.image.width, draft.image.height, dw, dh);
                if let Some(output) = std::env::var_os("UNVEIL_DRAFT_OUTPUT") {
                    if raw.extension().is_some_and(|e| e == "ARW") && preset == "neutral" && window {
                        let output = PathBuf::from(output);
                        std::fs::create_dir_all(&output).unwrap();
                        for (name, image) in [("full", &full.image), ("draft", &draft.image)] {
                            let mut encoder = png::Encoder::new(
                                std::fs::File::create(output.join(format!("{name}.png"))).unwrap(),
                                image.width as u32,
                                image.height as u32,
                            );
                            encoder.set_color(png::ColorType::Rgba);
                            encoder.set_depth(png::BitDepth::Eight);
                            let bytes: Vec<u8> = image.data.iter().flatten().copied().collect();
                            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
                        }
                    }
                }
                let mut delta: Vec<f64> = a
                    .into_iter()
                    .zip(b)
                    .map(|(a, b)| ciede2000(srgb8_to_lab(a), srgb8_to_lab(b)))
                    .collect();
                delta.sort_by(f64::total_cmp);
                let mean = delta.iter().sum::<f64>() / delta.len() as f64;
                let p99 = delta[(delta.len() - 1) * 99 / 100];
                let label = format!(
                    "{} {preset} window={window}",
                    raw.file_name().unwrap().to_string_lossy()
                );
                eprintln!(
                    "DRAFT-QUALITY {label} grid={dw}x{dh} mean={mean:.6} p99={p99:.6} max={:.6}",
                    delta.last().unwrap()
                );
                if mean > 0.5 || p99 > 2.0 {
                    failures.push(format!("{label}: mean={mean} p99={p99}"));
                }
                let after = job(&mut session).with_stages(full_stages).run();
                session.accept(&after);
                assert_eq!(
                    after.rendered.unwrap().image.data,
                    full.image.data,
                    "final Full changed: {label}"
                );
            }
        }
    }
    assert!(failures.is_empty(), "draft color gate failed: {failures:?}");
}
