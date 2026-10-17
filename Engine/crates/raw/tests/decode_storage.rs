//! Modified for Unveil (2026-10-17): development must keep only worker-sized completed tile scratch.
use std::sync::Mutex;

use lightcraft_raw::memory_trace::{self, Change, Event};
use lightcraft_raw::{Cfa, Method, Normalized, demosaic};

static BYTES: Mutex<(usize, usize)> = Mutex::new((0, 0));

fn observe(event: Event) {
    if event.owner != "ahd-tiles" {
        return;
    }
    let mut bytes = BYTES.lock().unwrap();
    match event.change {
        Change::Acquire => bytes.0 += event.capacity,
        Change::Release | Change::Handoff => bytes.0 -= event.capacity,
        Change::Transfer | Change::Checkpoint => {}
    }
    bytes.1 = bytes.1.max(bytes.0);
}

#[test]
fn completed_tile_storage_is_bounded_by_workers_not_photo_area() {
    memory_trace::set_sink(observe);
    memory_trace::set_enabled(true);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let n = Normalized {
        width: 1024,
        height: 1024,
        cpp: 1,
        data: (0..1024 * 1024).map(|i| (i * 73 % 1021) as f32 / 1020.0).collect(),
        cfa: Cfa::bayer("RGGB"),
    };
    let image = pool.install(|| demosaic(&n, Method::Ahd));
    memory_trace::set_enabled(false);
    assert_eq!((image.width, image.height), (1024, 1024));
    let (live, peak) = *BYTES.lock().unwrap();
    assert_eq!(live, 0);
    assert!(peak <= 2 * 128 * 128 * 12, "completed tiles retained {peak} bytes");
}
