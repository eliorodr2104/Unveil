//! uv_request_window: a window of a zoomed frame, placed over the whole-frame preview.

mod common;
use common::*;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use unveil_ffi::delta_e::{ciede2000, srgb8_to_lab};
use unveil_ffi::*;

/// Shot is one delivered frame: its UVFrame fields and its pixels as tight RGBA rows.
#[derive(Clone, Debug)]
struct Shot {
    generation: u64,
    window: bool,
    fallback: bool,
    width: u32,
    height: u32,
    display: (u32, u32),
    frame_width: u32,
    frame_height: u32,
    x: u32,
    y: u32,
    native: (u32, u32),
    rgba: Vec<u8>,
    histogram: Option<Histogram>,
}

/// Histogram is what a shot keeps of its UVHistogram: the sampled total, each channel's bins
/// summed, its edge bins, the clipped counts and the mean luminance bin.
#[derive(Clone, Debug)]
struct Histogram {
    total: u32,
    mean_luma: f64,
    sums: [u32; 4],
    edges: [[u32; 3]; 2],
    clipped: [[u32; 3]; 2],
}

#[derive(Default)]
struct Shots {
    list: Mutex<Vec<Shot>>,
    cv: Condvar,
}

extern "C" fn keep_shot(ctx: *mut std::ffi::c_void, frame: *const UVFrame) {
    // SAFETY: the engine passes a frame valid for the duration of the callback.
    let f = unsafe { &*frame };
    // SAFETY: the frame's pixels are stride * height readable bytes during the callback.
    let bytes = unsafe { std::slice::from_raw_parts(f.rgba, (f.stride * f.height) as usize) };
    let rgba = bytes
        .chunks_exact(f.stride as usize)
        .flat_map(|row| row[..(f.width * 4) as usize].iter().copied())
        .collect();
    let shot = Shot {
        generation: f.generation,
        window: f.window,
        fallback: f.fallback,
        width: f.width,
        height: f.height,
        display: (f.display_width, f.display_height),
        frame_width: f.frame_width,
        frame_height: f.frame_height,
        x: f.x,
        y: f.y,
        native: (f.native_width, f.native_height),
        rgba,
        // SAFETY: a non-NULL histogram is valid during the callback, like the frame.
        histogram: unsafe { f.histogram.as_ref() }.map(|h| Histogram {
            total: h.total,
            mean_luma: h
                .luma
                .iter()
                .enumerate()
                .map(|(i, &n)| i as f64 * n as f64)
                .sum::<f64>()
                / h.total.max(1) as f64,
            sums: [&h.r, &h.g, &h.b, &h.luma].map(|bins| bins.iter().sum()),
            edges: [[h.r[0], h.g[0], h.b[0]], [h.r[255], h.g[255], h.b[255]]],
            clipped: [h.clipped_low, h.clipped_high],
        }),
    };
    // SAFETY: ctx is the Shots the test keeps alive in an Arc for the whole session.
    let shots = unsafe { &*(ctx as *const Shots) };
    shots.list.lock().unwrap().push(shot);
    shots.cv.notify_all();
}

fn preview(s: &TestSession, shots: &Arc<Shots>, max: u32, draft: bool) -> u64 {
    let ctx = Arc::as_ptr(shots) as *mut _;
    // SAFETY: s.raw is live; every test declares shots before s, so it outlives the session.
    let g = unsafe { uv_request_preview(s.raw, max, draft, false, Some(keep_shot), ctx) };
    assert!(g > 0, "{}", last_error());
    g
}

fn window(s: &TestSession, shots: &Arc<Shots>, full: (u32, u32), win: [u32; 4], draft: bool) -> u64 {
    window_with(s, shots, full, win, draft, false)
}

/// window_with is `window` that may also ask for the whole photo's histogram.
fn window_with(
    s: &TestSession,
    shots: &Arc<Shots>,
    full: (u32, u32),
    win: [u32; 4],
    draft: bool,
    histogram: bool,
) -> u64 {
    let ctx = Arc::as_ptr(shots) as *mut _;
    let [x, y, w, h] = win;
    // SAFETY: s.raw is live; every test declares shots before s, so it outlives the session.
    let g = unsafe {
        uv_request_window(
            s.raw,
            full.0,
            full.1,
            x,
            y,
            w,
            h,
            draft,
            false,
            histogram,
            Some(keep_shot),
            ctx,
        )
    };
    assert!(g > 0, "{}", last_error());
    g
}

