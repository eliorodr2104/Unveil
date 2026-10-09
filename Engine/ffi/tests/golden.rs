//! Writes the Mac reference renders: every RAW in UNVEIL_RAW_DIR times every preset, as RGB8 PNGs in
//! Baseline/golden/mac. The iPad exports the same set and `compare_golden` measures the distance.
mod common;
use common::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use unveil_ffi::*;

/// PRESETS is the table in Docs/Baseline/raw-set.md, copied by hand into the Swift exporter.
const PRESETS: [(&str, &[(&str, f64)]); 5] = [
    ("neutral", &[]),
    ("exposure", &[("light.exposure", 1.0)]),
    ("contrast", &[("light.contrast", 60.0)]),
    ("temperature", &[("wb.temp", 4000.0)]),
    ("tones", &[("light.shadows", 50.0), ("light.highlights", -50.0)]),
];
const RAW_EXTENSIONS: [&str; 5] = ["raf", "nef", "arw", "cr3", "dng"];
const MAX_PIXELS: u32 = 2048;

/// Frame is a full-quality frame as RGB8: generation, width, height, pixels.
type Frame = (u64, u32, u32, Vec<u8>);

/// Latest is the slot `keep` fills; `render` waits on it until a frame new enough arrives.
#[derive(Default)]
struct Latest {
    frame: Mutex<Option<Frame>>,
    cv: Condvar,
}

/// keep is the preview callback. It runs on the engine thread, ignores drafts, and copies the
/// RGBA rows (stride may exceed width * 4) into a tight RGB8 frame, replacing whatever the slot held.
extern "C" fn keep(
    ctx: *mut std::ffi::c_void,
    rgba: *const u8,
    w: u32,
    h: u32,
    stride: u32,
    g: u64,
    draft: bool,
) {
    if draft {
        return;
    }
    // SAFETY: ctx is the Latest the test keeps alive in an Arc for the whole session.
    let latest = unsafe { &*(ctx as *const Latest) };
    // SAFETY: the engine promises stride * h readable bytes for the duration of the callback.
    let bytes = unsafe { std::slice::from_raw_parts(rgba, (stride * h) as usize) };
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in bytes.chunks_exact(stride as usize) {
        rgb.extend(
            row[..(w * 4) as usize]
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]]),
        );
    }
    *latest.frame.lock().unwrap() = Some((g, w, h, rgb));
    latest.cv.notify_all();
}

/// photo_id is the id `EngineManager.openPhoto` would take: imported, else restored, else the duplicate.
fn photo_id(report: &serde_json::Value) -> Result<u64, String> {
    let first = |k: &str| {
        report[k]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_u64())
    };
    first("imported")
        .or_else(|| first("restored"))
        .or_else(|| report["duplicates"][0]["existing"].as_u64())
        .ok_or_else(|| format!("not imported: {}", report["failed"]))
}

/// render asks for one full-quality preview of the active photo and returns it as RGB8.
///
/// The slot is cleared first and the wait ends on a frame whose generation is at least the one
/// `uv_request_preview` returned, so a late frame from an earlier request is never taken. It gives
/// up after 300 s (the 45 MP RAWs need seconds); the error says whether the request or the wait failed.
fn render(s: &TestSession, latest: &Latest) -> Result<(u32, u32, Vec<u8>), String> {
    *latest.frame.lock().unwrap() = None;
    let ctx = latest as *const Latest as *mut std::ffi::c_void;
    // SAFETY: s.raw is live; latest outlives the session because the caller declares it first.
    let g = unsafe { uv_request_preview(s.raw, MAX_PIXELS, false, Some(keep), ctx) };
    if g == 0 {
        return Err(format!("uv_request_preview: {}", last_error()));
    }
    let guard = latest.frame.lock().unwrap();
    let (guard, _) = latest
        .cv
        .wait_timeout_while(guard, Duration::from_secs(300), |f| {
            f.as_ref().is_none_or(|f| f.0 < g)
        })
        .unwrap();
    guard
        .clone()
        .map(|(_, w, h, rgb)| (w, h, rgb))
        .ok_or_else(|| "timed out waiting for a frame".to_string())
}

/// write_png stores tightly packed RGB8 pixels as an 8-bit RGB PNG, with no alpha channel.
fn write_png(path: &Path, w: u32, h: u32, rgb: &[u8]) {
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}

/// golden_mac_renders writes the Mac reference set: each RAW in UNVEIL_RAW_DIR through each preset.
///
/// Without the variable it prints a skip note and passes. A RAW that fails to import or render is
/// reported by name and the test goes on, then fails at the end with the full list: that is a
/// baseline fact, not something to patch around.
#[test]
fn golden_mac_renders() {
    let Some(raw_dir) = std::env::var_os("UNVEIL_RAW_DIR") else {
        println!("skipped: UNVEIL_RAW_DIR not set");
        return;
    };
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Baseline/golden/mac");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&raw_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            RAW_EXTENSIONS.contains(&ext.as_str())
        })
        .collect();
    raws.sort();

    let latest = Arc::new(Latest::default());
    let mut failures = Vec::new();
    let mut written = 0;
    for (n, raw) in raws.iter().enumerate() {
        let stem = raw.file_stem().unwrap().to_string_lossy().into_owned();
        let s = TestSession::new();
        let imported = exec_ok(
            &s,
            "library.import",
            &serde_json::json!({"paths": [raw], "mode": "add", "onDeleted": "restore"}).to_string(),
        );
        let id = match photo_id(&imported) {
            Ok(id) => id,
            Err(e) => {
                println!("FAILED {stem}: {e}");
                failures.push(format!("{stem}: {e}"));
                continue;
            }
        };
        exec_ok(
            &s,
            "library.select",
            &format!(r#"{{"ids":[{id}],"active":{id}}}"#),
        );
        if n == 0 {
            println!("app.gpu {}", exec_ok(&s, "app.gpu", "{}"));
        }
        for (preset, sets) in PRESETS {
            exec_ok(&s, "develop.reset", "{}");
            for (control, value) in sets {
                exec_ok(
                    &s,
                    "develop.set",
                    &format!(r#"{{"control":"{control}","value":{value}}}"#),
                );
            }
            match render(&s, &latest) {
                Ok((w, h, rgb)) => {
                    write_png(&out_dir.join(format!("{stem}__{preset}.png")), w, h, &rgb);
                    written += 1;
                    println!("wrote {stem}__{preset}.png {w}x{h}");
                }
                Err(e) => {
                    println!("FAILED {stem}__{preset}: {e}");
                    failures.push(format!("{stem}__{preset}: {e}"));
                }
            }
        }
    }
    println!("{written} PNGs in {}", out_dir.display());
    assert!(failures.is_empty(), "golden failures: {failures:#?}");
}