/// shot_at waits up to 30 s for the frame of generation `g`.
fn shot_at(shots: &Shots, g: u64) -> Shot {
    let list = shots.list.lock().unwrap();
    let (list, _) = shots
        .cv
        .wait_timeout_while(list, Duration::from_secs(30), |l| {
            l.iter().all(|f| f.generation < g)
        })
        .unwrap();
    let shot = list.iter().find(|f| f.generation == g);
    shot.cloned()
        .unwrap_or_else(|| panic!("no frame {g}: {:?}", generations(&list)))
}

fn generations(list: &[Shot]) -> Vec<(u64, bool)> {
    list.iter().map(|f| (f.generation, f.window)).collect()
}

/// crop cuts the (x, y, w, h) window out of a tight RGBA frame `width` pixels wide.
fn crop(rgba: &[u8], width: u32, [x, y, w, h]: [u32; 4]) -> Vec<u8> {
    (y..y + h)
        .flat_map(|row| {
            let start = ((row * width + x) * 4) as usize;
            rgba[start..start + (w * 4) as usize].iter().copied()
        })
        .collect()
}

/// delta_e_stats is the mean, 99th percentile and maximum CIEDE2000 difference between two RGBA
/// buffers of the same size.
fn delta_e_stats(a: &[u8], b: &[u8]) -> (f64, f64, f64) {
    let mut d: Vec<f64> = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .map(|(p, q)| ciede2000(srgb8_to_lab([p[0], p[1], p[2]]), srgb8_to_lab([q[0], q[1], q[2]])))
        .collect();
    d.sort_by(f64::total_cmp);
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    (mean, d[(d.len() - 1) * 99 / 100], d[d.len() - 1])
}

#[test]
fn a_window_is_the_same_pixels_as_the_whole_frame_rendered_at_that_size() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "zoom.png", 640, 427));
    exec_ok(&s, "develop.set", r#"{"control":"light.exposure","value":0.6}"#);
    exec_ok(&s, "develop.set", r#"{"control":"light.contrast","value":30}"#);

    // The photo shown at 200 %: the whole-frame preview at that size is the reference.
    let whole = shot_at(&shots, preview(&s, &shots, 1280, false));
    assert!(!whole.window && (whole.frame_width, whole.frame_height) == (whole.width, whole.height));
    let win = [300, 200, 256, 192];
    let part = shot_at(&shots, window(&s, &shots, (1280, 1280), win, false));

    assert!(part.window);
    assert_eq!((part.frame_width, part.frame_height), (whole.width, whole.height));
    assert_eq!((part.x, part.y, part.width, part.height), (300, 200, 256, 192));
    let reference = crop(&whole.rgba, whole.width, win);
    assert!(
        part.rgba == reference,
        "ΔE {:?}",
        delta_e_stats(&part.rgba, &reference)
    );
}

#[test]
fn a_window_past_the_edge_is_clamped_into_the_frame() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "edge.png", 640, 427));
    let part = shot_at(
        &shots,
        window(&s, &shots, (1280, 854), [1200, 800, 256, 192], false),
    );
    assert!(part.window);
    assert_eq!((part.frame_width, part.frame_height), (1280, 854));
    assert_eq!((part.x, part.y), (1200, 800));
    assert_eq!(
        (part.width, part.height),
        (80, 54),
        "the window ends at the frame's edge"
    );
}

#[test]
fn a_preview_replaces_an_older_window_but_a_window_waits_for_the_preview() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "slow.png", 3000, 2000));
    // The first render of each burst is still in flight when the rest arrive (a 3000 px decode).
    let first = preview(&s, &shots, 1024, false);
    window(&s, &shots, (3000, 2000), [1000, 600, 1024, 768], false);
    let last = preview(&s, &shots, 1024, false);
    shot_at(&shots, last);
    std::thread::sleep(Duration::from_millis(500));
    let got = generations(&shots.list.lock().unwrap());
    assert_eq!(
        got,
        vec![(first, false), (last, false)],
        "the window was not replaced"
    );

    shots.list.lock().unwrap().clear();
    exec_ok(&s, "develop.set", r#"{"control":"light.exposure","value":0.3}"#);
    let first = window(&s, &shots, (3000, 2000), [200, 200, 1024, 768], false);
    let fit = preview(&s, &shots, 1024, false);
    window(&s, &shots, (3000, 2000), [400, 200, 1024, 768], true);
    let last = window(&s, &shots, (3000, 2000), [600, 200, 1024, 768], false);
    shot_at(&shots, last);
    std::thread::sleep(Duration::from_millis(500));
    let got = generations(&shots.list.lock().unwrap());
    assert_eq!(got, vec![(first, true), (fit, false), (last, true)]);
}

#[test]
fn a_window_that_cannot_render_alone_falls_back_to_the_whole_frame() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "spot.png", 640, 427));
    // A spot at (0.2, 0.5) that copies from 0.6 of the width away: at 32x its source is 12288 px
    // from a window around it, farther than one render may read (8192 px).
    let spot = r#"{"settings":{"spots":[{"points":[{"x":0.2,"y":0.5}],"size":0.01,"source_offset":{"x":0.6,"y":0.0}}]}}"#;
    exec_ok(&s, "develop.merge", spot);
    let (full_w, full_h) = (640 * 32, 427 * 32);
    let win = [full_w / 5 - 100, full_h / 2 - 100, 400, 300];
    let shot = shot_at(&shots, window(&s, &shots, (full_w, full_h), win, false));
    assert!(!shot.window, "the window was rendered alone");
    assert!(shot.fallback, "the whole frame is not marked as a fallback");
    assert_eq!(
        shot.width.max(shot.height),
        400,
        "the whole frame fits the window's long edge"
    );
    assert_eq!(
        (shot.frame_width, shot.frame_height, shot.x, shot.y),
        (shot.width, shot.height, 0, 0)
    );

    // The same fallback again (a pan within the spot's reach) is not rendered; an edit renders it.
    let again = window(&s, &shots, (full_w, full_h), win, false);
    shot_at(&shots, preview(&s, &shots, 200, false));
    let got = generations(&shots.list.lock().unwrap());
    assert!(
        got.iter().all(|(g, _)| *g != again),
        "the same fallback rendered twice: {got:?}"
    );
    exec_ok(&s, "develop.set", r#"{"control":"light.exposure","value":0.4}"#);
    let edited = shot_at(&shots, window(&s, &shots, (full_w, full_h), win, false));
    assert!(edited.fallback);

    // A window away from the spot and its source still renders alone.
    let elsewhere = shot_at(
        &shots,
        window(&s, &shots, (full_w, full_h), [200, 200, 400, 300], false),
    );
    assert!(elsewhere.window);
}

#[test]
fn every_frame_carries_the_cropped_frame_at_the_photos_own_resolution() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "native.png", 640, 427));
    let whole = shot_at(&shots, preview(&s, &shots, 1280, false));
    assert_eq!(whole.native, (640, 427));

    // The middle quarter: about 320 x 213.5 of the photo's own pixels.
    exec_ok(&s, "crop.set", r#"{"rect":[0.25,0.25,0.75,0.75]}"#);
    let cropped = shot_at(&shots, preview(&s, &shots, 1280, false));
    let (native_w, native_h) = cropped.native;
    assert!(
        native_w == 320 && native_h.abs_diff(213) <= 1,
        "{:?}",
        cropped.native
    );
    let part = shot_at(&shots, window(&s, &shots, (320, 320), [0, 0, 64, 64], false));
    assert!(part.window);
    assert_eq!(part.native, cropped.native);
    // At 100 % the frame is the native one; its short edge, fitted from the aspect, may round apart.
    assert!(part.frame_width == native_w && part.frame_height.abs_diff(native_h) <= 1);
}

/// Closing the zoom releases the full source owner; a later zoom reconstructs the same pixels.
#[test]
fn releasing_the_windows_frees_the_resident_source_and_the_next_window_renders_again() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "big.png", 4200, 2400));
    let win = [1000, 800, 512, 384];
    let first = shot_at(&shots, window(&s, &shots, (4200, 4200), win, false));
    let before = exec_ok(&s, "library.memory", "{}");
    let gpu_bytes = before["fullResidency"]["gpuPixels"].as_u64().unwrap();
    assert!(
        gpu_bytes > 0 || std::env::var_os("UNVEIL_REQUIRE_GPU").is_none(),
        "window ownership test requires a GPU: {before}"
    );
    if gpu_bytes > 0 {
        assert_eq!(before["fullResidency"]["cpuPixels"], 0);
    }
    // SAFETY: s.raw is live, and the following command is ordered after the release message.
    unsafe { uv_release_windows(s.raw) };
    let after = exec_ok(&s, "library.memory", "{}");
    assert_eq!(
        after["fullSource"]["count"], 0,
        "full original survived the release: {after}"
    );
    assert_eq!(after["fullResidency"]["gpuPixels"], 0);
    let second = shot_at(&shots, window(&s, &shots, (4200, 4200), win, false));
    assert!(second.window && second.rgba == first.rgba);
    let next = exec_ok(&s, "library.memory", "{}");
    assert_eq!(next["fullSource"]["count"], 1);
    assert_eq!(next["fullResidency"]["gpuPixels"].as_u64().unwrap(), gpu_bytes);
}

#[test]
fn releasing_windows_keeps_the_full_original_needed_by_a_tight_crop_preview() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "tight.png", 1200, 800));
    exec_ok(
        &s,
        "develop.merge",
        r#"{"settings":{"crop":{"geometry":{"rect":{"x0":0.35,"y0":0.35,"x1":0.65,"y1":0.65},"angle":0}}}}"#,
    );
    let first = shot_at(&shots, preview(&s, &shots, 1280, false));
    let before = exec_ok(&s, "library.memory", "{}");
    assert_eq!(
        before["fullSource"]["count"], 1,
        "fit preview must select Full for this crop"
    );
    // SAFETY: s.raw is live; the preview's Full owner must survive a window release.
    unsafe { uv_release_windows(s.raw) };
    let after = exec_ok(&s, "library.memory", "{}");
    assert_eq!(after["fullSource"]["count"], 1);
    assert_eq!(
        after["fullResidency"]["gpuPixels"],
        before["fullResidency"]["gpuPixels"]
    );
    let second = shot_at(&shots, preview(&s, &shots, 1280, false));
    assert_eq!(second.rgba, first.rgba);
}

#[test]
fn reduced_drafts_reuse_sources_and_keep_fit_and_odd_window_coverage() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "draft.png", 1201, 801));
    let full = shot_at(&shots, preview(&s, &shots, 1025, false));
    let before = exec_ok(&s, "library.memory", "{}");
    let uploads = lightcraft_gpu::source_uploads();
    assert!(
        before["previewResidency"]["gpuPixels"].as_u64().unwrap() > 0
            || std::env::var_os("UNVEIL_REQUIRE_GPU").is_none()
    );
    for _ in 0..3 {
        let draft = shot_at(&shots, preview(&s, &shots, 1025, true));
        assert_eq!(draft.display, (full.width, full.height));
        assert_eq!((draft.frame_width, draft.frame_height), draft.display);
        assert_eq!(
            (draft.width, draft.height),
            (full.width.div_ceil(2), full.height.div_ceil(2))
        );
    }
    let active = exec_ok(&s, "library.memory", "{}");
    assert_eq!(
        active["unveil"]["previewStages"],
        before["unveil"]["previewStages"]
    );
    assert_eq!(
        active["previewResidency"]["readbacks"],
        before["previewResidency"]["readbacks"]
    );
    assert_eq!(lightcraft_gpu::source_uploads(), uploads);
    let settled = shot_at(&shots, preview(&s, &shots, 1025, false));
    assert_eq!(settled.rgba, full.rgba);
    let memory = exec_ok(&s, "library.memory", "{}");
    assert_eq!(
        memory["unveil"]["previewStages"],
        before["unveil"]["previewStages"]
    );

    let win = [101, 77, 301, 203];
    let full = shot_at(&shots, window_with(&s, &shots, (1201, 801), win, false, true));
    let window_memory = exec_ok(&s, "library.memory", "{}");
    let draft = shot_at(&shots, window_with(&s, &shots, (1201, 801), win, true, true));
    assert_eq!(draft.display, (301, 203));
    assert_eq!(
        (draft.x, draft.y, draft.frame_width, draft.frame_height),
        (101, 77, full.frame_width, full.frame_height)
    );
    assert_eq!((draft.width, draft.height), (151, 102));
    assert_eq!(
        draft.histogram.as_ref().unwrap().total,
        full.histogram.as_ref().unwrap().total
    );
    let settled = shot_at(&shots, window(&s, &shots, (1201, 801), win, false));
    assert_eq!(settled.rgba, full.rgba);
    let memory = exec_ok(&s, "library.memory", "{}");
    assert_eq!(
        memory["unveil"]["windowStages"],
        window_memory["unveil"]["windowStages"]
    );

    shot_at(&shots, preview(&s, &shots, 1025, true));
    // SAFETY: the session is live and the render has completed before suspension.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0);
    let memory = exec_ok(&s, "library.memory", "{}");
    assert_eq!(
        memory["unveil"]["previewStages"],
        serde_json::json!({"cpu":0,"gpu":0})
    );
    assert_eq!(
        memory["unveil"]["windowStages"],
        serde_json::json!({"cpu":0,"gpu":0})
    );
    // SAFETY: restore the process-wide GPU preference for the remaining tests.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
}

#[test]
fn a_window_request_with_an_empty_size_is_an_invalid_argument() {
    let s = TestSession::new();
    import_fixture(&s);
    let shots = Arc::new(Shots::default());
    let ctx = Arc::as_ptr(&shots) as *mut _;
    for (full, win) in [((0, 64), [0, 0, 8, 8]), ((96, 64), [0, 0, 0, 8])] {
        // SAFETY: s.raw is live; the request fails before queueing, so ctx is never used.
        let g = unsafe {
            uv_request_window(
                s.raw,
                full.0,
                full.1,
                win[0],
                win[1],
                win[2],
                win[3],
                false,
                false,
                false,
                Some(keep_shot),
                ctx,
            )
        };
        assert_eq!(g, 0);
        assert!(last_error().contains("size is 0"), "{}", last_error());
    }
}

/// RAW_MARGIN is the margin the app adds around the visible part of a window.
const RAW_MARGIN: u32 = 32;

/// raw_windows_match_the_whole_frame checks each RAW in UNVEIL_RAW_DIR at 100 %: the frame size the
/// window reports is the whole-frame render's, and a window in the middle matches its crop within the
/// spec's thresholds (mean ΔE2000 <= 0.5, p99 <= 2.0, see Docs/Baseline/equivalence.md) once its
/// margin is left out: spatial stages see less around the window's edge, and a whole frame this
/// large may render on the CPU where the window renders on the GPU. Without the variable it prints
/// a skip note and passes. It prints the ΔE it measures.
#[test]
fn raw_windows_match_the_whole_frame() {
    let Some(raw_dir) = std::env::var_os("UNVEIL_RAW_DIR") else {
        println!("skipped: UNVEIL_RAW_DIR not set");
        return;
    };
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&raw_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e != "md"))
        .collect();
    raws.sort();
    for raw in raws {
        let shots = Arc::new(Shots::default());
        let s = TestSession::new();
        let id = import_png(&s, &raw);
        let info = exec_ok(&s, "photo.inspect", &format!(r#"{{"id":{id}}}"#));
        let (w, h) = (
            info["width"].as_u64().unwrap() as u32,
            info["height"].as_u64().unwrap() as u32,
        );
        let long = w.max(h);
        let whole = shot_at(&shots, preview(&s, &shots, long, false));
        let win = [whole.width / 2 - 256, whole.height / 2 - 192, 512, 384];
        let part = shot_at(&shots, window(&s, &shots, (w, h), win, false));
        let reference = crop(&whole.rgba, whole.width, win);
        let (_, _, max) = delta_e_stats(&part.rgba, &reference);
        // The app asks for a margin of RAW_MARGIN px around what it shows.
        let inner = [RAW_MARGIN, RAW_MARGIN, 512 - 2 * RAW_MARGIN, 384 - 2 * RAW_MARGIN];
        let (mean, p99, inner_max) =
            delta_e_stats(&crop(&part.rgba, 512, inner), &crop(&reference, 512, inner));
        println!(
            "{}: photo {w}x{h}, whole {}x{}, window frame {}x{}, max ΔE {max:.3}; inside the margin \
             mean {mean:.4}, p99 {p99:.3}, max {inner_max:.3}",
            raw.file_name().unwrap().to_string_lossy(),
            whole.width,
            whole.height,
            part.frame_width,
            part.frame_height,
        );
        assert_eq!((part.frame_width, part.frame_height), (whole.width, whole.height));
        assert!(
            mean <= 0.5 && p99 <= 2.0,
            "inside the margin: mean {mean}, p99 {p99}"
        );
    }
}

/// a_window_that_asks_carries_the_whole_photos_histogram: the histogram comes from the whole frame
/// fitted in 512 px (every pixel sampled), not from the window, and follows an edit as the preview's
/// does, for full and draft windows.
#[test]
fn a_window_that_asks_carries_the_whole_photos_histogram() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "zoomed.png", 640, 427));
    for (exposure, draft) in [(0.0, false), (1.5, true)] {
        let control = format!(r#"{{"control":"light.exposure","value":{exposure}}}"#);
        exec_ok(&s, "develop.set", &control);
        let whole = shot_at(&shots, preview(&s, &shots, 640, draft));
        let win = [300, 200, 256, 192];
        let part = shot_at(&shots, window_with(&s, &shots, (1280, 1280), win, draft, true));
        assert!(part.window && !part.fallback, "not a window: {part:?}");
        let h = part.histogram.expect("a window that asked carried no histogram");
        let expected = whole.display.1 * 512 / whole.display.0;
        assert!(
            (expected..=expected + 1).any(|rows| h.total == 512 * rows),
            "sampled {} pixels, not a 512 px whole frame ({}x{} whole)",
            h.total,
            whole.width,
            whole.height
        );
        assert_eq!(
            h.sums, [h.total; 4],
            "every sampled pixel lands in one bin per channel"
        );
        let mean = whole.histogram.unwrap().mean_luma;
        assert!(
            (h.mean_luma - mean).abs() < 2.0,
            "exposure {exposure}: window histogram mean {} against the preview's {mean}",
            h.mean_luma
        );
    }
}

#[test]
fn whole_frames_carry_their_histogram_and_windows_do_not() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "histogram.png", 640, 427));
    for draft in [false, true] {
        let whole = shot_at(&shots, preview(&s, &shots, 640, draft));
        let h = whole.histogram.expect("a whole frame without a histogram");
        assert!(h.total > 0);
        assert_eq!(
            h.sums, [h.total; 4],
            "every sampled pixel lands in one bin per channel"
        );
        assert_eq!(h.clipped, h.edges, "the clipped counts are the edge bins");
    }
    let part = shot_at(
        &shots,
        window(&s, &shots, (1280, 1280), [300, 200, 256, 192], false),
    );
    assert!(
        part.window && part.histogram.is_none(),
        "a window carried a histogram"
    );
}

#[test]
fn an_exposure_that_clips_shows_in_the_clipped_counts() {
    let shots = Arc::new(Shots::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "clip.png", 640, 427));
    let mut clipped = Vec::new();
    for exposure in [-5.0, 0.0, 5.0] {
        let control = format!(r#"{{"control":"light.exposure","value":{exposure}}}"#);
        exec_ok(&s, "develop.set", &control);
        let h = shot_at(&shots, preview(&s, &shots, 640, false))
            .histogram
            .unwrap();
        let most = |side: [u32; 3]| side.into_iter().max().unwrap();
        clipped.push((most(h.clipped[0]), most(h.clipped[1]), h.total));
    }
    let [
        (low_dark, _, total),
        (low_plain, high_plain, _),
        (_, high_bright, _),
    ] = clipped[..]
    else {
        unreachable!()
    };
    assert!(
        high_bright > high_plain && high_bright > total / 1000,
        "{clipped:?}"
    );
    assert!(low_dark > low_plain && low_dark > total / 1000, "{clipped:?}");
}
